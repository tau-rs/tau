//! The invariant of ADR-0013 §8, with teeth: **subprocess-only auth**.
//!
//! An agent driver spawns a binary and reads its stdout. It never reads a
//! credential file, never queries the system key store, never names a
//! provider endpoint. The binary owns its login, and tau never sees a
//! secret — which is what makes the subscription billing cell usable at all
//! without doing the thing both vendors ban.
//!
//! That is not a review comment; it is this test. It walks every source
//! file under `drivers/src/agent/` and fails on any of the shapes below,
//! then proves the scan actually bites by planting each one. The list lives
//! here rather than in config, and grows on the day a new way to cheat is
//! named.
//!
//! Two things the guard cannot see, and where they are handled instead. The
//! environment the harness passes to the child is the harness's choice by
//! design, and `describe()` names it. The login probe's output *is* read by
//! the driver — it has to be, to learn the verdict — and what keeps the
//! user's email and organisation out of the log is the rule that only one
//! field ever becomes the reply's `mode`, tested in `agent_probe.rs`.

#![cfg(all(feature = "agent", unix))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::{Path, PathBuf};

/// What no source file under `drivers/src/agent/` may name.
///
/// Assembled at runtime so that this file — which must of course contain
/// them — is not itself a way to smuggle one in: the scan runs over the
/// module, never over the test.
fn forbidden() -> Vec<String> {
    [
        // A credential file either CLI keeps under the user's home.
        &[".credentials", ".json"][..],
        &["auth", ".json"][..],
        // The macOS key store, by the command that reads it.
        &["find-generic", "-password"][..],
        &["Key", "chain"][..],
        // Provider endpoints: an agent driver has no business at one.
        &["api.", "anthropic.com"][..],
        &["api.", "openai.com"][..],
        &["chatgpt", ".com"][..],
        // An HTTP client at all.
        &["req", "west"][..],
        // Finding the home directory is how a credential hunt begins.
        &["home_", "dir"][..],
    ]
    .iter()
    .map(|parts| parts.concat())
    .collect()
}

fn agent_sources() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/agent");
    let mut files = Vec::new();
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                files.push(path);
            }
        }
    }
    files.sort();
    assert!(
        files.len() >= 4,
        "the module is mod, wire, envelope, process"
    );
    files
}

/// The scan: the one function both this test and its own proof run.
fn scan(text: &str) -> Option<String> {
    forbidden().into_iter().find(|shape| text.contains(shape))
}

#[test]
fn no_source_in_the_agent_module_names_a_credential_or_a_provider() {
    for file in agent_sources() {
        let text = std::fs::read_to_string(&file).unwrap();
        if let Some(shape) = scan(&text) {
            panic!(
                "{}: names `{shape}`. An agent driver spawns a binary and \
                 reads its stdout; the binary owns its login (ADR-0013 §8).",
                file.display()
            );
        }
    }
}

#[test]
fn the_scan_catches_every_planted_shape() {
    for shape in forbidden() {
        let planted = format!("let path = home().join(\"{shape}\");");
        assert_eq!(
            scan(&planted).as_deref(),
            Some(shape.as_str()),
            "`{shape}` slipped through the scan"
        );
    }
    assert!(
        scan("let verdict = probe_login(&config, &probe);").is_none(),
        "the scan must not fire on the probe the driver is supposed to run"
    );
}

/// ADR-0013 §8: the `agent` feature enables no HTTP client, so a provider
/// call is unreachable rather than merely discouraged.
#[test]
fn the_agent_feature_pulls_in_no_http_client() {
    let manifest =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")).unwrap();
    let feature = manifest
        .lines()
        .find(|line| line.starts_with("agent "))
        .expect("the `agent` feature is declared");
    let client = ["req", "west"].concat();
    assert!(
        !feature.contains(&client),
        "the agent feature must not enable an HTTP client: {feature}"
    );

    // The same claim, resolved rather than read: nothing the feature turns
    // on brings a client along behind it.
    let tree = std::process::Command::new(std::env::var("CARGO").unwrap_or("cargo".to_owned()))
        .args([
            "tree",
            "-e",
            "features",
            "--offline",
            "-p",
            "tau-drivers",
            "--no-default-features",
            "--features",
            "agent",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output();
    match tree {
        Ok(output) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout);
            assert!(
                !text.contains(&client),
                "`cargo tree -e features` reached an HTTP client:\n{text}"
            );
            assert!(text.contains("tau-drivers"), "an empty tree proves nothing");
        }
        // No cargo, no index, no network: the manifest check above still
        // held, and this leg is a second opinion, not the only one.
        _ => eprintln!("skipped: `cargo tree` could not run here"),
    }
}
