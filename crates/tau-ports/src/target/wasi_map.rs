//! Capability → WASI/WIT mapping table for the wasm target (EPIC 3.1).
//!
//! `map_capability` lowers one [`tau_domain::Capability`] to its WASI/WIT
//! realization: the WIT interface [`WitInterface`] imports the generated world
//! must declare (3.2), the `WasiConfig` fragment the host `WasiCtx` consumes
//! (3.3), and the `Disposition` that says how the capability is satisfied on
//! wasm (3.4). Pure, total, and read-only over `tau_domain`.
//!
//! See `docs/superpowers/specs/2026-07-23-epic-3-1-cap-wit-table-design.md`.

extern crate alloc;

/// WASI preview-2 version this table pins (wasip2, wasmtime-45, β.7.5).
pub const WASI_VERSION: &str = "0.2.3";

/// The WASI interfaces this table references. [`WitInterface::package_id`]
/// returns the fully-qualified WIT package id, e.g.
/// `"wasi:http/outgoing-handler@0.2.3"`.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WitInterface {
    /// `wasi:http/types` — HTTP request/response value types.
    WasiHttpTypes,
    /// `wasi:http/outgoing-handler` — outbound HTTP; carries the host allow-list.
    WasiHttpOutgoingHandler,
    /// `wasi:filesystem/types` — filesystem descriptors and operations.
    WasiFilesystemTypes,
    /// `wasi:filesystem/preopens` — the set of preopened directories.
    WasiFilesystemPreopens,
}

impl WitInterface {
    /// Fully-qualified WIT package id (interface path + `@` + [`WASI_VERSION`]).
    pub fn package_id(&self) -> &'static str {
        match self {
            WitInterface::WasiHttpTypes => "wasi:http/types@0.2.3",
            WitInterface::WasiHttpOutgoingHandler => "wasi:http/outgoing-handler@0.2.3",
            WitInterface::WasiFilesystemTypes => "wasi:filesystem/types@0.2.3",
            WitInterface::WasiFilesystemPreopens => "wasi:filesystem/preopens@0.2.3",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_id_is_fully_qualified_and_version_pinned() {
        let all = [
            WitInterface::WasiHttpTypes,
            WitInterface::WasiHttpOutgoingHandler,
            WitInterface::WasiFilesystemTypes,
            WitInterface::WasiFilesystemPreopens,
        ];
        for iface in all {
            let id = iface.package_id();
            assert!(id.starts_with("wasi:"), "not fully qualified: {id}");
            assert!(
                id.ends_with(&alloc::format!("@{WASI_VERSION}")),
                "version drift: {id} != @{WASI_VERSION}"
            );
        }
        assert_eq!(
            WitInterface::WasiHttpOutgoingHandler.package_id(),
            "wasi:http/outgoing-handler@0.2.3"
        );
    }
}
