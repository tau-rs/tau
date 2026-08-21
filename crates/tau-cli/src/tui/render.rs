//! Pure frame rendering for the execution-trace waterfall TUI.
//!
//! [`draw`] renders a [`tau_trace::TraceModel`] snapshot plus [`UiState`]
//! into a ratatui [`Frame`]. It performs no terminal I/O — no raw-mode
//! setup, no alternate-screen switch, no stdout writes — so the whole
//! layout is exercisable with ratatui's `TestBackend`, as the test below
//! does. Terminal setup/teardown and the input event loop belong to the
//! caller (a later task), not here.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, Wrap};
use ratatui::Frame;

use tau_ports::CapabilityVerdict;
use tau_trace::{Span as TraceSpan, SpanKind, SpanStatus, TraceModel};

/// Fixed column width (in cells) of the waterfall bar, and the
/// `width_cols` passed to [`TraceModel::bar`] for every row so the
/// glyph string built in [`bar_cell`] lines up with the column.
const BAR_WIDTH: u16 = 20;

/// Which spans are visible in the waterfall table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Filter {
    /// Show every span.
    #[default]
    All,
    /// Only spans whose status is `Failed`.
    Errors,
    /// Only `Tool`-kind spans.
    Tools,
    /// Only `Reasoning`-kind spans.
    Reasoning,
}

impl Filter {
    /// Short label rendered as this filter's toolbar chip.
    fn label(self) -> &'static str {
        match self {
            Filter::All => "All",
            Filter::Errors => "Errors",
            Filter::Tools => "Tools",
            Filter::Reasoning => "Reasoning",
        }
    }
}

/// Whether `span` is visible under `filter` + a `search` substring match
/// over `span.label`.
///
/// The single shared predicate for span visibility — `draw`'s table/detail
/// filtering and `app::App`'s `visible_len`/selection-clamp both call this
/// so the two can never drift out of sync (they used to be two hand-kept
/// copies of the same match arms; see the M1 final-review fix wave).
pub(crate) fn span_matches(filter: Filter, span: &TraceSpan, search: &str) -> bool {
    let filter_matches = match filter {
        Filter::All => true,
        Filter::Errors => matches!(span.status, SpanStatus::Failed),
        Filter::Tools => matches!(span.kind, SpanKind::Tool),
        Filter::Reasoning => matches!(span.kind, SpanKind::Reasoning),
    };
    filter_matches && (search.is_empty() || span.label.contains(search))
}

/// Interactive state that [`draw`] renders. Owned and mutated by the
/// event loop (a later task); this module only ever reads it.
#[derive(Debug, Clone, Default)]
pub struct UiState {
    /// Index into the *filtered* (currently visible) span list that is
    /// highlighted in the table and shown in the detail pane.
    pub selected: usize,
    /// Active row filter.
    pub filter: Filter,
    /// Free-text search over span labels (substring match); empty means
    /// no search filtering.
    pub search: String,
    /// Vertical scroll offset (in rows) into the detail pane.
    pub scroll: u16,
}

/// Render `model` + `ui` into `frame`.
///
/// Pure frame rendering: lays out a toolbar (run summary, filter chips,
/// search box), a waterfall table (`Name | Tokens | Dur | Cap | Bar`)
/// filtered by `ui.filter`/`ui.search`, and a detail pane for the span at
/// `ui.selected`. Performs no terminal I/O, so it is safe to drive with
/// `ratatui::backend::TestBackend`.
pub fn draw(frame: &mut Frame, model: &TraceModel, ui: &UiState) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(4),
            Constraint::Length(3),
        ])
        .split(area);

    draw_toolbar(frame, chunks[0], ui);

    let visible: Vec<&TraceSpan> = model
        .spans()
        .iter()
        .filter(|s| span_matches(ui.filter, s, &ui.search))
        .collect();

    draw_table(frame, chunks[1], model, &visible, ui);
    draw_detail(frame, chunks[2], &visible, ui);
}

