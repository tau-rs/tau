//! Layer 4 container live spawn tests — sub-project D Task 6.
//!
//! Each test installs a real plugin binary into a tempdir scope, then
//! drives a golden-path agent invocation under the Container adapter
//! (`--sandbox container`) which engages Docker isolation. The plugin
//! actually runs under Docker; the test asserts the golden path completes
//! successfully.
//!
//! Skip-with-message if Docker is not available on the host.
//!
//! # v0.1 scope (Task 6, sub-project D)
//!
//! ## Tier A — fully implemented (shell + fs-read)
//!
//! These two tests force the Container adapter via
//! `resolve_adapter_forced(RegistryKind::Container)`, then drive a real
//! tool invocation (echo hello / file read) through the full
//! `spawn_tool_under_sandbox` driver path. Pattern mirrors Task 5
//! (layer4_native.rs) but targets the Container adapter.
//!
//! Skip-with-message on: (a) Docker not on PATH or daemon not running,
//! (b) container adapter probe returns Unavailable.
//!
//! ## Tier B — HTTP cassette-replay (anthropic, ollama, openai)
//!
//! Three LLM-backend tests exercising the Container adapter's proxy
//! bind-mount (T7, sub-project H).  Each test starts an in-process
//! `CassetteServer` on the host, spawns the plugin binary inside Docker
//! with a `Network(Http)` plan, and verifies the response matches the
//! cassette's expected "Hi there" reply.  The proxy (running in the host's
//! network namespace) bridges the container to the host's `127.0.0.1`.

#![cfg(feature = "integration-tests")]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use tau_domain::{
    fixtures as domain_fixtures, AgentInstanceId, Capability, PluginKind, PluginManifest, PortKind,
};
use tau_pkg::LockedPlugin;
use tau_ports::fixtures::scratch_dir;
use tau_ports::{
    CapabilityPlan, CapabilityProbe, CompletionRequest, ContentBlock, LlmProviderMessage,
    SessionContext,
};
use tau_runtime_tokio::process_gate::registry::RegistryKind;
use tau_runtime_tokio::process_gate::resolve_adapter_forced;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Skip the current test with a clear message if Docker isn't available.
///
/// Checks both `which docker` (binary on PATH) and `docker info` (daemon
/// reachable). Skips if either fails — a Docker binary without a running
/// daemon can't actually enforce container isolation.
fn require_docker() -> Result<(), String> {
    let which = Command::new("which")
        .arg("docker")
        .output()
        .map_err(|e| format!("which docker: {e}"))?;
    if !which.status.success() {
        return Err("docker not on PATH; skipping container layer 4 test".to_string());
    }
    let info = Command::new("docker")
        .arg("info")
        .arg("--format")
        .arg("{{.ServerVersion}}")
        .output()
        .map_err(|e| format!("docker info: {e}"))?;
    if !info.status.success() {
        return Err(
            "docker daemon not running or not reachable; skipping container layer 4 test"
                .to_string(),
        );
    }
    Ok(())
}

/// Assert the per-plugin container image is built locally.
///
/// Panics if neither podman nor docker can inspect the image — tests
/// using this helper are gated with `#[ignore = "..."]` so they only
/// run when the image-build prerequisite has been satisfied.
fn require_image_present(bin_name: &str) {
    let tag = format!("tau-plugin-{bin_name}:dev");
    // Probe podman first, then docker (matching ContainerRuntime::Auto).
    for runtime in ["podman", "docker"] {
        let out = std::process::Command::new(runtime)
            .args(["image", "inspect", &tag])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        if matches!(out, Ok(s) if s.success()) {
            return;
        }
    }
    panic!(
        "container image {tag} not present locally; \
         run `cargo xtask build-plugin-images --name {bin_name}` first"
    );
}

/// Locate the pre-built plugin binary.
///
/// Resolution order mirrors `layer4_native.rs`:
/// 1. `$CARGO_TARGET_DIR/release/<bin_name>` (CLAUDE.md-mandated override).
/// 2. Workspace-root `target/release/<bin_name>` fallback.
fn locate_plugin_bin(bin_name: &str) -> PathBuf {
    if let Ok(target_dir) = std::env::var("CARGO_TARGET_DIR") {
        let candidate = Path::new(&target_dir).join("release").join(bin_name);
        if candidate.exists() {
            return candidate;
        }
        let abs = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join(&target_dir)
            .join("release")
            .join(bin_name);
        if abs.exists() {
            return abs;
        }
    }
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    workspace_root.join("target").join("release").join(bin_name)
}

