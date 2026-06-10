//! REPL loop, command parser, and rustyline integration for `tau dev`.

use anyhow::Result;
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;

use crate::cmd::dev::session::DevSession;
use crate::output::Output;

/// Parsed user input from the REPL prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Plain text — send to the current agent as a turn.
    Prompt(String),
    /// `:reload` — apply pending manifest changes, keep history.
    Reload,
    /// `:state` — print session stats.
    State,
    /// `:history` — print message log (last 20).
    History,
    /// `:agents` — list agents in the project.
    Agents,
    /// `:agent <name>` — switch the active agent.
    SwitchAgent(String),
    /// `:clear` — reset history, keep manifest.
    Clear,
    /// `:help` — print command list.
    Help,
    /// `:quit` — exit (also fired by Ctrl-D / EOF).
    Quit,
    /// Empty line — no-op.
    Empty,
    /// Unrecognised `:foo` — print error, stay at prompt.
    UnknownColon(String),
}

/// Parse one line of REPL input.
pub fn parse_command(line: &str) -> Command {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Command::Empty;
    }
    if !trimmed.starts_with(':') {
        return Command::Prompt(trimmed.to_string());
    }
    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let verb = parts.next().unwrap_or("");
    let arg = parts.next().unwrap_or("").trim();
    match verb {
        ":reload" => Command::Reload,
        ":state" => Command::State,
        ":history" => Command::History,
        ":agents" => Command::Agents,
        ":agent" => {
            if arg.is_empty() {
                Command::UnknownColon(":agent (missing name)".into())
            } else {
                Command::SwitchAgent(arg.to_string())
            }
        }
        ":clear" => Command::Clear,
        ":help" => Command::Help,
        ":quit" => Command::Quit,
        other => Command::UnknownColon(other.to_string()),
    }
}

/// Run the REPL loop until the user quits.
///
/// When `watch_mode` is `true`, a pending reload is applied automatically at
/// the top of each iteration (before the next prompt). When `false`, the REPL
/// prints a hint asking the user to type `:reload` instead.
pub async fn run_loop(
    session: &mut DevSession,
    output: &mut Output,
    watch_mode: bool,
) -> Result<()> {
    let mut editor = DefaultEditor::new()?;
    print_banner(output, session);
    loop {
        use std::sync::atomic::Ordering;
        if session.pending_reload.load(Ordering::Acquire) {
            if watch_mode {
                match session.auto_reload_if_pending().await {
                    Ok(true) => output.human(&format!(
                        "(auto-reloaded; {} messages preserved)",
                        session.history.len()
                    ))?,
                    Ok(false) => {} // race: cleared between observing and acting
                    Err(e) => output.human(&format!("auto-reload failed: {e}"))?,
                }
            } else {
                output.human("(manifest changed; type :reload to apply)")?;
            }
        }
        let prompt = format!("({}) > ", session.current_agent_name());
        let line = match editor.readline(&prompt) {
            Ok(l) => l,
            Err(ReadlineError::Interrupted) => {
                output.human("(Ctrl-C: use :quit or Ctrl-D to exit)")?;
                continue;
            }
            Err(ReadlineError::Eof) => break,
            Err(e) => return Err(e.into()),
        };
        editor.add_history_entry(&line).ok();

        match parse_command(&line) {
            Command::Prompt(p) => {
                if let Err(e) = session.run_turn(&p, output).await {
                    let _ = output.human(&format!("turn failed: {e:#}"));
                }
            }
            Command::Reload => {
                match session.reload().await {
                    Ok(true) => output.human(&format!(
                        "reloaded; {} messages preserved",
                        session.history.len()
                    ))?,
                    Ok(false) => output.human("nothing to reload")?,
                    Err(e) => output.human(&format!(
                        "reload failed: {e}\n(keeping previous config; fix and try :reload again)"
                    ))?,
                }
            }
            Command::State => output.human("(:state stub — Phase 5)")?,
            Command::History => output.human("(:history stub — Phase 5)")?,
            Command::Agents => print_agents(session, output)?,
            Command::SwitchAgent(name) => switch_agent(session, &name, output)?,
            Command::Clear => {
                session.history.clear();
                output.human("history cleared")?;
            }
            Command::Help => print_help(output)?,
            Command::Quit => break,
            Command::Empty => continue,
            Command::UnknownColon(s) => {
                output.human(&format!("unknown command `{s}` — try :help"))?;
            }
        }
    }
    Ok(())
}

fn print_banner(output: &mut Output, session: &DevSession) {
    let _ = output.human(&format!(
        "tau dev — {} ({} agents, {} tools)",
        session.project_root.display(),
        session.project.agents.len(),
        session.project.tools.len(),
    ));
    let _ = output.human("type :help, :reload, :state, :quit");
}

fn print_help(output: &mut Output) -> Result<()> {
    output.human(
        "commands:\n  \
         > <text>           run a turn with the current agent\n  \
         :reload            apply pending manifest changes (history preserved)\n  \
         :state             session stats\n  \
         :history           recent messages\n  \
         :agents            list agents\n  \
         :agent <name>      switch active agent\n  \
         :clear             reset history (manifest unchanged)\n  \
         :help              this list\n  \
         :quit | Ctrl-D     exit\n\n\
         note: Ctrl-C during a turn cancels best-effort; the underlying turn\n\
         may complete in background (β.3 PR-5.1 deferral).",
    )?;
    Ok(())
}

fn print_agents(session: &DevSession, output: &mut Output) -> Result<()> {
    for name in session.project.agents.keys() {
        let marker = if name == session.current_agent_name() {
            "*"
        } else {
            " "
        };
        output.human(&format!(" {marker} {name}"))?;
    }
    Ok(())
}

fn switch_agent(session: &mut DevSession, name: &str, output: &mut Output) -> Result<()> {
    if !session.project.agents.contains_key(name) {
        output.human(&format!("agent `{name}` not in tau.toml"))?;
        return Ok(());
    }
    session.current_agent = name.to_string();
    output.human(&format!("switched to `{name}`"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_prompt() {
        assert_eq!(
            parse_command("hello world"),
            Command::Prompt("hello world".into())
        );
    }

    #[test]
    fn parses_reload() {
        assert_eq!(parse_command(":reload"), Command::Reload);
        assert_eq!(parse_command("  :reload  "), Command::Reload);
    }

    #[test]
    fn parses_switch_agent() {
        assert_eq!(
            parse_command(":agent fan-monitor"),
            Command::SwitchAgent("fan-monitor".into())
        );
    }

    #[test]
    fn empty_line_is_empty() {
        assert_eq!(parse_command(""), Command::Empty);
        assert_eq!(parse_command("   "), Command::Empty);
    }

    #[test]
    fn unknown_colon_command() {
        match parse_command(":foobar") {
            Command::UnknownColon(s) => assert_eq!(s, ":foobar"),
            other => panic!("expected UnknownColon, got {other:?}"),
        }
    }
}
