//! EPIC 4.5: dynamic-region spawn gate. One `SpawnTool` per offered kind is
//! registered into the coordinator's tool registry (`agent.<kind>.spawn`,
//! Task-tool shape); the admission gate lives in `invoke()`:
//! bounds counters → meet-attenuation → child agent run.

use alloc::string::{String, ToString};
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU64, Ordering};

use tau_ir::capability::CapabilityRequirements;
use tau_ir::{IrModule, ToolId};
use tau_ports::{
    tool::{SessionContext, ToolContent, ToolResult},
    ToolError, ToolSpec,
};

use crate::interpreter::agent_loop::{last_assistant_text, run_agent};
use crate::interpreter::attenuate::AttenuatedDispatcher;
use crate::interpreter::pipeline::user_message;
use crate::interpreter::tool_dispatch::ToolDispatcher;
use crate::outcome::RunOutcome;

// ---------------------------------------------------------------------------
// Region admission counters
// ---------------------------------------------------------------------------

/// Per-`Dynamic` step bounds accounting: total spawns ever admitted and
/// currently in-flight spawns. Shared (`Arc`) across every `SpawnTool`
/// offered by one dynamic region so all offered kinds draw from the same
/// pool.
pub(crate) struct RegionCounters {
    max_spawns: u64,
    max_concurrency: u64,
    spawned: AtomicU64,
    in_flight: AtomicU64,
}

/// Typed admission refusal — carries the snapshot needed to render both
/// the soft-deny tool-result text and the `runtime.dynamic.spawn_denied`
/// tracing event.
#[derive(Debug, Clone, Copy)]
pub(crate) enum AdmitError {
    /// The region's `max_spawns` (lifetime spawn count) is exhausted.
    Bounds { spawned: u64, max: u64 },
    /// The region's `max_concurrency` (simultaneously in-flight spawns) is
    /// exhausted. Defensive only — unreachable under today's sequential
    /// per-turn tool dispatch (no two `invoke()` calls run concurrently
    /// against the same region), but guards the invariant if that changes.
    Concurrency { in_flight: u64, max: u64 },
}

impl RegionCounters {
    pub(crate) fn new(max_spawns: u64, max_concurrency: u64) -> Self {
        Self {
            max_spawns,
            max_concurrency,
            spawned: AtomicU64::new(0),
            in_flight: AtomicU64::new(0),
        }
    }

    /// Admit one spawn: returns its 0-based index, or the typed refusal.
    ///
    /// Reserves a `spawned` slot via a compare-exchange loop (bounds
    /// check), then bumps `in_flight` and verifies it didn't overshoot
    /// `max_concurrency` — rolling the `in_flight` bump back on overshoot
    /// (the `spawned` reservation is NOT rolled back: a concurrency
    /// refusal still counts against the region's lifetime spawn budget,
    /// matching `max_spawns` semantics of "attempts", not "successes").
    pub(crate) fn try_admit(&self) -> Result<u64, AdmitError> {
        let index = loop {
            let cur = self.spawned.load(Ordering::SeqCst);
            if cur >= self.max_spawns {
                return Err(AdmitError::Bounds {
                    spawned: cur,
                    max: self.max_spawns,
                });
            }
            if self
                .spawned
                .compare_exchange(cur, cur + 1, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                break cur;
            }
        };

        let in_flight = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        if in_flight > self.max_concurrency {
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            return Err(AdmitError::Concurrency {
                in_flight: in_flight - 1,
                max: self.max_concurrency,
            });
        }
        Ok(index)
    }

    /// Paired with every successful `try_admit`; call when the child finishes.
    pub(crate) fn release(&self) {
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
    }
}

// ---------------------------------------------------------------------------
// Attenuation: child grant = meet(region envelope, offered kind's caps)
// ---------------------------------------------------------------------------

/// The child agent's capability grant: the meet (greatest lower bound) of
/// the dynamic region's envelope and the spawned kind's declared
/// capabilities. Composes with `AttenuatedDispatcher`'s own frame-nesting
/// meet exactly like a subflow frame — see
/// `docs/superpowers/specs/2026-07-19-subflow-runtime-attenuation-design.md`.
pub(crate) fn child_grant(
    envelope: &CapabilityRequirements,
    kind_caps: &CapabilityRequirements,
) -> CapabilityRequirements {
    CapabilityRequirements {
        declared: tau_domain::package::capability::lattice::meet(
            &envelope.declared,
            &kind_caps.declared,
        ),
    }
}

// ---------------------------------------------------------------------------
// ToolSpec constructor (bypasses #[non_exhaustive] via serde) — mirrors
// agent_loop.rs::make_tool_spec (private to that module, so duplicated here
// rather than exposed cross-module for a two-line helper).
// ---------------------------------------------------------------------------

