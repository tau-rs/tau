use std::path::Path;

/// `RunEvent` is a root-level `oneOf` of single-key objects (struct
/// variants) or `const` strings (unit variants). Collect both forms.
fn variant_keys(schema: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(one_of) = schema.get("oneOf").and_then(|v| v.as_array()) {
        for entry in one_of {
            if let Some(k) = entry
                .get("required")
                .and_then(|r| r.as_array())
                .and_then(|a| a.first())
                .and_then(|s| s.as_str())
            {
                out.push(k.to_string()); // struct variant
            } else if let Some(c) = entry.get("const").and_then(|s| s.as_str()) {
                out.push(c.to_string()); // unit variant
            }
        }
    }
    out
}

fn to_kebab(pascal: &str) -> String {
    let mut s = String::new();
    for (i, ch) in pascal.chars().enumerate() {
        if ch.is_uppercase() && i != 0 {
            s.push('-');
        }
        s.extend(ch.to_lowercase());
    }
    s
}

#[test]
fn run_event_ts_covers_every_schema_variant() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let schema: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(repo_root.join("schemas/run-event/run-event.v1.schema.json"))
            .unwrap(),
    )
    .unwrap();
    let ts = std::fs::read_to_string(repo_root.join("sdk/embed-js/src/RunEvent.ts")).unwrap();
    let mut missing = Vec::new();
    for key in variant_keys(&schema) {
        let tag = to_kebab(&key);
        if !ts.contains(&format!("type: \"{tag}\"")) {
            missing.push(tag);
        }
    }
    assert!(
        missing.is_empty(),
        "RunEvent.ts missing variants: {missing:?}"
    );
}
