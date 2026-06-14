//! Build a concrete per-turn context pipeline from an IR [`ContextConfig`].
//!
//! [`build_context_pipeline`] is the only public entry point. Callers
//! supply an optional [`ContextTransformerRegistry`] for resolving
//! custom (user-supplied native) nodes; omitting it is fine as long as
//! the config contains only builtins.

use alloc::sync::Arc;
use alloc::vec::Vec;
use tau_ir::context::{ContextConfig, ContextNodeKind};

use super::transformers::{CompactToolOutputs, FitBudget, TrimOld};
use super::ContextTransformer;
use crate::error::RuntimeError;

/// Optional registry of user-supplied (native) custom transformers.
///
/// Implement this trait to plug custom context nodes into
/// [`build_context_pipeline`]. The registry only needs to handle the
/// `ContextNodeKind::Custom` arm; builtins are resolved before
/// the registry is consulted.
pub trait ContextTransformerRegistry: Send + Sync {
    /// Resolve a custom node by name; `None` if unknown.
    fn resolve(&self, name: &str) -> Option<Arc<dyn ContextTransformer>>;
}

/// Build the concrete per-turn pipeline from an IR [`ContextConfig`].
///
/// Each [`tau_ir::context::ContextStep`] is converted to an
/// `Arc<dyn ContextTransformer>`:
///
/// - `Builtin` steps are matched by name against the three v1 builtins
///   (`trim_old`, `compact_tool_outputs`, `fit_budget`).
/// - `Custom` steps are resolved via `registry`. If no registry is
///   provided and a custom step is present, `RuntimeError::Internal` is
///   returned.
///
/// # Errors
///
/// Returns [`RuntimeError::Internal`] for unknown builtin names or
/// unresolvable custom nodes.
pub fn build_context_pipeline(
    cfg: &ContextConfig,
    registry: Option<&dyn ContextTransformerRegistry>,
) -> Result<Vec<Arc<dyn ContextTransformer>>, RuntimeError> {
    let mut out: Vec<Arc<dyn ContextTransformer>> = Vec::with_capacity(cfg.pipeline.len());
    for step in &cfg.pipeline {
        let t: Arc<dyn ContextTransformer> = match &step.kind {
            ContextNodeKind::Builtin => match step.transformer.as_str() {
                "trim_old" => Arc::new(TrimOld::from_config(&step.config)),
                "compact_tool_outputs" => Arc::new(CompactToolOutputs::from_config(&step.config)),
                "fit_budget" => Arc::new(FitBudget::from_config(&step.config)),
                other => {
                    return Err(RuntimeError::Internal {
                        message: alloc::format!("unknown builtin context transformer '{other}'"),
                    })
                }
            },
            ContextNodeKind::Custom { .. } => {
                let reg = registry.ok_or_else(|| RuntimeError::Internal {
                    message: alloc::format!(
                        "custom context node '{}' but no registry provided",
                        step.transformer
                    ),
                })?;
                reg.resolve(&step.transformer)
                    .ok_or_else(|| RuntimeError::Internal {
                        message: alloc::format!(
                            "custom context node '{}' not registered",
                            step.transformer
                        ),
                    })?
            }
        };
        out.push(t);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tau_ir::context::{ContextNodeKind, ContextStep, DeterminismClass};

    fn make_step(name: &str) -> ContextStep {
        ContextStep {
            transformer: name.into(),
            determinism: DeterminismClass::Pure,
            kind: ContextNodeKind::Builtin,
            config: Default::default(),
        }
    }

    fn unwrap_pipeline(
        r: Result<Vec<Arc<dyn ContextTransformer>>, RuntimeError>,
    ) -> Vec<Arc<dyn ContextTransformer>> {
        match r {
            Ok(v) => v,
            Err(e) => panic!("expected Ok, got Err: {e}"),
        }
    }

    fn unwrap_err_pipeline(
        r: Result<Vec<Arc<dyn ContextTransformer>>, RuntimeError>,
    ) -> RuntimeError {
        match r {
            Ok(_) => panic!("expected Err, got Ok"),
            Err(e) => e,
        }
    }

    #[test]
    fn builds_three_builtins() {
        // ContextConfig is #[non_exhaustive] outside tau-ir, so use Default +
        // field assignment instead of a struct literal.
        let mut cfg = tau_ir::context::ContextConfig::default();
        cfg.pipeline = alloc::vec![make_step("trim_old"), make_step("fit_budget")];

        let pipe = unwrap_pipeline(build_context_pipeline(&cfg, None));
        assert_eq!(pipe.len(), 2);
        assert_eq!(pipe[0].name(), "trim_old");
        assert_eq!(pipe[1].name(), "fit_budget");
    }

    #[test]
    fn builds_compact_tool_outputs() {
        let mut cfg = tau_ir::context::ContextConfig::default();
        cfg.pipeline = alloc::vec![make_step("compact_tool_outputs")];

        let pipe = unwrap_pipeline(build_context_pipeline(&cfg, None));
        assert_eq!(pipe.len(), 1);
        assert_eq!(pipe[0].name(), "compact_tool_outputs");
    }

    #[test]
    fn unknown_builtin_returns_internal_error() {
        let mut cfg = tau_ir::context::ContextConfig::default();
        cfg.pipeline = alloc::vec![make_step("nonexistent_transformer")];

        let err = unwrap_err_pipeline(build_context_pipeline(&cfg, None));
        match err {
            RuntimeError::Internal { message } => {
                assert!(
                    message.contains("nonexistent_transformer"),
                    "got: {message}"
                );
            }
            other => panic!("expected Internal, got: {other:?}"),
        }
    }

    #[test]
    fn custom_node_without_registry_returns_error() {
        let mut cfg = tau_ir::context::ContextConfig::default();
        cfg.pipeline = alloc::vec![ContextStep {
            transformer: "my_custom".into(),
            determinism: DeterminismClass::Pure,
            kind: ContextNodeKind::Custom {
                source: "native".into(),
                package: "my-pkg@^0.1".into(),
            },
            config: Default::default(),
        }];

        let err = unwrap_err_pipeline(build_context_pipeline(&cfg, None));
        assert!(matches!(err, RuntimeError::Internal { .. }));
    }

    #[test]
    fn custom_node_with_registry_resolves() {
        use alloc::collections::BTreeMap;
        use alloc::string::String;

        struct StubTransformer;
        impl ContextTransformer for StubTransformer {
            fn name(&self) -> &str {
                "my_custom"
            }
            fn determinism(&self) -> DeterminismClass {
                DeterminismClass::Pure
            }
            fn required_capabilities(&self) -> &[crate::context::CapabilityNeed] {
                &[]
            }
            fn transform<'a>(
                &'a self,
                _cx: &'a crate::context::TransformCx<'a>,
                msgs: alloc::vec::Vec<tau_domain::Message>,
            ) -> crate::context::ContextFuture<'a> {
                alloc::boxed::Box::pin(async move { Ok(msgs) })
            }
        }

        struct StubRegistry;
        impl ContextTransformerRegistry for StubRegistry {
            fn resolve(&self, name: &str) -> Option<Arc<dyn ContextTransformer>> {
                if name == "my_custom" {
                    Some(Arc::new(StubTransformer))
                } else {
                    None
                }
            }
        }

        let mut cfg = tau_ir::context::ContextConfig::default();
        cfg.pipeline = alloc::vec![ContextStep {
            transformer: "my_custom".into(),
            determinism: DeterminismClass::Pure,
            kind: ContextNodeKind::Custom {
                source: "native".into(),
                package: "my-pkg@^0.1".into(),
            },
            config: BTreeMap::<String, serde_json::Value>::new(),
        }];

        let reg = StubRegistry;
        let pipe = unwrap_pipeline(build_context_pipeline(&cfg, Some(&reg)));
        assert_eq!(pipe.len(), 1);
        assert_eq!(pipe[0].name(), "my_custom");
    }

    #[test]
    fn empty_pipeline_returns_empty_vec() {
        let cfg = tau_ir::context::ContextConfig::default();
        let pipe = unwrap_pipeline(build_context_pipeline(&cfg, None));
        assert!(pipe.is_empty());
    }
}
