//! Layer 2 install-time cross-check: spawn the plugin binary, perform the
//! `meta.handshake` RPC, and compare the capabilities the binary claims
//! (via `tool.describe_capabilities`) against what the manifest declares.
//!
//! For tool-port plugins the check is bidirectional — binary-claims-extra
//! and manifest-declares-unused both hard-fail. For LLM-backend and storage
//! plugins the binary-side wire mechanism is deferred per ADR-0016 Decision 1;
//! those paths return the manifest's capability list verbatim.
//!
//! The public entry point is [`cross_check_plugin_capabilities`].

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use tau_domain::PackageManifest;
use tau_domain::{Capability, CapabilityShape, PortKind};
use tau_plugin_protocol::{
    handshake::{meta, HandshakeRequest, HandshakeResponse, TraceContext, PROTOCOL_VERSION},
    Frame, FramedReader, FramedWriter, FramerOptions,
};
use tokio::process::Command;
use tokio::time::{timeout, Duration};

const TOOL_DESCRIBE_CAPABILITIES_METHOD: &str = "tool.describe_capabilities";

/// Timeout for the `meta.handshake` response. A plugin that spawns but never
/// writes its handshake reply would otherwise stall `tau install` indefinitely.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Per-method timeout for each `tool.describe_capabilities` response.
const DESCRIBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Errors that may arise during a plugin cross-check.
///
/// `#[non_exhaustive]`: future verification layers may add variants without
/// breaking callers.
///
/// # Example
///
/// ```
/// use tau_pkg::sandbox_check::CrossCheckError;
///
/// let err = CrossCheckError::SpawnFailed("permission denied".to_string());
/// let display = format!("{err}");
/// assert!(display.contains("plugin spawn failed"));
/// assert!(display.contains("permission denied"));
///
/// let err2 = CrossCheckError::HandshakeFailed("timed out after 10s".to_string());
/// let display2 = format!("{err2}");
/// assert!(display2.contains("handshake failed"));
/// ```
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum CrossCheckError {
    /// The plugin binary could not be launched.
    #[error("plugin spawn failed: {0}")]
    SpawnFailed(String),
    /// The cross-check spawn could not be sandboxed and the unsandboxed escape
    /// hatch was not enabled (audit S2).
    #[error(
        "refusing to spawn an unsandboxed cross-check of untrusted plugin code; \
         re-run with --allow-unsandboxed-build to override (audit S2)"
    )]
    SandboxRefused,
    /// The `meta.handshake` round-trip failed (encode / IO / decode /
    /// protocol error).
    #[error("plugin handshake failed: {0}")]
    HandshakeFailed(String),
    /// The binary reports a capability that the manifest does not declare.
    #[error(
        "plugin '{plugin}' declares capability {claimed:?} via tool.describe_capabilities \
         but manifest does not include it"
    )]
    BinaryClaimsExtra {
        /// Plugin name (from the handshake response).
        plugin: String,
        /// The extra capability the binary claims.
        claimed: Capability,
    },
    /// The manifest declares a capability that the binary never requests.
    #[error(
        "manifest of '{plugin}' declares capability {declared:?} but binary does not request it"
    )]
    ManifestDeclaresUnused {
        /// Plugin name (from the handshake response).
        plugin: String,
        /// The capability declared in the manifest but not requested by the binary.
        declared: Capability,
    },
}

/// Spawn the plugin binary, perform the `meta.handshake` RPC, enumerate
/// capabilities via `tool.describe_capabilities` (tool-port plugins only),
/// and return the deduplicated [`CapabilityShape`] set.
///
/// For LLM-backend and storage plugins the binary-side wire mechanism is
/// deferred (ADR-0016 Decision 1); those paths return
/// `manifest.capabilities()` mapped to [`CapabilityShape`] values.
///
/// # Errors
///
/// Returns [`CrossCheckError::SpawnFailed`] if the binary cannot be launched,
/// [`CrossCheckError::HandshakeFailed`] if the handshake round-trip fails, and
/// [`CrossCheckError::BinaryClaimsExtra`] /
/// [`CrossCheckError::ManifestDeclaresUnused`] if the capability sets diverge.
///
/// **Tests:** the `diff_capabilities` helper + error display are unit-tested
/// in this file. The end-to-end function `cross_check_plugin_capabilities`
/// itself is exercised via real-binary integration tests in `tau-plugin-compat`
/// (Tasks 6-9 of sub-project B's plan).
pub async fn cross_check_plugin_capabilities(
    binary_path: &Path,
    manifest: &PackageManifest,
) -> Result<Vec<CapabilityShape>, CrossCheckError> {
    // Back-compat entry point (e.g. `tau check`): spawns unsandboxed. The
    // install-time path uses [`cross_check_plugin_capabilities_gated`] with a
    // real gate. `tau check`'s sandbox posture is out of scope for audit S2.
    cross_check_plugin_capabilities_gated(binary_path, manifest, None, true).await
}

