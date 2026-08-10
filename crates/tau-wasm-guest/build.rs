//! Bakes the workflow IR into the guest. `tau build wasm` (and the host
//! roundtrip test) set `TAU_IR_BYTES` to a file of canonical IR bytes; this
//! copies it to `$OUT_DIR/baked_ir.bin`, which `src/baked.rs` `include_bytes!`s.
//! When unset (standalone smoke build) an empty file is written, and the guest
//! `run` returns its error arm.
//!
//! Also assembles `wit-gen/`, a self-contained WIT resolution root that
//! `guest.rs`'s `wit_bindgen::generate!` points `path` at. `wit_bindgen`
//! resolves a single directory tree, but the three WIT sources the guest
//! depends on live in different places: the vendored WASI deps
//! (`wit/deps/`, committed), the frozen host contract
//! (workspace-root `wit/tau-host.wit`), and the capability-derived world
//! (`TAU_WORLD_WIT`, set per-build by `tau build wasm`, or the committed
//! empty-cap baseline for standalone/CI builds). `wit-gen/` is gitignored
//! and rebuilt fresh on every `cargo build`.

use std::path::{Path, PathBuf};

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

    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("set by cargo"));
    let wit_gen = manifest.join("wit-gen");
    let wit_gen_deps = wit_gen.join("deps");
    // Wipe any stale wit-gen/ tree before reassembling it fresh, so a since-
    // removed vendored dep (or a stale runner.wit) can never linger and shadow
    // this build's copy.
    let _ = std::fs::remove_dir_all(&wit_gen);
    std::fs::create_dir_all(&wit_gen_deps).expect("mkdir wit-gen/deps");

    // Vendored WASI deps (committed under the crate's own `wit/deps/`).
    copy_dir_contents(&manifest.join("wit/deps"), &wit_gen_deps);

    // Frozen host contract (`interface host`), lives at the workspace root.
    // Must live in its own `deps/` subdirectory (its own package, `tau:host`,
    // distinct from `tau:generated` in `runner.wit` at the root) — wit-parser
    // requires every `.wit` file directly in the root to share one package
    // identity, and treats each subdirectory as a separate dependency
    // package (the same mechanism the vendored `wasi:*` deps rely on).
    let host_wit_dir = wit_gen_deps.join("tau-host");
    std::fs::create_dir_all(&host_wit_dir).expect("mkdir wit-gen/deps/tau-host");
    let host_wit_src = manifest.join("../../wit/tau-host.wit");
    println!("cargo:rerun-if-changed={}", host_wit_src.display());
    std::fs::copy(&host_wit_src, host_wit_dir.join("tau-host.wit"))
        .unwrap_or_else(|e| panic!("copying {}: {e}", host_wit_src.display()));

    // Capability-derived world: from `TAU_WORLD_WIT` if set (dynamic
    // injection, Task 5), else the committed empty-cap baseline (standalone
    // / CI builds, no WASI imports).
    println!("cargo:rerun-if-env-changed=TAU_WORLD_WIT");
    let world = match std::env::var_os("TAU_WORLD_WIT") {
        Some(path) => {
            let path = PathBuf::from(path);
            println!("cargo:rerun-if-changed={}", path.display());
            std::fs::read(&path)
                .unwrap_or_else(|e| panic!("reading TAU_WORLD_WIT {}: {e}", path.display()))
        }
        None => {
            let base = manifest.join("wit-baseline/runner.wit");
            println!("cargo:rerun-if-changed={}", base.display());
            std::fs::read(&base).expect("reading wit-baseline/runner.wit")
        }
    };
    std::fs::write(wit_gen.join("runner.wit"), &world).expect("writing wit-gen/runner.wit");

    // 3.6: the guest's net-effect arm (dispatcher.rs) is cfg-gated on whether
    // the capability-derived world grants wasi:http. When it does, the arm is
    // compiled and statically reachable from `run`, so the wasi:http import
    // survives wasm-ld DCE (binary-observable). When it doesn't, the arm is
    // absent and no wasi:http binding is referenced (the world has no wasi:http
    // to generate bindings from anyway). The check-cfg is unconditional so the
    // guest compiles cleanly on every target/world without an `unexpected cfg`
    // warning (workspace lints are -D warnings).
    println!("cargo:rustc-check-cfg=cfg(tau_cap_net_http)");
    if String::from_utf8_lossy(&world).contains("wasi:http") {
        println!("cargo:rustc-cfg=tau_cap_net_http");
    }
}

/// Recursively copy the contents of `src` into `dst` (both assumed to
/// exist / be creatable), emitting `rerun-if-changed` for every file
/// visited so edits to the vendored WASI deps invalidate the build.
fn copy_dir_contents(src: &Path, dst: &Path) {
    let entries =
        std::fs::read_dir(src).unwrap_or_else(|e| panic!("reading dir {}: {e}", src.display()));
    for entry in entries {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        let dest_path = dst.join(entry.file_name());
        println!("cargo:rerun-if-changed={}", path.display());
        if path.is_dir() {
            std::fs::create_dir_all(&dest_path).expect("mkdir");
            copy_dir_contents(&path, &dest_path);
        } else {
            std::fs::copy(&path, &dest_path).unwrap_or_else(|e| {
                panic!("copying {} -> {}: {e}", path.display(), dest_path.display())
            });
        }
    }
}
