//! A minimal WASI 0.2 component that exercises host WasiCtx enforcement:
//! its `run(path)` attempts `std::fs::read(path)` and reports the outcome.
//! Built only for `wasm32-wasip2` by `tests/wasi_fs_enforcement.rs`.

wit_bindgen::generate!({
    world: "runner",
    path: "../../../../../wit",
});

struct Component;

impl Guest for Component {
    fn run(path: String) -> Result<String, String> {
        match std::fs::read(&path) {
            Ok(bytes) => Ok(format!("read {} bytes", bytes.len())),
            Err(e) => Err(format!("denied: {e}")),
        }
    }
}

export!(Component);
