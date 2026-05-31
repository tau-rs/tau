//! `tau plugin run <binary> [--interactive | --script <path>]` —
//! standalone plugin REPL / scripted driver.
//!
//! Per spec §9 (debug tier): spawns an arbitrary plugin binary
//! (typically built locally without going through `tau install`),
//! drives the handshake, and lets the operator pump requests through
//! either an interactive REPL or a scripted JSONL file. The handler
//! never installs the plugin into a lockfile; the binary path is the
//! only contract.
//!
//! Wire protocol details mirror [`tau_runtime_tokio::plugin_host`] but the
//! implementation is intentionally standalone: this command exists to
//! probe arbitrary binaries that aren't necessarily wired into a
//! project, so we don't reach for the `LockedPlugin`-keyed loader.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, Context};
use tau_domain::PortKind;
use tau_plugin_protocol::handshake::{meta, HandshakeRequest, TraceContext, PROTOCOL_VERSION};
use tau_plugin_protocol::{Frame, FramedReader, FramedWriter, FramerOptions};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::cli::PluginRunArgs;
use crate::output::Output;

/// Run `tau plugin run`.
pub async fn run(args: &PluginRunArgs, output: &mut Output) -> anyhow::Result<()> {
    if !args.interactive && args.script.is_none() {
        anyhow::bail!("`tau plugin run` requires either --interactive or --script <path>");
    }

    // Spawn the plugin binary directly. Mirrors the env scrubbing of
    // `PluginProcess::spawn_and_handshake`: env_clear + the two
    // TAU_PLUGIN_* env vars + PATH for shared-library resolution.
    let mut command = Command::new(&args.binary);
    command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .env_clear()
        .env(
            "TAU_PLUGIN_RUN_ID",
            format!(
                "tau-plugin-run-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ),
        )
        .env("TAU_PLUGIN_AGENT_ID", "tau-plugin-run")
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .kill_on_drop(true);

    let mut child = command
        .spawn()
        .with_context(|| format!("spawning plugin binary {:?}", args.binary))?;

    let stdin = child
        .stdin
        .take()
        .expect("stdin piped via stdin(Stdio::piped())");
    let stdout = child
        .stdout
        .take()
        .expect("stdout piped via stdout(Stdio::piped())");

    let mut writer = FramedWriter::new(stdin);
    let mut reader = FramedReader::new(stdout, FramerOptions::default());

    // ---- Handshake ----
    //
    // Drive a `meta.handshake` request directly. We don't use the
    // host-side handshake driver because that one expects a specific
    // port (taken from `LockedPlugin::manifest.provides`); for
    // `tau plugin run` we don't know what the plugin advertises until
    // it tells us. We send `LlmBackend` as a placeholder and let the
    // plugin respond with whatever it actually provides.
    //
    // (The plugin itself decides what `provides` to send back; SDK-
    // built plugins ignore the host's `port` field and report their
    // own. Plugins that strictly validate the requested port will
    // reject this — operators of those should use `tau plugin describe`
    // against an installed package instead.)
    let trace_context = TraceContext::new(
        format!(
            "tau-plugin-run-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ),
        "tau-plugin-run".to_string(),
        "root".to_string(),
    );
    let handshake = HandshakeRequest::new(
        PROTOCOL_VERSION.to_string(),
        PortKind::LlmBackend,
        trace_context,
        serde_json::Value::Null,
    );
    let params_bytes =
        rmp_serde::to_vec(&vec![&handshake]).context("encoding meta.handshake request params")?;
    let request = Frame::Request {
        id: 1,
        method: meta::HANDSHAKE_METHOD.to_string(),
        params: params_bytes,
    };
    let request_bytes = request.encode().context("encoding meta.handshake frame")?;
    writer
        .write_frame(&request_bytes)
        .await
        .context("writing meta.handshake frame")?;

    let response_body = tokio::time::timeout(Duration::from_secs(5), reader.next_frame())
        .await
        .context("waiting for meta.handshake response")?
        .context("reading meta.handshake response")?
        .ok_or_else(|| anyhow!("plugin closed stdout before sending handshake response"))?;
    let response = Frame::decode(&response_body).context("decoding meta.handshake response")?;

    let plugin_name = match response {
        Frame::Response {
            id: 1,
            error: None,
            result: Some(bytes),
        } => {
            let resp: tau_plugin_protocol::HandshakeResponse =
                rmp_serde::from_slice(&bytes).context("decoding handshake response body")?;
            output.status(format!(
                "Connected to plugin {} {} (provides {:?}, protocol v{}).",
                resp.plugin_name, resp.plugin_version, resp.provides, resp.protocol_version
            ))?;
            if !resp.methods.is_empty() {
                output.status(format!("Methods: {}", resp.methods.join(", ")))?;
            }
            resp.plugin_name
        }
        Frame::Response {
            error: Some(env), ..
        } => {
            anyhow::bail!(
                "plugin returned handshake error: code={} message={}",
                env.code,
                env.message
            );
        }
        other => {
            anyhow::bail!("expected handshake Response, got {other:?}");
        }
    };

    // ---- Driver loop ----
    let driver_result = if args.interactive {
        run_interactive(&mut reader, &mut writer, &plugin_name, output).await
    } else if let Some(script) = &args.script {
        run_scripted(&mut reader, &mut writer, &plugin_name, script, output).await
    } else {
        // Already validated above.
        Ok(())
    };

    // Best-effort shutdown: send `meta.shutdown` and wait briefly.
    let shutdown_frame = Frame::Notification {
        method: meta::SHUTDOWN_METHOD.to_string(),
        params: rmp_serde::to_vec::<Vec<()>>(&Vec::new())
            .expect("encoding empty Vec<()> never fails"),
    };
    if let Ok(body) = shutdown_frame.encode() {
        let _ = writer.write_frame(&body).await;
    }
    let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;

    driver_result
}

/// Drive an interactive REPL against a connected plugin.
///
/// Reads from stdin one line at a time. Each line is `<method> [json-args]`
/// where `json-args` is an optional JSON value (defaults to `[]`).
/// Special command `exit` (or EOF) terminates the loop.
async fn run_interactive<R, W>(
    reader: &mut FramedReader<R>,
    writer: &mut FramedWriter<W>,
    plugin_name: &str,
    output: &mut Output,
) -> anyhow::Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    output.status("Type `<method> [json-args]` to send a request. `exit` (or Ctrl-D) to quit.")?;

    let stdin = tokio::io::stdin();
    let mut stdin_lines = BufReader::new(stdin).lines();

    let mut next_msgid: u32 = 2;

    loop {
        // Print a prompt to stderr so the user can read responses on
        // stdout without ANSI mixing.
        output.status(format!("{plugin_name}> "))?;

        let line = match stdin_lines.next_line().await {
            Ok(Some(l)) => l,
            Ok(None) => break,
            Err(e) => return Err(e.into()),
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed == "exit" || trimmed == "/exit" {
            break;
        }

        let (method, args_json) = parse_repl_line(trimmed)?;
        let id = next_msgid;
        next_msgid = next_msgid.wrapping_add(1).max(2);

        match send_request(reader, writer, id, &method, args_json, output).await {
            Ok(()) => {}
            Err(e) => {
                output.error(format!("error: {e}"))?;
                continue;
            }
        }
    }

    Ok(())
}

/// Drive a scripted JSONL file against a connected plugin.
async fn run_scripted<R, W>(
    reader: &mut FramedReader<R>,
    writer: &mut FramedWriter<W>,
    _plugin_name: &str,
    script_path: &PathBuf,
    output: &mut Output,
) -> anyhow::Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let contents = tokio::fs::read_to_string(script_path)
        .await
        .with_context(|| format!("reading script {}", script_path.display()))?;

    let mut next_msgid: u32 = 2;
    for (lineno, line) in contents.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let entry: ScriptEntry = serde_json::from_str(line).with_context(|| {
            format!(
                "parsing script line {} ({}): expected `{{ \"method\": \"...\", \"params\": [...] }}`",
                lineno + 1,
                script_path.display()
            )
        })?;
        let params = entry.params.unwrap_or_else(|| serde_json::json!([]));
        let id = next_msgid;
        next_msgid = next_msgid.wrapping_add(1).max(2);
        send_request(reader, writer, id, &entry.method, params, output).await?;
    }

    Ok(())
}

