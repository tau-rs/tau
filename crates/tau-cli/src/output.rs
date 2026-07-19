//! Output discipline for tau-cli.
//!
//! All five subcommands route their output through [`Output`], which:
//! - Sends scriptable content to stdout (agent text, list rows, JSON).
//! - Sends status, tracing, warnings, and errors to stderr.
//! - Respects `--quiet` (suppresses status messages).
//! - Respects `--json` (suppresses human-readable stdout, emits JSON instead).
//! - Resolves color choice from `--color` flag, `NO_COLOR` env var, and
//!   `is-terminal` auto-detection on stdout.
//!
//! Per spec §3.6.

use std::fmt::Display;
use std::io::{self, Write};

use is_terminal::IsTerminal;
use serde::Serialize;

use crate::cli::{Cli, ColorMode};

/// Resolved color choice (after merging --color flag, NO_COLOR env,
/// and stdout's is-terminal detection).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorChoice {
    /// Always emit ANSI escapes.
    Always,
    /// Never emit ANSI escapes.
    Never,
}

impl ColorChoice {
    /// Resolve the effective color choice from CLI flag + env + tty detection.
    pub fn resolve(mode: ColorMode, stdout_is_tty: bool, no_color_env: bool) -> Self {
        if no_color_env {
            return ColorChoice::Never;
        }
        match mode {
            ColorMode::Always => ColorChoice::Always,
            ColorMode::Never => ColorChoice::Never,
            ColorMode::Auto => {
                if stdout_is_tty {
                    ColorChoice::Always
                } else {
                    ColorChoice::Never
                }
            }
        }
    }
}

/// Output writer for tau-cli subcommands. Routes content to stdout
/// or stderr per the discipline in module docs.
pub struct Output {
    stdout: Box<dyn Write + Send>,
    stderr: Box<dyn Write + Send>,
    json: bool,
    quiet: bool,
    color: ColorChoice,
}

impl Output {
    /// Construct from parsed CLI flags. Resolves color via `is-terminal`
    /// on stdout + `NO_COLOR` env var.
    pub fn from_cli(cli: &Cli) -> Self {
        let stdout_is_tty = io::stdout().is_terminal();
        let no_color_env = std::env::var_os("NO_COLOR").is_some();
        let color = ColorChoice::resolve(cli.color, stdout_is_tty, no_color_env);
        Self {
            stdout: Box::new(io::stdout()),
            stderr: Box::new(io::stderr()),
            json: cli.json,
            quiet: cli.quiet,
            color,
        }
    }

    /// Construct with explicit writers and config (test-only convenience).
    #[doc(hidden)]
    pub fn with_writers(
        stdout: Box<dyn Write + Send>,
        stderr: Box<dyn Write + Send>,
        json: bool,
        quiet: bool,
        color: ColorChoice,
    ) -> Self {
        Self {
            stdout,
            stderr,
            json,
            quiet,
            color,
        }
    }

    /// Whether `--json` is enabled.
    pub fn is_json(&self) -> bool {
        self.json
    }

    /// Resolved color choice.
    pub fn color(&self) -> ColorChoice {
        self.color
    }

    /// Emit human-readable content to stdout. No-op when `--json` is set.
    pub fn human<W: Display + ?Sized>(&mut self, w: &W) -> io::Result<()> {
        if self.json {
            return Ok(());
        }
        writeln!(self.stdout, "{w}")
    }

    /// Emit a JSON object to stdout. No-op when `--json` is NOT set.
    pub fn json<T: Serialize>(&mut self, value: &T) -> io::Result<()> {
        if !self.json {
            return Ok(());
        }
        let s = serde_json::to_string(value).map_err(io::Error::other)?;
        writeln!(self.stdout, "{s}")
    }

    /// Emit a status line to stderr. No-op when `--quiet` is set.
    pub fn status(&mut self, msg: impl Display) -> io::Result<()> {
        if self.quiet {
            return Ok(());
        }
        writeln!(self.stderr, "{msg}")
    }

    /// Emit a `[dry-run]`-prefixed line to stderr. Always emits regardless
    /// of `--quiet` (dry-run is the user's explicit request for visibility).
    pub fn dry_run(&mut self, line: impl Display) -> io::Result<()> {
        writeln!(self.stderr, "[dry-run] {line}")
    }

    /// Emit a warning to stderr. Always emits regardless of `--quiet`.
    pub fn warn(&mut self, msg: impl Display) -> io::Result<()> {
        match self.color {
            ColorChoice::Always => writeln!(self.stderr, "\x1b[33mwarning:\x1b[0m {msg}"),
            ColorChoice::Never => writeln!(self.stderr, "warning: {msg}"),
        }
    }

    /// Emit a pre-rendered multi-line diagnostic to stderr verbatim, with no
    /// `error:`/`warning:` prefix. Always emits regardless of `--quiet`. Used
    /// for diagnostics that carry their own `error[CODE]:` header (e.g. the
    /// governance `GOV000` gate and bundle-verify failures).
    pub fn diagnostic(&mut self, block: impl Display) -> io::Result<()> {
        writeln!(self.stderr, "{block}")
    }

