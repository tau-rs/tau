//! Interactive shell around the pure [`crate::tui::draw`] renderer.
//!
//! [`App`] holds the [`TraceModel`] + [`UiState`] and reduces key input via
//! [`App::apply_key`], which is deliberately pure (no terminal I/O) so it is
//! unit-testable without a real terminal. [`run_tui`] is the impure half: it
//! owns raw-mode/alternate-screen setup and teardown, and drives the read
//! loop — tailing a jsonl file or draining a live [`TraceEvent`] channel —
//! redrawing after every batch of input.

use std::fs::File;
use std::io::{ErrorKind, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use tokio::sync::mpsc::UnboundedReceiver;

use tau_ports::TraceEvent;
use tau_trace::TraceModel;

use super::render::span_matches;
use super::{draw, Filter, UiState};

/// Result of feeding one key to [`App::apply_key`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Loop {
    /// Keep running the event loop.
    Continue,
    /// Exit the event loop (terminal teardown happens in [`run_tui`]).
    Quit,
}

/// Where [`run_tui`] reads [`TraceEvent`]s from.
pub enum TraceSource {
    /// Tail a `.tau/runs/<id>.jsonl` file: read to EOF, then poll for
    /// appended lines (the run may still be writing).
    File(PathBuf),
    /// Drain a live channel, e.g. the receiver handed back by
    /// `tau_runtime_tokio::drive_with_live_trace` — this is the exact
    /// channel type that function returns, so a caller in `tau run` can
    /// hand its `rx` straight to [`run_tui`] with no adapter.
    Live(UnboundedReceiver<TraceEvent>),
}

/// Owns the render model + interactive UI state for the execution-trace TUI.
///
/// `apply_key` and `ingest_line` are the only mutators; both are pure
/// (no I/O), which is what makes `apply_key` unit-testable without a
/// terminal. [`run_tui`] is the impure shell that drives them from real
/// input and a real trace source.
pub struct App {
    model: TraceModel,
    ui: UiState,
    /// `true` while `/` search-input mode is active: subsequent
    /// character keys append to `ui.search` instead of being reduced as
    /// navigation/filter commands.
    search_mode: bool,
    /// `true` (the tail -f default) while the newest span should stay
    /// selected as more events arrive. Cleared by `Up` (the user is
    /// browsing history); restored once `Down` catches back up to the
    /// newest visible row.
    follow: bool,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    /// Fresh app over an empty [`TraceModel`].
    pub fn new() -> Self {
        Self::with_model(TraceModel::new())
    }

    /// Fresh app over a caller-supplied model (test construction, and any
    /// future "load a finished run's jsonl, then attach" path).
    pub fn with_model(model: TraceModel) -> Self {
        Self {
            model,
            ui: UiState::default(),
            search_mode: false,
            follow: true,
        }
    }

    /// Index into the filtered-visible span list currently highlighted.
    pub fn selected(&self) -> usize {
        self.ui.selected
    }

    /// The active row filter.
    pub fn filter(&self) -> Filter {
        self.ui.filter
    }

    /// Whether the detail/expand toggle (`Enter`) is currently on.
    pub fn expanded(&self) -> bool {
        self.ui.expanded
    }

    /// Parse `line` as one jsonl trace event and fold it into the model.
    ///
    /// Silently no-ops on a blank line (`Ok(None)`) or a line that fails to
    /// parse (`Err`) — callers that need to distinguish a genuine parse
    /// error from a still-growing tail line (see `tau_trace::parse_line`'s
    /// docs) probe with `tau_trace::parse_line` themselves before calling
    /// this, as the file-tail loop below does.
    pub fn ingest_line(&mut self, line: &str) {
        if let Ok(Some(event)) = tau_trace::parse_line(line) {
            self.apply_event(&event);
        }
    }

