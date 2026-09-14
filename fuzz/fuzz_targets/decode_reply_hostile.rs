//! `libtau::decode_reply` over structurally-valid-but-hostile replies.
//!
//! The byte-level target finds framing bugs; this one starts past the
//! framing. An `Arbitrary` shape mirroring ADR-0006 §3 is rendered to JSON
//! that is always well-formed and often wrong in the ways a model or a
//! misbehaving driver could be wrong: a `v` that is not 1, a `stop` nobody
//! defined, blocks of an unknown `type`, fields that do not exist, numbers
//! at the edges of `u64`, and text repeated until it is large.
//!
//! What must hold: no panic; a reply that decodes carries `v == 1`; a
//! `Version` error names the `v` that was sent; and a reply that decodes
//! survives a round trip.

#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use serde_json::{json, Map, Value};
use tau_kernel::bridge::VERSION;

/// How `v` is rendered. The interesting cases are the ones near the real one.
#[derive(Arbitrary, Debug)]
enum Version {
    Current,
    Zero,
    Next,
    Max,
    Any(u16),
    Negative,
    Text(String),
    Missing,
}

impl Version {
    fn render(&self) -> Option<Value> {
        Some(match self {
            Self::Current => json!(VERSION),
            Self::Zero => json!(0),
            Self::Next => json!(VERSION.wrapping_add(1)),
            Self::Max => json!(u16::MAX),
            Self::Any(v) => json!(v),
            Self::Negative => json!(-1),
            Self::Text(s) => json!(s),
            Self::Missing => return None,
        })
    }
}

#[derive(Arbitrary, Debug)]
enum Block {
    Text {
        text: String,
        repeat: u8,
    },
    ToolCall {
        id: String,
        name: String,
        input: Leaf,
    },
    ToolResult {
        call_id: String,
        content: String,
        is_error: Option<bool>,
        error_kind: Option<String>,
    },
    Unknown {
        r#type: String,
    },
    Untagged(Leaf),
}

/// A JSON leaf or a shallow container, so `input` can be anything a provider
/// might put there without the fuzzer having to discover JSON syntax.
#[derive(Arbitrary, Debug)]
enum Leaf {
    Null,
    Bool(bool),
    Int(i64),
    Big(u64),
    Text(String),
    List(Vec<String>),
    Object(Vec<(String, String)>),
}

impl Leaf {
    fn render(&self) -> Value {
        match self {
            Self::Null => Value::Null,
            Self::Bool(b) => json!(b),
            Self::Int(i) => json!(i),
            Self::Big(u) => json!(u),
            Self::Text(s) => json!(s),
            Self::List(items) => json!(items),
            Self::Object(pairs) => Value::Object(
                pairs
                    .iter()
                    .map(|(k, v)| (k.clone(), json!(v)))
                    .collect::<Map<_, _>>(),
            ),
        }
    }
}

