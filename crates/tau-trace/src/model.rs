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
    /// Owning agent for each span, index-aligned with `spans`. `None` for
    /// spans with no known owner (e.g. a `ToolCall` whose top-level
    /// `TraceEvent.agent_id` was `None`). Used, together with
    /// `agent_parent`, to (re)compute `Span::parent` in a full pass after
    /// every `apply` — see `resolve_parents`. Real emission order (traced
    /// against `tau-runtime-core`) has a `Spawn` arrive before the child's
    /// own `Turn`s, and mid-turn `ToolCall`s arrive before the `Turn` event
    /// that summarizes them, so parent resolution cannot be a one-shot
    /// lookup at push time — it must be re-derivable from whatever has
    /// arrived so far, regardless of order.
    owners: Vec<Option<AgentId>>,
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
                self.push_span(
                    Span {
                        id: 0,
                        kind: SpanKind::Agent,
                        label: agent_id.clone(),
                        start,
                        end: Some(event.ts),
                        tokens: Some(*tokens),
                        capability: None,
                        parent: None, // filled in by resolve_parents below
                        status: SpanStatus::Ok,
                    },
                    Some(agent_id.clone()),
                );
            }
            TraceEventKind::ToolCall {
                tool_name,
                duration_ms,
                status,
                capability,
            } => {
                let start = event.ts - Duration::milliseconds(*duration_ms as i64);
                self.push_span(
                    Span {
                        id: 0,
                        kind: SpanKind::Tool,
                        label: tool_name.clone(),
                        start,
                        end: Some(event.ts),
                        tokens: None,
                        capability: capability.clone(),
                        parent: None, // filled in by resolve_parents below
                        status: status_from_str(status),
                    },
                    event.agent_id.clone(),
                );
            }
            TraceEventKind::Spawn { child_id, .. } => {
                if let Some(parent_id) = &event.agent_id {
                    self.agent_parent
                        .insert(child_id.clone(), parent_id.clone());
                }
            }
            TraceEventKind::Completion { agent_id, status } => {
                if let Some(&idx) = self.agent_anchors().get(agent_id.as_str()) {
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

        // Re-derive every span's parent from the full current state rather
        // than pinning it once at push time. Real trace order can deliver a
        // `Spawn` before the parent's own `Turn`, or a `ToolCall` before the
        // `Turn` that summarizes it — a one-shot lookup at push time leaves
        // those permanently un-parented. O(n) per event is fine at
        // TUI-sized trace volumes.
        self.resolve_parents();
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

    /// Each agent's representative span — the earliest `Agent`-kind span
    /// owned by that agent id — used as the nesting anchor both for that
    /// agent's own `Spawn`ed children and for its `Tool` spans. Recomputed
    /// on demand (not cached) so it always reflects whatever spans exist
    /// so far, independent of arrival order.
    fn agent_anchors(&self) -> HashMap<&str, usize> {
        let mut anchors: HashMap<&str, usize> = HashMap::new();
        for (i, owner) in self.owners.iter().enumerate() {
            if self.spans[i].kind != SpanKind::Agent {
                continue;
            }
            if let Some(owner) = owner {
                anchors.entry(owner.as_str()).or_insert(i);
            }
        }
        anchors
    }

    /// Recompute `Span::parent` for every span from the current
    /// `owners`/`agent_parent`/anchor state. A `Tool` span's parent is its
    /// owning agent's anchor span; an `Agent` span's parent is the anchor
    /// span of whichever agent spawned it (per `agent_parent`), if known.
    fn resolve_parents(&mut self) {
        let anchors = self.agent_anchors();
        let mut parents = vec![None; self.spans.len()];
        for (i, owner) in self.owners.iter().enumerate() {
            let Some(owner) = owner else { continue };
            let parent = match self.spans[i].kind {
                SpanKind::Tool => anchors.get(owner.as_str()).copied(),
                SpanKind::Agent => self
                    .agent_parent
                    .get(owner)
                    .and_then(|p| anchors.get(p.as_str()).copied()),
                _ => None,
            };
            // Guard against a self-loop (e.g. malformed/self-spawned data).
            parents[i] = parent.filter(|&p| p != i);
        }
        for (span, parent) in self.spans.iter_mut().zip(parents) {
            span.parent = parent;
        }
    }

    fn push_span(&mut self, mut span: Span, owner: Option<AgentId>) -> usize {
        let idx = self.spans.len();
        span.id = idx;
        self.spans.push(span);
        self.owners.push(owner);
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

    #[test]
    fn spawn_before_parent_turn_still_nests_child_under_parent() {
        let mut m = TraceModel::new();
        let mut spawn_ev = ev(
            100,
            TraceEventKind::Spawn {
                child_id: "child".into(),
                agent_kind: "worker".into(),
                grant_size: 0,
            },
        );
        spawn_ev.agent_id = Some("parent".into());
        m.apply(&spawn_ev);
        m.apply(&ev(
            101,
            TraceEventKind::Turn {
                agent_id: "child".into(),
                turn_index: 0,
                duration_ms: 1,
                tokens: 5,
            },
        ));
        m.apply(&ev(
            105,
            TraceEventKind::Turn {
                agent_id: "parent".into(),
                turn_index: 0,
                duration_ms: 5,
                tokens: 20,
            },
        ));
        let child_span = &m.spans()[0];
        assert_eq!(child_span.parent, Some(1)); // parent's span index; currently None
    }

    #[test]
    fn tool_call_before_its_turn_still_nests_under_that_turn() {
        let mut m = TraceModel::new();
        let mut call_ev = ev(
            100,
            TraceEventKind::ToolCall {
                tool_name: "t".into(),
                duration_ms: 10,
                status: "ok".into(),
                capability: None,
            },
        );
        call_ev.agent_id = Some("a".into());
        m.apply(&call_ev);
        m.apply(&ev(
            105,
            TraceEventKind::Turn {
                agent_id: "a".into(),
                turn_index: 0,
                duration_ms: 5,
                tokens: 3,
            },
        ));
        let tool_span = &m.spans()[0];
        assert_eq!(tool_span.parent, Some(1)); // the turn span; mid-turn tool calls precede it
    }
}