/// Construct a `LockedPlugin` pointing at the given binary path.
fn make_locked_plugin(bin_name: &str, binary_path: PathBuf) -> LockedPlugin {
    let manifest = PluginManifest::new(PortKind::Tool, PluginKind::RustCargo, bin_name.to_string());
    LockedPlugin::new(
        manifest,
        binary_path,
        std::time::SystemTime::UNIX_EPOCH,
        String::new(),
    )
}

/// Build a test `SessionContext` with the given granted capabilities.
fn make_session_context_with_caps(caps: Vec<Capability>) -> SessionContext {
    SessionContext::new(AgentInstanceId::new(), tau_domain::Uuid::new_v4(), None)
        .with_granted_capabilities(caps)
}

/// Resolve the container sandbox adapter, panicking if unavailable.
///
/// Tests using this helper are gated with `#[ignore = "..."]` so they
/// only run in environments where Docker/Podman is expected to be
/// available. A probe failure is a real test failure when explicitly run.
async fn resolve_container_adapter() -> tau_runtime_tokio::process_gate::SandboxAdapter {
    let adapter = resolve_adapter_forced(RegistryKind::Container)
        .await
        .expect("container adapter must resolve (requires Docker or Podman)");
    assert!(
        !matches!(adapter.probe().await, CapabilityProbe::Unavailable { .. }),
        "container adapter probe returned Unavailable (requires Docker or Podman)"
    );
    adapter
}

/// Construct a `LockedPlugin` for an LLM-backend plugin.
fn make_llm_locked_plugin(bin_name: &str, binary_path: PathBuf) -> LockedPlugin {
    let manifest = PluginManifest::new(
        PortKind::LlmBackend,
        PluginKind::RustCargo,
        bin_name.to_string(),
    );
    LockedPlugin::new(
        manifest,
        binary_path,
        std::time::SystemTime::UNIX_EPOCH,
        String::new(),
    )
}

/// Return the workspace root (two levels above this crate's manifest dir).
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

/// Build a minimal `CompletionRequest` that matches the cassette's "say hi"
/// fixture.  The model string is provider-specific; pass whatever the
/// cassette's response carries.
fn make_cassette_completion_request(model: &str) -> CompletionRequest {
    let mut req = CompletionRequest::new(model.to_string());
    req.messages
        .push(LlmProviderMessage::user(vec![ContentBlock::Text(
            "say hi".into(),
        )]));
    req.max_tokens = Some(20);
    req
}

/// Build the `net.http` capability that allows the plugin process to
/// reach the in-process cassette server running on the host.
///
/// The plugin's reqwest client routes all HTTP/HTTPS requests through
/// `HTTP_PROXY` / `HTTPS_PROXY` (set by `wrap_command` to point at the
/// in-container bridge). The bridge forwards the byte stream over a
/// Unix socket to the host-side proxy task, which parses the request
/// line, validates `Host` against this allowlist, and opens a TCP
/// connection from the host's network namespace.
///
/// `127.0.0.1` is the cassette server's address in the host's loopback —
/// the proxy task runs on the host, so dialing `127.0.0.1:<port>` reaches
/// the cassette directly.
fn make_net_http_localhost_cap() -> Capability {
    domain_fixtures::cap_net_http(&["127.0.0.1", "localhost"], &["POST", "GET"])
}

/// Minimal base64 encoding for the test fixture assertion.
/// Avoids importing the base64 crate into the test binary directly
/// (tau-plugin-compat doesn't depend on it).
fn base64_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::new();
    let mut i = 0;
    while i < input.len() {
        let b0 = input[i] as usize;
        let b1 = if i + 1 < input.len() {
            input[i + 1] as usize
        } else {
            0
        };
        let b2 = if i + 2 < input.len() {
            input[i + 2] as usize
        } else {
            0
        };
        output.push(ALPHABET[b0 >> 2] as char);
        output.push(ALPHABET[((b0 & 0x3) << 4) | (b1 >> 4)] as char);
        if i + 1 < input.len() {
            output.push(ALPHABET[((b1 & 0xf) << 2) | (b2 >> 6)] as char);
        } else {
            output.push('=');
        }
        if i + 2 < input.len() {
            output.push(ALPHABET[b2 & 0x3f] as char);
        } else {
            output.push('=');
        }
        i += 3;
    }
    output
}

