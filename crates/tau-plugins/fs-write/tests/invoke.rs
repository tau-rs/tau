//! Integration tests: FsWritePlugin driven via FakeStdioPeer.
//!
//! Mirrors crates/tau-plugins/fs-read/tests/invoke.rs.

use base64::Engine as _;
use fs_write_plugin_lib::plugin::FsWritePlugin;
use std::time::SystemTime;
use tau_domain::{AgentInstanceId, Capability, PortKind, Value};
use tau_plugin_protocol::{
    handshake::{meta, HandshakeRequest, TraceContext},
    test_support::FakeStdioPeer,
    Frame, PROTOCOL_VERSION,
};
use tau_plugin_sdk::{run_tool_with_io, Configure};
use tau_ports::{DenyEntry, SessionContext};
use uuid::Uuid;

// ---- helpers ----

/// Build an `fs.write` capability via JSON (FsCapability is `#[non_exhaustive]`).
fn fs_write_cap(paths: &[&str], max_bytes: Option<u64>) -> Capability {
    #[derive(serde::Deserialize)]
    struct Wrapper {
        cap: Capability,
    }
    let paths_json: Vec<serde_json::Value> = paths
        .iter()
        .map(|p| serde_json::Value::String((*p).to_string()))
        .collect();
    let mut cap_obj = serde_json::json!({ "kind": "fs.write", "paths": paths_json });
    if let Some(mb) = max_bytes {
        cap_obj
            .as_object_mut()
            .unwrap()
            .insert("max_bytes".to_string(), serde_json::json!(mb));
    }
    let json = serde_json::json!({ "cap": cap_obj });
    serde_json::from_value::<Wrapper>(json)
        .expect("test fs.write capability must parse")
        .cap
}

async fn do_handshake(peer: &mut FakeStdioPeer) {
    let req = HandshakeRequest::new(
        PROTOCOL_VERSION.to_string(),
        PortKind::Tool,
        TraceContext::new("r".into(), "a".into(), "s".into()),
        serde_json::Value::Null,
    );
    let params_bytes = rmp_serde::to_vec(&vec![&req]).unwrap();
    peer.writer
        .write_frame(
            &Frame::Request {
                id: 1,
                method: meta::HANDSHAKE_METHOD.to_string(),
                params: params_bytes,
            }
            .encode()
            .unwrap(),
        )
        .await
        .unwrap();
    let _ = peer.reader.next_frame().await.unwrap().unwrap();
}

async fn send_tool_call(
    peer: &mut FakeStdioPeer,
    id: u32,
    ctx: &SessionContext,
    args: serde_json::Value,
) {
    let args_value: Value = serde_json::from_value(args).expect("args round-trip to tau Value");
    let params_bytes = rmp_serde::to_vec(&(ctx, &args_value)).unwrap();
    peer.writer
        .write_frame(
            &Frame::Request {
                id,
                method: "tool.call".to_string(),
                params: params_bytes,
            }
            .encode()
            .unwrap(),
        )
        .await
        .unwrap();
}

async fn recv_tool_response(peer: &mut FakeStdioPeer) -> Result<tau_ports::ToolResult, String> {
    let body = peer.reader.next_frame().await.unwrap().unwrap();
    let frame = Frame::decode(&body).map_err(|e| format!("frame decode: {e}"))?;
    match frame {
        Frame::Response {
            result: Some(bytes),
            error: None,
            ..
        } => {
            let result: tau_ports::ToolResult =
                rmp_serde::from_slice(&bytes).map_err(|e| format!("rmp decode ToolResult: {e}"))?;
            Ok(result)
        }
        Frame::Response {
            error: Some(env),
            result: None,
            ..
        } => Err(format!("rpc error code={} msg={}", env.code, env.message)),
        other => Err(format!("unexpected frame: {other:?}")),
    }
}

async fn shutdown(peer: &mut FakeStdioPeer) {
    peer.writer
        .write_frame(
            &Frame::Notification {
                method: meta::SHUTDOWN_METHOD.to_string(),
                params: rmp_serde::to_vec::<Vec<()>>(&Vec::new()).unwrap(),
            }
            .encode()
            .unwrap(),
        )
        .await
        .unwrap();
}

fn make_ctx(grants: Vec<Capability>) -> SessionContext {
    SessionContext::new(
        AgentInstanceId::new(),
        Uuid::now_v7(),
        Some(SystemTime::UNIX_EPOCH),
    )
    .with_granted_capabilities(grants)
}

