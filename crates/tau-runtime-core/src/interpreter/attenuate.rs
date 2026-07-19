//! Subflow capability attenuation decorator.
//!
//! Wraps a child agent's `ToolDispatcher` at a `ToolImpl::Subflow` spawn,
//! gating every child tool call against the subflow tool's declared
//! `capabilities` (the frame grant). Nesting composes into the exact meet
//! of all ancestor frames — see the design spec
//! `docs/superpowers/specs/2026-07-19-subflow-runtime-attenuation-design.md`.
//!
//! The static half (EPIC 1.5 lattice L2) checks cap_subset ⊆ agent-effective
//! for tau-cli-authored workflows; this runtime half additionally clamps
//! descendants under the runtime narrowing chain and catches hand-crafted IR.
//!
//! # Dead-code allow
//!
//! `AttenuatedDispatcher` is exercised by this module's `tests` submodule
//! only until the follow-up task wires it into the subflow-spawn call site
//! in `agent_loop.rs`; until then it warns under the `dead_code` lint
//! (same rationale as the module-level allow in `crate::capability`).

#![allow(dead_code)]

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use core::future::Future;
use core::pin::Pin;

use serde_json::Value;

use tau_ir::capability::CapabilityRequirements;
use tau_ir::{IrModule, ToolId};

use crate::builder::DynLlmBackend;
use crate::error::{CapabilityDenial, RuntimeError};
use crate::interpreter::tool_dispatch::{ToolDispatcher, ToolInvocationResult};

/// A `ToolDispatcher` decorator enforcing one subflow frame's cap_subset.
pub(crate) struct AttenuatedDispatcher {
    /// This frame's cap_subset (the invoking subflow tool's `capabilities`).
    grant: CapabilityRequirements,
    /// The subflow tool id that imposed this frame — denial provenance.
    frame: ToolId,
    /// The child agent id running under this frame — denial `agent_id`.
    agent_id: String,
    /// Source of a called tool's declared required caps.
    module: Arc<IrModule>,
    /// `dyn` so recursive nesting does not create unbounded monomorphized types.
    inner: Arc<dyn ToolDispatcher + Send + Sync>,
}

impl AttenuatedDispatcher {
    pub(crate) fn new(
        grant: CapabilityRequirements,
        frame: ToolId,
        agent_id: String,
        module: Arc<IrModule>,
        inner: Arc<dyn ToolDispatcher + Send + Sync>,
    ) -> Self {
        Self {
            grant,
            frame,
            agent_id,
            module,
            inner,
        }
    }
}

impl ToolDispatcher for AttenuatedDispatcher {
    fn invoke<'a>(
        &'a self,
        tool_id: &'a ToolId,
        args: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolInvocationResult, RuntimeError>> + Send + 'a>> {
        // A called tool's declared caps live on its Tool node; absent ⇒ none ⇒ allowed.
        let required: &[tau_domain::Capability] = self
            .module
            .workflow
            .tools
            .get(tool_id)
            .map(|t| t.capabilities.declared.as_slice())
            .unwrap_or(&[]);