/// Schema for one line of a scripted JSONL input file.
#[derive(serde::Deserialize)]
struct ScriptEntry {
    method: String,
    /// Defaults to `[]` if omitted.
    params: Option<serde_json::Value>,
}

/// Parse a REPL line like `llm.complete {"prompt":"hi"}` into a method
/// + `serde_json::Value` params (defaulting to `[]` when no args
///   follow).
fn parse_repl_line(line: &str) -> anyhow::Result<(String, serde_json::Value)> {
    let mut parts = line.splitn(2, char::is_whitespace);
    let method = parts
        .next()
        .ok_or_else(|| anyhow!("empty REPL line"))?
        .to_string();
    let rest = parts.next().unwrap_or("").trim();
    let params: serde_json::Value = if rest.is_empty() {
        serde_json::json!([])
    } else {
        serde_json::from_str(rest).with_context(|| format!("parsing JSON args {rest:?}"))?
    };
    Ok((method, params))
}

/// Encode `params_json` as MessagePack, send the request, and print
/// the response.
async fn send_request<R, W>(
    reader: &mut FramedReader<R>,
    writer: &mut FramedWriter<W>,
    id: u32,
    method: &str,
    params_json: serde_json::Value,
    output: &mut Output,
) -> anyhow::Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    // Convert the JSON params to MessagePack bytes. rmp-serde encodes
    // serde-compatible values directly; we serialize through
    // serde_json::Value's serde impl into a Vec<u8>.
    let params_bytes =
        rmp_serde::to_vec(&params_json).context("encoding request params as MessagePack")?;
    let frame = Frame::Request {
        id,
        method: method.to_string(),
        params: params_bytes,
    };
    let body = frame.encode().context("encoding request frame")?;
    writer
        .write_frame(&body)
        .await
        .context("writing request frame")?;

    // Drain frames until we see the matching response. Streaming
    // chunks (`stream.chunk` notifications) are surfaced verbatim.
    loop {
        let body = tokio::time::timeout(Duration::from_secs(30), reader.next_frame())
            .await
            .context("waiting for response")?
            .context("reading response frame")?
            .ok_or_else(|| anyhow!("plugin closed stdout while awaiting response"))?;
        let frame = Frame::decode(&body).context("decoding response frame")?;
        match frame {
            Frame::Response {
                id: rid,
                error,
                result,
            } if rid == id => {
                if let Some(env) = error {
                    let json = serde_json::json!({
                        "error": {
                            "code": env.code,
                            "message": env.message,
                        },
                    });
                    output.human(&serde_json::to_string_pretty(&json).unwrap_or_default())?;
                } else if let Some(bytes) = result {
                    let value: rmpv::Value = rmpv::decode::read_value(&mut bytes.as_slice())
                        .context("decoding response result MessagePack")?;
                    let json = msgpack_to_json(&value);
                    output.human(&serde_json::to_string_pretty(&json).unwrap_or_default())?;
                } else {
                    output.human("(null result)")?;
                }
                return Ok(());
            }
            Frame::Notification { method, params } => {
                // Decode the notification params and emit verbatim
                // (for stream.chunk and friends).
                let params_value: rmpv::Value =
                    rmpv::decode::read_value(&mut params.as_slice()).unwrap_or(rmpv::Value::Nil);
                let json = msgpack_to_json(&params_value);
                output.human(&format!(
                    "[notification] {method}: {}",
                    serde_json::to_string(&json).unwrap_or_default()
                ))?;
            }
            Frame::Response { id: rid, .. } => {
                output.error(format!(
                    "warning: ignoring stray response with id={rid} (expecting {id})"
                ))?;
            }
            // Frame is `#[non_exhaustive]`; skip unknown variants.
            _ => {}
        }
    }
}