/// As [`cross_check_plugin_capabilities`], but applies the install-time
/// sandbox gate before spawning the untrusted binary (audit S2).
///
/// Fail-closed: with no enforcing `gate` and no `allow_unsandboxed_build`,
/// returns [`CrossCheckError::SandboxRefused`] before spawning.
pub async fn cross_check_plugin_capabilities_gated(
    binary_path: &Path,
    manifest: &PackageManifest,
    gate: Option<&dyn crate::install_sandbox::InstallSandbox>,
    allow_unsandboxed_build: bool,
) -> Result<Vec<CapabilityShape>, CrossCheckError> {
    use crate::install_sandbox::{cross_check_envelope, sandbox_decision, SandboxDecision};

    // ── 1. Spawn ─────────────────────────────────────────────────────────────
    let mut command = Command::new(binary_path);
    command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true); // ensure child is reaped on every early-return path

    // Decide-then-wrap-then-spawn. The guard holds any sandbox resources and
    // must outlive the child.
    let _sandbox_guard = match sandbox_decision(gate, allow_unsandboxed_build) {
        SandboxDecision::Sandbox => {
            let plan = cross_check_envelope();
            let g = gate.expect("Sandbox decision implies a gate is present");
            Some(
                g.wrap(&plan, command.as_std_mut())
                    .map_err(|e| CrossCheckError::SpawnFailed(format!("sandbox wrap: {e}")))?,
            )
        }
        SandboxDecision::Unsandboxed => {
            tracing::warn!(
                target: "tau_pkg::install",
                "spawning cross-check WITHOUT a sandbox (--allow-unsandboxed-build)"
            );
            None
        }
        SandboxDecision::Refuse => return Err(CrossCheckError::SandboxRefused),
    };

    let mut child = command
        .spawn()
        .map_err(|e| CrossCheckError::SpawnFailed(format!("{e}")))?;

    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| CrossCheckError::SpawnFailed("child stdin not piped".to_string()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| CrossCheckError::SpawnFailed("child stdout not piped".to_string()))?;

    let mut writer = FramedWriter::new(stdin);
    let mut reader = FramedReader::new(stdout, FramerOptions::default());

    // ── 2. Handshake ─────────────────────────────────────────────────────────
    let port = manifest
        .plugin()
        .map(|p| p.provides)
        .unwrap_or(PortKind::Tool);

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();

    // Send `{}` rather than Null: plugin config structs typically don't
    // accept null as the top-level value (only objects), so a Null here
    // causes the SDK runner to fail config deserialization and exit
    // silently — the host then sees EOF on the next read. An empty
    // object lets every plugin with `#[derive(Default)]`-style configs
    // construct their defaults.
    let handshake_req = HandshakeRequest::new(
        PROTOCOL_VERSION.to_string(),
        port,
        TraceContext::new(
            format!("install-cross-check-{nanos}"),
            "install".to_string(),
            "cross-check".to_string(),
        ),
        serde_json::Value::Object(serde_json::Map::new()),
    );

    // The wire shape for `meta.handshake` params is a 1-element array
    // (`Vec<HandshakeRequest>`) per the SDK's matching decoder; see
    // `tau_plugin_sdk::handshake::drive_handshake` and the parallel
    // call site at `tau_runtime::plugin_host::handshake`.
    let params = rmp_serde::to_vec(&vec![&handshake_req])
        .map_err(|e| CrossCheckError::HandshakeFailed(format!("encode handshake request: {e}")))?;

    let req_frame = Frame::Request {
        id: 1,
        method: meta::HANDSHAKE_METHOD.to_string(),
        params,
    };
    let req_bytes = req_frame
        .encode()
        .map_err(|e| CrossCheckError::HandshakeFailed(format!("frame encode: {e}")))?;

    writer
        .write_frame(&req_bytes)
        .await
        .map_err(|e| CrossCheckError::HandshakeFailed(format!("write frame: {e}")))?;

    // ── 3. Read handshake response ────────────────────────────────────────────
    let body = timeout(HANDSHAKE_TIMEOUT, reader.next_frame())
        .await
        .map_err(|_| {
            CrossCheckError::HandshakeFailed("handshake response timed out after 10s".to_string())
        })?
        .map_err(|e| CrossCheckError::HandshakeFailed(format!("read frame: {e}")))?
        .ok_or_else(|| {
            CrossCheckError::HandshakeFailed("EOF before handshake response".to_string())
        })?;

    let frame = Frame::decode(&body)
        .map_err(|e| CrossCheckError::HandshakeFailed(format!("decode frame: {e}")))?;

    let response_bytes = match frame {
        Frame::Response {
            id: 1,
            error: None,
            result: Some(bytes),
        } => bytes,
        Frame::Response {
            error: Some(env), ..
        } => {
            return Err(CrossCheckError::HandshakeFailed(format!(
                "plugin returned RPC error {}: {}",
                env.code, env.message
            )));
        }
        Frame::Response {
            id,
            error: None,
            result: None,
            ..
        } => {
            return Err(CrossCheckError::HandshakeFailed(format!(
                "handshake response id={id} has neither result nor error"
            )));
        }
        Frame::Response { id, .. } => {
            return Err(CrossCheckError::HandshakeFailed(format!(
                "unexpected response id={id} (expected 1)"
            )));
        }
        other => {
            return Err(CrossCheckError::HandshakeFailed(format!(
                "expected Response frame, got {other:?}"
            )));
        }
    };

    let response: HandshakeResponse = rmp_serde::from_slice(&response_bytes)
        .map_err(|e| CrossCheckError::HandshakeFailed(format!("decode HandshakeResponse: {e}")))?;

    let plugin_name = response.plugin_name.clone();

    // ── 4. Enumerate capabilities ─────────────────────────────────────────────
    let binary_caps: Vec<Capability> = match response.provides {
        PortKind::Tool => {
            enumerate_tool_capabilities(&mut writer, &mut reader, &response.methods).await?
        }
        PortKind::LlmBackend | PortKind::Storage => {
            tracing::debug!(
                plugin = %plugin_name,
                provides = %response.provides,
                "manifest-only capability path: binary-side wire mechanism deferred \
                 per ADR-0016 Decision 1"
            );
            manifest.capabilities().to_vec()
        }
        other => {
            tracing::debug!(
                plugin = %plugin_name,
                provides = %other,
                "unrecognised port kind — returning manifest capabilities verbatim"
            );
            manifest.capabilities().to_vec()
        }
    };

    // ── 5. Best-effort shutdown ───────────────────────────────────────────────
    // Encoding an empty Vec to msgpack is infallible (produces [0x90]).
    let empty_params = rmp_serde::to_vec::<Vec<()>>(&Vec::new())
        .expect("encoding empty Vec to msgpack is infallible");
    let shutdown = Frame::Notification {
        method: meta::SHUTDOWN_METHOD.to_string(),
        params: empty_params,
    };
    if let Ok(shutdown_bytes) = shutdown.encode() {
        let _ = writer.write_frame(&shutdown_bytes).await;
    }
    let _ = child.wait().await;

    // ── 6. Diff ───────────────────────────────────────────────────────────────
    diff_capabilities(&plugin_name, &binary_caps, manifest.capabilities())?;

    // ── 7. Map to CapabilityShape ─────────────────────────────────────────────
    let shapes: Vec<CapabilityShape> = binary_caps
        .iter()
        .map(|c| c.required_shape())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();

    Ok(shapes)
}