    /// Fold an already-parsed event into the model (shared by
    /// `ingest_line` and the live-channel loop, which has no jsonl line to
    /// parse). Re-clamps `selected` and, while `follow` is on, snaps it to
    /// the newest visible row.
    fn apply_event(&mut self, event: &TraceEvent) {
        self.model.apply(event);
        self.clamp_selected();
        if self.follow {
            self.ui.selected = self.visible_len().saturating_sub(1);
        }
    }

    /// Reduce one key press. Pure: no I/O, so this is unit-testable without
    /// a real terminal.
    ///
    /// `Down`/`Up` move `selected` (clamped to the filtered-visible
    /// length); `f` cycles [`Filter`] All -> Errors -> Tools -> Reasoning
    /// -> All; `/` enters search-input mode (subsequent chars append to
    /// `ui.search`, `Backspace` deletes, `Enter`/`Esc` exits search mode);
    /// `Enter` outside search mode toggles the detail/expand flag; `q`/
    /// `Q`/`Esc` outside search mode quits.
    pub fn apply_key(&mut self, key: KeyCode) -> Loop {
        if self.search_mode {
            match key {
                KeyCode::Enter | KeyCode::Esc => self.search_mode = false,
                KeyCode::Backspace => {
                    self.ui.search.pop();
                }
                KeyCode::Char(c) => self.ui.search.push(c),
                _ => {}
            }
            self.clamp_selected();
            return Loop::Continue;
        }

        match key {
            KeyCode::Down => {
                let last = self.visible_len().saturating_sub(1);
                if self.ui.selected < last {
                    self.ui.selected += 1;
                }
                if self.ui.selected >= last {
                    self.follow = true;
                }
            }
            KeyCode::Up => {
                if self.ui.selected > 0 {
                    self.ui.selected -= 1;
                }
                self.follow = false;
            }
            KeyCode::Char('f') => {
                self.ui.filter = next_filter(self.ui.filter);
                self.clamp_selected();
            }
            KeyCode::Char('/') => self.search_mode = true,
            KeyCode::Enter => self.ui.expanded = !self.ui.expanded,
            KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => return Loop::Quit,
            _ => {}
        }
        Loop::Continue
    }

    /// Number of spans currently visible under `ui.filter` + `ui.search`.
    /// Uses the same [`span_matches`] predicate `draw` filters with, so the
    /// two can never drift out of sync.
    fn visible_len(&self) -> usize {
        self.model
            .spans()
            .iter()
            .filter(|s| span_matches(self.ui.filter, s, &self.ui.search))
            .count()
    }

    /// Clamp `ui.selected` into `[0, visible_len())` (or `0` if nothing is
    /// visible) after any change that can shrink the visible set: a new
    /// event, a filter cycle, or a search-text edit.
    fn clamp_selected(&mut self) {
        let len = self.visible_len();
        self.ui.selected = if len == 0 {
            0
        } else {
            self.ui.selected.min(len - 1)
        };
    }
}

/// `All -> Errors -> Tools -> Reasoning -> All`.
fn next_filter(filter: Filter) -> Filter {
    match filter {
        Filter::All => Filter::Errors,
        Filter::Errors => Filter::Tools,
        Filter::Tools => Filter::Reasoning,
        Filter::Reasoning => Filter::All,
    }
}

/// How long each iteration blocks waiting for a key press before polling
/// the trace source again.
const POLL_INTERVAL: Duration = Duration::from_millis(150);

/// Restores the terminal on every exit path — normal return, `?`
/// early-return, or panic (ratatui's `init()` installs a panic hook that
/// calls `restore()` too; this `Drop` impl is the belt to that hook's
/// braces, covering the non-panic early-return paths a hook can't see).
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

/// Run the execution-trace TUI to completion.
///
/// Owns raw-mode + alternate-screen setup via `ratatui::init()` and
/// guarantees teardown via [`TerminalGuard`]. Polls both keyboard input
/// and `source` on a fixed interval, redrawing after each batch; returns
/// once the user quits (`q`/`Esc`) or the underlying I/O fails.
pub fn run_tui(source: TraceSource) -> anyhow::Result<()> {
    let mut terminal = ratatui::init();
    let _guard = TerminalGuard;
    let mut app = App::new();

    match source {
        TraceSource::File(path) => run_with_file(&mut terminal, &mut app, &path),
        TraceSource::Live(rx) => run_with_live(&mut terminal, &mut app, rx),
    }
}