// ---------------------------------------------------------------------------
// Tier A tests — Container adapter e2e (sub-project D Task 6)
// ---------------------------------------------------------------------------

/// Test 1 (Tier A): shell plugin — spawn under Container adapter, invoke
/// `shell.call({command: "echo", args: ["hello"]})`, assert "hello" in result.
///
/// This exercises:
/// - `resolve_adapter_forced(RegistryKind::Container)`
/// - `driver::spawn_tool_under_sandbox` → `plugin_host::load_tool`
/// - The container adapter's `wrap_spawn` pipeline (Docker isolation)
/// - The shell plugin's `SessionContext.granted_capabilities` path
///   admission check (process.spawn allow-list)
///
/// Skips cleanly if Docker is not available or container adapter probe
/// returns Unavailable.
#[tokio::test]
#[ignore = "requires Docker/Podman daemon + tau-plugin-shell-plugin:dev image + pre-built shell-plugin binary"]
async fn shell_layer4_container_runs_echo_hello() {
    // 1. Require Docker — without a running daemon, Container adapter is a no-op.
    require_docker().expect("docker daemon required for container layer 4 test");
    require_image_present("shell-plugin");

    // 2. Locate the pre-built shell plugin binary.
    let bin_path = locate_plugin_bin("shell-plugin");
    assert!(
        bin_path.exists(),
        "shell-plugin binary not found at {}; \
         run `cargo build -p tau-plugins-shell --release` first",
        bin_path.display()
    );

    // 3. Resolve the container sandbox adapter (Docker/Podman required).
    let adapter = resolve_container_adapter().await;

    // 4. Build the CapabilityPlan. Shell plugin needs process.spawn capability.
    let spawn_cap: Capability = domain_fixtures::cap_process_spawn(&["echo"]);

    let plan = CapabilityPlan::new(vec![spawn_cap.clone()], None, None);

    // 5. Synthesise a LockedPlugin for the shell binary.
    let plugin = make_locked_plugin("shell-plugin", bin_path);

    // 6. Spawn under the container sandbox via the driver.
    let dyn_tool = tau_plugin_compat::driver::spawn_tool_under_sandbox(
        &plugin,
        serde_json::json!({}),
        Some(Arc::new(adapter)),
        Some(&plan),
    )
    .await;

    let dyn_tool = match dyn_tool {
        Ok(t) => t,
        Err(e) => {
            panic!("spawn shell-plugin under container adapter failed: {e:?}");
        }
    };

    // 7. Build a SessionContext granting process.spawn for "echo".
    let ctx = make_session_context_with_caps(vec![spawn_cap]);
    let mut session = ();

    // 8. Invoke shell.call({command: "echo", args: ["hello"]}).
    let result = dyn_tool
        .invoke(
            &ctx,
            &mut session,
            serde_json::from_value(serde_json::json!({
                "command": "echo",
                "args": ["hello"]
            }))
            .expect("tool args must deserialize"),
        )
        .await
        .expect("shell.call must succeed");

    // 9. Assert "hello" appears somewhere in the result.
    let result_debug = format!("{result:?}");
    assert!(
        result_debug.contains("hello"),
        "expected 'hello' in shell.call result; got: {result_debug}"
    );
    assert!(
        !result.is_error,
        "shell.call returned is_error=true; result: {result_debug}"
    );
}

