//! npm/cargo-style line-feed printer for multi-agent runs.
//!
//! Subscribes to a TraceEvent receiver, renders one line per significant
//! event, prints a summary table at end. No TUI; no cursor magic. Pipe-
//! friendly via space-padded alignment.

use std::collections::BTreeMap;

use tau_ports::{RunSnapshot, TraceEvent, TraceEventKind};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::output::Output;

/// Per-agent aggregated stats for the summary table.
#[derive(Default, Clone)]
pub struct AgentStats {
    /// Number of turns completed.
    pub turns: u32,
    /// Total duration across all turns in milliseconds.
    pub duration_ms: u64,
    /// Total tokens used.
    pub tokens: u64,
}

/// Drain `rx` until the channel closes, rendering the trace live through
/// `output`. Returns the final aggregated stats keyed by agent id.
///
/// Honors `--json`: in human mode one padded line is emitted per
/// significant event via [`Output::human`]; in JSON mode each raw
/// [`TraceEvent`] is emitted as one JSON object per line via
/// [`Output::json`] (mirroring the JSONL run-log shape). Stats are
/// aggregated in both modes so [`print_summary`] can render totals.
pub async fn run_printer(
    mut rx: UnboundedReceiver<TraceEvent>,
    output: &mut Output,
) -> BTreeMap<String, AgentStats> {
    let mut stats: BTreeMap<String, AgentStats> = BTreeMap::new();
    let json = output.is_json();

    while let Some(event) = rx.recv().await {
        // Aggregate stats in every mode so the summary is populated even
        // under `--json`, where the human table is suppressed.
        match &event.kind {
            TraceEventKind::Turn {
                agent_id,
                duration_ms,
                tokens,
                ..
            } => {
                let entry = stats.entry(agent_id.clone()).or_default();
                entry.turns += 1;
                entry.duration_ms += *duration_ms;
                entry.tokens += *tokens;
            }
            TraceEventKind::Completion { agent_id, .. } => {
                stats.entry(agent_id.clone()).or_default();
            }
            _ => {}
        }

        if json {
            // One JSON object per event; the run future's live emissions
            // stream to stdout as they arrive.
            let _ = output.json(&event);
            continue;
        }

        let line = match &event.kind {
            TraceEventKind::Spawn {
                child_id,
                agent_kind,
                ..
            } => format!(
                "  \u{25c6} {:<60} spawned",
                format!("{agent_kind} ({child_id})")
            ),
            TraceEventKind::Turn {
                agent_id,
                turn_index,
                duration_ms,
                tokens,
            } => format!(
                "        Turn {agent_id}: {} ({:.1}s \u{00b7} {tokens} tok)",
                turn_index + 1,
                *duration_ms as f64 / 1000.0
            ),
            TraceEventKind::ToolCall {
                tool_name,
                duration_ms,
                status,
            } => {
                let marker = if status == "ok" { "  " } else { "\u{2717} " };
                format!(
                    "        {}Tool {tool_name:<30} {:.1}s",
                    marker,
                    *duration_ms as f64 / 1000.0
                )
            }
            TraceEventKind::TaskMutation { task_id, mutation } => {
                let icon = match mutation.as_str() {
                    "created" => "\u{2514} task created:",
                    "claimed" => "\u{2514} task claimed:",
                    "completed" => "\u{2514} task done:   ",
                    "failed" => "\u{2514} task failed: ",
                    "discarded" => "\u{2514} task discarded:",
                    _ => "\u{2514} task event:  ",
                };
                format!("    {icon} [{task_id}]")
            }
            TraceEventKind::PlanNote { snippet } => format!("        plan: {snippet}"),
            TraceEventKind::BudgetWarn {
                budget,
                current,
                limit,
            } => format!("    \u{26a0} budget {budget}: {current} / {limit}"),
            TraceEventKind::BudgetExceeded {
                budget,
                final_value,
                limit,
            } => format!("    \u{2717} budget {budget} EXCEEDED: {final_value} > {limit}"),
            TraceEventKind::Completion { agent_id, status } => {
                let icon = if status == "completed" {
                    "\u{2713}"
                } else {
                    "\u{2717}"
                };
                let entry = stats.entry(agent_id.clone()).or_default();
                format!(
                    "  {icon} {agent_id:<60} {:.1}s \u{00b7} {} tok",
                    entry.duration_ms as f64 / 1000.0,
                    entry.tokens
                )
            }
            TraceEventKind::Abort { reason } => format!("  \u{2717} aborted: {reason}"),
            TraceEventKind::OrphanedTasksAtTermination { task_ids } => {
                format!("  \u{26a0} orphaned tasks: {task_ids:?}")
            }
        };
        let _ = output.human(&line);
    }

    stats
}