/// Issue `tool.describe_capabilities` for every method in `methods` that
/// begins with `"tool."`, union the results, and deduplicate.
async fn enumerate_tool_capabilities<W, R>(
    writer: &mut FramedWriter<W>,
    reader: &mut FramedReader<R>,
    methods: &[String],
) -> Result<Vec<Capability>, CrossCheckError>
where
    W: tokio::io::AsyncWrite + Unpin,
    R: tokio::io::AsyncRead + Unpin,
{
    let mut all: Vec<Capability> = Vec::new();

    let params_bytes = rmp_serde::to_vec::<Vec<()>>(&Vec::new()).map_err(|e| {
        CrossCheckError::HandshakeFailed(format!("encode tool.describe_capabilities params: {e}"))
    })?;

    for (id, method) in (2_u32..).zip(methods.iter().filter(|m| m.starts_with("tool."))) {
        let req_frame = Frame::Request {
            id,
            method: TOOL_DESCRIBE_CAPABILITIES_METHOD.to_string(),
            params: params_bytes.clone(),
        };
        let req_bytes = req_frame
            .encode()
            .map_err(|e| CrossCheckError::HandshakeFailed(format!("frame encode: {e}")))?;

        writer
            .write_frame(&req_bytes)
            .await
            .map_err(|e| CrossCheckError::HandshakeFailed(format!("write frame: {e}")))?;

        let body = timeout(DESCRIBE_TIMEOUT, reader.next_frame())
            .await
            .map_err(|_| {
                CrossCheckError::HandshakeFailed(format!(
                    "tool.describe_capabilities response for '{method}' timed out after 5s"
                ))
            })?
            .map_err(|e| CrossCheckError::HandshakeFailed(format!("read frame: {e}")))?
            .ok_or_else(|| {
                CrossCheckError::HandshakeFailed(
                    "EOF while reading tool.describe_capabilities response".to_string(),
                )
            })?;

        let frame = Frame::decode(&body)
            .map_err(|e| CrossCheckError::HandshakeFailed(format!("decode frame: {e}")))?;

        match frame {
            Frame::Response {
                error: Some(env), ..
            } => {
                // Warn rather than hard-fail: plugins predating priority 12 may
                // not implement `tool.describe_capabilities`. Upgrading to a
                // hard-fail is tracked as a separate plan item. A malicious
                // plugin could exploit this to hide capabilities, so the warn
                // level ensures the gap is visible in logs.
                tracing::warn!(
                    method = %method,
                    code = env.code,
                    message = %env.message,
                    "tool.describe_capabilities returned RPC error — skipping method \
                     (backward-compat tolerance for pre-priority-12 plugins)"
                );
            }
            Frame::Response {
                result: Some(bytes),
                ..
            } => {
                let caps: Vec<Capability> = rmp_serde::from_slice(&bytes).map_err(|e| {
                    CrossCheckError::HandshakeFailed(format!(
                        "decode Vec<Capability> from tool.describe_capabilities: {e}"
                    ))
                })?;
                all.extend(caps);
            }
            Frame::Response {
                error: None,
                result: None,
                ..
            } => {
                tracing::debug!(
                    method = %method,
                    "tool.describe_capabilities returned empty response — skipping"
                );
            }
            other => {
                tracing::debug!(
                    method = %method,
                    frame = ?other,
                    "unexpected frame type for tool.describe_capabilities — skipping"
                );
            }
        }
    }

    // Deduplicate via PartialEq (Capability is PartialEq but not Hash; O(n²)
    // is fine for the small counts we expect, and avoids relying on Debug
    // stability as a dedup key).
    let mut deduped: Vec<Capability> = Vec::new();
    for cap in all {
        if !deduped.contains(&cap) {
            deduped.push(cap);
        }
    }

    Ok(deduped)
}