/// Toolbar: static title, one chip per [`Filter`] (the active one
/// bracketed), and the current search text.
fn draw_toolbar(frame: &mut Frame, area: Rect, ui: &UiState) {
    let chips = [
        Filter::All,
        Filter::Errors,
        Filter::Tools,
        Filter::Reasoning,
    ]
    .iter()
    .map(|f| {
        if *f == ui.filter {
            format!("[{}]", f.label())
        } else {
            format!(" {} ", f.label())
        }
    })
    .collect::<Vec<_>>()
    .join(" ");
    let text = format!("tau trace   {chips}   search: {}", ui.search);
    let block = Block::default().borders(Borders::ALL).title("tau trace");
    frame.render_widget(Paragraph::new(text).block(block), area);
}

/// Waterfall table: one row per visible span, columns
/// `Name | Tokens | Dur | Cap | Bar`. The `Cap` cell is colored by
/// [`CapabilityVerdict`] (green allow / amber clamp / red drop); the
/// currently-selected row is reverse-video highlighted.
fn draw_table(
    frame: &mut Frame,
    area: Rect,
    model: &TraceModel,
    visible: &[&TraceSpan],
    ui: &UiState,
) {
    let header = Row::new(vec!["Name", "Tokens", "Dur", "Cap", "Bar"])
        .style(Style::default().add_modifier(Modifier::BOLD));

    let rows: Vec<Row> = visible
        .iter()
        .enumerate()
        .map(|(i, span)| {
            let tokens = span
                .tokens
                .map(|t| t.to_string())
                .unwrap_or_else(|| "-".to_string());
            let (badge, badge_style) = capability_badge(span.capability.as_ref());
            let mut row = Row::new(vec![
                Cell::from(span.label.clone()),
                Cell::from(tokens),
                Cell::from(duration_label(span)),
                Cell::from(badge).style(badge_style),
                Cell::from(bar_cell(model, span)),
            ]);
            if i == ui.selected {
                row = row.style(Style::default().add_modifier(Modifier::REVERSED));
            }
            row
        })
        .collect();

    let widths = [
        Constraint::Length(30),
        Constraint::Length(8),
        Constraint::Length(10),
        Constraint::Length(20),
        Constraint::Length(BAR_WIDTH),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::ALL).title("Waterfall"));
    frame.render_widget(table, area);
}

/// Detail pane for the span at `ui.selected` within `visible`. Rendered as
/// a single wrapped line (rather than one line per field) so it stays
/// legible even in the minimum-height layout a small terminal forces.
fn draw_detail(frame: &mut Frame, area: Rect, visible: &[&TraceSpan], ui: &UiState) {
    let block = Block::default().borders(Borders::ALL).title("Detail");
    let text = match visible.get(ui.selected) {
        Some(span) => format!(
            "{}  |  kind: {:?}  |  status: {:?}  |  tokens: {}  |  capability: {}",
            span.label,
            span.kind,
            span.status,
            span.tokens
                .map(|t| t.to_string())
                .unwrap_or_else(|| "-".to_string()),
            capability_detail(span.capability.as_ref()),
        ),
        None => "(no span selected)".to_string(),
    };
    frame.render_widget(
        Paragraph::new(text)
            .block(block)
            .wrap(Wrap { trim: true })
            .scroll((ui.scroll, 0)),
        area,
    );
}

/// Short badge word + verdict color for the `Cap` table cell.
fn capability_badge(verdict: Option<&CapabilityVerdict>) -> (String, Style) {
    match verdict {
        None => ("-".to_string(), Style::default()),
        Some(CapabilityVerdict::Allow) => ("allow".to_string(), Style::default().fg(Color::Green)),
        Some(CapabilityVerdict::Clamp { to }) => {
            (format!("clamp:{to}"), Style::default().fg(Color::Yellow))
        }
        Some(CapabilityVerdict::Drop { reason }) => {
            (format!("drop:{reason}"), Style::default().fg(Color::Red))
        }
    }
}