/// Best-effort `rmpv::Value` → `serde_json::Value` projection for
/// human-readable display. Maps that contain non-string keys collapse
/// keys to their `Debug` representation.
fn msgpack_to_json(value: &rmpv::Value) -> serde_json::Value {
    match value {
        rmpv::Value::Nil => serde_json::Value::Null,
        rmpv::Value::Boolean(b) => serde_json::Value::Bool(*b),
        rmpv::Value::Integer(i) => {
            if let Some(n) = i.as_i64() {
                serde_json::Value::Number(n.into())
            } else if let Some(n) = i.as_u64() {
                serde_json::Value::Number(n.into())
            } else {
                serde_json::Value::String(format!("{i:?}"))
            }
        }
        rmpv::Value::F32(f) => serde_json::Number::from_f64((*f).into())
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        rmpv::Value::F64(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        rmpv::Value::String(s) => match s.as_str() {
            Some(s) => serde_json::Value::String(s.to_string()),
            None => serde_json::Value::String(format!("{s:?}")),
        },
        rmpv::Value::Binary(b) => serde_json::Value::String(format!("<{}b binary>", b.len())),
        rmpv::Value::Array(a) => serde_json::Value::Array(a.iter().map(msgpack_to_json).collect()),
        rmpv::Value::Map(m) => {
            let mut obj = serde_json::Map::with_capacity(m.len());
            for (k, v) in m {
                let key = match k {
                    rmpv::Value::String(s) => s.as_str().unwrap_or("<binary>").to_string(),
                    other => format!("{other:?}"),
                };
                obj.insert(key, msgpack_to_json(v));
            }
            serde_json::Value::Object(obj)
        }
        rmpv::Value::Ext(tag, _bytes) => serde_json::Value::String(format!("<ext tag={tag}>")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_repl_line_with_no_args_defaults_to_empty_array() {
        let (method, params) = parse_repl_line("llm.complete").unwrap();
        assert_eq!(method, "llm.complete");
        assert_eq!(params, serde_json::json!([]));
    }

    #[test]
    fn parse_repl_line_with_json_args() {
        let (method, params) = parse_repl_line(r#"llm.complete {"prompt":"hi"}"#).unwrap();
        assert_eq!(method, "llm.complete");
        assert_eq!(params, serde_json::json!({"prompt": "hi"}));
    }

    #[test]
    fn parse_repl_line_with_array_args() {
        let (method, params) = parse_repl_line(r#"tool.call ["arg1", 42]"#).unwrap();
        assert_eq!(method, "tool.call");
        assert_eq!(params, serde_json::json!(["arg1", 42]));
    }

    #[test]
    fn parse_repl_line_rejects_invalid_json() {
        let err = parse_repl_line("method {oops").unwrap_err();
        assert!(format!("{err}").contains("parsing JSON"));
    }

    #[test]
    fn msgpack_to_json_translates_primitives() {
        assert_eq!(msgpack_to_json(&rmpv::Value::Nil), serde_json::Value::Null);
        assert_eq!(
            msgpack_to_json(&rmpv::Value::Boolean(true)),
            serde_json::Value::Bool(true)
        );
        assert_eq!(
            msgpack_to_json(&rmpv::Value::Integer(42i64.into())),
            serde_json::Value::Number(42.into())
        );
    }

    #[test]
    fn msgpack_to_json_translates_map_with_string_keys() {
        let map = rmpv::Value::Map(vec![(
            rmpv::Value::String("k".into()),
            rmpv::Value::Integer(1i64.into()),
        )]);
        let json = msgpack_to_json(&map);
        assert_eq!(json, serde_json::json!({"k": 1}));
    }
}
