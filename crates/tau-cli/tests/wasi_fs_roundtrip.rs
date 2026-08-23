//! EPIC 3.6-b live host-enforcement round-trips through the REAL production
//! guest, built by `tau build wasm` and driven by real IR.
//!
//! - Denial: a `Read` at an UNGRANTED path — the host granted no preopen for
//!   it, so the guest holds no descriptor and surfaces `FsAccessDenied`.
//! - Positive (#604 hardening): nested preopens bind the LONGEST-prefix
//!   grant (a write under an RW child of an RO parent succeeds), `Write`
//!   truncates (no stale tail when overwriting a longer file), and a `/`
//!   (root) preopen serves subpaths.
//!
//! All tests build the wasm32-wasip2 guest, so they are #[ignore]d.

use std::path::PathBuf;
use std::process::Command;

use tau_cli::cmd::build_wasm::{lower_to_wasm_ir, wasm_world_for_project};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/wasm-build")
        .join(name)
}

/// Build the guest for a fixture and return its component bytes. Copied from
/// `wasi_http_roundtrip.rs::build_guest`.
fn build_guest(fixture_name: &str) -> Vec<u8> {
    let (_module, ir_bytes) = lower_to_wasm_ir(&fixture(fixture_name)).unwrap();
    let world = wasm_world_for_project(&fixture(fixture_name)).unwrap();
    let ir = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(ir.path(), &ir_bytes).unwrap();
    let wit = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(wit.path(), world.as_bytes()).unwrap();

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let out = Command::new(env!("CARGO"))
        .current_dir(&root)
        .args([
            "build",
            "-p",
            "tau-wasm-guest",
            "--target",
            "wasm32-wasip2",
            "--release",
            "--message-format=json",
        ])
        .env("CARGO_INCREMENTAL", "0")
        // Per-fixture target dir: sibling wasm-lane tests build the guest
        // with DIFFERENT worlds (distinct TAU_WORLD_WIT); a shared output
        // path lets a concurrent build clobber the .wasm between build and
        // read (#604 infra note).
        .env(
            "CARGO_TARGET_DIR",
            root.join(format!("target/tau-build-wasm-{fixture_name}")),
        )
        .env("TAU_IR_BYTES", ir.path())
        .env("TAU_WORLD_WIT", wit.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "guest build failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    let wasm_path = stdout
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|m| m["reason"] == "compiler-artifact")
        .filter(|m| {
            m["target"]["name"]
                .as_str()
                .is_some_and(|n| n == "tau-wasm-guest" || n == "tau_wasm_guest")
        })
        .flat_map(|m| {
            m["filenames"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|f| f.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .find(|f| f.ends_with(".wasm"))
        .unwrap();
    std::fs::read(&wasm_path).unwrap()
}

/// One cassette turn invoking `tool` with `input`. Field spellings mirror
/// `tau_ports::llm` exactly (verified in `wasi_http_roundtrip.rs`).
fn tool_turn(id: &str, tool: &str, input: serde_json::Value) -> String {
    serde_json::json!({
        "text": "",
        "tool_uses": [{ "id": id, "name": tool, "input": input }],
        "stop_reason": "ToolUse",
        "usage": null
    })
    .to_string()
}

/// The cassette's terminating turn.
fn end_turn() -> String {
    serde_json::json!({
        "text": "done",
        "tool_uses": [],
        "stop_reason": "EndTurn",
        "usage": null
    })
    .to_string()
}

/// A cassette that reads one ungranted path, then ends.
fn cassette() -> Vec<String> {
    vec![
        tool_turn(
            "call_1",
            "read_file",
            serde_json::json!({ "path": "/etc/secret" }),
        ),
        end_turn(),
    ]
}

/// Manifest-authoring caps path, as in the denial test: `Capability`'s
/// `Deserialize` impl (the fs variants are `#[non_exhaustive]`).
fn caps(json: &str) -> Vec<tau_domain::Capability> {
    serde_json::from_str::<Vec<tau_domain::Capability>>(json).unwrap()
}

#[test]
#[ignore = "builds the wasm32-wasip2 guest; run with --run-ignored"]
fn ungranted_path_is_denied_at_runtime_through_real_guest() {
    let wasm = build_guest("fs-read");

    // Grant fs.read on /data/** → the host preopens <sandbox>/data as guest
    // path "/data". The cassette reads "/etc/secret", for which the host
    // granted NO preopen, so the guest holds no descriptor. Constructed via
    // Capability's Deserialize impl (FsCapability::Read is #[non_exhaustive],
    // same manifest-authoring path used by wasi_http_roundtrip.rs / wasi_map).
    let caps = vec![serde_json::from_str::<tau_domain::Capability>(
        r#"{"kind":"fs.read","paths":["/data/**"]}"#,
    )
    .unwrap()];

    let sandbox = tempfile::tempdir().unwrap();
    let (_payload, emitted) =
        tau_wasm_host::run_component_with_caps(&wasm, "go", cassette(), &caps, sandbox.path())
            .expect("run completes: the denial is a tool-result error, not a host trap");

    // Ungranted-path denial is guest-observed ABSENCE (no host error-code exists
    // by construction — the guest never calls the host for a path it holds no
    // descriptor for), so the marker is the guest's exact `FsAccessDenied`, not
    // net's host `HttpRequestDenied`. See ADR-0066.
    assert!(
        emitted.iter().any(|e| e.contains("FsAccessDenied")),
        "ungranted path must be denied with FsAccessDenied; emitted events:\n{emitted:#?}"
    );
}

/// #604 positive path, gaps 1+3: with nested grants (`/data` read-only +
/// `/data/logs` read-write) a `Write` under `/data/logs` must bind the
/// LONGEST-prefix (RW) preopen — first-match would bind the RO `/data`
/// preopen (BTreeMap order) and be host-denied at `open-at`. A second,
/// shorter `Write` to the same file must leave no stale tail (`Write`
/// implies TRUNCATE). A `Read` through the RO parent then proves the
/// granted positive read path.
#[test]
#[ignore = "builds the wasm32-wasip2 guest; run with --run-ignored"]
fn nested_preopens_bind_longest_prefix_and_write_truncates() {
    let wasm = build_guest("fs-rw");
    let granted = caps(
        r#"[{"kind":"fs.read","paths":["/data/**"]},
            {"kind":"fs.write","paths":["/data/logs/**"]}]"#,
    );

    let sandbox = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(sandbox.path().join("data")).unwrap();
    std::fs::write(sandbox.path().join("data/seed.txt"), b"seeded-content").unwrap();

    let script = vec![
        // Longer content first, shorter second: without TRUNCATE the second
        // write would leave "short56789" on disk.
        tool_turn(
            "call_1",
            "write_file",
            serde_json::json!({ "path": "/data/logs/note.txt", "content": "0123456789" }),
        ),
        tool_turn(
            "call_2",
            "write_file",
            serde_json::json!({ "path": "/data/logs/note.txt", "content": "short" }),
        ),
        tool_turn(
            "call_3",
            "read_file",
            serde_json::json!({ "path": "/data/seed.txt" }),
        ),
        end_turn(),
    ];

    let (_payload, emitted) =
        tau_wasm_host::run_component_with_caps(&wasm, "go", script, &granted, sandbox.path())
            .expect("granted writes and read complete without a host trap");

    assert!(
        !emitted.iter().any(|e| e.contains("FsAccessDenied")),
        "granted paths must not be denied; emitted events:\n{emitted:#?}"
    );
    // The write reached the host filesystem through the RW `/data/logs`
    // preopen, and the second write truncated the first.
    let on_disk = std::fs::read_to_string(sandbox.path().join("data/logs/note.txt")).unwrap();
    assert_eq!(on_disk, "short", "Write must truncate (no stale tail)");
    // The positive read round-tripped the seeded content through wasi:filesystem.
    assert!(
        emitted.iter().any(|e| e.contains("seeded-content")),
        "granted read must surface the file content; emitted events:\n{emitted:#?}"
    );
}

/// #604 positive path, gap 2: a `/**` cap resolves to a `/` (root) preopen;
/// subpaths under it must resolve (pre-fix, only the literal path `/`
/// matched, so every subpath was FsAccessDenied despite the whole-FS grant).
#[test]
#[ignore = "builds the wasm32-wasip2 guest; run with --run-ignored"]
fn root_preopen_serves_subpaths() {
    let wasm = build_guest("fs-rw");
    let granted = caps(r#"[{"kind":"fs.read","paths":["/**"]}]"#);

    let sandbox = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(sandbox.path().join("data")).unwrap();
    std::fs::write(sandbox.path().join("data/ok.txt"), b"root-grant-content").unwrap();

    let script = vec![
        tool_turn(
            "call_1",
            "read_file",
            serde_json::json!({ "path": "/data/ok.txt" }),
        ),
        end_turn(),
    ];

    let (_payload, emitted) =
        tau_wasm_host::run_component_with_caps(&wasm, "go", script, &granted, sandbox.path())
            .expect("read under a root preopen completes without a host trap");

    assert!(
        !emitted.iter().any(|e| e.contains("FsAccessDenied")),
        "a root preopen must serve subpaths; emitted events:\n{emitted:#?}"
    );
    assert!(
        emitted.iter().any(|e| e.contains("root-grant-content")),
        "granted read must surface the file content; emitted events:\n{emitted:#?}"
    );
}