/// Test 2 (Tier A): fs-read plugin — spawn under Container adapter, write a
/// data.txt into a tempdir, invoke `fs_read.call({path: <data.txt>})`, and
/// assert the content is read back.
///
/// This exercises:
/// - `resolve_adapter_forced(RegistryKind::Container)` + `CapabilityPlan` with
///   `FsCapability::Read` allowing the tempdir.
/// - The container adapter's Docker-based enforcement for file reads.
/// - The fs-read plugin's glob-based path admission check.
///
/// Skips cleanly if Docker is not available or container adapter probe
/// returns Unavailable.
#[tokio::test]
#[ignore = "requires Docker/Podman daemon + tau-plugin-fs-read-plugin:dev image + pre-built fs-read-plugin binary"]
async fn fs_read_layer4_container_reads_data_file() {
    // 1. Require Docker.
    require_docker().expect("docker daemon required for container layer 4 test");
    require_image_present("fs-read-plugin");

    // 2. Locate the pre-built fs-read-plugin binary.
    let bin_path = locate_plugin_bin("fs-read-plugin");
    assert!(
        bin_path.exists(),
        "fs-read-plugin binary not found at {}; \
         run `cargo build -p tau-plugins-fs-read --release` first",
        bin_path.display()
    );

    // 3. Resolve the container sandbox adapter (Docker/Podman required).
    let adapter = resolve_container_adapter().await;

    // 4. Write the data fixture into a tempdir.
    let scope = scratch_dir("l4-container-fs-read");
    let data_path = scope.path().join("data.txt");
    let data_content = "layer4-container-fs-read-fixture";
    std::fs::write(&data_path, data_content).expect("write data.txt must succeed");

    // The fs-read plugin needs an fs.read capability granting access to the
    // tempdir. Use a glob that covers the whole tempdir.
    let tmpdir_glob = format!("{}/**", scope.path().display());

    let fs_read_cap: Capability = domain_fixtures::cap_fs_read(&[&tmpdir_glob]);

    let plan = CapabilityPlan::new(vec![fs_read_cap.clone()], None, None);

    // 5. Synthesise a LockedPlugin for the fs-read binary.
    let plugin = make_locked_plugin("fs-read-plugin", bin_path);

    // 6. Spawn under the container sandbox via the driver.
    let dyn_tool = tau_plugin_compat::driver::spawn_tool_under_sandbox(
        &plugin,
        serde_json::json!({}),
        Some(Arc::new(adapter)),
        Some(&plan),
    )
    .await;

    let dyn_tool = match dyn_tool {
        Ok(t) => t,
        Err(e) => {
            panic!("spawn fs-read-plugin under container adapter failed: {e:?}");
        }
    };

    // 7. Build a SessionContext granting fs.read for the tempdir glob.
    let ctx = make_session_context_with_caps(vec![fs_read_cap]);
    let mut session = ();

    // 8. Invoke fs_read.call({path: <data_path>}).
    let data_path_str = data_path
        .to_str()
        .expect("data path must be valid UTF-8")
        .to_string();
    let result = dyn_tool
        .invoke(
            &ctx,
            &mut session,
            serde_json::from_value(serde_json::json!({
                "path": data_path_str
            }))
            .expect("tool args must deserialize"),
        )
        .await
        .expect("fs_read.call must succeed");

    // 9. Assert the result contains the file content (base64-encoded).
    assert!(
        !result.is_error,
        "fs_read.call returned is_error=true; result: {result:?}"
    );
    assert!(
        !result.content.is_empty(),
        "fs_read.call returned empty content; result: {result:?}"
    );
    let result_debug = format!("{result:?}");
    // base64 of "layer4-container-fs-read-fixture"
    let expected_b64 = base64_encode(data_content.as_bytes());
    assert!(
        result_debug.contains(&expected_b64),
        "expected base64-encoded content '{expected_b64}' in fs_read.call result; \
         got: {result_debug}"
    );
}

// ---------------------------------------------------------------------------
// Tier B tests — HTTP cassette-replay via Container adapter (sub-project H T9)
// ---------------------------------------------------------------------------
//
// T7 (commit e64a23d) wired the Container adapter to spawn a userspace proxy
// on the host, bind-mount its Unix socket + tau-net-bridge binary into the
// container, set HTTPS_PROXY, and wrap the entrypoint with the bridge.  The
// proxy runs in the host's network namespace, so it can reach 127.0.0.1
// (where the in-process CassetteServer listens).  The container process
// routes through the bridge → proxy → host loopback — no veth or
// nftables-in-netns shenanigans needed.

