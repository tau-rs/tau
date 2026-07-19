//! Freezes the minimal WIT host surface (EPIC 2.3, ADR-0056) and proves it stays
//! in correspondence with the `tau-ports` traits it projects.
//!
//! Signature drift between these 3 functions and their ports already breaks
//! compilation via `tau-wasm-guest/src/host_ports.rs` (the `LlmBackend`/`Clock`/
//! `RandomSource` impls over the WIT-generated imports). THIS test freezes the
//! *set* and *shape* of the host surface so growing it is a deliberate,
//! test-breaking act. The `run` export payload is intentionally NOT frozen.
//!
//! # wit-parser 0.251 API notes (deviations from the brief)
//!
//! - `Function.params` is `Vec<Param>` (not `Vec<(String, Type)>`); each
//!   `Param` has a `.name: String` field.
//! - `world.imports` / `world.exports` keys are `WorldKey` which is either
//!   `WorldKey::Name(String)` or `WorldKey::Interface(InterfaceId)`. An
//!   `import host;` that refers to a named interface in the same package is
//!   stored as `WorldKey::Interface(id)`, NOT `WorldKey::Name("host")`.
//!   `Resolve::name_world_key` resolves either variant to a human-readable
//!   name (e.g. `"tau:host/host@0.1.0"`), so we use that rather than
//!   `format!("{k:?}")` to reliably detect "host" and "run".

use std::collections::BTreeSet;
use std::path::PathBuf;
use wit_parser::Resolve;

/// The single Rust declaration of the host-crossing surface: each WIT host
/// function and the `tau-ports` trait it projects. The `.wit` is checked
/// against this.
const HOST_PORT_REGISTRY: &[(&str, &str)] = &[
    ("complete", "LlmBackend"),
    ("now-millis", "Clock"),
    ("next-u64", "RandomSource"),
];

fn wit_path() -> PathBuf {
    // CARGO_MANIFEST_DIR = crates/tau-wasm-host ; repo root is two levels up.
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../wit/tau-host.wit")
}

fn load() -> Resolve {
    let mut resolve = Resolve::new();
    // tau-host.wit is self-contained (no package deps), so push_file suffices.
    resolve
        .push_file(wit_path())
        .expect("parse wit/tau-host.wit");
    resolve
}

#[test]
fn package_is_tau_host_0_1_0() {
    let resolve = load();
    let pkg = resolve.packages.iter().next().expect("one package").1;
    assert_eq!(pkg.name.namespace, "tau");
    assert_eq!(pkg.name.name, "host");
    assert_eq!(
        pkg.name.version.as_ref().map(|v| v.to_string()),
        Some("0.1.0".to_string()),
        "embedding-contract version (ADR-0056) must stay tau:host@0.1.0"
    );
}

#[test]
fn host_interface_is_frozen_to_the_three_functions() {
    let resolve = load();
    let host = resolve
        .interfaces
        .iter()
        .find(|(_, i)| i.name.as_deref() == Some("host"))
        .map(|(_, i)| i)
        .expect("`host` interface present");

    let got: BTreeSet<&str> = host.functions.keys().map(String::as_str).collect();
    let want: BTreeSet<&str> = HOST_PORT_REGISTRY.iter().map(|(f, _)| *f).collect();
    assert_eq!(
        got, want,
        "host surface drifted; update wit/tau-host.wit AND host_ports.rs AND this \
         test + the registry deliberately (ADR-0056 freeze)"
    );
}

#[test]
fn host_function_param_shapes_are_frozen() {
    let resolve = load();
    let host = resolve
        .interfaces
        .iter()
        .find(|(_, i)| i.name.as_deref() == Some("host"))
        .map(|(_, i)| i)
        .expect("`host` interface present");

    // complete(request-json: string) -> result<string, string>
    // wit-parser 0.251: Function.params is Vec<Param>; each Param has a .name field.
    let complete = &host.functions["complete"];
    let cparams: Vec<&str> = complete.params.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(cparams, vec!["request-json"], "complete params frozen");

    // now-millis() -> u64  and  next-u64() -> u64  take no params
    assert!(
        host.functions["now-millis"].params.is_empty(),
        "now-millis takes no params"
    );
    assert!(
        host.functions["next-u64"].params.is_empty(),
        "next-u64 takes no params"
    );
}

#[test]
fn runner_world_imports_host_and_exports_run() {
    let resolve = load();
    let world = resolve
        .worlds
        .iter()
        .find(|(_, w)| w.name == "runner")
        .map(|(_, w)| w)
        .expect("`runner` world present");

    // wit-parser 0.251: `import host;` for a named interface is stored as
    // WorldKey::Interface(id), NOT WorldKey::Name("host"). Use
    // Resolve::name_world_key to get a human-readable string for either variant.
    let import_names: BTreeSet<String> = world
        .imports
        .keys()
        .map(|k| resolve.name_world_key(k))
        .collect();
    assert!(
        import_names.iter().any(|k| k.contains("host")),
        "runner must import the host interface; got {import_names:?}"
    );

    // `export run: func(...)` is a direct function export → WorldKey::Name("run").
    let export_names: BTreeSet<String> = world
        .exports
        .keys()
        .map(|k| resolve.name_world_key(k))
        .collect();
    assert!(
        export_names.iter().any(|k| k.contains("run")),
        "runner must export run; got {export_names:?}"
    );
}