    /// Emit an error to stderr. Always emits regardless of `--quiet`.
    pub fn error(&mut self, msg: impl Display) -> io::Result<()> {
        match self.color {
            ColorChoice::Always => writeln!(self.stderr, "\x1b[31merror:\x1b[0m {msg}"),
            ColorChoice::Never => writeln!(self.stderr, "error: {msg}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Shared writer that captures bytes and is `Send + Write`.
    #[derive(Clone, Default)]
    struct SharedBuf(Arc<Mutex<Vec<u8>>>);

    impl Write for SharedBuf {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl SharedBuf {
        fn snapshot(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    fn test_output(json: bool, quiet: bool, color: ColorChoice) -> (Output, SharedBuf, SharedBuf) {
        let stdout = SharedBuf::default();
        let stderr = SharedBuf::default();
        let out = Output::with_writers(
            Box::new(stdout.clone()),
            Box::new(stderr.clone()),
            json,
            quiet,
            color,
        );
        (out, stdout, stderr)
    }

    #[test]
    fn human_writes_to_stdout() {
        let (mut out, stdout, stderr) = test_output(false, false, ColorChoice::Never);
        out.human("hello").unwrap();
        assert_eq!(stdout.snapshot(), "hello\n");
        assert_eq!(stderr.snapshot(), "");
    }

    #[test]
    fn human_no_op_when_json_enabled() {
        let (mut out, stdout, _) = test_output(true, false, ColorChoice::Never);
        out.human("hello").unwrap();
        assert_eq!(stdout.snapshot(), "");
    }

    #[test]
    fn json_writes_to_stdout_when_enabled() {
        let (mut out, stdout, _) = test_output(true, false, ColorChoice::Never);
        let payload = serde_json::json!({"key": "value"});
        out.json(&payload).unwrap();
        assert_eq!(stdout.snapshot(), r#"{"key":"value"}"#.to_string() + "\n");
    }

    #[test]
    fn json_no_op_when_disabled() {
        let (mut out, stdout, _) = test_output(false, false, ColorChoice::Never);
        let payload = serde_json::json!({"key": "value"});
        out.json(&payload).unwrap();
        assert_eq!(stdout.snapshot(), "");
    }

    #[test]
    fn status_writes_to_stderr() {
        let (mut out, _, stderr) = test_output(false, false, ColorChoice::Never);
        out.status("installing...").unwrap();
        assert_eq!(stderr.snapshot(), "installing...\n");
    }

    #[test]
    fn status_no_op_when_quiet() {
        let (mut out, _, stderr) = test_output(false, true, ColorChoice::Never);
        out.status("installing...").unwrap();
        assert_eq!(stderr.snapshot(), "");
    }

    #[test]
    fn dry_run_prefixes_with_marker() {
        let (mut out, _, stderr) = test_output(false, false, ColorChoice::Never);
        out.dry_run("would create tau.toml").unwrap();
        assert_eq!(stderr.snapshot(), "[dry-run] would create tau.toml\n");
    }

    #[test]
    fn dry_run_emits_even_when_quiet() {
        let (mut out, _, stderr) = test_output(false, true, ColorChoice::Never);
        out.dry_run("would create tau.toml").unwrap();
        assert_eq!(stderr.snapshot(), "[dry-run] would create tau.toml\n");
    }

    #[test]
    fn warn_includes_prefix_no_color() {
        let (mut out, _, stderr) = test_output(false, false, ColorChoice::Never);
        out.warn("be careful").unwrap();
        assert_eq!(stderr.snapshot(), "warning: be careful\n");
    }

    #[test]
    fn warn_includes_yellow_ansi_when_color_always() {
        let (mut out, _, stderr) = test_output(false, false, ColorChoice::Always);
        out.warn("be careful").unwrap();
        assert!(stderr.snapshot().contains("\x1b[33m"));
    }

    #[test]
    fn error_includes_red_ansi_when_color_always() {
        let (mut out, _, stderr) = test_output(false, false, ColorChoice::Always);
        out.error("boom").unwrap();
        assert!(stderr.snapshot().contains("\x1b[31m"));
    }

    #[test]
    fn color_resolve_no_color_env_overrides_always() {
        let resolved = ColorChoice::resolve(ColorMode::Always, true, true);
        assert_eq!(resolved, ColorChoice::Never);
    }

    #[test]
    fn color_resolve_auto_with_tty_is_always() {
        let resolved = ColorChoice::resolve(ColorMode::Auto, true, false);
        assert_eq!(resolved, ColorChoice::Always);
    }

    #[test]
    fn color_resolve_auto_without_tty_is_never() {
        let resolved = ColorChoice::resolve(ColorMode::Auto, false, false);
        assert_eq!(resolved, ColorChoice::Never);
    }
}
