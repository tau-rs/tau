//! Second lowering stage: fill content hashes from caller-supplied caches.

use crate::error::IrError;
use crate::lower::Caches;
use crate::tool_impl::ToolImpl;

use super::parse::Parsed;

/// Run the resolve stage on a `Parsed` value.
pub fn resolve(mut parsed: Parsed, caches: &Caches<'_>) -> Result<Parsed, IrError> {
    for (_id, tool) in parsed.workflow.tools.iter_mut() {
        match &mut tool.impl_ {
            ToolImpl::Native {
                fn_ref,
                content_hash,
            } => {
                if let Some(h) = (caches.native_tool)(&fn_ref.name) {
                    *content_hash = h;
                }
                // If the cache returns None we KEEP the zero sentinel and
                // let typecheck (Task 2.4) decide whether that's an error.
                // The reason: `tau dev` typically has every native tool in
                // its registry, but a mocked-out test fixture might not.
            }
            ToolImpl::Mcp {
                url,
                contract_hash,
                capability_subset,
            } => {
                if let Some((h, caps)) = (caches.mcp_contract)(url) {
                    *contract_hash = h;
                    // The MCP server's declared capability subset must be a
                    // superset of the workflow's narrowed subset. v0 only
                    // checks at the lowering boundary; runtime enforces.
                    *capability_subset = caps;
                }
            }
        }
    }
    Ok(parsed)
}
