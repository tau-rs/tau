//! A minimal WASI 0.2 component that exercises host `wasi:http` egress
//! enforcement: its `run(input)` issues one outgoing request and reports
//! whether the host let it through.
//!
//! `input` is `"<METHOD> <authority>"`, e.g. `"GET blocked.invalid"`. The
//! probe returns `Ok("ok")` if the request was dispatched and `Err("denied:
//! …")` if the host's `EgressPolicy` rejected it before a socket was opened.
//! Built only for `wasm32-wasip2` by `tests/wasi_http_enforcement.rs`.

use wasi::http::outgoing_handler;
use wasi::http::types::{Fields, Method, OutgoingRequest, Scheme};

wit_bindgen::generate!({
    world: "runner",
    path: "../../../../../wit",
});

struct Component;

fn method_from_str(s: &str) -> Method {
    match s {
        "GET" => Method::Get,
        "HEAD" => Method::Head,
        "POST" => Method::Post,
        "PUT" => Method::Put,
        "DELETE" => Method::Delete,
        "PATCH" => Method::Patch,
        other => Method::Other(other.to_string()),
    }
}

impl Guest for Component {
    fn run(input: String) -> Result<String, String> {
        // Parse "<METHOD> <authority>".
        let mut parts = input.splitn(2, ' ');
        let method = method_from_str(parts.next().unwrap_or("GET"));
        let authority = parts.next().unwrap_or("").to_string();

        let request = OutgoingRequest::new(Fields::new());
        request
            .set_method(&method)
            .map_err(|()| "set_method rejected".to_string())?;
        request
            .set_scheme(Some(&Scheme::Https))
            .map_err(|()| "set_scheme rejected".to_string())?;
        request
            .set_authority(Some(&authority))
            .map_err(|()| "set_authority rejected".to_string())?;
        request
            .set_path_with_query(Some("/"))
            .map_err(|()| "set_path rejected".to_string())?;

        // The host's `WasiHttpHooks::send_request` runs during dispatch. A denied
        // authority/method surfaces either as a synchronous `handle` error or as
        // the future resolving to an `ErrorCode` — both mean "no socket opened".
        let future = match outgoing_handler::handle(request, None) {
            Ok(future) => future,
            Err(code) => return Err(format!("denied: {code:?}")),
        };
        let pollable = future.subscribe();
        pollable.block();
        match future.get() {
            Some(Ok(Ok(_response))) => Ok("ok".to_string()),
            Some(Ok(Err(code))) => Err(format!("denied: {code:?}")),
            Some(Err(())) => Err("denied: future already consumed".to_string()),
            None => Err("denied: no result after block".to_string()),
        }
    }
}

export!(Component);