impl Block {
    fn render(&self) -> Value {
        match self {
            Self::Text { text, repeat } => {
                // Bounded: at most 255 copies of a fuzzer-sized string.
                json!({ "type": "text", "text": text.repeat(usize::from(*repeat)) })
            }
            Self::ToolCall { id, name, input } => {
                json!({ "type": "tool_call", "id": id, "name": name, "input": input.render() })
            }
            Self::ToolResult {
                call_id,
                content,
                is_error,
                error_kind,
            } => {
                let mut block = Map::new();
                block.insert("type".into(), json!("tool_result"));
                block.insert("call_id".into(), json!(call_id));
                block.insert("content".into(), json!(content));
                if let Some(e) = is_error {
                    block.insert("is_error".into(), json!(e));
                }
                if let Some(k) = error_kind {
                    block.insert("error_kind".into(), json!(k));
                }
                Value::Object(block)
            }
            Self::Unknown { r#type } => json!({ "type": r#type }),
            Self::Untagged(leaf) => leaf.render(),
        }
    }
}

#[derive(Arbitrary, Debug)]
enum Stop {
    EndTurn,
    ToolCall,
    MaxTokens,
    Sequence,
    Refusal,
    Error { kind: String, message: String },
    KnownError { message: String },
    Unknown(String),
    Leaf(Leaf),
    Missing,
}

impl Stop {
    fn render(&self) -> Option<Value> {
        Some(match self {
            Self::EndTurn => json!("end_turn"),
            Self::ToolCall => json!("tool_call"),
            Self::MaxTokens => json!("max_tokens"),
            Self::Sequence => json!("stop_sequence"),
            Self::Refusal => json!("refusal"),
            Self::Error { kind, message } => {
                json!({ "error": { "kind": kind, "message": message } })
            }
            Self::KnownError { message } => {
                json!({ "error": { "kind": "provider", "message": message } })
            }
            Self::Unknown(s) => json!(s),
            Self::Leaf(leaf) => leaf.render(),
            Self::Missing => return None,
        })
    }
}

#[derive(Arbitrary, Debug)]
enum Usage {
    Counts {
        input_tokens: u64,
        output_tokens: u64,
    },
    Partial {
        input_tokens: Option<u64>,
    },
    Negative,
    Leaf(Leaf),
    Missing,
}

impl Usage {
    fn render(&self) -> Option<Value> {
        Some(match self {
            Self::Counts {
                input_tokens,
                output_tokens,
            } => {
                json!({ "input_tokens": input_tokens, "output_tokens": output_tokens })
            }
            Self::Partial { input_tokens } => match input_tokens {
                Some(n) => json!({ "input_tokens": n }),
                None => json!({}),
            },
            Self::Negative => json!({ "input_tokens": -1, "output_tokens": -1 }),
            Self::Leaf(leaf) => leaf.render(),
            Self::Missing => return None,
        })
    }
}

#[derive(Arbitrary, Debug)]
struct Hostile {
    v: Version,
    model: Option<Leaf>,
    content: Option<Vec<Block>>,
    stop: Stop,
    usage: Usage,
    /// Fields ADR-0006 does not define. Additive fields are the bridge's
    /// evolution story, so a reader must ignore rather than reject them.
    extra: Vec<(String, Leaf)>,
}

impl Hostile {
    fn render(&self) -> Vec<u8> {
        let mut reply = Map::new();
        if let Some(v) = self.v.render() {
            reply.insert("v".into(), v);
        }
        if let Some(model) = &self.model {
            reply.insert("model".into(), model.render());
        }
        if let Some(blocks) = &self.content {
            reply.insert(
                "content".into(),
                Value::Array(blocks.iter().map(Block::render).collect()),
            );
        }
        if let Some(stop) = self.stop.render() {
            reply.insert("stop".into(), stop);
        }
        if let Some(usage) = self.usage.render() {
            reply.insert("usage".into(), usage);
        }
        for (key, leaf) in &self.extra {
            reply.entry(key.clone()).or_insert_with(|| leaf.render());
        }
        // Serializing a `Value` cannot fail: every key is a string.
        serde_json::to_vec(&Value::Object(reply)).unwrap_or_default()
    }
}

fuzz_target!(|hostile: Hostile| {
    let bytes = hostile.render();
    match libtau::decode_reply(&bytes) {
        Ok(reply) => {
            assert_eq!(reply.v, VERSION, "decode_reply accepted a foreign version");
            let again = serde_json::to_vec(&reply)
                .map_err(|e| e.to_string())
                .and_then(|b| libtau::decode_reply(&b).map_err(|e| e.to_string()));
            assert_eq!(again, Ok(reply), "reply did not survive a round trip");
        }
        Err(libtau::InferError::Version { found }) => {
            assert_ne!(found, VERSION, "a Version error named the current version");
            // The error names the `v` that was sent, and nothing else.
            let sent = hostile.v.render();
            assert_eq!(
                sent,
                Some(json!(found)),
                "Version error does not name the v that was sent"
            );
        }
        Err(_) => {}
    }
});