/// Verbose verdict line for the detail pane.
fn capability_detail(verdict: Option<&CapabilityVerdict>) -> String {
    match verdict {
        None => "-".to_string(),
        Some(CapabilityVerdict::Allow) => "allow".to_string(),
        Some(CapabilityVerdict::Clamp { to }) => format!("clamp -> {to}"),
        Some(CapabilityVerdict::Drop { reason }) => format!("drop: {reason}"),
    }
}

/// `"<ms>ms"` for a completed span, `"…"` while still running.
fn duration_label(span: &TraceSpan) -> String {
    match span.end {
        Some(end) => {
            let ms = (end - span.start).num_milliseconds().max(0);
            format!("{ms}ms")
        }
        None => "…".to_string(),
    }
}

/// Build a `BAR_WIDTH`-wide glyph string for `span`'s waterfall bar:
/// `░` background, `▊` over the span's `[offset, offset+len)` columns as
/// returned by [`TraceModel::bar`].
fn bar_cell(model: &TraceModel, span: &TraceSpan) -> String {
    let (offset, len) = model.bar(span, BAR_WIDTH);
    let mut glyphs = vec!['░'; BAR_WIDTH as usize];
    let start = usize::from(offset).min(glyphs.len());
    let end = start.saturating_add(usize::from(len)).min(glyphs.len());
    for glyph in &mut glyphs[start..end] {
        *glyph = '▊';
    }
    glyphs.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use tau_ports::{TraceEvent, TraceEventKind};

    #[test]
    fn renders_tool_row_with_capability_badge() {
        let mut model = TraceModel::new();
        model.apply(&TraceEvent {
            id: "evt-1".into(),
            ts: Utc.timestamp_opt(100, 0).unwrap(),
            run_id: "run-1".into(),
            agent_id: Some("agent-1".into()),
            kind: TraceEventKind::ToolCall {
                tool_name: "net.http".into(),
                duration_ms: 250,
                status: "ok".into(),
                capability: Some(CapabilityVerdict::Drop {
                    reason: "egress denied".into(),
                }),
            },
        });

        let mut term = Terminal::new(TestBackend::new(120, 10)).unwrap();
        let ui = UiState {
            selected: 0,
            filter: Filter::All,
            search: String::new(),
            scroll: 0,
        };
        term.draw(|f| draw(f, &model, &ui)).unwrap();
        let buf = term.backend().buffer().clone();
        let text: String = buf.content().iter().map(|c| c.symbol()).collect();

        assert!(text.contains("drop"), "expected a drop badge in:\n{text}");
        assert!(
            text.contains("net.http"),
            "expected the tool label in:\n{text}"
        );
    }

    #[test]
    fn filter_hides_non_matching_spans() {
        let mut model = TraceModel::new();
        model.apply(&TraceEvent {
            id: "evt-1".into(),
            ts: Utc.timestamp_opt(100, 0).unwrap(),
            run_id: "run-1".into(),
            agent_id: Some("agent-1".into()),
            kind: TraceEventKind::ToolCall {
                tool_name: "net.http".into(),
                duration_ms: 250,
                status: "ok".into(),
                capability: Some(CapabilityVerdict::Allow),
            },
        });

        let mut term = Terminal::new(TestBackend::new(120, 10)).unwrap();
        let ui = UiState {
            selected: 0,
            filter: Filter::Reasoning,
            search: String::new(),
            scroll: 0,
        };
        term.draw(|f| draw(f, &model, &ui)).unwrap();
        let buf = term.backend().buffer().clone();
        let text: String = buf.content().iter().map(|c| c.symbol()).collect();

        assert!(
            !text.contains("net.http"),
            "Reasoning filter should hide the Tool span:\n{text}"
        );
    }
}
