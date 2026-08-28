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

/// Fixed widths of the four leading table columns (`Name`, `Tokens`,
/// `Dur`, `Cap`). The waterfall `Bar` column takes whatever horizontal
/// space is left, so it grows/shrinks as the terminal is resized.
const NAME_W: u16 = 30;
const TOKENS_W: u16 = 8;
const DUR_W: u16 = 10;
const CAP_W: u16 = 20;
/// Smallest bar column we will render, so the waterfall never vanishes on
/// a narrow terminal.
const MIN_BAR_W: u16 = 8;

/// Cells available for the waterfall bar within `area`: the table's inner
/// width (minus its border) less the four fixed columns and the
/// single-cell gaps ratatui inserts between the five columns. Floored at
/// [`MIN_BAR_W`]. This is both the `width_cols` handed to
/// [`TraceModel::bar`] and the glyph-string length built in [`bar_cell`],
/// so the two always line up with the rendered column.
fn bar_width(area: Rect) -> u16 {
    const BORDERS: u16 = 2; // left + right table border
    const GAPS: u16 = 4; // default 1-cell spacing between 5 columns
    area.width
        .saturating_sub(BORDERS + NAME_W + TOKENS_W + DUR_W + CAP_W + GAPS)
        .max(MIN_BAR_W)
}

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
    /// When `true`, the detail pane renders each field on its own line
    /// (an expanded view) instead of a single wrapped summary line.
    /// Toggled by `Enter` (see [`crate::tui::app::App::apply_key`]).
    pub expanded: bool,
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
    // The detail pane grows when expanded so its one-field-per-line view is
    // not clipped; collapsed it stays a single summary line (3 rows incl.
    // borders). The waterfall (`Min`) absorbs the difference.
    let detail_h = if ui.expanded { 8 } else { 3 };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(4),
            Constraint::Length(detail_h),
        ])
        .split(area);

    draw_toolbar(frame, chunks[0], ui, model.run_id());

    let visible: Vec<&TraceSpan> = model
        .spans()
        .iter()
        .filter(|s| span_matches(ui.filter, s, &ui.search))
        .collect();

    draw_table(frame, chunks[1], model, &visible, ui);
    draw_detail(frame, chunks[2], &visible, ui);
}

/// Toolbar: run id, one chip per [`Filter`] (the active one bracketed),
/// and the current search text.
fn draw_toolbar(frame: &mut Frame, area: Rect, ui: &UiState, run_id: Option<&str>) {
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
    let run = run_id.unwrap_or("(pending)");
    let text = format!("run {run}   {chips}   search: {}", ui.search);
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

    // Derive the bar column from the current area so the waterfall adapts
    // to the terminal width (ratatui re-lays-out from `area` every draw, so
    // a resize is picked up automatically on the next frame).
    let bar_w = bar_width(area);

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
                Cell::from(bar_cell(model, span, bar_w)),
            ]);
            if i == ui.selected {
                row = row.style(Style::default().add_modifier(Modifier::REVERSED));
            }
            row
        })
        .collect();

    let widths = [
        Constraint::Length(NAME_W),
        Constraint::Length(TOKENS_W),
        Constraint::Length(DUR_W),
        Constraint::Length(CAP_W),
        Constraint::Length(bar_w),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::ALL).title("Waterfall"));
    frame.render_widget(table, area);
}

