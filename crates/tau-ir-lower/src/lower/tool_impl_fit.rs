//! #735: wasm-only build-time tool-dispatch gate (`tool_impl_fit`).
//!
//! Third sibling of [`feature_fit`](super::feature_fit) and
//! [`predicate_fit`](super::predicate_fit). Where feature-fit asks "can the
//! target *execute* this IR shape?" and predicate-fit asks "can the guest
//! *answer* this fn?", tool-impl-fit asks "can the guest *dispatch* this
//! tool at all?".
//!
//! `crates/tau-runtime-core/src/interpreter/agent_loop.rs` splits the four
//! [`ToolImpl`] arms two ways: `Subflow` and `Step` are serviced by the
//! interpreter itself (recursive spawn, and the dispatcher's
//! `deterministic_registry()` — already gated by predicate-fit), while
//! `Native` and `Mcp` are forwarded to the [`ToolDispatcher`]. The wasm
//! guest's dispatcher (`crates/tau-wasm-guest/src/dispatcher.rs`) resolves
//! only `Native`; an `Mcp` tool falls through to
//! `tau_native_tools::invoke`, which does not know it, and the run dies
//! mid-turn with `RuntimeError::Internal`.
//!
//! MCP needs a live transport (a spawned stdio server or an HTTP session)
//! that a wasm guest cannot own, so this is not an oversight the guest can
//! close on its own — it needs a host import. Until that exists, refusing
//! at build time is strictly better than emitting a component that traps
//! the first time the model reaches for the tool. **No override flag**,
//! matching the Rust-like build-time enforcement principle its two
//! siblings follow.

use alloc::vec::Vec;
use tau_ir::ids::ToolId;
use tau_ir::ToolImpl;
use tau_ports::target::{AdapterFamily, TargetTriple};

use crate::error::LowerError;

use super::parse::Parsed;

/// Run the tool-impl-fit check on a `Parsed` workflow against a target.
///
/// Returns `Ok(())` for every non-Wasi target (this gate is wasm-only) and
/// for a Wasi target whose tools are all guest-dispatchable. Returns
/// `Err(LowerError::WasmToolImplUnsupported)` carrying the offending tool
/// ids otherwise. Ids come out in `parsed.workflow.tools` iteration order,
/// which is a `BTreeMap`, so the list is sorted and stable across runs.
pub(super) fn check(parsed: &Parsed, target: &TargetTriple) -> Result<(), LowerError> {
    if target.adapter_family != AdapterFamily::Wasi {
        return Ok(());
    }

    let offending: Vec<ToolId> = parsed
        .workflow
        .tools
        .iter()
        .filter(|(_, tool)| matches!(tool.impl_, ToolImpl::Mcp { .. }))
        .map(|(tool_id, _)| tool_id.clone())
        .collect();

    if offending.is_empty() {
        Ok(())
    } else {
        Err(LowerError::WasmToolImplUnsupported {
            tools: offending,
            target: *target,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lower::parse;
    use crate::lower::parse::no_prompt_files;
    use alloc::string::ToString;
    use tau_pkg::project::ProjectConfig;

    /// One MCP tool, the shape `fan_monitor` uses (#691's spike fixture).
    const MCP_TOOL_TOML: &str = r#"
[project]
name = "demo"

[tools.weather]
mcp = "cassette:./weather.cassette.jsonl"
description = "Look up current weather via cassette replay."
capabilities = [{ kind = "net.http", hosts = "any" }]
"#;

    /// Native tools only — the guest dispatches these fine.
    const NATIVE_TOOLS_TOML: &str = r#"
[project]
name = "demo"

[tools.read_temp]
native = "ReadTemp"
description = "Read the current temperature."
capabilities = []

[tools.set_fan]
native = "SetFan"
description = "Set the fan on or off."
capabilities = []
"#;

    /// Native + MCP mixed: only the MCP tool is blamed, and `alpha` sorting
    /// before `weather` proves the native one is not swept in.
    const MIXED_TOOLS_TOML: &str = r#"
[project]
name = "demo"

[tools.alpha]
native = "ReadTemp"
description = "Read the current temperature."
capabilities = []

[tools.weather]
mcp = "cassette:./weather.cassette.jsonl"
description = "Look up current weather via cassette replay."
capabilities = [{ kind = "net.http", hosts = "any" }]
"#;

    /// Two MCP tools — covers the sorted-and-stable ordering claim.
    const TWO_MCP_TOOLS_TOML: &str = r#"
[project]
name = "demo"

[tools.zulu]
mcp = "cassette:./z.cassette.jsonl"
description = "Z."
capabilities = [{ kind = "net.http", hosts = "any" }]

[tools.alpha]
mcp = "cassette:./a.cassette.jsonl"
description = "A."
capabilities = [{ kind = "net.http", hosts = "any" }]
"#;

    fn parsed(toml: &str) -> Parsed {
        let config = ProjectConfig::parse_str(toml).expect("toml parses");
        parse::parse(&config, &no_prompt_files).expect("parse stage")
    }

    fn wasm() -> TargetTriple {
        "any-wasi-strict".parse().unwrap()
    }

    fn native() -> TargetTriple {
        "linux-native-strict".parse().unwrap()
    }

    #[test]
    fn wasm_rejects_mcp_tool() {
        let t = wasm();
        let err = check(&parsed(MCP_TOOL_TOML), &t).expect_err("wasm must refuse an MCP tool");
        match err {
            LowerError::WasmToolImplUnsupported { tools, target } => {
                assert_eq!(tools, alloc::vec![ToolId("weather".to_string())]);
                assert_eq!(target, t);
            }
            other => panic!("expected WasmToolImplUnsupported, got {other:?}"),
        }
    }

    #[test]
    fn wasm_accepts_native_tools() {
        assert!(check(&parsed(NATIVE_TOOLS_TOML), &wasm()).is_ok());
    }

    #[test]
    fn wasm_blames_only_the_mcp_tool() {
        let err = check(&parsed(MIXED_TOOLS_TOML), &wasm()).expect_err("refused");
        match err {
            LowerError::WasmToolImplUnsupported { tools, .. } => {
                assert_eq!(tools, alloc::vec![ToolId("weather".to_string())]);
            }
            other => panic!("expected WasmToolImplUnsupported, got {other:?}"),
        }
    }

    #[test]
    fn wasm_reports_mcp_tools_sorted() {
        let err = check(&parsed(TWO_MCP_TOOLS_TOML), &wasm()).expect_err("refused");
        match err {
            LowerError::WasmToolImplUnsupported { tools, .. } => {
                assert_eq!(
                    tools,
                    alloc::vec![ToolId("alpha".to_string()), ToolId("zulu".to_string()),],
                    "ids must come out in sorted BTreeMap order, not TOML order"
                );
            }
            other => panic!("expected WasmToolImplUnsupported, got {other:?}"),
        }
    }

    /// The gate is wasm-only: a native target dispatches MCP normally.
    #[test]
    fn native_target_accepts_mcp_tool() {
        assert!(check(&parsed(MCP_TOOL_TOML), &native()).is_ok());
    }
}
