//! Bakes the workflow IR into the guest. `tau build wasm` (and the host
//! roundtrip test) set `TAU_IR_BYTES` to a file of canonical IR bytes; this
//! copies it to `$OUT_DIR/baked_ir.bin`, which `src/baked.rs` `include_bytes!`s.
//! When unset (standalone smoke build) an empty file is written, and the guest
//! `run` returns its error arm.

use std::path::PathBuf;

fn main() {
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR set by cargo"));
    let dest = out.join("baked_ir.bin");

    println!("cargo:rerun-if-env-changed=TAU_IR_BYTES");
    match std::env::var_os("TAU_IR_BYTES") {
        Some(path) => {
            let path = PathBuf::from(path);
            println!("cargo:rerun-if-changed={}", path.display());
            let bytes = std::fs::read(&path)
                .unwrap_or_else(|e| panic!("reading TAU_IR_BYTES {}: {e}", path.display()));
            std::fs::write(&dest, bytes).expect("writing baked_ir.bin");
        }
        None => {
            std::fs::write(&dest, []).expect("writing empty baked_ir.bin");
        }
    }
}
