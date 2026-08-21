//! Span tree + time-axis math over `tau_ports::TraceEvent`.
//!
//! [`TraceModel`] is a pure, incremental fold: [`TraceModel::apply`] consumes
//! one [`tau_ports::TraceEvent`] at a time and appends/updates [`Span`]s. It
//! holds no clock, no I/O, no runtime dependency — callers (e.g. a `tau run`
//! event subscriber, or a replay of a recorded trace file) drive it.

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};
use tau_ports::{AgentId, CapabilityVerdict, TraceEvent, TraceEventKind};

/// What a [`Span`] represents in the execution tree.
///
/// `Reasoning`, `Branch`, `Parallel`, `Loop`, and `Suspend` are not produced
/// by any [`TraceEventKind`] as of M1 — they exist so a later milestone can
/// add new `apply` arms (e.g. control-flow spans) without breaking this
/// enum's shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpanKind {
    /// One agent turn.
    Agent,
    /// One tool call.
    Tool,
    /// A model reasoning step (not yet produced; reserved for M2).
    Reasoning,
    /// A `Branch` control-flow node (not yet produced; reserved).
    Branch,
    /// A `Parallel` control-flow node (not yet produced; reserved).
    Parallel,
    /// A `Loop` control-flow node (not yet produced; reserved).
    Loop,
    /// A `Suspend` control-flow node (not yet produced; reserved).
    Suspend,
}

/// Terminal (or in-flight) status of a [`Span`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpanStatus {
    /// Started, not yet observed to complete.
    Running,
    /// Completed successfully.
    Ok,
    /// Completed with an error/failure.
    Failed,
}

/// One renderable node in the execution trace.
#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    /// Index into [`TraceModel::spans`]; stable for the life of the model.
    pub id: usize,
    /// What kind of unit of work this span represents.
    pub kind: SpanKind,
    /// Human-readable label (agent id, tool name, ...).
    pub label: String,
    /// Derived start time (`ts - duration`).
    pub start: DateTime<Utc>,
    /// Derived end time; `None` while still running.
    pub end: Option<DateTime<Utc>>,
    /// Token count, if this span carries one (turns only).
    pub tokens: Option<u64>,
    /// Governance verdict, if this span was capability-gated (tool calls only).
    pub capability: Option<CapabilityVerdict>,
    /// Parent span id, for tree rendering.
    pub parent: Option<usize>,
    /// Current status.
    pub status: SpanStatus,
}

/// Incremental fold of `TraceEvent`s into a renderable span tree + time axis.
#[derive(Debug, Default)]
pub struct TraceModel {
    spans: Vec<Span>,
    /// Each agent's own representative span (the first `Turn` span emitted
    /// for that agent id), used to resolve `parent` for children and for
    /// tool calls emitted on that agent's behalf.
    agent_span: HashMap<AgentId, usize>,
    /// child agent id -> parent agent id, populated by `Spawn` events.
    agent_parent: HashMap<AgentId, AgentId>,
    /// Max `ts` seen across every applied event (not just span bounds) —
    /// the upper bound of the time axis while spans are still running.
    max_ts: Option<DateTime<Utc>>,
}

impl TraceModel {
    /// Empty model.
    pub fn new() -> Self {
        Self::default()
    }

    /// Fold one event into the model, appending or updating spans.
    pub fn apply(&mut self, event: &TraceEvent) {
        self.max_ts = Some(match self.max_ts {
            Some(t) if t >= event.ts => t,
            _ => event.ts,
        });

        match &event.kind {
            TraceEventKind::Turn {
                agent_id,
                duration_ms,
                tokens,
                ..
            } => {
                let start = event.ts - Duration::milliseconds(*duration_ms as i64);
                let parent = self.parent_for_agent(agent_id);
                let idx = self.push_span(Span {
                    id: 0,
                    kind: SpanKind::Agent,
                    label: agent_id.clone(),
                    start,
                    end: Some(event.ts),
                    tokens: Some(*tokens),
                    capability: None,
                    parent,
                    status: SpanStatus::Ok,
                });
                self.agent_span.entry(agent_id.clone()).or_insert(idx);
            }
            TraceEventKind::ToolCall {
                tool_name,
                duration_ms,
                status,
                capability,
            } => {
                let start = event.ts - Duration::milliseconds(*duration_ms as i64);
                let parent = event
                    .agent_id
                    .as_ref()
                    .and_then(|aid| self.agent_span.get(aid).copied());
                self.push_span(Span {
                    id: 0,
                    kind: SpanKind::Tool,
                    label: tool_name.clone(),
                    start,
                    end: Some(event.ts),
                    tokens: None,
                    capability: capability.clone(),
                    parent,
                    status: status_from_str(status),
                });
            }
            TraceEventKind::Spawn { child_id, .. } => {
                if let Some(parent_id) = &event.agent_id {
                    self.agent_parent
                        .insert(child_id.clone(), parent_id.clone());
                }
            }
            TraceEventKind::Completion { agent_id, status } => {
                if let Some(&idx) = self.agent_span.get(agent_id) {
                    self.spans[idx].status = status_from_str(status);
                }
            }
            // Abort/budget/orphan/task-mutation/plan-note: no bar in M1.
            // Forward-compat catch-all for any future TraceEventKind variant.
            TraceEventKind::Abort { .. }
            | TraceEventKind::TaskMutation { .. }
            | TraceEventKind::PlanNote { .. }
            | TraceEventKind::BudgetWarn { .. }
            | TraceEventKind::BudgetExceeded { .. }
            | TraceEventKind::OrphanedTasksAtTermination { .. } => {}
        }
    }