/// Print the summary after the run completes.
///
/// Honors `--json`: in human mode a padded table is written via
/// [`Output::human`]; in JSON mode a single `{"event":"summary", ...}`
/// object carrying the run totals and per-agent stats is emitted via
/// [`Output::json`].
pub fn print_summary(
    snapshot: &RunSnapshot,
    stats: &BTreeMap<String, AgentStats>,
    output: &mut Output,
) {
    if output.is_json() {
        let agents: Vec<serde_json::Value> = stats
            .iter()
            .map(|(agent_id, s)| {
                serde_json::json!({
                    "agent_id": agent_id,
                    "turns": s.turns,
                    "duration_ms": s.duration_ms,
                    "tokens": s.tokens,
                })
            })
            .collect();
        let _ = output.json(&serde_json::json!({
            "event": "summary",
            "run_id": snapshot.run_id,
            "status": format!("{:?}", snapshot.status),
            "tokens_used": snapshot.tokens_used,
            "elapsed_secs": snapshot.elapsed_secs,
            "agents_spawned": snapshot.agents_spawned,
            "agents": agents,
        }));
        return;
    }

    let rule = "  \u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}";
    let _ = output.human("");
    let _ = output.human(rule);
    let _ = output.human(&format!(
        "  Summary                                          {} tok \u{00b7} {:.1}s",
        snapshot.tokens_used, snapshot.elapsed_secs
    ));
    let _ = output.human("");
    let id_w = stats
        .keys()
        .map(String::len)
        .max()
        .unwrap_or(0)
        .max("agent".len())
        .max(16);
    let _ = output.human(&format!(
        "      {:<id_w$}  turns    duration    tokens",
        "agent"
    ));
    for (agent_id, s) in stats {
        let _ = output.human(&format!(
            "      {:<id_w$}  {:>5}   {:>7.1}s   {:>7}",
            agent_id,
            s.turns,
            s.duration_ms as f64 / 1000.0,
            s.tokens
        ));
    }
    let _ = output.human(rule);
    let _ = output.human("");
    let _ = output.human(&format!("  run_id: {}", snapshot.run_id));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::{ColorChoice, Output};
    use chrono::Utc;
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use tau_ports::{RunBudget, RunStatus};
    use tokio::sync::mpsc;

    /// Shared writer that captures bytes and is `Send + Write`.
    #[derive(Clone, Default)]
    struct SharedBuf(Arc<Mutex<Vec<u8>>>);

    impl Write for SharedBuf {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl SharedBuf {
        fn snapshot(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    /// Build an `Output` writing to a capturable stdout buffer.
    fn test_output(json: bool) -> (Output, SharedBuf) {
        let stdout = SharedBuf::default();
        let out = Output::with_writers(
            Box::new(stdout.clone()),
            Box::new(SharedBuf::default()),
            json,
            false,
            ColorChoice::Never,
        );
        (out, stdout)
    }

    fn evt(kind: TraceEventKind) -> TraceEvent {
        TraceEvent {
            id: "e".into(),
            ts: Utc::now(),
            run_id: "r".into(),
            agent_id: None,
            kind,
        }
    }

    fn snapshot_fixture() -> RunSnapshot {
        RunSnapshot {
            run_id: "run_1".into(),
            root_agent_id: "root".into(),
            task_list: vec![],
            plan: String::new(),
            budget: RunBudget {
                max_total_tokens: None,
                max_total_duration_secs: None,
                max_total_agents: None,
            },
            tokens_used: 42,
            elapsed_secs: 3,
            agents_spawned: 1,
            status: RunStatus::Completed,
            started_at: Utc::now(),
            ended_at: None,
        }
    }

    #[tokio::test]
    async fn printer_drains_events() {
        let (tx, rx) = mpsc::unbounded_channel();
        tx.send(evt(TraceEventKind::Spawn {
            child_id: "agent_x".into(),
            agent_kind: "researcher".into(),
            grant_size: 2,
        }))
        .unwrap();
        tx.send(evt(TraceEventKind::Completion {
            agent_id: "agent_x".into(),
            status: "completed".into(),
        }))
        .unwrap();
        drop(tx);
        let (mut out, _) = test_output(false);
        let stats = run_printer(rx, &mut out).await;
        assert!(stats.contains_key("agent_x"));
    }

    #[tokio::test]
    async fn printer_human_mode_renders_lines() {
        let (tx, rx) = mpsc::unbounded_channel();
        tx.send(evt(TraceEventKind::Spawn {
            child_id: "agent_x".into(),
            agent_kind: "researcher".into(),
            grant_size: 2,
        }))
        .unwrap();
        drop(tx);
        let (mut out, stdout) = test_output(false);
        run_printer(rx, &mut out).await;
        let s = stdout.snapshot();
        assert!(s.contains("spawned"), "human line missing: {s:?}");
        assert!(
            !s.trim_start().starts_with('{'),
            "human mode must not emit JSON: {s:?}"
        );
    }

    #[tokio::test]
    async fn printer_human_mode_turn_line_includes_tokens() {
        let (tx, rx) = mpsc::unbounded_channel();
        tx.send(evt(TraceEventKind::Turn {
            agent_id: "orchestrator".into(),
            turn_index: 1,
            duration_ms: 300,
            tokens: 20,
        }))
        .unwrap();
        drop(tx);
        let (mut out, stdout) = test_output(false);
        run_printer(rx, &mut out).await;
        let s = stdout.snapshot();
        assert!(
            s.contains("\u{00b7} 20 tok"),
            "turn line missing token count: {s:?}"
        );
    }

    #[tokio::test]
    async fn printer_json_mode_emits_json_lines_not_human() {
        let (tx, rx) = mpsc::unbounded_channel();
        tx.send(evt(TraceEventKind::Spawn {
            child_id: "agent_x".into(),
            agent_kind: "researcher".into(),
            grant_size: 2,
        }))
        .unwrap();
        drop(tx);
        let (mut out, stdout) = test_output(true);
        run_printer(rx, &mut out).await;
        let s = stdout.snapshot();
        // Raw TraceEvent JSON: enum internally tagged with `kind`.
        assert!(
            s.contains("\"kind\":\"spawn\""),
            "json event missing: {s:?}"
        );
        assert!(
            !s.contains("spawned"),
            "json mode must not emit human lines: {s:?}"
        );
        // Each event is one JSON object per line.
        assert!(s.trim_start().starts_with('{'), "not a JSON line: {s:?}");
    }

    #[tokio::test]
    async fn printer_json_mode_still_aggregates_stats() {
        let (tx, rx) = mpsc::unbounded_channel();
        tx.send(evt(TraceEventKind::Turn {
            agent_id: "a".into(),
            turn_index: 0,
            duration_ms: 1500,
            tokens: 42,
        }))
        .unwrap();
        drop(tx);
        let (mut out, _) = test_output(true);
        let stats = run_printer(rx, &mut out).await;
        let a = stats.get("a").expect("stats aggregated in json mode");
        assert_eq!(a.turns, 1);
        assert_eq!(a.duration_ms, 1500);
        assert_eq!(a.tokens, 42);
    }

    #[test]
    fn summary_human_mode_prints_table() {
        let (mut out, stdout) = test_output(false);
        let mut stats: BTreeMap<String, AgentStats> = BTreeMap::new();
        stats.insert(
            "a".into(),
            AgentStats {
                turns: 2,
                duration_ms: 3000,
                tokens: 0,
            },
        );
        print_summary(&snapshot_fixture(), &stats, &mut out);
        let s = stdout.snapshot();
        assert!(s.contains("Summary"), "summary header missing: {s:?}");
        assert!(s.contains("run_id: run_1"), "run_id line missing: {s:?}");
    }

    #[test]
    fn summary_human_mode_aligns_columns_for_long_agent_ids() {
        let (mut out, stdout) = test_output(false);
        let mut stats: BTreeMap<String, AgentStats> = BTreeMap::new();
        // Short id (12 chars) and a spawned-child id (21 chars) that
        // overflows the historical 16-char pad.
        stats.insert(
            "orchestrator".into(),
            AgentStats {
                turns: 2,
                duration_ms: 0,
                tokens: 40,
            },
        );
        stats.insert(
            "orchestrator-01kzjwjg".into(),
            AgentStats {
                turns: 1,
                duration_ms: 0,
                tokens: 20,
            },
        );
        print_summary(&snapshot_fixture(), &stats, &mut out);
        let s = stdout.snapshot();
        let rows: Vec<&str> = s
            .lines()
            .filter(|l| l.trim_start().starts_with("orchestrator"))
            .collect();
        assert_eq!(rows.len(), 2, "expected two agent rows: {s:?}");
        // Every field is fixed-width once the id column absorbs the widest
        // id, so aligned rows have identical rendered length.
        assert_eq!(
            rows[0].chars().count(),
            rows[1].chars().count(),
            "agent rows misaligned when an id exceeds the pad: {rows:?}"
        );
    }

    #[test]
    fn summary_json_mode_emits_json_object() {
        let (mut out, stdout) = test_output(true);
        let stats: BTreeMap<String, AgentStats> = BTreeMap::new();
        print_summary(&snapshot_fixture(), &stats, &mut out);
        let s = stdout.snapshot();
        assert!(
            s.contains("\"event\":\"summary\""),
            "summary json missing: {s:?}"
        );
        assert!(
            !s.contains("run_id: run_1"),
            "json mode must not emit human summary line: {s:?}"
        );
    }
}