        if let Some(missing) = crate::capability::check_capabilities(&self.grant.declared, required)
        {
            let kind = crate::capability::capability_kind_str(missing);
            let denial = CapabilityDenial::new(
                self.agent_id.clone(),
                "ir-agent",
                tool_id.0.clone(),
                kind.clone(),
                alloc::format!("{missing:?}"),
            )
            .with_narrowing_frame(self.frame.0.clone());
            tracing::warn!(
                name = "runtime.subflow.attenuation_denied",
                tool = %tool_id.0,
                missing = %kind,
                frame = %self.frame.0,
            );
            let msg = denial.to_string();
            return Box::pin(async move {
                Ok(ToolInvocationResult {
                    body: None,
                    error: Some(msg),
                })
            });
        }
        // Permitted at this frame — delegate inward (which may re-check a parent frame).
        self.inner.invoke(tool_id, args)
    }

    fn llm_backend_for(&self, backend: &str) -> Result<Arc<dyn DynLlmBackend>, RuntimeError> {
        self.inner.llm_backend_for(backend)
    }
    fn deterministic_registry(
        &self,
    ) -> Option<Arc<dyn crate::interpreter::deterministic::DeterministicRegistry>> {
        self.inner.deterministic_registry()
    }
    fn clock(&self) -> Option<Arc<dyn tau_ports::Clock>> {
        self.inner.clock()
    }
    fn random(&self) -> Option<Arc<dyn tau_ports::RandomSource>> {
        self.inner.random()
    }
    fn artifact_reader(&self) -> Option<Arc<dyn crate::interpreter::artifact::ArtifactReader>> {
        self.inner.artifact_reader()
    }
    fn context_transformer_registry(
        &self,
    ) -> Option<Arc<dyn crate::context::ContextTransformerRegistry>> {
        self.inner.context_transformer_registry()
    }
    fn checkpointing(&self) -> Option<crate::interpreter::tool_dispatch::DurableHandles> {
        self.inner.checkpointing()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::RuntimeError;
    use crate::interpreter::tool_dispatch::{ToolDispatcher, ToolInvocationResult};
    use alloc::string::{String, ToString};
    use alloc::sync::Arc;
    use core::sync::atomic::{AtomicBool, Ordering};
    use serde_json::json;
    use tau_ir::capability::CapabilityRequirements;
    use tau_ir::ids::ToolId;
    use tau_ir::module::{IrFormatVersion, IrModule, Workflow};
    use tau_ir::node::{Tool, ToolSpec};
    use tau_ir::tool_impl::{NativeFnRef, ToolImpl};

    // capability from canonical TOML (variant #[non_exhaustive]).
    fn cap(toml_str: &str) -> tau_domain::Capability {
        #[derive(serde::Deserialize)]
        struct W {
            cap: tau_domain::Capability,
        }
        toml::from_str::<W>(toml_str).expect("cap toml").cap
    }
    fn reqs(caps: alloc::vec::Vec<tau_domain::Capability>) -> CapabilityRequirements {
        CapabilityRequirements { declared: caps }
    }

    /// Inner dispatcher that flips a flag when `invoke` is reached.
    struct Spy(Arc<AtomicBool>);
    impl ToolDispatcher for Spy {
        fn invoke<'a>(
            &'a self,
            _tool_id: &'a ToolId,
            _args: &'a serde_json::Value,
        ) -> core::pin::Pin<
            alloc::boxed::Box<
                dyn core::future::Future<Output = Result<ToolInvocationResult, RuntimeError>>
                    + Send
                    + 'a,
            >,
        > {
            self.0.store(true, Ordering::SeqCst);
            alloc::boxed::Box::pin(async {
                Ok(ToolInvocationResult {
                    body: Some(json!("ok")),
                    error: None,
                })
            })
        }
        fn llm_backend_for(
            &self,
            _b: &str,
        ) -> Result<Arc<dyn crate::builder::DynLlmBackend>, RuntimeError> {
            Err(RuntimeError::Internal {
                message: "spy: no backend".into(),
            })
        }
    }

    /// Module with one tool `t` carrying `t_caps`.
    fn module_with_tool(tool_id: &str, t_caps: CapabilityRequirements) -> Arc<IrModule> {
        let mut tools = alloc::collections::BTreeMap::new();
        tools.insert(
            ToolId(tool_id.to_string()),
            Tool {
                id: ToolId(tool_id.to_string()),
                impl_: ToolImpl::Native {
                    fn_ref: NativeFnRef {
                        name: tool_id.into(),
                    },
                    content_hash: [1u8; 32],
                },
                capabilities: t_caps,
                spec: ToolSpec {
                    name: tool_id.into(),
                    description: String::new(),
                    input_schema: serde_json::Value::Null,
                },
            },
        );
        // `IrModule` has no `Default` impl (unlike `Workflow`, which does),
        // so every field is supplied explicitly; `target` is taken from the
        // target-triple registry the same way `tests/run_ir_streaming.rs`
        // builds its fixture module.
        let target = tau_ports::target::list_available()
            .next()
            .expect("at least one available target")
            .triple;
        Arc::new(IrModule {
            ir_format: IrFormatVersion::current(),
            tau_version: env!("CARGO_PKG_VERSION").into(),
            target,
            workflow: Workflow {
                tools,
                ..Default::default()
            },
            triggers: alloc::vec::Vec::new(),
        })
    }

    fn block_on<F: core::future::Future>(f: F) -> F::Output {
        futures_executor::block_on(f)
    }

    #[test]
    fn denies_when_required_exceeds_frame_grant() {
        let reached = Arc::new(AtomicBool::new(false));
        let module = module_with_tool("page", reqs(alloc::vec![cap("[cap]\nkind=\"net.http\"\n")]));
        let att = AttenuatedDispatcher::new(
            reqs(alloc::vec![]), // empty cap_subset
            ToolId("notify".into()),
            "worker".into(),
            module,
            Arc::new(Spy(reached.clone())),
        );
        let res = block_on(att.invoke(&ToolId("page".into()), &json!({}))).unwrap();
        assert!(res.error.is_some(), "expected denial");
        let msg = res.error.unwrap();
        assert!(msg.contains("page") && msg.contains("net.http"), "{msg}");
        assert!(msg.contains("narrowed by subflow `notify`"), "{msg}");
        assert!(
            !reached.load(Ordering::SeqCst),
            "inner.invoke must NOT run on denial"
        );
    }

    #[test]
    fn allows_when_required_within_frame_grant() {
        let reached = Arc::new(AtomicBool::new(false));
        let module = module_with_tool("page", reqs(alloc::vec![cap("[cap]\nkind=\"net.http\"\n")]));
        let att = AttenuatedDispatcher::new(
            reqs(alloc::vec![cap("[cap]\nkind=\"net.http\"\n")]),
            ToolId("notify".into()),
            "worker".into(),
            module,
            Arc::new(Spy(reached.clone())),
        );
        let res = block_on(att.invoke(&ToolId("page".into()), &json!({}))).unwrap();
        assert!(res.error.is_none() && reached.load(Ordering::SeqCst));
    }

    #[test]
    fn allows_tool_with_no_declared_caps() {
        let reached = Arc::new(AtomicBool::new(false));
        let module = module_with_tool("noop", reqs(alloc::vec![]));
        let att = AttenuatedDispatcher::new(
            reqs(alloc::vec![]),
            ToolId("notify".into()),
            "worker".into(),
            module,
            Arc::new(Spy(reached.clone())),
        );
        let res = block_on(att.invoke(&ToolId("noop".into()), &json!({}))).unwrap();
        assert!(res.error.is_none() && reached.load(Ordering::SeqCst));
    }

    #[test]
    fn nested_frames_compose_to_meet() {
        // outer grant C2 = {fs.read /proj/**}; inner grant C1 = {net.http}.
        // tool needs net.http: allowed by C1 but not C2 -> denied at outer.
        let reached = Arc::new(AtomicBool::new(false));
        let module = module_with_tool("page", reqs(alloc::vec![cap("[cap]\nkind=\"net.http\"\n")]));
        let inner = AttenuatedDispatcher::new(
            reqs(alloc::vec![cap("[cap]\nkind=\"net.http\"\n")]),
            ToolId("c1".into()),
            "child".into(),
            module.clone(),
            Arc::new(Spy(reached.clone())),
        );
        let outer = AttenuatedDispatcher::new(
            reqs(alloc::vec![cap(
                "[cap]\nkind=\"fs.read\"\npaths=[\"/proj/**\"]\n"
            )]),
            ToolId("c2".into()),
            "grandchild".into(),
            module,
            Arc::new(inner),
        );
        let res = block_on(outer.invoke(&ToolId("page".into()), &json!({}))).unwrap();
        assert!(res.error.is_some(), "outer frame C2 must deny net.http");
        assert!(!reached.load(Ordering::SeqCst));
    }
}
