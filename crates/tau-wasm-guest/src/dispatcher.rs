//! In-guest `ToolDispatcher` for the E2 cassette-only scenario: no tools,
//! a single host-backed LLM backend, and host-backed clock/random for
//! determinism.

extern crate alloc;

use alloc::boxed::Box;
use alloc::format;
use alloc::string::ToString;
use alloc::sync::Arc;
use core::future::Future;
use core::pin::Pin;

use serde_json::Value;

use tau_ir::{IrModule, ToolId, ToolImpl};
use tau_ports::{Clock, RandomSource};
use tau_runtime_core::builder::DynLlmBackend;
use tau_runtime_core::error::RuntimeError;
use tau_runtime_core::interpreter::tool_dispatch::{ToolDispatcher, ToolInvocationResult};

pub struct GuestDispatcher {
    backend: Arc<dyn DynLlmBackend>,
    clock: Arc<dyn Clock>,
    random: Arc<dyn RandomSource>,
    module: Arc<IrModule>,
}

impl GuestDispatcher {
    pub fn new(
        backend: Arc<dyn DynLlmBackend>,
        clock: Arc<dyn Clock>,
        random: Arc<dyn RandomSource>,
        module: Arc<IrModule>,
    ) -> Self {
        Self {
            backend,
            clock,
            random,
            module,
        }
    }

    /// Resolve a tool-ref id to its declared native fn name (the stable
    /// contract), e.g. `[tools.fetch] native = "Fetch"` → `"Fetch"`. The
    /// wasi-backed effect arm keys on THIS, not the arbitrary tool-ref key.
    fn native_fn_name(&self, tool_id: &ToolId) -> Option<&str> {
        match &self.module.workflow.tools.get(tool_id)?.impl_ {
            ToolImpl::Native { fn_ref, .. } => Some(fn_ref.name.as_str()),
            _ => None,
        }
    }
}

impl ToolDispatcher for GuestDispatcher {
    fn invoke<'a>(
        &'a self,
        tool_id: &'a ToolId,
        args: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolInvocationResult, RuntimeError>> + Send + 'a>> {
        let name = tool_id.0.clone();
        let native = self.native_fn_name(tool_id).map(|s| s.to_string());
        let args_owned = args.clone();
        Box::pin(async move {
            // 3.6 net effect: a tool declared `native = "Fetch"` routes through
            // wasi:http when net.http was granted (the cfg gate). Enforcement is
            // the HOST WasiCtx/EgressPolicy (3.3/3.4) — NOT an in-guest gate.
            #[cfg(tau_cap_net_http)]
            if native.as_deref() == Some("Fetch") {
                return match fetch_via_wasi(&args_owned) {
                    Ok(body) => Ok(ToolInvocationResult {
                        body: Some(body),
                        error: None,
                    }),
                    Err(msg) => Ok(ToolInvocationResult {
                        body: None,
                        error: Some(msg),
                    }),
                };
            }
            // 3.6-b fs effects: a tool declared `native = "Read"`/`"Write"`
            // routes through wasi:filesystem when fs.* was granted (the cfg
            // gate). Enforcement is the HOST preopen set + open-at error-codes
            // (3.3/3.4) — NOT an in-guest gate.
            #[cfg(tau_cap_fs_read)]
            if native.as_deref() == Some("Read") {
                return match fs_read_via_wasi(&args_owned) {
                    Ok(body) => Ok(ToolInvocationResult {
                        body: Some(body),
                        error: None,
                    }),
                    Err(msg) => Ok(ToolInvocationResult {
                        body: None,
                        error: Some(msg),
                    }),
                };
            }
            #[cfg(tau_cap_fs_write)]
            if native.as_deref() == Some("Write") {
                return match fs_write_via_wasi(&args_owned) {
                    Ok(body) => Ok(ToolInvocationResult {
                        body: Some(body),
                        error: None,
                    }),
                    Err(msg) => Ok(ToolInvocationResult {
                        body: None,
                        error: Some(msg),
                    }),
                };
            }
            let _ = &native; // silence unused when the cfg arm is compiled out

            match tau_native_tools::invoke(&name, &args_owned) {
                Some(body) => Ok(ToolInvocationResult {
                    body: Some(body),
                    error: None,
                }),
                None => Err(RuntimeError::Internal {
                    message: format!("tau-wasm-guest: unknown native tool `{name}`"),
                }),
            }
        })
    }

    fn llm_backend_for(&self, _backend: &str) -> Result<Arc<dyn DynLlmBackend>, RuntimeError> {
        Ok(self.backend.clone())
    }

    fn clock(&self) -> Option<Arc<dyn Clock>> {
        Some(self.clock.clone())
    }

    fn random(&self) -> Option<Arc<dyn RandomSource>> {
        Some(self.random.clone())
    }

    fn deterministic_registry(
        &self,
    ) -> Option<Arc<dyn tau_runtime_core::interpreter::deterministic::DeterministicRegistry>> {
        // #689: when the baked IR reaches no goal predicate at all, the
        // registry is never constructed and `goal_registry` is not compiled,
        // so nothing references `tau_native_tools::goal_predicates` and
        // wasm-ld drops the regex engine with it.
        //
        // `None` here is a genuine can't-happen for a build whose IR needs a
        // predicate, not a silent fallback: `build.rs` derives the cfg from
        // the same IR that is baked in, and the interpreter turns a missing
        // registry into a hard error ("branch … needs a deterministic
        // registry") rather than a default verdict.
        #[cfg(tau_goal_predicates)]
        {
            Some(Arc::new(crate::goal_registry::GuestGoalRegistry))
        }
        #[cfg(not(tau_goal_predicates))]
        {
            None
        }
    }
}

