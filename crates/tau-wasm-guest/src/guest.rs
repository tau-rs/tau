//! The WASI 0.2 component body (wasm32 only).
//!
//! PR-C milestone: `run` returns a hardcoded `Ok` to prove the
//! `wit-bindgen` + component toolchain end to end, including the release
//! profile (where the `_rdl_*` allocator-symbol LTO bug would surface).
//! Driving the baked IR through `run_ir_streaming` lands in a follow-up.

extern crate alloc;

use alloc::string::{String, ToString};

wit_bindgen::generate!({
    world: "runner",
    path: "../../wit",
});

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

struct Component;

impl Guest for Component {
    fn run(_prompt: String) -> Result<String, String> {
        // Milestone 1: prove the toolchain. Real IR execution follows.
        Ok("{}".to_string())
    }
}

export!(Component);
