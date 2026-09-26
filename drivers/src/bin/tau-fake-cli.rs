//! `tau-fake-cli`: a scripted stand-in for an agent CLI (ADR-0013 §10).
//!
//! The agent drivers spawn a binary, write a task on its stdin, read one
//! JSON event per stdout line, and climb a cancel ladder when a run has to
//! stop (ADR-0013 §5). Every test of that path points
//! `AgentConfig::binary` here instead of at `claude` or `codex`: this
//! binary replays a recorded stdout transcript line by line, with optional
//! delays, and reacts to stdin lines and to `SIGINT`/`SIGTERM` exactly as
//! a script says. #130's cancel runs are its first scripts.
//!
//! The script is a JSONL file named by the `TAU_FAKE_CLI_SCRIPT`
//! environment variable: one directive per line, blank lines skipped.
//!
//! # Directives
//!
//! | directive | what it does |
//! |---|---|
//! | `{"line": <json>, "delay_ms": 250}` | prints `<json>` on stdout after the delay (default 0). A string is printed verbatim; anything else is printed as compact JSON. |
//! | `{"exit": 1, "delay_ms": 250}` | exits with that status after the delay. |
//! | `{"on": {"stdin": "<substring>"}, "delay_ms": 0, "lines": [<json>…], "exit": 1}` | when a stdin line containing `<substring>` arrives: after the delay, print the lines, then exit if `exit` is given. |
//! | `{"on": {"signal": "SIGINT"}, "delay_ms": 0, "lines": [<json>…], "exit": 0}` | the same, for `SIGINT` or `SIGTERM`. |
//! | `{"on": {"signal": "SIGTERM"}, "ignore": true}` | swallows the signal, so the driver has to reach `SIGKILL`. |
//! | `{"withhold": "<substring>"}` | never prints a line containing `<substring>`, wherever the script would have printed it. Withholding `"type":"result"` produces a run with no terminal event: `lost`. |
//! | `{"touch": "<path>", "delay_ms": 0}` | creates (or truncates) the file at `<path>` after the delay. A readiness marker: the signal handlers are installed before the first timeline step runs, so a test that waits for the file before sending a signal knows the signal will be caught, however slowly the binary started. |
//!
//! `line`, `touch` and `exit` directives form the **timeline**, in file order. A
//! reaction **replaces** whatever the timeline still had to print: the
//! CLI was interrupted mid-turn, and what follows is the interruption's
//! own output. `ignore` leaves the timeline alone.
//!
//! A signal with no `on` directive keeps its default disposition, so an
//! unscripted `SIGTERM` kills the binary the way it kills `claude`
//! (#130 §5c: nothing printed, exit 143). A stdin line no directive
//! matches is read and dropped.
//!
//! Lines whose delay is zero are printed without looking for a stimulus in
//! between, so a script with no delays replays its transcript whole before
//! any reaction runs. A stimulus that arrives during a delay and is ignored
//! restarts that delay.
//!
//! When the timeline is spent and stdin has reached end of file, the binary
//! exits 0, as a CLI does once its parent closes the pipe. While stdin is
//! open it waits for a stimulus.
//!
//! # Example
//!
//! #130 §5a, the in-band interrupt, as a script:
//!
//! ```text
//! {"line": {"type":"system","subtype":"init","session_id":"a16000ab"}}
//! {"line": {"type":"assistant","message":{"content":[{"type":"tool_use","name":"Write"}]}}, "delay_ms": 50}
//! {"on": {"stdin": "\"subtype\":\"interrupt\""}, "lines": [
//!     {"type":"control_response","response":{"subtype":"success","request_id":"tau-cancel-1"}},
//!     {"type":"result","subtype":"error_during_execution","terminal_reason":"aborted_tools","num_turns":3}
//! ], "exit": 1}
//! {"on": {"signal": "SIGTERM"}, "exit": 143}
//! ```
//!
//! (One directive per line in the file; the `on` directive is wrapped here
//! for the page.)
//!
//! # What it is not
//!
//! No clock is read: delays are `recv_timeout` on the channel the stdin and
//! signal threads feed, the way the driver and the sandbox shim bound
//! time. Plain `std` and `signal-hook`, no `tokio`. Exit `2` with a message
//! on stderr when the script is missing or malformed; nothing is printed on
//! stdout in that case, so the driver reports `lost` rather than parsing a
//! fake's complaint as a CLI's event.

#[cfg(unix)]
mod fake {
    use std::collections::{BTreeMap, VecDeque};
    use std::io::{self, BufRead, Write};
    use std::path::PathBuf;
    use std::process::ExitCode;
    use std::sync::mpsc;
    use std::time::Duration;

    use serde_json::{Map, Value};
    use signal_hook::consts::{SIGINT, SIGTERM};
    use signal_hook::iterator::Signals;

    /// The environment variable naming the script.
    const SCRIPT_VAR: &str = "TAU_FAKE_CLI_SCRIPT";