/// Handle one polled input event: reduce a key press through `apply_key`,
/// or just let a resize fall through to the next `draw` (ratatui
/// re-computes layout from `frame.area()` every call, so a resize needs no
/// special handling here — see the M1 constraint against reworking
/// `draw`'s fixed-width bar).
///
/// Returns `true` once the app should quit.
fn handle_input(app: &mut App) -> anyhow::Result<bool> {
    if !event::poll(POLL_INTERVAL)? {
        return Ok(false);
    }
    match event::read()? {
        Event::Key(key) if key.kind == KeyEventKind::Press => {
            Ok(matches!(app.apply_key(key.code), Loop::Quit))
        }
        _ => Ok(false),
    }
}

fn run_with_live(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    mut rx: UnboundedReceiver<TraceEvent>,
) -> anyhow::Result<()> {
    loop {
        while let Ok(event) = rx.try_recv() {
            app.apply_event(&event);
        }

        terminal.draw(|f| draw(f, &app.model, &app.ui))?;

        if handle_input(app)? {
            return Ok(());
        }
    }
}

fn run_with_file(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    path: &Path,
) -> anyhow::Result<()> {
    let mut file =
        File::open(path).with_context(|| format!("opening trace file {}", path.display()))?;
    let mut pos: u64 = 0;
    let mut buf = String::new();

    loop {
        poll_file_tail(&mut file, &mut pos, &mut buf, app)?;

        terminal.draw(|f| draw(f, &app.model, &app.ui))?;

        if handle_input(app)? {
            return Ok(());
        }
    }
}