/// Issue one outgoing HTTP request through the generated wasi:http bindings.
/// A host `EgressPolicy` denial (ungranted host/method) surfaces as
/// `Err("<ErrorCode>")` carrying the exact wasi:http error code (e.g.
/// `HttpRequestDenied`) — asserted by the round-trip test. Never panics.
#[cfg(tau_cap_net_http)]
fn fetch_via_wasi(args: &Value) -> Result<Value, alloc::string::String> {
    use crate::wit_wasi::http::outgoing_handler;
    use crate::wit_wasi::http::types::{Fields, Method, OutgoingRequest, Scheme};
    use alloc::string::String;
    use alloc::vec::Vec;

    let url = args
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| "Fetch: missing string arg `url`".to_string())?;
    let method_str = args.get("method").and_then(Value::as_str).unwrap_or("GET");

    let (scheme, rest) = if let Some(r) = url.strip_prefix("https://") {
        (Scheme::Https, r)
    } else if let Some(r) = url.strip_prefix("http://") {
        (Scheme::Http, r)
    } else {
        return Err(format!("Fetch: unsupported url scheme: {url}"));
    };
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let method = match method_str {
        "GET" => Method::Get,
        "HEAD" => Method::Head,
        "POST" => Method::Post,
        "PUT" => Method::Put,
        "DELETE" => Method::Delete,
        "PATCH" => Method::Patch,
        other => Method::Other(other.to_string()),
    };

    let request = OutgoingRequest::new(Fields::new());
    request
        .set_method(&method)
        .map_err(|()| "Fetch: set_method rejected".to_string())?;
    request
        .set_scheme(Some(&scheme))
        .map_err(|()| "Fetch: set_scheme rejected".to_string())?;
    request
        .set_authority(Some(authority))
        .map_err(|()| "Fetch: set_authority rejected".to_string())?;
    request
        .set_path_with_query(Some(path))
        .map_err(|()| "Fetch: set_path rejected".to_string())?;

    // Host WasiHttpHooks::send_request runs here; a denied host/method returns
    // before any socket is opened.
    let future = outgoing_handler::handle(request, None).map_err(|code| format!("{code:?}"))?;
    let pollable = future.subscribe();
    pollable.block();
    let response = match future.get() {
        Some(Ok(Ok(resp))) => resp,
        Some(Ok(Err(code))) => return Err(format!("{code:?}")),
        Some(Err(())) => return Err("Fetch: future already consumed".to_string()),
        None => return Err("Fetch: no result after block".to_string()),
    };
    let status = response.status();

    // Response-body read (offline-untested; needs real connectivity).
    let body = response
        .consume()
        .map_err(|()| "Fetch: consume body".to_string())?;
    let stream = body
        .stream()
        .map_err(|()| "Fetch: body stream".to_string())?;
    use crate::wit_wasi::io::streams::StreamError;
    let mut buf: Vec<u8> = Vec::new();
    loop {
        match stream.blocking_read(8192) {
            Ok(chunk) => buf.extend_from_slice(&chunk),
            // Normal EOF: the peer signalled end-of-body. Return the
            // accumulated bytes as a complete response.
            Err(StreamError::Closed) => break,
            // A real mid-transfer transport failure — surface it instead of
            // silently returning a truncated body with the response status.
            Err(StreamError::LastOperationFailed(err)) => {
                return Err(format!(
                    "Fetch: body read failed: {}",
                    err.to_debug_string()
                ));
            }
        }
    }
    let body_str = String::from_utf8_lossy(&buf).into_owned();

    Ok(serde_json::json!({ "status": status, "body": body_str }))
}