/// Detail pane for the span at `ui.selected` within `visible`.
///
/// Collapsed (the default) renders a single wrapped summary line so it
/// stays legible in the minimum-height layout a small terminal forces.
/// Expanded (`ui.expanded`, toggled by `Enter`) renders one field per line
/// for a fuller view.
fn draw_detail(frame: &mut Frame, area: Rect, visible: &[&TraceSpan], ui: &UiState) {
    let title = if ui.expanded {
        "Detail (expanded)"
    } else {
        "Detail"
    };
    let block = Block::default().borders(Borders::ALL).title(title);
    let text = match visible.get(ui.selected) {
        Some(span) => {
            let tokens = span
                .tokens
                .map(|t| t.to_string())
                .unwrap_or_else(|| "-".to_string());
            let cap = capability_detail(span.capability.as_ref());
            if ui.expanded {
                format!(
                    "name:       {}\nkind:       {:?}\nstatus:     {:?}\ntokens:     {}\nduration:   {}\ncapability: {}",
                    span.label,
                    span.kind,
                    span.status,
                    tokens,
                    duration_label(span),
                    cap,
                )
            } else {
                format!(
                    "{}  |  kind: {:?}  |  status: {:?}  |  tokens: {}  |  capability: {}",
                    span.label, span.kind, span.status, tokens, cap,
                )
            }
        }
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

/// Build a `width`-wide glyph string for `span`'s waterfall bar:
/// `░` background, `▊` over the span's `[offset, offset+len)` columns as
/// returned by [`TraceModel::bar`]. `width` is [`bar_width`] of the current
/// area, so the string always matches the rendered `Bar` column.
fn bar_cell(model: &TraceModel, span: &TraceSpan, width: u16) -> String {
    let (offset, len) = model.bar(span, width);
    let mut glyphs = vec!['░'; width as usize];
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
                turn_index: 0,
            },
        });

        let mut term = Terminal::new(TestBackend::new(120, 10)).unwrap();
        let ui = UiState {
            selected: 0,
            filter: Filter::All,
            search: String::new(),
            scroll: 0,
            expanded: false,
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
                turn_index: 0,
            },
        });

        let mut term = Terminal::new(TestBackend::new(120, 10)).unwrap();
        let ui = UiState {
            selected: 0,
            filter: Filter::Reasoning,
            search: String::new(),
            scroll: 0,
            expanded: false,
        };
        term.draw(|f| draw(f, &model, &ui)).unwrap();
        let buf = term.backend().buffer().clone();
        let text: String = buf.content().iter().map(|c| c.symbol()).collect();

        assert!(
            !text.contains("net.http"),
            "Reasoning filter should hide the Tool span:\n{text}"
        );
    }

    #[test]
    fn bar_width_grows_with_area_and_floors_on_narrow_terminals() {
        let narrow = bar_width(Rect::new(0, 0, 80, 20));
        let wide = bar_width(Rect::new(0, 0, 160, 20));
        assert!(
            wide > narrow,
            "a wider terminal must yield a wider bar column ({wide} !> {narrow})"
        );
        // Far too narrow to fit the fixed columns → floored, not zero/underflow.
        assert_eq!(bar_width(Rect::new(0, 0, 10, 20)), MIN_BAR_W);
    }

    #[test]
    fn expanded_detail_renders_multiline_labels() {
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
                turn_index: 0,
            },
        });

        let mut term = Terminal::new(TestBackend::new(120, 20)).unwrap();
        let ui = UiState {
            selected: 0,
            filter: Filter::All,
            search: String::new(),
            scroll: 0,
            expanded: true,
        };
        term.draw(|f| draw(f, &model, &ui)).unwrap();
        let buf = term.backend().buffer().clone();
        let text: String = buf.content().iter().map(|c| c.symbol()).collect();

        assert!(
            text.contains("Detail (expanded)"),
            "expanded detail pane must be titled as such:\n{text}"
        );
        assert!(
            text.contains("duration:") && text.contains("capability:"),
            "expanded view must show per-field labels:\n{text}"
        );
    }

    #[test]
    fn toolbar_shows_run_id() {
        let mut model = TraceModel::new();
        model.apply(&TraceEvent {
            id: "evt-1".into(),
            ts: Utc.timestamp_opt(100, 0).unwrap(),
            run_id: "run-abc".into(),
            agent_id: Some("agent-1".into()),
            kind: TraceEventKind::ToolCall {
                tool_name: "net.http".into(),
                duration_ms: 250,
                status: "ok".into(),
                capability: None,
                turn_index: 0,
            },
        });

        let mut term = Terminal::new(TestBackend::new(120, 10)).unwrap();
        term.draw(|f| draw(f, &model, &UiState::default())).unwrap();
        let buf = term.backend().buffer().clone();
        let text: String = buf.content().iter().map(|c| c.symbol()).collect();

        assert!(
            text.contains("run-abc"),
            "toolbar must show the run id:\n{text}"
        );
    }

    #[test]
    fn clamp_verdict_renders_amber_badge_with_host_list() {
        // Spec §13.6: the amber `clamp:<to>` arm shipped in M1.5 without a
        // test; #631 makes it reachable, so pin the rendering.
        let (label, style) = capability_badge(Some(&CapabilityVerdict::Clamp {
            to: "api.weather.com".into(),
        }));
        assert_eq!(label, "clamp:api.weather.com");
        assert_eq!(style.fg, Some(Color::Yellow));
    }

    #[test]
    fn clamp_verdict_detail_pane_shows_an_arrow() {
        assert_eq!(
            capability_detail(Some(&CapabilityVerdict::Clamp {
                to: "a.com,b.com".into()
            })),
            "clamp -> a.com,b.com"
        );
    }
}
