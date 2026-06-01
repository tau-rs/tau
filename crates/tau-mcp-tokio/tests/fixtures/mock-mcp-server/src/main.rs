//! Mock MCP server for tau-mcp-tokio integration tests.
//!
//! Speaks line-delimited JSON-RPC over stdin/stdout. Supports:
//!
//! - `initialize` — returns `{name:"mock", version:"0.0.0"}` + protocol.
//! - `tools/list` — returns one tool, `echo`, schema {message: string}.
//! - `tools/call name=echo` — returns the message in a text content block.
//! - Anything else — JSON-RPC error code -32601 method not found.
//!
//! Scenarios via `TAU_MCP_FIXTURE_SCENARIO` env var:
//!
//! - `happy` (default) — normal behavior.
//! - `handshake_slow` — sleeps 5s before responding to initialize.
//! - `refuse_initialize` — returns JSON-RPC error to initialize.
//! - `crash_on_call` — exits with code 137 on first tools/call.

use std::io::{BufRead, Write};
use std::time::Duration;

fn main() {
    let scenario = std::env::var("TAU_MCP_FIXTURE_SCENARIO").unwrap_or_else(|_| "happy".into());
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) if l.trim().is_empty() => continue,
            Ok(l) => l,
            Err(_) => break,
        };
        let req: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let id = req.get("id").cloned();
        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");

        let resp_payload = match method {
            "initialize" => {
                if scenario == "handshake_slow" {
                    std::thread::sleep(Duration::from_secs(5));
                }
                if scenario == "refuse_initialize" {
                    serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {"code": -32603, "message": "refused by scenario"}
                    })
                } else {
                    serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "protocolVersion": "2025-03-26",
                            "serverInfo": {"name": "mock", "version": "0.0.0"}
                        }
                    })
                }
            }
            "tools/list" => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "tools": [
                        {
                            "name": "echo",
                            "description": "Echo a message",
                            "inputSchema": {
                                "type": "object",
                                "properties": {"message": {"type": "string"}},
                                "required": ["message"]
                            }
                        }
                    ]
                }
            }),
            "tools/call" => {
                if scenario == "crash_on_call" {
                    std::process::exit(137);
                }
                let params = req.get("params").cloned().unwrap_or_default();
                let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or_default();
                if name == "echo" {
                    let msg = args.get("message").and_then(|m| m.as_str()).unwrap_or("");
                    serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {"content": [{"type": "text", "text": msg}]}
                    })
                } else {
                    serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {"code": -32602, "message": format!("unknown tool: {name}")}
                    })
                }
            }
            _ => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": -32601, "message": format!("method not found: {method}")}
            }),
        };

        let bytes = serde_json::to_vec(&resp_payload).unwrap();
        out.write_all(&bytes).unwrap();
        out.write_all(b"\n").unwrap();
        out.flush().unwrap();
    }
}