/// Resolve `path` against the host's preopen set (`get-directories`) and return
/// the `(preopen-descriptor, relative-path)` to `open-at` from. Pure descriptor
/// plumbing over HOST-provided state — NOT a capability check. `None` means the
/// host granted no preopen containing `path` (absence of capability); the caller
/// surfaces `FsAccessDenied`. Selection is the longest-prefix, root-aware match
/// of [`crate::preopen::select_preopen`] (#604): overlapping grants bind the
/// most specific preopen, and a `/` preopen serves every absolute path. Does
/// not touch `..` — the host `open-at` rejects escapes.
#[cfg(any(tau_cap_fs_read, tau_cap_fs_write))]
fn resolve_preopen(
    path: &str,
) -> Option<(
    crate::wit_wasi::filesystem::types::Descriptor,
    alloc::string::String,
)> {
    use crate::wit_wasi::filesystem::preopens::get_directories;
    let dirs = get_directories();
    let (idx, rel) = crate::preopen::select_preopen(path, dirs.iter().map(|(_, g)| g.as_str()))?;
    let rel = rel.to_string();
    dirs.into_iter().nth(idx).map(|(desc, _)| (desc, rel))
}

/// `Read`: `{path} → {content, bytes}`. A no-preopen path → `FsAccessDenied`
/// (host granted no descriptor). A host `open-at`/stream failure → `Err(code)`.
/// Never panics.
#[cfg(tau_cap_fs_read)]
fn fs_read_via_wasi(args: &Value) -> Result<Value, alloc::string::String> {
    use crate::wit_wasi::filesystem::types::{DescriptorFlags, OpenFlags, PathFlags};
    use alloc::string::String;
    use alloc::vec::Vec;

    let path = args
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| "Read: missing string arg `path`".to_string())?;

    let (dir, rel) =
        resolve_preopen(path).ok_or_else(|| format!("FsAccessDenied: no preopen grants {path}"))?;

    let file = dir
        .open_at(
            PathFlags::SYMLINK_FOLLOW,
            &rel,
            OpenFlags::empty(),
            DescriptorFlags::READ,
        )
        .map_err(|code| format!("{code:?}"))?;
    let stream = file
        .read_via_stream(0)
        .map_err(|code| format!("{code:?}"))?;
    let mut buf: Vec<u8> = Vec::new();
    // Closed / stream error → end of file.
    while let Ok(chunk) = stream.blocking_read(4096) {
        if chunk.is_empty() {
            break;
        }
        buf.extend_from_slice(&chunk);
    }
    let content = String::from_utf8_lossy(&buf).into_owned();
    Ok(serde_json::json!({ "bytes": buf.len(), "content": content }))
}

/// `Write`: `{path, content} → {bytes}`. Requires an fs.write-granted (RW)
/// preopen; a write to a read-only preopen fails at the host `open-at`. Never
/// panics.
///
/// Write REPLACES the file: `open-at` passes `CREATE | TRUNCATE`, so
/// overwriting a longer existing file leaves no stale tail (#604). The tool
/// contract is full-content replace — append semantics would be a distinct
/// future tool, not a flag on this one.
#[cfg(tau_cap_fs_write)]
fn fs_write_via_wasi(args: &Value) -> Result<Value, alloc::string::String> {
    use crate::wit_wasi::filesystem::types::{DescriptorFlags, OpenFlags, PathFlags};

    let path = args
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| "Write: missing string arg `path`".to_string())?;
    let content = args
        .get("content")
        .and_then(Value::as_str)
        .ok_or_else(|| "Write: missing string arg `content`".to_string())?;

    let (dir, rel) =
        resolve_preopen(path).ok_or_else(|| format!("FsAccessDenied: no preopen grants {path}"))?;

    let file = dir
        .open_at(
            PathFlags::SYMLINK_FOLLOW,
            &rel,
            OpenFlags::CREATE | OpenFlags::TRUNCATE,
            DescriptorFlags::WRITE,
        )
        .map_err(|code| format!("{code:?}"))?;
    let stream = file
        .write_via_stream(0)
        .map_err(|code| format!("{code:?}"))?;
    // blocking-write-and-flush permits ≤4096 bytes per call; chunk defensively.
    let bytes = content.as_bytes();
    for chunk in bytes.chunks(4096) {
        stream
            .blocking_write_and_flush(chunk)
            .map_err(|e| format!("Write: {e:?}"))?;
    }
    Ok(serde_json::json!({ "bytes": bytes.len() }))
}