/// Test 3: anthropic — container adapter + cassette replay.
///
/// Spin up a `CassetteServer` for the anthropic happy-path cassette, spawn
/// the anthropic plugin binary under the Container adapter with a
/// `Network(Http)` plan, invoke `DynLlmBackend::complete`, and assert the
/// response text matches the cassette's expected "Hi there" reply.
///
/// The proxy bind-mount (T7) lets the containerised plugin reach the host's
/// `127.0.0.1:<port>` cassette server via tau-net-bridge.
///
/// Skips if: (a) Docker not available, (b) anthropic-plugin binary not built.
#[tokio::test]
#[ignore = "requires Docker/Podman daemon + tau-plugin-anthropic-plugin:dev image + pre-built anthropic-plugin binary"]
async fn anthropic_layer4_container_completes_via_cassette() {
    // 1. Require Docker.
    require_docker().expect("docker daemon required for container layer 4 test");
    require_image_present("anthropic-plugin");

    // 2. Locate the pre-built anthropic plugin binary.
    let bin_path = locate_plugin_bin("anthropic-plugin");
    assert!(
        bin_path.exists(),
        "anthropic-plugin binary not found at {}; \
         run `cargo build -p tau-plugins-anthropic --release` first",
        bin_path.display()
    );

    // 3. Resolve the container sandbox adapter (Docker/Podman required).
    let adapter = resolve_container_adapter().await;

    // 4. Start a cassette-replay HTTP server from the anthropic happy-path
    //    cassette.  The server binds to 0.0.0.0:<random_port> on the host,
    //    but we address it via 127.0.0.1 which the proxy (running on the
    //    host) can reach.
    let cassette_path = workspace_root()
        .join("crates/tau-plugins/anthropic/tests/cassettes/complete_happy_path.yaml");
    let cassette_server = tau_plugin_test_support::cassette::replay(&cassette_path).await;
    let api_base = cassette_server.uri().to_string();

    // 5. Build the CapabilityPlan with Network(Http) for localhost.
    //    The proxy validates and allows loopback addresses; T7's wrap_command
    //    injects HTTPS_PROXY + bind-mounts the proxy socket automatically.
    let net_cap = make_net_http_localhost_cap();
    let plan = CapabilityPlan::new(vec![net_cap], None, None);

    // 6. Synthesise a LockedPlugin for the anthropic binary (LlmBackend port).
    let plugin = make_llm_locked_plugin("anthropic-plugin", bin_path);

    // 7. Build the plugin config JSON, pointing base_url at the cassette
    //    server.  Set api_key directly (test-only path).
    let config = serde_json::json!({
        "base_url": api_base,
        "api_key": "sk-ant-test"
    });

    // 8. Spawn the anthropic plugin under the container sandbox via the driver.
    let dyn_llm = tau_plugin_compat::driver::spawn_llm_under_sandbox(
        &plugin,
        config,
        Some(Arc::new(adapter)),
        Some(&plan),
    )
    .await;

    let dyn_llm = match dyn_llm {
        Ok(b) => b,
        Err(e) => {
            panic!("spawn anthropic-plugin under container adapter failed: {e:?}");
        }
    };

    // 9. Invoke complete with a request that matches the cassette.
    let req = make_cassette_completion_request("claude-3-5-haiku-latest");
    let result = dyn_llm.complete(req).await;

    // 10. Assert the response matches the cassette's expected outcome.
    let resp = result.expect("anthropic complete via cassette must succeed");
    assert_eq!(
        resp.text, "Hi there",
        "expected cassette response 'Hi there'; got: {:?}",
        resp.text
    );

    let received = cassette_server.received_requests();
    assert_eq!(
        received.len(),
        1,
        "cassette server should have received exactly 1 request; got: {}",
        received.len()
    );
}

/// Test 4: ollama — container adapter + cassette replay.
///
/// Spin up a `CassetteServer` for the ollama happy-path cassette, spawn
/// the ollama plugin binary under the Container adapter with a
/// `Network(Http)` plan, invoke `DynLlmBackend::complete`, and assert the
/// response text matches the cassette's expected "Hi there" reply.
///
/// Ollama doesn't require an API key; `bearer_token_env` is set to an
/// unset name so no Authorization header is injected.
///
/// Skips if: (a) Docker not available, (b) ollama-plugin binary not built.
#[tokio::test]
#[ignore = "requires Docker/Podman daemon + tau-plugin-ollama-plugin:dev image + pre-built ollama-plugin binary"]
async fn ollama_layer4_container_completes_via_cassette() {
    // 1. Require Docker.
    require_docker().expect("docker daemon required for container layer 4 test");
    require_image_present("ollama-plugin");

    // 2. Locate the pre-built ollama plugin binary.
    let bin_path = locate_plugin_bin("ollama-plugin");
    assert!(
        bin_path.exists(),
        "ollama-plugin binary not found at {}; \
         run `cargo build -p tau-plugins-ollama --release` first",
        bin_path.display()
    );

    // 3. Resolve the container sandbox adapter (Docker/Podman required).
    let adapter = resolve_container_adapter().await;

    // 4. Start the cassette-replay server.
    let cassette_path =
        workspace_root().join("crates/tau-plugins/ollama/tests/cassettes/complete_happy_path.yaml");
    let cassette_server = tau_plugin_test_support::cassette::replay(&cassette_path).await;
    let api_base = cassette_server.uri().to_string();

    // 5. Build the CapabilityPlan with Network(Http) for localhost.
    let net_cap = make_net_http_localhost_cap();
    let plan = CapabilityPlan::new(vec![net_cap], None, None);

    // 6. Synthesise the LockedPlugin (LlmBackend port).
    let plugin = make_llm_locked_plugin("ollama-plugin", bin_path);

    // 7. Build the config JSON.
    let config = serde_json::json!({
        "base_url": api_base,
        "bearer_token_env": "OLLAMA_BEARER_TOKEN_DEFINITELY_NOT_SET_FOR_TESTS"
    });

    // 8. Spawn under the container sandbox.
    let dyn_llm = tau_plugin_compat::driver::spawn_llm_under_sandbox(
        &plugin,
        config,
        Some(Arc::new(adapter)),
        Some(&plan),
    )
    .await;

    let dyn_llm = match dyn_llm {
        Ok(b) => b,
        Err(e) => {
            panic!("spawn ollama-plugin under container adapter failed: {e:?}");
        }
    };

    // 9. Invoke complete.
    let req = make_cassette_completion_request("llama3.2");
    let result = dyn_llm.complete(req).await;

    // 10. Assert response matches cassette.
    let resp = result.expect("ollama complete via cassette must succeed");
    assert_eq!(
        resp.text, "Hi there",
        "expected cassette response 'Hi there'; got: {:?}",
        resp.text
    );

    let received = cassette_server.received_requests();
    assert_eq!(
        received.len(),
        1,
        "cassette server should have received exactly 1 request; got: {}",
        received.len()
    );
}