fn json_to_domain_value(v: serde_json::Value) -> tau_domain::Value {
    serde_json::from_value(v).unwrap_or(tau_domain::Value::Null)
}

fn make_tool_spec(name: &str, description: &str, input_schema: &serde_json::Value) -> ToolSpec {
    let input_schema_domain = json_to_domain_value(input_schema.clone());
    let json = serde_json::json!({
        "name": name,
        "description": description,
        "input_schema": input_schema_domain,
    });
    serde_json::from_value(json).unwrap_or_else(|_| {
        let fallback = serde_json::json!({
            "name": name,
            "description": description,
            "input_schema": tau_domain::Value::Object(Default::default()),
        });
        serde_json::from_value(fallback).expect("fallback ToolSpec must deserialize")
    })
}

// ---------------------------------------------------------------------------
// SpawnTool — one per offered kind in a Dynamic region
// ---------------------------------------------------------------------------

/// A `tau_ports::Tool` implementing `agent.<kind>.spawn`: the LLM-facing
/// surface for spawning a dynamic-region child agent of one offered
/// `kind`. `invoke()` is the runtime gate: bounds/concurrency admission →
/// meet-attenuated capability grant → child `Agent` construction → run.
pub(crate) struct SpawnTool<D> {
    spawn: tau_ir::pipeline::DynamicSpawn,
    envelope: CapabilityRequirements,
    counters: Arc<RegionCounters>,
    region_step: String,
    module: Arc<IrModule>,
    dispatcher: Arc<D>,
    tool_name: String,
}

impl<D> SpawnTool<D>
where
    D: ToolDispatcher + Send + Sync + 'static,
{
    pub(crate) fn new(
        spawn: tau_ir::pipeline::DynamicSpawn,
        envelope: CapabilityRequirements,
        counters: Arc<RegionCounters>,
        region_step: String,
        module: Arc<IrModule>,
        dispatcher: Arc<D>,
    ) -> Self {
        let tool_name = alloc::format!("agent.{}.spawn", spawn.kind);
        Self {
            spawn,
            envelope,
            counters,
            region_step,
            module,
            dispatcher,
            tool_name,
        }
    }
}

