//! The WASI 0.2 component body (wasm32 only).
//!
//! PR-C milestone: `run` returns a hardcoded `Ok` to prove the
//! `wit-bindgen` + component toolchain end to end, including the release
//! profile (where the `_rdl_*` allocator-symbol LTO bug would surface).
//! Driving the baked IR through `run_ir_streaming` lands in a follow-up.

extern crate alloc;

use alloc::string::{String, ToString};

wit_bindgen::generate!({
    world: "tau:generated/runner",
    path: "wit-gen",
    generate_all,
});

/// Re-export the WIT-generated host imports so sibling modules (host_ports.rs)
/// can access them without knowing the exact generated module path.
/// The path `tau::host::host` is what wit_bindgen generates for `import host`
/// in the `tau:host` package's `runner` world (identical to the wasmtime host
/// side which uses `tau::host::host`). `wit-gen/` is assembled by `build.rs`
/// from the vendored WASI deps, the frozen `tau:host` contract, and the
/// capability-derived (or baseline) `tau:generated` world; `generate_all` is
/// required once the world imports WASI interfaces (`wit_bindgen` otherwise
/// errors with "missing `with` mapping" for interfaces reachable both
/// directly and transitively, e.g. `wasi:io/poll`).
pub(crate) mod wit_host {
    pub(crate) use super::tau::host::host::*;
}

/// dlmalloc is a portable no_std allocator; the guest has no std heap.
#[global_allocator]
static ALLOC: dlmalloc::GlobalDlmalloc = dlmalloc::GlobalDlmalloc;

/// no_std requires a panic handler; trap (the component aborts).
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}

/// On `wasm32-wasip2`, wit-bindgen leaves `cabi_realloc` to std
/// (it gates its own shim behind `not(target_env = "p2")`). A no_std
/// component must export the canonical-ABI realloc itself, over the
/// global allocator.
#[export_name = "cabi_realloc"]
unsafe extern "C" fn cabi_realloc(
    old_ptr: *mut u8,
    old_len: usize,
    align: usize,
    new_len: usize,
) -> *mut u8 {
    use alloc::alloc::{alloc, dealloc, realloc};
    use core::alloc::Layout;
    if new_len == 0 {
        if old_len != 0 {
            dealloc(old_ptr, Layout::from_size_align_unchecked(old_len, align));
        }
        return align as *mut u8;
    }
    let new_layout = Layout::from_size_align_unchecked(new_len, align);
    let ptr = if old_len == 0 {
        alloc(new_layout)
    } else {
        realloc(
            old_ptr,
            Layout::from_size_align_unchecked(old_len, align),
            new_len,
        )
    };
    if ptr.is_null() {
        core::arch::wasm32::unreachable();
    }
    ptr
}

/// `memcmp` for the no_std wasip2 component.
///
/// `wasm32-wasip2` expects libc to supply the `mem*` intrinsics, but a no_std
/// cdylib links no libc — leaving `memcmp` as an unresolved `env` import that
/// the component encoder rejects (`failed to resolve import env::memcmp`).
/// `compiler_builtins` provides `memcpy`/`memset`/`memmove` on this target but
/// not `memcmp`, so the guest fills just this one. Byte-wise (no slice ops, to
/// avoid lowering back into an intrinsic call).
#[no_mangle]
unsafe extern "C" fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    let mut i = 0;
    while i < n {
        let x = *a.add(i);
        let y = *b.add(i);
        if x != y {
            return x as i32 - y as i32;
        }
        i += 1;
    }
    0
}

struct Component;

impl Guest for Component {
    fn run(_prompt: String) -> Result<String, String> {
        use alloc::sync::Arc;
        use alloc::vec::Vec;

        let bytes = crate::baked::BAKED_IR;
        if bytes.is_empty() {
            return Err("tau-wasm-guest: no baked IR".to_string());
        }
        let module = tau_ir::from_canonical_bytes(bytes).map_err(|e| e.to_string())?;

        // The guest drives a single entry agent (run_ir_streaming below), not
        // the pipeline executor (run_pipeline). Reject any pipeline-bearing IR
        // — Branch/Parallel/Loop and linear pipelines alike — at load rather
        // than silently running only the entry agent and skipping the pipeline.
        // Driving run_pipeline in-wasm is a follow-up slice.
        if module.workflow.pipeline.is_some() {
            return Err(
                "tau-wasm-guest: pipelines (incl. Branch) are not yet executed in-wasm".to_string(),
            );
        }

        // E2 scope: exactly one agent; it is the entry.
        if module.workflow.agents.len() != 1 {
            return Err(alloc::format!(
                "tau-wasm-guest: E2 supports exactly one agent, found {}",
                module.workflow.agents.len()
            ));
        }
        let entry = module
            .workflow
            .agents
            .keys()
            .next()
            .expect("len checked == 1")
            .clone();

        let backend: Arc<dyn tau_runtime_core::builder::DynLlmBackend> =
            Arc::new(crate::host_ports::HostLlmBackend);
        let clock: Arc<dyn tau_ports::Clock> = Arc::new(crate::host_ports::HostClock);
        let random: Arc<dyn tau_ports::RandomSource> = Arc::new(crate::host_ports::HostRandom);
        let dispatcher = Arc::new(crate::dispatcher::GuestDispatcher::new(
            backend, clock, random,
        ));

        let module = Arc::new(module);
        let stream = crate::executor::block_on(tau_runtime_core::interpreter::run_ir_streaming(
            module,
            &entry,
            dispatcher,
            Vec::new(),
        ))
        .map_err(|e| e.to_string())?;

        let events = crate::executor::collect_stream(stream);
        serde_json::to_string(&events).map_err(|e| e.to_string())
    }
}

export!(Component);
