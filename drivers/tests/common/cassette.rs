//! The cassette: one real exchange sequence with a provider, credentials
//! never read, committed and replayed by the stub.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::Captured;

pub(crate) const ALLOWED_HEADERS: [&str; 3] =
    ["content-type", "anthropic-version", "anthropic-beta"];
pub(crate) const TRANSPORT_HEADERS: [&str; 6] = [
    "host",
    "content-length",
    "accept",
    "user-agent",
    "accept-encoding",
    "connection",
];
pub(crate) const DROPPED_AUTH_HEADERS: [&str; 2] = ["x-api-key", "authorization"];

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct Cassette {
    pub(crate) v: u16,
    pub(crate) recorded_at: String,
    pub(crate) target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) model: Option<String>,
    pub(crate) exchanges: Vec<Exchange>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct Exchange {
    pub(crate) request: RecordedRequest,
    pub(crate) response: RecordedResponse,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct RecordedRequest {
    pub(crate) method: String,
    pub(crate) path: String,
    pub(crate) headers: BTreeMap<String, String>,
    pub(crate) body: Value,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct RecordedResponse {
    pub(crate) status: u16,
    pub(crate) body: Value,
}

/// Copies the request by allowlist. `Err(name)` names the first header that
/// is neither allowlisted, transport, nor a known auth header.
pub(crate) fn redact(captured: &Captured) -> Result<RecordedRequest, String> {
    let mut lines = captured.head.lines();
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split(' ');
    let method = parts.next().unwrap_or("").to_owned();
    let path = parts.next().unwrap_or("").to_owned();
    let mut headers = BTreeMap::new();
    for line in lines {
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let name = k.trim().to_ascii_lowercase();
        if ALLOWED_HEADERS.contains(&name.as_str()) {
            headers.insert(name, v.trim().to_owned());
        } else if TRANSPORT_HEADERS.contains(&name.as_str())
            || DROPPED_AUTH_HEADERS.contains(&name.as_str())
        {
            continue;
        } else {
            return Err(name);
        }
    }
    let body = serde_json::from_slice(&captured.body).unwrap_or(Value::Null);
    Ok(RecordedRequest {
        method,
        path,
        headers,
        body,
    })
}

pub(crate) fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("cassettes")
}

pub(crate) fn path(target: &str, scenario: &str) -> PathBuf {
    dir().join(target).join(format!("{scenario}.json"))
}

pub(crate) fn load(target: &str, scenario: &str) -> Option<Cassette> {
    let text = std::fs::read_to_string(path(target, scenario)).ok()?;
    Some(serde_json::from_str(&text).expect("cassette parses"))
}

pub(crate) fn save(c: &Cassette, scenario: &str) {
    let p = path(&c.target, scenario);
    std::fs::create_dir_all(p.parent().expect("has parent")).expect("mkdir");
    let text = serde_json::to_string_pretty(c).expect("serializes");
    if let Some(pattern) = scan_for_secrets(&text) {
        panic!("refusing to write {}: matches {pattern}", p.display());
    }
    std::fs::write(&p, text + "\n").expect("write cassette");
}

/// `Some(name)` if `text` contains a key-shaped string.
pub(crate) fn scan_for_secrets(text: &str) -> Option<String> {
    if text.contains("sk-ant-") {
        return Some("sk-ant-".into());
    }
    if text.contains("sk-proj-") {
        return Some("sk-proj-".into());
    }
    if text.contains("Bearer ") {
        return Some("Bearer ".into());
    }
    // `sk-` followed by 20+ key characters.
    let bytes = text.as_bytes();
    let mut i = 0;
    while let Some(pos) = text.get(i..).and_then(|t| t.find("sk-")) {
        let start = i + pos + 3;
        let run = bytes.get(start..).map_or(0, |rest| {
            rest.iter()
                .take_while(|b| b.is_ascii_alphanumeric() || **b == b'_' || **b == b'-')
                .count()
        });
        if run >= 20 {
            return Some("sk-<20+ chars>".into());
        }
        i = start;
    }
    None
}

pub(crate) fn all_files() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(targets) = std::fs::read_dir(dir()) else {
        return out;
    };
    for target in targets.flatten() {
        let Ok(files) = std::fs::read_dir(target.path()) else {
            continue;
        };
        for f in files.flatten() {
            if f.path().extension().is_some_and(|e| e == "json") {
                out.push(f.path());
            }
        }
    }
    out.sort();
    out
}