/// Spawn the plugin runner over a fresh FakeStdioPeer.
fn spawn_plugin() -> (
    FakeStdioPeer,
    tokio::task::JoinHandle<Result<(), tau_plugin_sdk::SdkError>>,
) {
    let (peer, mut sut_reader, mut sut_writer) = FakeStdioPeer::new();
    let plugin = FsWritePlugin::from_config(Default::default()).unwrap();
    let runner = tokio::spawn(async move {
        run_tool_with_io(
            &mut sut_reader,
            &mut sut_writer,
            plugin,
            "fs-write",
            "0.1.0",
        )
        .await
    });
    (peer, runner)
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn int_field(result: &tau_ports::ToolResult, key: &str) -> i64 {
    let tau_ports::ToolContent::Json { data } = &result.content[0] else {
        panic!("expected Json content, got {result:?}")
    };
    data.as_object()
        .and_then(|m| m.get(key))
        .and_then(Value::as_integer)
        .unwrap_or_else(|| panic!("missing integer field {key} in {result:?}"))
}

// ---- write mode ----

#[tokio::test]
async fn integration_write_creates_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("out.txt");
    let path_str = path.to_str().unwrap().to_string();
    let glob = format!("{}/**", dir.path().to_str().unwrap());

    let (mut peer, runner) = spawn_plugin();
    do_handshake(&mut peer).await;
    let ctx = make_ctx(vec![fs_write_cap(&[&glob], None)]);
    let payload = b"hello tau\n";
    send_tool_call(
        &mut peer,
        2,
        &ctx,
        serde_json::json!({ "mode": "write", "path": path_str, "contents": b64(payload) }),
    )
    .await;
    let result = recv_tool_response(&mut peer).await.expect("Ok response");

    assert!(!result.is_error, "expected success; got {result:?}");
    assert_eq!(int_field(&result, "bytes_written"), payload.len() as i64);
    assert_eq!(std::fs::read(&path).unwrap(), payload);

    shutdown(&mut peer).await;
    drop(peer);
    let _ = runner.await;
}

#[tokio::test]
async fn integration_write_out_of_scope_bad_args() {
    let dir = tempfile::tempdir().unwrap();
    let path_str = dir.path().join("out.txt").to_str().unwrap().to_string();

    let (mut peer, runner) = spawn_plugin();
    do_handshake(&mut peer).await;
    let ctx = make_ctx(vec![fs_write_cap(&["/var/nope/**"], None)]);
    send_tool_call(
        &mut peer,
        2,
        &ctx,
        serde_json::json!({ "mode": "write", "path": path_str, "contents": b64(b"x") }),
    )
    .await;
    let err = recv_tool_response(&mut peer).await.expect_err("RPC error");
    assert!(err.contains("not in capability scope"), "got: {err}");

    shutdown(&mut peer).await;
    drop(peer);
    let _ = runner.await;
}

#[tokio::test]
async fn integration_write_over_max_bytes_bad_args() {
    let dir = tempfile::tempdir().unwrap();
    let path_str = dir.path().join("out.txt").to_str().unwrap().to_string();
    let glob = format!("{}/**", dir.path().to_str().unwrap());

    let (mut peer, runner) = spawn_plugin();
    do_handshake(&mut peer).await;
    let ctx = make_ctx(vec![fs_write_cap(&[&glob], Some(4))]);
    send_tool_call(
        &mut peer,
        2,
        &ctx,
        serde_json::json!({ "mode": "write", "path": path_str, "contents": b64(b"too many bytes") }),
    )
    .await;
    let err = recv_tool_response(&mut peer).await.expect_err("RPC error");
    assert!(err.contains("max_bytes"), "got: {err}");

    shutdown(&mut peer).await;
    drop(peer);
    let _ = runner.await;
}

// ---- edit mode ----

#[tokio::test]
async fn integration_edit_single_match_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("code.rs");
    let path_str = path.to_str().unwrap().to_string();
    std::fs::write(&path, "fn main() {}\n").unwrap();
    let glob = format!("{}/**", dir.path().to_str().unwrap());

    let (mut peer, runner) = spawn_plugin();
    do_handshake(&mut peer).await;
    let ctx = make_ctx(vec![fs_write_cap(&[&glob], None)]);
    send_tool_call(
        &mut peer,
        2,
        &ctx,
        serde_json::json!({
            "mode": "edit", "path": path_str,
            "old_str": "fn main() {}", "new_str": "fn main() { run(); }"
        }),
    )
    .await;
    let result = recv_tool_response(&mut peer).await.expect("Ok response");

    assert!(!result.is_error, "got {result:?}");
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "fn main() { run(); }\n"
    );

    shutdown(&mut peer).await;
    drop(peer);
    let _ = runner.await;
}

