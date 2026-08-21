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
    let mut buf: Vec<u8> = Vec::new();
    // Closed / stream error → end of body.
    while let Ok(chunk) = stream.blocking_read(8192) {
        buf.extend_from_slice(&chunk);
    }
    let body_str = String::from_utf8_lossy(&buf).into_owned();

    Ok(serde_json::json!({ "status": status, "body": body_str }))
}
