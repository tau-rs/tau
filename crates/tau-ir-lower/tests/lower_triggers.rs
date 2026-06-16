//! Lowering of `[trigger.*]` config into `IrModule.triggers`.

use tau_ir::trigger::TriggerKind;
use tau_ir::IrFormatVersion;
use tau_ir_lower::LowerError;
use tau_ir_lower::{lower_project, Caches};
use tau_pkg::project::ProjectConfig;
use tau_ports::target::registry;

fn caches() -> Caches<'static> {
    Caches {
        native_tool: &|_| None,
        mcp_contract: &|_| None,
        skill: &|_| None,
    }
}

fn target() -> tau_ports::target::TargetTriple {
    registry::list_available().next().unwrap().triple
}

#[test]
fn lowers_cron_trigger_into_module() {
    let toml = r#"
        [project]
        name = "demo"

        [agents.summarizer]
        display_name = "S"
        package      = "p@^0.1"
        llm_backend  = "anthropic"

        [trigger.nightly]
        kind     = "cron"
        agent    = "summarizer"
        schedule = "0 3 * * *"

        [trigger.nightly.retry]
        max_attempts = 3
        backoff      = { strategy = "exponential", base = "30s", max = "10m" }
        dead_letter  = "dlq-sink"
    "#;
    let config = ProjectConfig::parse_str(toml).unwrap();
    let module = lower_project(&config, &target(), &caches()).unwrap();

    assert_eq!(module.triggers.len(), 1);
    let t = &module.triggers[0];
    assert_eq!(t.name, "nightly");
    assert_eq!(t.kind, TriggerKind::Cron);
    assert_eq!(t.agent.0, "summarizer");
    assert_eq!(t.schedule.as_deref(), Some("0 3 * * *"));
    assert_eq!(t.timezone.as_deref(), Some("UTC"));
    let r = t.retry.as_ref().unwrap();
    assert_eq!(r.max_attempts, 3);
    assert_eq!(r.backoff.base, "30s");
    assert_eq!(r.dead_letter.as_deref(), Some("dlq-sink"));
    assert_eq!(
        r.backoff.strategy,
        tau_ir::trigger::BackoffStrategy::Exponential
    );
    assert_eq!(r.backoff.max, "10m");

    // Option B: ir_format is NOT bumped for a trigger-bearing module.
    assert_eq!(module.ir_format.0, IrFormatVersion::CURRENT);
}

#[test]
fn trigger_less_module_keeps_v1_0_0() {
    let toml = r#"
        [project]
        name = "demo"
        [agents.a]
        display_name = "A"
        package = "p@^0.1"
        llm_backend = "anthropic"
    "#;
    let config = ProjectConfig::parse_str(toml).unwrap();
    let module = lower_project(&config, &target(), &caches()).unwrap();
    assert!(module.triggers.is_empty());
    assert_eq!(module.ir_format.0, IrFormatVersion::CURRENT);
}

#[test]
fn triggers_are_sorted_by_name() {
    let toml = r#"
        [project]
        name = "demo"
        [agents.a]
        display_name = "A"
        package = "p@^0.1"
        llm_backend = "anthropic"
        [trigger.zeta]
        kind = "manual"
        agent = "a"
        [trigger.alpha]
        kind = "manual"
        agent = "a"
    "#;
    let config = ProjectConfig::parse_str(toml).unwrap();
    let module = lower_project(&config, &target(), &caches()).unwrap();
    let names: Vec<&str> = module.triggers.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, vec!["alpha", "zeta"], "canonical order is by name");
    assert!(
        module.triggers.iter().all(|t| t.timezone.is_none()),
        "manual triggers should have no timezone"
    );
}

#[test]
fn rejects_trigger_referencing_unknown_agent() {
    let toml = r#"
        [project]
        name = "demo"
        [agents.a]
        display_name = "A"
        package = "p@^0.1"
        llm_backend = "anthropic"
        [trigger.t]
        kind = "manual"
        agent = "ghost"
    "#;
    let config = ProjectConfig::parse_str(toml).unwrap();
    let err = lower_project(&config, &target(), &caches()).unwrap_err();
    assert!(
        matches!(&err, LowerError::UnknownTriggerAgent { trigger, agent }
            if trigger == "t" && agent.0 == "ghost"),
        "expected UnknownTriggerAgent; got {err:?}"
    );
}