/// Bidirectional capability set-diff. Returns the first divergence found.
///
/// * [`CrossCheckError::BinaryClaimsExtra`] — binary has a capability the
///   manifest does not declare.
/// * [`CrossCheckError::ManifestDeclaresUnused`] — manifest declares a
///   capability the binary never requests.
///
/// O(n*m); fine for the small capability counts we expect (typically < 10 per plugin).
fn diff_capabilities(
    plugin_name: &str,
    binary: &[Capability],
    manifest: &[Capability],
) -> Result<(), CrossCheckError> {
    for claimed in binary {
        if !manifest.contains(claimed) {
            return Err(CrossCheckError::BinaryClaimsExtra {
                plugin: plugin_name.to_string(),
                claimed: claimed.clone(),
            });
        }
    }
    for declared in manifest {
        if !binary.contains(declared) {
            return Err(CrossCheckError::ManifestDeclaresUnused {
                plugin: plugin_name.to_string(),
                declared: declared.clone(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deserialize a `Capability` from a JSON literal.
    /// Required because `FsCapability::Read` et al. are `#[non_exhaustive]`
    /// variants — struct-expression construction is blocked outside the
    /// defining crate (E0639).
    fn cap_from_json(json: &str) -> Capability {
        serde_json::from_str(json).expect("valid capability JSON")
    }

    fn fs_read() -> Capability {
        cap_from_json(r#"{"kind":"fs.read","paths":["/tmp/**"]}"#)
    }

    fn net_http() -> Capability {
        cap_from_json(r#"{"kind":"net.http","hosts":["api.example.com"],"methods":["GET"]}"#)
    }

    // ── error display ─────────────────────────────────────────────────────────

    #[test]
    fn cross_check_error_display_includes_plugin_name() {
        let err = CrossCheckError::BinaryClaimsExtra {
            plugin: "evil-plugin".to_string(),
            claimed: fs_read(),
        };
        let msg = err.to_string();
        assert!(
            msg.contains("evil-plugin"),
            "message should contain plugin name: {msg}"
        );
        assert!(
            msg.contains("Filesystem"),
            "message should contain capability type: {msg}"
        );
    }

    // ── diff_capabilities unit tests ──────────────────────────────────────────

    #[test]
    fn diff_capabilities_returns_ok_when_match() {
        let cap = fs_read();
        let cap2 = fs_read();
        let result = diff_capabilities("my-plugin", &[cap], &[cap2]);
        assert!(result.is_ok());
    }

    #[test]
    fn diff_capabilities_rejects_binary_claims_extra() {
        let cap = fs_read();
        let result = diff_capabilities("my-plugin", &[cap], &[]);
        assert!(
            matches!(result, Err(CrossCheckError::BinaryClaimsExtra { .. })),
            "expected BinaryClaimsExtra, got {result:?}"
        );
    }

    #[test]
    fn diff_capabilities_rejects_manifest_declares_unused() {
        let cap = fs_read();
        let result = diff_capabilities("my-plugin", &[], &[cap]);
        assert!(
            matches!(result, Err(CrossCheckError::ManifestDeclaresUnused { .. })),
            "expected ManifestDeclaresUnused, got {result:?}"
        );
    }

    #[test]
    fn diff_capabilities_returns_ok_on_dual_empty() {
        let result = diff_capabilities("my-plugin", &[], &[]);
        assert!(result.is_ok());
    }

    // ── non_exhaustive discipline ─────────────────────────────────────────────

    #[test]
    fn cross_check_error_is_non_exhaustive_via_match() {
        // In-crate tests can exhaust the match without a wildcard (same crate,
        // so the compiler sees all current variants). The real proof that
        // `#[non_exhaustive]` is present is that external crates CANNOT omit
        // the wildcard. Here we add `#[allow(unreachable_patterns)]` to keep the
        // wildcard arm as documentation of that contract without a compile error.
        let err = CrossCheckError::SpawnFailed("bad".to_string());
        #[allow(unreachable_patterns)]
        let _handled = match err {
            CrossCheckError::SpawnFailed(_) => "spawn",
            CrossCheckError::HandshakeFailed(_) => "handshake",
            CrossCheckError::BinaryClaimsExtra { .. } => "extra",
            CrossCheckError::ManifestDeclaresUnused { .. } => "unused",
            _ => "future-variant",
        };
    }

    // ── dedup / order independence ────────────────────────────────────────────

    #[test]
    fn diff_capabilities_dedup_safe() {
        // Binary has two identical entries; manifest has one.
        // After dedup (done in enumerate_tool_capabilities) the diff helper
        // sees [cap, cap] from binary. Since diff iterates and checks
        // manifest.contains() each time, duplicates still pass — they just
        // check twice. Verify Ok.
        let cap = fs_read();
        let result = diff_capabilities("my-plugin", &[cap.clone(), cap.clone()], &[cap]);
        assert!(result.is_ok());
    }

    #[test]
    fn diff_capabilities_order_independent() {
        let a = fs_read();
        let b = net_http();
        // binary: [b, a], manifest: [a, b] — different order, same set
        let result = diff_capabilities("my-plugin", &[b.clone(), a.clone()], &[a, b]);
        assert!(result.is_ok());
    }
}