    /// One thing the binary does.
    #[derive(Clone)]
    enum Action {
        /// Print this line on stdout.
        Emit(String),
        /// Create this file: a readiness marker for a test.
        Touch(PathBuf),
        /// Exit with this status.
        Exit(i32),
    }

    /// An action, after a delay.
    #[derive(Clone)]
    struct Step {
        delay: Duration,
        action: Action,
    }

    /// What a scripted stimulus does.
    #[derive(Clone)]
    enum Reaction {
        /// Nothing: the timeline goes on.
        Ignore,
        /// Replace the timeline with these steps.
        Replace(Vec<Step>),
    }

    /// The parsed script.
    struct Script {
        timeline: VecDeque<Step>,
        on_stdin: Vec<(String, Reaction)>,
        on_signal: BTreeMap<i32, Reaction>,
        withhold: Vec<String>,
    }

    /// What the main loop waits for.
    enum Event {
        Stdin(String),
        Eof,
        Signal(i32),
    }

    /// A line as it is printed: strings verbatim, anything else compact.
    fn render(value: &Value) -> String {
        match value {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        }
    }

    fn delay_of(directive: &Map<String, Value>) -> Result<Duration, String> {
        match directive.get("delay_ms") {
            None => Ok(Duration::ZERO),
            Some(Value::Number(n)) => n
                .as_u64()
                .map(Duration::from_millis)
                .ok_or_else(|| format!("delay_ms must be a non-negative integer, got {n}")),
            Some(other) => Err(format!("delay_ms must be a number, got {other}")),
        }
    }

    fn exit_of(directive: &Map<String, Value>) -> Result<Option<i32>, String> {
        match directive.get("exit") {
            None => Ok(None),
            Some(Value::Number(n)) => n
                .as_i64()
                .and_then(|code| i32::try_from(code).ok())
                .map(Some)
                .ok_or_else(|| format!("exit must be an exit status, got {n}")),
            Some(other) => Err(format!("exit must be a number, got {other}")),
        }
    }

    /// The steps of a reaction: the delay, the lines, the exit.
    fn reaction_of(directive: &Map<String, Value>) -> Result<Reaction, String> {
        if directive.get("ignore") == Some(&Value::Bool(true)) {
            return Ok(Reaction::Ignore);
        }
        let mut delay = delay_of(directive)?;
        let mut steps = Vec::new();
        let lines = match directive.get("lines") {
            None => Vec::new(),
            Some(Value::Array(lines)) => lines.clone(),
            Some(other) => return Err(format!("lines must be an array, got {other}")),
        };
        for line in &lines {
            steps.push(Step {
                delay,
                action: Action::Emit(render(line)),
            });
            delay = Duration::ZERO;
        }
        if let Some(code) = exit_of(directive)? {
            steps.push(Step {
                delay,
                action: Action::Exit(code),
            });
        }
        Ok(Reaction::Replace(steps))
    }

    fn signal_named(name: &str) -> Result<i32, String> {
        match name {
            "SIGINT" => Ok(SIGINT),
            "SIGTERM" => Ok(SIGTERM),
            other => Err(format!("signal must be SIGINT or SIGTERM, got {other}")),
        }
    }

    fn parse(text: &str) -> Result<Script, String> {
        let mut script = Script {
            timeline: VecDeque::new(),
            on_stdin: Vec::new(),
            on_signal: BTreeMap::new(),
            withhold: Vec::new(),
        };
        for (index, raw) in text.lines().enumerate() {
            let number = index.saturating_add(1);
            if raw.trim().is_empty() {
                continue;
            }
            let value: Value =
                serde_json::from_str(raw).map_err(|e| format!("line {number}: {e}"))?;
            let Value::Object(directive) = value else {
                return Err(format!("line {number}: a directive is a JSON object"));
            };
            let with = |what: &str| format!("line {number}: {what}");
            if let Some(on) = directive.get("on") {
                let reaction = reaction_of(&directive).map_err(|e| with(&e))?;
                match on {
                    Value::Object(on) => match (on.get("stdin"), on.get("signal")) {
                        (Some(Value::String(pattern)), None) => {
                            script.on_stdin.push((pattern.clone(), reaction));
                        }
                        (None, Some(Value::String(name))) => {
                            let signal = signal_named(name).map_err(|e| with(&e))?;
                            script.on_signal.insert(signal, reaction);
                        }
                        _ => return Err(with("on takes {\"stdin\": …} or {\"signal\": …}")),
                    },
                    other => return Err(with(&format!("on must be an object, got {other}"))),
                }
            } else if let Some(pattern) = directive.get("withhold") {
                match pattern {
                    Value::String(pattern) => script.withhold.push(pattern.clone()),
                    other => return Err(with(&format!("withhold must be a string, got {other}"))),
                }
            } else if let Some(line) = directive.get("line") {
                script.timeline.push_back(Step {
                    delay: delay_of(&directive).map_err(|e| with(&e))?,
                    action: Action::Emit(render(line)),
                });
            } else if let Some(path) = directive.get("touch") {
                let Value::String(path) = path else {
                    return Err(with(&format!("touch must be a string, got {path}")));
                };
                script.timeline.push_back(Step {
                    delay: delay_of(&directive).map_err(|e| with(&e))?,
                    action: Action::Touch(PathBuf::from(path)),
                });
            } else if let Some(code) = exit_of(&directive).map_err(|e| with(&e))? {
                script.timeline.push_back(Step {
                    delay: delay_of(&directive).map_err(|e| with(&e))?,
                    action: Action::Exit(code),
                });
            } else {
                return Err(with("a directive is line, touch, exit, on, or withhold"));
            }
        }
        Ok(script)
    }