    /// All spans in emission order; index == [`Span::id`].
    pub fn spans(&self) -> &[Span] {
        &self.spans
    }

    /// The time axis' bounds: `(min start, max end)`.
    ///
    /// A span still running (`end.is_none()`) contributes the max `ts` seen
    /// across every applied event as its provisional end, so the axis keeps
    /// extending as more events arrive.
    pub fn window(&self) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
        if self.spans.is_empty() {
            return None;
        }
        let lo = self.spans.iter().map(|s| s.start).min()?;
        let fallback_hi = self.max_ts.unwrap_or(lo);
        let hi = self
            .spans
            .iter()
            .map(|s| s.end.unwrap_or(fallback_hi))
            .max()?
            .max(fallback_hi);
        Some((lo, hi))
    }

    /// Map `span`'s `[start, end]` onto `[0, width_cols]` given the current
    /// [`TraceModel::window`]. Returns `(offset_cols, len_cols)`, both
    /// clamped to `[0, width_cols]`. A zero-length span renders `len == 1`.
    pub fn bar(&self, span: &Span, width_cols: u16) -> (u16, u16) {
        let Some((lo, hi)) = self.window() else {
            return (0, 0);
        };
        let width = i64::from(width_cols);
        let total_ms = (hi - lo).num_milliseconds().max(1);

        let to_col = |t: DateTime<Utc>| -> i64 {
            let ms = (t - lo).num_milliseconds();
            (ms * width / total_ms).clamp(0, width)
        };

        let offset = to_col(span.start);
        let end_off = to_col(span.end.unwrap_or(hi)).max(offset);
        let mut len = (end_off - offset).max(1);
        if offset + len > width {
            len = (width - offset).max(1);
        }
        (offset as u16, len as u16)
    }

    /// Resolve the parent span index for `agent_id`: the representative
    /// span of whichever agent spawned it, if known.
    fn parent_for_agent(&self, agent_id: &str) -> Option<usize> {
        let parent_agent = self.agent_parent.get(agent_id)?;
        self.agent_span.get(parent_agent).copied()
    }

    fn push_span(&mut self, mut span: Span) -> usize {
        let idx = self.spans.len();
        span.id = idx;
        self.spans.push(span);
        idx
    }
}

fn status_from_str(status: &str) -> SpanStatus {
    match status {
        "ok" | "completed" => SpanStatus::Ok,
        _ => SpanStatus::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use tau_ports::{CapabilityVerdict, TraceEvent, TraceEventKind};

    fn ev(secs: i64, kind: TraceEventKind) -> TraceEvent {
        TraceEvent {
            id: "x".into(),
            ts: Utc.timestamp_opt(secs, 0).unwrap(),
            run_id: Default::default(),
            agent_id: None,
            kind,
        }
    }

    #[test]
    fn tool_call_becomes_span_with_derived_start_and_verdict() {
        let mut m = TraceModel::new();
        m.apply(&ev(
            100,
            TraceEventKind::ToolCall {
                tool_name: "net.http".into(),
                duration_ms: 2000,
                status: "ok".into(),
                capability: Some(CapabilityVerdict::Drop {
                    reason: "egress".into(),
                }),
            },
        ));
        let s = &m.spans()[0];
        assert!(matches!(s.kind, SpanKind::Tool));
        assert_eq!(s.end.unwrap(), Utc.timestamp_opt(100, 0).unwrap());
        // start = ts - duration
        assert_eq!(s.start, Utc.timestamp_opt(98, 0).unwrap());
        assert!(matches!(s.status, SpanStatus::Ok));
        assert!(s.capability.is_some());
    }

    #[test]
    fn window_spans_min_start_to_max_end() {
        let mut m = TraceModel::new();
        m.apply(&ev(
            100,
            TraceEventKind::Turn {
                agent_id: Default::default(),
                turn_index: 0,
                duration_ms: 1000,
                tokens: 10,
            },
        ));
        m.apply(&ev(
            105,
            TraceEventKind::ToolCall {
                tool_name: "t".into(),
                duration_ms: 500,
                status: "ok".into(),
                capability: None,
            },
        ));
        let (lo, hi) = m.window().unwrap();
        assert_eq!(lo, Utc.timestamp_opt(99, 0).unwrap()); // 100-1s
        assert_eq!(hi, Utc.timestamp_opt(105, 0).unwrap()); // 105 end
    }

    #[test]
    fn bar_maps_span_onto_column_width() {
        let mut m = TraceModel::new();
        m.apply(&ev(
            100,
            TraceEventKind::Turn {
                agent_id: Default::default(),
                turn_index: 0,
                duration_ms: 0,
                tokens: 0,
            },
        )); // point at t=100
        m.apply(&ev(
            110,
            TraceEventKind::Turn {
                agent_id: Default::default(),
                turn_index: 1,
                duration_ms: 0,
                tokens: 0,
            },
        )); // point at t=110
        let last = m.spans()[1].clone();
        let (off, _len) = m.bar(&last, 100);
        assert_eq!(off, 100); // t=110 is the right edge of a 100-col window [100,110]
    }
}