#[tokio::test]
async fn integration_edit_not_found_is_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("code.rs");
    let path_str = path.to_str().unwrap().to_string();
    std::fs::write(&path, "alpha\n").unwrap();
    let glob = format!("{}/**", dir.path().to_str().unwrap());

    let (mut peer, runner) = spawn_plugin();
    do_handshake(&mut peer).await;
    let ctx = make_ctx(vec![fs_write_cap(&[&glob], None)]);
    send_tool_call(
        &mut peer,
        2,
        &ctx,
        serde_json::json!({
            "mode": "edit", "path": path_str, "old_str": "zzz", "new_str": "q"
        }),
    )
    .await;
    let result = recv_tool_response(&mut peer).await.expect("Ok response");
    assert!(result.is_error, "expected is_error; got {result:?}");

    shutdown(&mut peer).await;
    drop(peer);
    let _ = runner.await;
}

#[tokio::test]
async fn integration_edit_ambiguous_is_error_then_replace_all_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("code.rs");
    let path_str = path.to_str().unwrap().to_string();
    std::fs::write(&path, "a\na\n").unwrap();
    let glob = format!("{}/**", dir.path().to_str().unwrap());

    // First: ambiguous (2 matches, replace_all default false) → is_error.
    let (mut peer, runner) = spawn_plugin();
    do_handshake(&mut peer).await;
    let ctx = make_ctx(vec![fs_write_cap(&[&glob], None)]);
    send_tool_call(
        &mut peer,
        2,
        &ctx,
        serde_json::json!({ "mode": "edit", "path": path_str, "old_str": "a", "new_str": "b" }),
    )
    .await;
    let result = recv_tool_response(&mut peer).await.expect("Ok response");
    assert!(
        result.is_error,
        "expected ambiguity is_error; got {result:?}"
    );
    shutdown(&mut peer).await;
    drop(peer);
    let _ = runner.await;
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "a\na\n",
        "file untouched"
    );

    // Then: replace_all true → all replaced, success.
    let (mut peer, runner) = spawn_plugin();
    do_handshake(&mut peer).await;
    let ctx = make_ctx(vec![fs_write_cap(&[&glob], None)]);
    send_tool_call(
        &mut peer,
        2,
        &ctx,
        serde_json::json!({
            "mode": "edit", "path": path_str,
            "old_str": "a", "new_str": "b", "replace_all": true
        }),
    )
    .await;
    let result = recv_tool_response(&mut peer).await.expect("Ok response");
    assert!(!result.is_error, "got {result:?}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "b\nb\n");
    shutdown(&mut peer).await;
    drop(peer);
    let _ = runner.await;
}

// ---- shared validation (mirrors fs-read) ----

#[cfg(unix)]
#[tokio::test]
async fn integration_traversal_rejected() {
    let (mut peer, runner) = spawn_plugin();
    do_handshake(&mut peer).await;
    let ctx = make_ctx(vec![fs_write_cap(&["/**"], None)]);
    send_tool_call(
        &mut peer,
        2,
        &ctx,
        serde_json::json!({ "mode": "write", "path": "/tmp/../etc/x", "contents": "" }),
    )
    .await;
    let err = recv_tool_response(&mut peer).await.expect_err("RPC error");
    assert!(
        err.contains("`..` segment") || err.contains("traversal"),
        "got: {err}"
    );
    shutdown(&mut peer).await;
    drop(peer);
    let _ = runner.await;
}

#[cfg(unix)]
#[tokio::test]
async fn integration_deny_overrides_allow() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("secret.txt");
    let path_str = path.to_str().unwrap().to_string();
    std::fs::write(&path, b"old").unwrap();
    let allow_glob = format!("{}/**", dir.path().to_str().unwrap());

    let (mut peer, runner) = spawn_plugin();
    do_handshake(&mut peer).await;
    let ctx = SessionContext::new(
        AgentInstanceId::new(),
        Uuid::now_v7(),
        Some(SystemTime::UNIX_EPOCH),
    )
    .with_granted_capabilities(vec![fs_write_cap(&[&allow_glob], None)])
    .with_deny_entries(vec![DenyEntry::new(
        "fs.write".into(),
        vec![path_str.clone()],
    )]);
    send_tool_call(
        &mut peer,
        2,
        &ctx,
        serde_json::json!({ "mode": "write", "path": path_str, "contents": b64(b"new") }),
    )
    .await;
    let err = recv_tool_response(&mut peer).await.expect_err("RPC error");
    assert!(err.contains("not in capability scope"), "got: {err}");
    shutdown(&mut peer).await;
    drop(peer);
    let _ = runner.await;
}