/// Test 5: openai — container adapter + cassette replay.
///
/// Spin up a `CassetteServer` for the openai happy-path cassette, spawn
/// the openai plugin binary under the Container adapter with a
/// `Network(Http)` plan, invoke `DynLlmBackend::complete`, and assert the
/// response text matches the cassette's expected "Hi there" reply.
///
/// Skips if: (a) Docker not available, (b) openai-plugin binary not built.
#[tokio::test]
#[ignore = "requires Docker/Podman daemon + tau-plugin-openai-plugin:dev image + pre-built openai-plugin binary"]
async fn openai_layer4_container_completes_via_cassette() {
    // 1. Require Docker.
    require_docker().expect("docker daemon required for container layer 4 test");
    require_image_present("openai-plugin");

    // 2. Locate the pre-built openai plugin binary.
    let bin_path = locate_plugin_bin("openai-plugin");
    assert!(
        bin_path.exists(),
        "openai-plugin binary not found at {}; \
         run `cargo build -p tau-plugins-openai --release` first",
        bin_path.display()
    );

    // 3. Resolve the container sandbox adapter (Docker/Podman required).
    let adapter = resolve_container_adapter().await;

    // 4. Start the cassette-replay server.
    let cassette_path =
        workspace_root().join("crates/tau-plugins/openai/tests/cassettes/complete_happy_path.yaml");
    let cassette_server = tau_plugin_test_support::cassette::replay(&cassette_path).await;
    let api_base = cassette_server.uri().to_string();

    // 5. Build the CapabilityPlan with Network(Http) for localhost.
    let net_cap = make_net_http_localhost_cap();
    let plan = CapabilityPlan::new(vec![net_cap], None, None);

    // 6. Synthesise the LockedPlugin (LlmBackend port).
    let plugin = make_llm_locked_plugin("openai-plugin", bin_path);

    // 7. Build the config JSON.
    let config = serde_json::json!({
        "base_url": api_base,
        "api_key": "sk-test"
    });

    // 8. Spawn under the container sandbox.
    let dyn_llm = tau_plugin_compat::driver::spawn_llm_under_sandbox(
        &plugin,
        config,
        Some(Arc::new(adapter)),
        Some(&plan),
    )
    .await;

    let dyn_llm = match dyn_llm {
        Ok(b) => b,
        Err(e) => {
            panic!("spawn openai-plugin under container adapter failed: {e:?}");
        }
    };

    // 9. Invoke complete.
    let req = make_cassette_completion_request("gpt-4o-mini");
    let result = dyn_llm.complete(req).await;

    // 10. Assert response matches cassette.
    let resp = result.expect("openai complete via cassette must succeed");
    assert_eq!(
        resp.text, "Hi there",
        "expected cassette response 'Hi there'; got: {:?}",
        resp.text
    );

    let received = cassette_server.received_requests();
    assert_eq!(
        received.len(),
        1,
        "cassette server should have received exactly 1 request; got: {}",
        received.len()
    );
}
