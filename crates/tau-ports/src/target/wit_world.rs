//! WIT-world generation for the wasm target (EPIC 3.2).
//!
//! [`generate_world`] folds a capability set through the 3.1
//! [`map_capability`](super::wasi_map::map_capability) table, unions the
//! `Disposition::Wasi` WIT imports, expands their hardcoded transitive
//! closure, and renders a deterministic WIT `world`. The world is the frozen
//! `tau:host` `runner` world's superset with the cap-derived WASI imports
//! added. An `Unsupported` capability (fs.exec, process.spawn) is a hard error.
//!
//! Output is a deterministic ABI manifest. A follow-on
//! (`docs/superpowers/specs/2026-08-08-epic-3-2-load-bearing-wit-world-design.md`)
//! vendors the WASI `.wit` packages this world imports and assembles them
//! alongside the frozen `tau:host` contract into the guest's `wit-gen/`
//! resolution root, making the world standalone-resolvable *and*
//! load-bearing: `tau-wasm-guest` is compiled against exactly this generated
//! world. Determinism is the contract 3.5's `verify --bundle` byte-compare
//! relies on.
//!
//! See `docs/superpowers/specs/2026-08-08-epic-3-2-load-bearing-wit-world-design.md`.

use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::String;

use tau_domain::Capability;

use super::wasi_map::{map_capability, Disposition, WitInterface};

/// Error raised when a capability cannot be realized on the wasm target.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum WitWorldError {
    /// A capability maps to `Disposition::Unsupported` on wasm (fs.exec,
    /// process.spawn) — it has no WASI ABI realization.
    #[error("capability `{cap}` cannot target wasm: {reason}")]
    UnsupportedOnWasm {
        /// Debug rendering of the offending capability.
        cap: String,
        /// The reason carried by `Disposition::Unsupported`.
        reason: &'static str,
    },
}

/// Transitive WASI interfaces (as fully-qualified package-ids at
/// `WASI_VERSION`) that a direct [`WitInterface`] pulls in. These interfaces
/// are NOT in 3.1's `WitInterface` enum — they are the closure 3.2 owns.
///
/// Edges (WASI 0.2.3): `http/types` → io/{streams,poll,error} +
/// clocks/monotonic-clock; `filesystem/types` → io/{streams,poll,error} +
/// clocks/wall-clock; `io/streams` → io/{error,poll};
/// `clocks/monotonic-clock` → io/poll (all folded into the sets below).
fn transitive_closure(iface: WitInterface) -> &'static [&'static str] {
    match iface {
        WitInterface::WasiHttpTypes | WitInterface::WasiHttpOutgoingHandler => &[
            "wasi:io/streams@0.2.3",
            "wasi:io/poll@0.2.3",
            "wasi:io/error@0.2.3",
            "wasi:clocks/monotonic-clock@0.2.3",
        ],
        WitInterface::WasiFilesystemTypes | WitInterface::WasiFilesystemPreopens => &[
            "wasi:io/streams@0.2.3",
            "wasi:io/poll@0.2.3",
            "wasi:io/error@0.2.3",
            "wasi:clocks/wall-clock@0.2.3",
        ],
        // NOTE: no wildcard arm. `WitInterface` is `#[non_exhaustive]` for
        // *other* crates, but this match is in the crate that defines it, so
        // rustc checks it exhaustively today (an unreachable-arm wildcard
        // here would be a hard `-D warnings` error). If 3.1 ever adds a
        // variant, this match fails to compile until extended — fail-closed
        // by construction rather than silently contributing no closure.
    }
}