    /// Performs one step. `Some(code)` means exit.
    fn perform(step: Step, withhold: &[String]) -> Result<Option<i32>, String> {
        match step.action {
            Action::Exit(code) => Ok(Some(code)),
            Action::Touch(path) => {
                std::fs::write(&path, b"").map_err(|e| format!("{}: {e}", path.display()))?;
                Ok(None)
            }
            Action::Emit(text) => {
                if withhold.iter().any(|pattern| text.contains(pattern)) {
                    return Ok(None);
                }
                let mut out = io::stdout().lock();
                out.write_all(text.as_bytes())
                    .and_then(|()| out.write_all(b"\n"))
                    .and_then(|()| out.flush())
                    .map_err(|e| format!("stdout: {e}"))?;
                Ok(None)
            }
        }
    }

    /// Runs the script to its exit.
    fn run(mut script: Script, rx: &mpsc::Receiver<Event>) -> Result<i32, String> {
        let mut eof = false;
        loop {
            let event = match script.timeline.front().map(|step| step.delay) {
                Some(delay) if delay.is_zero() => {
                    if let Some(step) = script.timeline.pop_front() {
                        if let Some(code) = perform(step, &script.withhold)? {
                            return Ok(code);
                        }
                    }
                    continue;
                }
                Some(delay) => match rx.recv_timeout(delay) {
                    Ok(event) => event,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if let Some(step) = script.timeline.pop_front() {
                            if let Some(code) = perform(step, &script.withhold)? {
                                return Ok(code);
                            }
                        }
                        continue;
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        return Err("every stimulus thread is gone".to_owned());
                    }
                },
                None if eof => return Ok(0),
                None => rx
                    .recv()
                    .map_err(|_| "every stimulus thread is gone".to_owned())?,
            };
            let reaction = match event {
                Event::Eof => {
                    eof = true;
                    None
                }
                Event::Stdin(line) => script
                    .on_stdin
                    .iter()
                    .find(|(pattern, _)| line.contains(pattern.as_str()))
                    .map(|(_, reaction)| reaction.clone()),
                Event::Signal(signal) => script.on_signal.get(&signal).cloned(),
            };
            if let Some(Reaction::Replace(steps)) = reaction {
                script.timeline = steps.into();
            }
        }
    }

    fn start(script: Script) -> Result<i32, String> {
        let (tx, rx) = mpsc::channel::<Event>();

        let stdin_tx = tx.clone();
        drop(
            std::thread::Builder::new()
                .name("tau-fake-cli-stdin".to_owned())
                .spawn(move || {
                    for line in io::stdin().lock().lines() {
                        let Ok(line) = line else { break };
                        if stdin_tx.send(Event::Stdin(line)).is_err() {
                            return;
                        }
                    }
                    let _ = stdin_tx.send(Event::Eof);
                })
                .map_err(|e| format!("stdin thread: {e}"))?,
        );

        let scripted: Vec<i32> = script.on_signal.keys().copied().collect();
        if !scripted.is_empty() {
            let mut signals = Signals::new(scripted).map_err(|e| format!("signal handler: {e}"))?;
            drop(
                std::thread::Builder::new()
                    .name("tau-fake-cli-signals".to_owned())
                    .spawn(move || {
                        for signal in signals.forever() {
                            if tx.send(Event::Signal(signal)).is_err() {
                                return;
                            }
                        }
                    })
                    .map_err(|e| format!("signal thread: {e}"))?,
            );
        }
        run(script, &rx)
    }

    fn load() -> Result<Script, String> {
        let path = std::env::var_os(SCRIPT_VAR)
            .ok_or_else(|| format!("{SCRIPT_VAR} is not set: nothing to replay"))?;
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("{}: {e}", path.to_string_lossy()))?;
        parse(&text).map_err(|e| format!("{}: {e}", path.to_string_lossy()))
    }

    pub(super) fn main() -> ExitCode {
        let outcome = load().and_then(start);
        match outcome {
            Ok(code) => match u8::try_from(code) {
                Ok(code) => ExitCode::from(code),
                Err(_) => {
                    eprintln!("tau-fake-cli: exit status {code} is not a byte");
                    ExitCode::from(2)
                }
            },
            Err(message) => {
                eprintln!("tau-fake-cli: {message}");
                ExitCode::from(2)
            }
        }
    }
}

fn main() -> std::process::ExitCode {
    #[cfg(unix)]
    {
        fake::main()
    }
    #[cfg(not(unix))]
    {
        std::process::ExitCode::FAILURE
    }
}