impl<D> tau_ports::tool::Tool for SpawnTool<D>
where
    D: ToolDispatcher + Send + Sync + 'static,
{
    type Session = ();

    fn name(&self) -> &str {
        &self.tool_name
    }

    fn schema(&self) -> ToolSpec {
        make_tool_spec(
            &self.tool_name,
            &self.spawn.description,
            &serde_json::json!({
                "type": "object",
                "properties": { "message": { "type": "string" } },
                "required": ["message"]
            }),
        )
    }

    async fn init(&self, _ctx: SessionContext) -> Result<Self::Session, ToolError> {
        Ok(())
    }

    async fn invoke(
        &self,
        _session: &mut Self::Session,
        args: tau_domain::Value,
    ) -> Result<ToolResult, ToolError> {
        // 1. Parse `message: String` from args.
        let message = match args
            .as_object()
            .and_then(|o| o.get("message"))
            .and_then(|v| v.as_string())
        {
            Some(m) => m.to_string(),
            None => {
                return Ok(ToolResult::new(
                    alloc::vec![ToolContent::Text {
                        text: alloc::format!("{}: missing required arg `message`", self.tool_name),
                    }],
                    true,
                ));
            }
        };

        // 2. Bounds/concurrency admission gate — denied BEFORE any child is
        //    constructed.
        let n = match self.counters.try_admit() {
            Ok(n) => n,
            Err(err) => {
                let (reason, spawned_snapshot, max_snapshot, text) = match err {
                    AdmitError::Bounds { spawned, max } => (
                        "bounds",
                        spawned,
                        max,
                        alloc::format!(
                            "spawn denied: region `{}` max_spawns exhausted ({spawned}/{max}) — kind `{}`; proceed with the results you have",
                            self.region_step, self.spawn.kind,
                        ),
                    ),
                    AdmitError::Concurrency { in_flight, max } => (
                        "concurrency",
                        in_flight,
                        max,
                        alloc::format!(
                            "spawn denied: region `{}` max_concurrency exceeded ({in_flight}/{max}) — kind `{}`; proceed with the results you have",
                            self.region_step, self.spawn.kind,
                        ),
                    ),
                };
                tracing::warn!(
                    name = "runtime.dynamic.spawn_denied",
                    region_step = %self.region_step,
                    kind = %self.spawn.kind,
                    reason = %reason,
                    spawned = spawned_snapshot,
                    max_spawns = max_snapshot,
                );
                return Ok(ToolResult::new(
                    alloc::vec![ToolContent::Text { text }],
                    true,
                ));
            }
        };

        // 3-5. Child id, meet-attenuated grant, child Agent node.
        let child_id = alloc::format!("{}:{}#{n}", self.region_step, self.spawn.kind);
        let grant = child_grant(&self.envelope, &self.spawn.capabilities);
        let child_agent = tau_ir::node::Agent {
            id: tau_ir::ids::AgentId(child_id.clone()),
            prompt: self.spawn.prompt.clone(),
            model_ref: self.spawn.model_ref.clone(),
            tool_refs: self.spawn.tool_refs.clone(),
            context: None,
            budget: tau_ir::budget::AgentBudget {
                max_turns: None,
                max_tokens: None,
            },
            produces: alloc::vec::Vec::new(),
            output_schema: None,
            durable: None,
        };

        // 6. Admission accepted.
        tracing::info!(
            name = "runtime.dynamic.spawned",
            region_step = %self.region_step,
            kind = %self.spawn.kind,
            child_id = %child_id,
            spawned = n,
            max_spawns = self.counters.max_spawns,
        );

        // 7. Attenuated dispatcher: the child (and its descendants) run
        //    under `grant`, with its own dynamic-region denial event name.
        let att = Arc::new(AttenuatedDispatcher::new_with_event(
            grant,
            ToolId(self.tool_name.clone()),
            child_id.clone(),
            self.module.clone(),
            self.dispatcher.clone(),
            "runtime.dynamic.attenuation_denied",
        ));

        // 8. Run the child agent. `counters.release()` always runs after
        //    the await, on both success and failure paths.
        let user_msg = user_message(&message);
        let outcome = alloc::boxed::Box::pin(run_agent(
            self.module.clone(),
            &child_agent,
            att,
            alloc::vec![user_msg],
        ))
        .await;
        self.counters.release();

        // 9. Map the outcome to a ToolResult.
        let (is_error, text) = match &outcome {
            Err(e) => (true, alloc::format!("spawn `{child_id}` failed: {e}")),
            Ok(o @ RunOutcome::Failed { status, .. }) => {
                let last_text = last_assistant_text(o);
                let detail = if last_text.is_empty() {
                    alloc::format!("{status:?}")
                } else {
                    last_text
                };
                (true, alloc::format!("spawn `{child_id}` failed: {detail}"))
            }
            Ok(o) => (false, last_assistant_text(o)),
        };

        Ok(ToolResult::new(
            alloc::vec![ToolContent::Text { text }],
            is_error,
        ))
    }

    async fn teardown(&self, _session: Self::Session) -> Result<(), ToolError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::String;
    use alloc::sync::Arc;

    use tau_ir::capability::CapabilityRequirements;
    use tau_ir::module::{IrFormatVersion, IrModule, Workflow};
    use tau_ports::tool::{Tool, ToolContent};

    use crate::error::RuntimeError;
    use crate::interpreter::tool_dispatch::{ToolDispatcher, ToolInvocationResult};

    use super::*;

    // capability from canonical TOML (variant #[non_exhaustive]) — copied
    // from `attenuate.rs`'s test helper of the same name (module-private,
    // not reusable across files).
    fn cap(toml_str: &str) -> tau_domain::Capability {
        #[derive(serde::Deserialize)]
        struct W {
            cap: tau_domain::Capability,
        }
        toml::from_str::<W>(toml_str).expect("cap toml").cap
    }

    /// `Tool::invoke` takes `tau_domain::Value`, not `serde_json::Value` — the
    /// two are distinct types (see `agent_loop.rs::json_to_domain_value`).
    /// Bridge test-literal `serde_json::json!` args through the same
    /// serde round-trip the production dispatcher path uses.
    fn domain_args(v: serde_json::Value) -> tau_domain::Value {
        serde_json::from_value(v).expect("valid domain value")
    }

    fn flatten(content: &[ToolContent]) -> String {
        match &content[0] {
            ToolContent::Text { text } => text.clone(),
            other => panic!("expected Text content, got {other:?}"),
        }
    }

    fn test_spawn(kind: &str) -> tau_ir::pipeline::DynamicSpawn {
        tau_ir::pipeline::DynamicSpawn {
            kind: kind.into(),
            capabilities: CapabilityRequirements::default(),
            description: alloc::format!("Spawns a {kind} agent"),
            prompt: tau_ir::prompt::PromptSource::inline("You are a researcher."),
            model_ref: tau_ir::model_ref::ModelRef {
                backend: "mock".into(),
                model_id: "m".into(),
            },
            tool_refs: alloc::vec::Vec::new(),
        }
    }

    /// Mirrors `attenuate.rs::module_with_tool`'s `IrModule` construction,
    /// with an empty tools map (the child agent's `tool_refs` is empty).
    fn test_module() -> Arc<IrModule> {
        let target = tau_ports::target::list_available()
            .next()
            .expect("at least one available target")
            .triple;
        Arc::new(IrModule {
            ir_format: IrFormatVersion::current(),
            tau_version: env!("CARGO_PKG_VERSION").into(),
            target,
            workflow: Workflow::default(),
            triggers: alloc::vec::Vec::new(),
        })
    }

    /// Dispatcher that panics if a child agent is ever run — proves a
    /// denied spawn never reaches child construction.
    struct PanicDispatcher;
    impl ToolDispatcher for PanicDispatcher {
        fn invoke<'a>(
            &'a self,
            _tool_id: &'a tau_ir::ToolId,
            _args: &'a serde_json::Value,
        ) -> core::pin::Pin<
            alloc::boxed::Box<
                dyn core::future::Future<Output = Result<ToolInvocationResult, RuntimeError>>
                    + Send
                    + 'a,
            >,
        > {
            panic!("child must not run")
        }
        fn llm_backend_for(
            &self,
            _backend: &str,
        ) -> Result<Arc<dyn crate::builder::DynLlmBackend>, RuntimeError> {
            panic!("child must not run")
        }
    }

    #[tokio::test]
    async fn spawn_denied_when_max_spawns_exhausted() {
        // Counters pre-saturated: spawned == max_spawns, so invoke() must deny
        // BEFORE constructing any child (no LLM backend needed).
        let counters = Arc::new(RegionCounters::new(1, 1));
        counters.try_admit().expect("first admit"); // saturate: spawned = 1
        let tool = SpawnTool::new(
            test_spawn("researcher"),
            CapabilityRequirements::default(),
            counters,
            "fanout".into(),
            test_module(),
            Arc::new(PanicDispatcher),
        );
        let mut session = ();
        let res = tool
            .invoke(
                &mut session,
                domain_args(serde_json::json!({"message": "go"})),
            )
            .await
            .expect("soft-deny returns Ok(ToolResult)");
        assert!(res.is_error, "denial must be is_error");
        let text = flatten(&res.content);
        assert!(text.contains("spawn denied"), "{text}");
        assert!(text.contains("max_spawns exhausted (1/1)"), "{text}");
        assert!(text.contains("fanout"), "{text}");
        assert!(text.contains("researcher"), "{text}");
    }

    #[test]
    fn child_grant_is_meet_of_envelope_and_kind() {
        // envelope = net.http hosts=["crates.io"]; kind caps = net.http
        // hosts=any (hand-crafted over-reach) → meet clamps to the
        // envelope's narrower hosts set.
        let envelope = CapabilityRequirements {
            declared: alloc::vec![cap("[cap]\nkind=\"net.http\"\nhosts=[\"crates.io\"]\n")],
        };
        let kind_caps = CapabilityRequirements {
            declared: alloc::vec![cap("[cap]\nkind=\"net.http\"\nhosts=\"any\"\n")],
        };
        let grant = child_grant(&envelope, &kind_caps);
        assert_eq!(grant.declared.len(), 1, "{grant:?}");
        let rendered = alloc::format!("{:?}", grant.declared[0]);
        assert!(
            rendered.contains("crates.io") && !rendered.contains("Any"),
            "meet must clamp to the envelope's narrower hosts set, got {rendered}"
        );
    }

    #[tokio::test]
    async fn spawn_denied_when_concurrency_saturated() {
        let counters = Arc::new(RegionCounters::new(4, 1));
        // Saturate concurrency without releasing — spawned=1, in_flight=1.
        counters.try_admit().expect("first admit");
        let tool = SpawnTool::new(
            test_spawn("researcher"),
            CapabilityRequirements::default(),
            counters,
            "fanout".into(),
            test_module(),
            Arc::new(PanicDispatcher),
        );
        let mut session = ();
        let res = tool
            .invoke(
                &mut session,
                domain_args(serde_json::json!({"message": "go"})),
            )
            .await
            .expect("soft-deny returns Ok(ToolResult)");
        assert!(res.is_error, "denial must be is_error");
        let text = flatten(&res.content);
        assert!(text.contains("spawn denied"), "{text}");
        assert!(text.contains("max_concurrency exceeded (1/1)"), "{text}");
    }

    #[test]
    fn admitted_spawn_indexes_are_sequential() {
        let counters = RegionCounters::new(5, 5);
        assert_eq!(counters.try_admit().expect("admit 0"), 0);
        assert_eq!(counters.try_admit().expect("admit 1"), 1);
        assert_eq!(counters.try_admit().expect("admit 2"), 2);
        // release() only affects in_flight, never the spawned counter.
        counters.release();
        counters.release();
        counters.release();
        assert_eq!(counters.try_admit().expect("admit 3"), 3);
    }
}