/// Read whatever `file` has grown by since `pos` and ingest every
/// complete (newline-terminated) line.
///
/// A still-growing last line (no trailing `\n` yet) is speculatively
/// parsed too: if it already parses (the writer flushed the JSON before
/// the trailing newline), it is ingested and consumed like any other
/// line. If it fails to parse, that is treated as the expected
/// mid-write/truncation artifact documented on `tau_trace::parse_line` —
/// `pos` is *not* advanced past it, so the same bytes (plus whatever the
/// writer appends next) are re-read on the next poll instead of being
/// dropped or crashing the loop. A read that lands mid a multi-byte UTF-8
/// character at the current EOF is treated the same way.
fn poll_file_tail(
    file: &mut File,
    pos: &mut u64,
    buf: &mut String,
    app: &mut App,
) -> anyhow::Result<()> {
    file.seek(SeekFrom::Start(*pos))?;
    buf.clear();
    match file.read_to_string(buf) {
        Ok(0) => return Ok(()),
        Ok(_) => {}
        Err(e) if e.kind() == ErrorKind::InvalidData => return Ok(()),
        Err(e) => return Err(e).context("tailing trace file"),
    }

    let ends_with_newline = buf.ends_with('\n');
    let mut segments: Vec<&str> = buf.split('\n').collect();
    let trailing_partial = if ends_with_newline {
        segments.pop(); // the empty segment after the final '\n'
        None
    } else {
        segments.pop()
    };

    let mut consumed: u64 = 0;
    for line in &segments {
        app.ingest_line(line);
        consumed += line.len() as u64 + 1; // + the '\n' that terminated it
    }

    if let Some(partial) = trailing_partial.filter(|p| !p.is_empty()) {
        match tau_trace::parse_line(partial) {
            Ok(_) => {
                app.ingest_line(partial);
                consumed += partial.len() as u64;
            }
            Err(_) => {
                // Retryable: leave `pos` before this segment.
            }
        }
    }

    *pos += consumed;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use tau_ports::{CapabilityVerdict, TraceEventKind};

    /// Three tool-call spans, in emission order, no filter/search applied.
    fn three_span_model() -> TraceModel {
        let mut model = TraceModel::new();
        for (i, status) in ["ok", "ok", "failed"].into_iter().enumerate() {
            model.apply(&TraceEvent {
                id: format!("evt-{i}"),
                ts: Utc.timestamp_opt(100 + i as i64, 0).unwrap(),
                run_id: "run-1".into(),
                agent_id: Some("agent-1".into()),
                kind: TraceEventKind::ToolCall {
                    tool_name: format!("tool-{i}"),
                    duration_ms: 10,
                    status: status.into(),
                    capability: Some(CapabilityVerdict::Allow),
                    turn_index: 0,
                },
            });
        }
        model
    }

    #[test]
    fn keys_navigate_and_quit() {
        let mut app = App::with_model(three_span_model());
        app.apply_key(KeyCode::Down);
        assert_eq!(app.selected(), 1);
        app.apply_key(KeyCode::Char('f')); // cycle filter All->Errors
        assert!(matches!(app.filter(), Filter::Errors));
        assert!(matches!(app.apply_key(KeyCode::Char('q')), Loop::Quit));
    }

    #[test]
    fn uppercase_q_also_quits() {
        let mut app = App::with_model(three_span_model());
        assert!(matches!(app.apply_key(KeyCode::Char('Q')), Loop::Quit));
    }

    #[test]
    fn up_down_clamp_to_visible_range() {
        let mut app = App::with_model(three_span_model());
        // Up at 0 stays at 0.
        app.apply_key(KeyCode::Up);
        assert_eq!(app.selected(), 0);
        // Down x5 clamps at the last visible index (2).
        for _ in 0..5 {
            app.apply_key(KeyCode::Down);
        }
        assert_eq!(app.selected(), 2);
    }

    #[test]
    fn filter_cycles_through_all_variants_and_wraps() {
        let mut app = App::with_model(three_span_model());
        assert!(matches!(app.filter(), Filter::All));
        app.apply_key(KeyCode::Char('f'));
        assert!(matches!(app.filter(), Filter::Errors));
        app.apply_key(KeyCode::Char('f'));
        assert!(matches!(app.filter(), Filter::Tools));
        app.apply_key(KeyCode::Char('f'));
        assert!(matches!(app.filter(), Filter::Reasoning));
        app.apply_key(KeyCode::Char('f'));
        assert!(matches!(app.filter(), Filter::All));
    }

    #[test]
    fn slash_enters_search_mode_and_enter_exits_it() {
        let mut app = App::with_model(three_span_model());
        app.apply_key(KeyCode::Char('/'));
        app.apply_key(KeyCode::Char('t'));
        app.apply_key(KeyCode::Char('o'));
        assert_eq!(app.ui.search, "to");
        app.apply_key(KeyCode::Backspace);
        assert_eq!(app.ui.search, "t");
        // 'q' while in search mode is text, not quit.
        assert!(matches!(app.apply_key(KeyCode::Char('q')), Loop::Continue));
        assert_eq!(app.ui.search, "tq");
        // Enter exits search mode without quitting.
        assert!(matches!(app.apply_key(KeyCode::Enter), Loop::Continue));
        // Now Enter toggles expand instead of editing search text.
        assert!(!app.expanded());
        app.apply_key(KeyCode::Enter);
        assert!(app.expanded());
    }

    #[test]
    fn esc_quits_outside_search_but_exits_search_mode_inside_it() {
        let mut app = App::with_model(three_span_model());
        app.apply_key(KeyCode::Char('/'));
        assert!(matches!(app.apply_key(KeyCode::Esc), Loop::Continue));
        assert!(matches!(app.apply_key(KeyCode::Esc), Loop::Quit));
    }

    #[test]
    fn ingest_line_parses_and_applies_a_valid_event() {
        let mut app = App::new();
        let evt = TraceEvent {
            id: "evt-1".into(),
            ts: Utc.timestamp_opt(100, 0).unwrap(),
            run_id: "run-1".into(),
            agent_id: Some("agent-1".into()),
            kind: TraceEventKind::ToolCall {
                tool_name: "net.http".into(),
                duration_ms: 10,
                status: "ok".into(),
                capability: None,
                turn_index: 0,
            },
        };
        let line = serde_json::to_string(&evt).unwrap();

        app.ingest_line(&line);

        assert_eq!(app.model.spans().len(), 1);
        // Auto-follow snaps `selected` onto the (only, newest) span.
        assert_eq!(app.selected(), 0);
    }

    #[test]
    fn ingest_line_ignores_blank_and_malformed_lines() {
        let mut app = App::new();
        app.ingest_line("");
        app.ingest_line("   ");
        app.ingest_line("{not json");
        assert_eq!(app.model.spans().len(), 0);
    }

    fn tool_event(id: &str, secs: i64) -> TraceEvent {
        TraceEvent {
            id: id.into(),
            ts: Utc.timestamp_opt(secs, 0).unwrap(),
            run_id: "run-1".into(),
            agent_id: Some("agent-1".into()),
            kind: TraceEventKind::ToolCall {
                tool_name: format!("tool-{id}"),
                duration_ms: 10,
                status: "ok".into(),
                capability: None,
                turn_index: 0,
            },
        }
    }

    #[test]
    fn follow_tracks_newest_until_user_scrolls_up() {
        let mut app = App::with_model(three_span_model());
        assert_eq!(app.selected(), 0); // fresh UiState, not yet followed

        // Simulate the tail-follow path via apply_event directly (what
        // ingest_line and the live-channel loop both funnel through).
        app.apply_event(&tool_event("evt-3", 200));
        assert_eq!(app.selected(), 3); // 4 spans now; followed to the newest

        app.apply_key(KeyCode::Up);
        assert_eq!(app.selected(), 2);

        // follow is off after Up: a new event doesn't move `selected`.
        app.apply_event(&tool_event("evt-4", 201));
        assert_eq!(app.selected(), 2);

        // Catch back up to the newest visible row (index 4 of 5 spans) ->
        // follow re-arms.
        app.apply_key(KeyCode::Down);
        app.apply_key(KeyCode::Down);
        assert_eq!(app.selected(), 4);

        app.apply_event(&tool_event("evt-5", 202));
        assert_eq!(app.selected(), 5); // followed to the new newest
    }

    #[test]
    fn poll_file_tail_retries_a_truncated_last_line() {
        use std::io::Write;

        let evt = TraceEvent {
            id: "evt-1".into(),
            ts: Utc.timestamp_opt(100, 0).unwrap(),
            run_id: "run-1".into(),
            agent_id: Some("agent-1".into()),
            kind: TraceEventKind::ToolCall {
                tool_name: "net.http".into(),
                duration_ms: 10,
                status: "ok".into(),
                capability: None,
                turn_index: 0,
            },
        };
        let full_line = serde_json::to_string(&evt).unwrap();
        let truncated = &full_line[..full_line.len() / 2];

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run.jsonl");
        std::fs::write(&path, truncated).unwrap();

        let mut file = File::open(&path).unwrap();
        let mut pos: u64 = 0;
        let mut buf = String::new();
        let mut app = App::new();

        // First poll: only a truncated line is on disk. Must not crash,
        // must not consume it (no event applied, `pos` stays at 0).
        poll_file_tail(&mut file, &mut pos, &mut buf, &mut app).unwrap();
        assert_eq!(app.model.spans().len(), 0);
        assert_eq!(pos, 0);

        // Writer finishes flushing the line (+ newline). Re-open to append
        // since `file` above was opened read-only.
        {
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap();
            writeln!(f, "{}", &full_line[full_line.len() / 2..]).unwrap();
        }

        // Second poll re-reads from `pos` (still 0) and now sees the
        // complete, newline-terminated line.
        poll_file_tail(&mut file, &mut pos, &mut buf, &mut app).unwrap();
        assert_eq!(app.model.spans().len(), 1);
        assert!(pos > 0);
    }
}