/// Generate the guest component's WIT `world` from a capability set.
///
/// Folds each capability through [`map_capability`], keeps the
/// `Disposition::Wasi` imports, unions them, expands the transitive closure,
/// and renders a deterministic `world runner` importing `tau:host` + the
/// resulting WASI interfaces and exporting `run`. `InGuest`/`HostMediated`
/// capabilities contribute no import; an `Unsupported` capability is a hard
/// error ([`WitWorldError::UnsupportedOnWasm`]).
///
/// # Example
///
/// ```
/// use tau_ports::target::wit_world::generate_world;
/// use tau_domain::fixtures::cap_fs_read;
///
/// let wit = generate_world(&[cap_fs_read(&["/d"])]).unwrap();
/// assert!(wit.contains("import wasi:filesystem/types@0.2.3;"));
/// assert!(wit.contains("export run: func(prompt: string)"));
/// ```
pub fn generate_world(caps: &[Capability]) -> Result<String, WitWorldError> {
    // 1. Union the direct WASI interfaces the granted caps require.
    let mut ifaces: BTreeSet<WitInterface> = BTreeSet::new();
    for cap in caps {
        let mapping = map_capability(cap);
        match mapping.disposition {
            Disposition::Wasi => ifaces.extend(mapping.imports),
            Disposition::Unsupported { reason } => {
                return Err(WitWorldError::UnsupportedOnWasm {
                    cap: format!("{cap:?}"),
                    reason,
                });
            }
            // No WASI surface — contributes nothing to the world.
            //
            // NOTE: no wildcard arm here either, for the same reason as
            // `transitive_closure` above: `Disposition` is `#[non_exhaustive]`
            // only for external crates, and this match is exhaustive today
            // within tau-ports, so a wildcard would be an unreachable-pattern
            // `-D warnings` error. A future `Disposition` variant fails this
            // match at compile time until handled explicitly.
            Disposition::InGuest | Disposition::HostMediated => {}
        }
    }

    // 2. Expand to fully-qualified package-ids (direct + transitive closure),
    //    deduped and sorted via BTreeSet → deterministic output.
    let mut imports: BTreeSet<&'static str> = BTreeSet::new();
    for iface in &ifaces {
        imports.insert(iface.package_id());
        for id in transitive_closure(*iface) {
            imports.insert(id);
        }
    }

    // 3. Render. The generated world lives in its own package
    //    (`tau:generated`), distinct from the frozen host contract's package
    //    (`tau:host`, `wit/tau-host.wit`) so wit-parser can resolve them as
    //    two separate packages in one directory (Task 4's `wit-gen/`
    //    assembly). Cross-package imports must be fully qualified AND
    //    version-pinned (`tau:host/host@0.1.0`, not bare `tau:host/host`) —
    //    wit-parser's dependency toposort keys foreign deps by the exact
    //    `PackageName` (namespace+name+version); an unversioned import
    //    doesn't match the versioned `package tau:host@0.1.0;` declaration,
    //    so the dep is silently left out of topological order and the
    //    import fails to resolve (`package 'tau:host' not found`).
    let mut out = String::new();
    out.push_str(
        "package tau:generated@0.1.0;\n\nworld runner {\n    import tau:host/host@0.1.0;\n",
    );
    for id in &imports {
        out.push_str("    import ");
        out.push_str(id);
        out.push_str(";\n");
    }
    out.push_str("\n    export run: func(prompt: string) -> result<string, string>;\n}\n");
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tau_domain::fixtures::{cap_agent_spawn, cap_fs_read, cap_net_http, cap_process_spawn};

    #[test]
    fn empty_cap_set_yields_host_only_world() {
        let world = generate_world(&[]).expect("empty is ok");
        assert_eq!(
            world,
            "package tau:generated@0.1.0;\n\
             \n\
             world runner {\n\
             \x20   import tau:host/host@0.1.0;\n\
             \n\
             \x20   export run: func(prompt: string) -> result<string, string>;\n\
             }\n"
        );
    }

    #[test]
    fn net_only_imports_http_plus_transitive() {
        let world = generate_world(&[cap_net_http(&["api.anthropic.com"], &["POST"])]).unwrap();
        for want in [
            "import tau:host/host@0.1.0;",
            "import wasi:http/types@0.2.3;",
            "import wasi:http/outgoing-handler@0.2.3;",
            "import wasi:io/streams@0.2.3;",
            "import wasi:io/poll@0.2.3;",
            "import wasi:io/error@0.2.3;",
            "import wasi:clocks/monotonic-clock@0.2.3;",
        ] {
            assert!(world.contains(want), "missing `{want}` in:\n{world}");
        }
        // fs / wall-clock interfaces must NOT appear for a net-only cap set.
        assert!(
            !world.contains("wasi:filesystem"),
            "net-only leaked fs:\n{world}"
        );
        assert!(
            !world.contains("wall-clock"),
            "net-only leaked wall-clock:\n{world}"
        );
    }

    #[test]
    fn fs_only_imports_filesystem_plus_transitive() {
        let world = generate_world(&[cap_fs_read(&["/data/**"])]).unwrap();
        for want in [
            "import wasi:filesystem/types@0.2.3;",
            "import wasi:filesystem/preopens@0.2.3;",
            "import wasi:io/streams@0.2.3;",
            "import wasi:clocks/wall-clock@0.2.3;",
        ] {
            assert!(world.contains(want), "missing `{want}` in:\n{world}");
        }
        assert!(
            !world.contains("wasi:http"),
            "fs-only leaked http:\n{world}"
        );
        assert!(
            !world.contains("monotonic-clock"),
            "fs-only leaked monotonic:\n{world}"
        );
    }

    #[test]
    fn mixed_unions_and_dedupes() {
        let caps = [cap_net_http(&["h"], &[]), cap_fs_read(&["/d"])];
        let world = generate_world(&caps).unwrap();
        assert!(world.contains("wasi:http/types@0.2.3;"));
        assert!(world.contains("wasi:filesystem/types@0.2.3;"));
        // io/streams is shared by both families — appears exactly once.
        assert_eq!(world.matches("import wasi:io/streams@0.2.3;").count(), 1);
    }

    #[test]
    fn in_guest_caps_contribute_no_import() {
        let world = generate_world(&[cap_agent_spawn(&["worker"])]).unwrap();
        assert!(
            !world.contains("wasi:"),
            "in-guest cap leaked a wasi import:\n{world}"
        );
        assert!(world.contains("import tau:host/host@0.1.0;"));
    }

    #[test]
    fn unsupported_cap_is_a_hard_error() {
        let err = generate_world(&[cap_process_spawn(&["ls"])]).unwrap_err();
        match err {
            WitWorldError::UnsupportedOnWasm { reason, .. } => assert!(!reason.is_empty()),
        }
    }

    #[test]
    fn output_is_deterministic_regardless_of_cap_order() {
        let a = [cap_net_http(&["h"], &[]), cap_fs_read(&["/d"])];
        let b = [cap_fs_read(&["/d"]), cap_net_http(&["h"], &[])];
        assert_eq!(generate_world(&a).unwrap(), generate_world(&b).unwrap());
    }

    #[test]
    fn every_transitive_id_is_version_pinned() {
        for iface in [
            WitInterface::WasiHttpTypes,
            WitInterface::WasiFilesystemTypes,
        ] {
            for id in transitive_closure(iface) {
                assert!(id.starts_with("wasi:"), "not qualified: {id}");
                assert!(id.ends_with("@0.2.3"), "version drift: {id}");
            }
        }
    }
}
