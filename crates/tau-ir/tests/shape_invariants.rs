//! Drift test: `tau_ir::MessagePayload` must mirror
//! `tau_domain::MessagePayload` field-for-field. New variant on either side →
//! must update the other. Modeled on the β.1.5 vocabulary drift test
//! (`crates/tau-runtime-tokio/tests/vocabulary_drift.rs`).

use tau_domain::message::MessagePayload as DomainPayload;
use tau_domain::Value;
use tau_ir::MessagePayload as IrPayload;

#[test]
fn tau_ir_message_payload_mirrors_tau_domain() {
    // For every variant tau_domain provides, the IR adapter must round-trip
    // it without panicking and without losing fields.

    // Construct tau_domain::Value instances using actual variant constructors.
    // Use JSON-compatible shapes that survive round-trip through serde_json.
    let json_v = Value::Object({
        let mut m = std::collections::BTreeMap::new();
        m.insert("key".into(), Value::String("value".into()));
        m
    });

    let cases: Vec<DomainPayload> = vec![
        DomainPayload::Text {
            content: "hello".into(),
        },
        DomainPayload::ToolCall { args: json_v.clone() },
        DomainPayload::ToolResult { body: json_v.clone() },
        DomainPayload::ToolError {
            kind: "not_found".into(),
            message: "file not found".into(),
            details: Some(json_v.clone()),
        },
        DomainPayload::Lifecycle(tau_domain::AgentStatus::Ready),
        DomainPayload::Custom {
            kind: "custom_kind".into(),
            body: vec![1, 2, 3],
        },
    ];

    for dp in cases {
        let ir: IrPayload = dp.clone().into();
        let back: DomainPayload = ir.into();
        // PartialEq is derived on both; the round-trip must be a fixed point.
        assert_eq!(dp, back, "round-trip lost data for variant {:?}", dp);
    }
}
