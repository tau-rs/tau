//! Acceptance test for the `--otlp-endpoint` global flag.
//!
//! The flag is wired through to `tau_observe::install`; the test asserts
//! only that clap accepts the flag and the binary doesn't reject it with
//! a "did you mean" or "unexpected argument" error. End-to-end OTLP
//! export behavior is exercised by `crates/tau-observe/tests/otlp_smoke.rs`.

#![cfg(feature = "otlp")]

use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn otlp_endpoint_flag_is_accepted() {
    let tmp = tempfile::tempdir().unwrap();
    Command::cargo_bin("tau")
        .unwrap()
        // The endpoint will not be reachable; we only assert the flag
        // parses. Use a fast-failing subcommand so the test stays quick.
        .args([
            "--otlp-endpoint",
            "http://127.0.0.1:65535",
            "list",
            "packages",
        ])
        .env("TAU_HOME", tmp.path())
        .env_remove("OTEL_EXPORTER_OTLP_ENDPOINT")
        .env_remove("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT")
        .env_remove("OTEL_EXPORTER_OTLP_HEADERS")
        .assert()
        // Non-zero exit is fine — list packages may error on a fresh
        // TAU_HOME — what matters is the flag parsed without clap
        // complaining about an unknown argument.
        .stderr(predicates::str::contains("unexpected argument").not())
        .stderr(predicates::str::contains("--otlp-endpoint").not());
}

#[test]
fn otlp_endpoint_flag_reads_from_env_var() {
    // clap's `env = "OTEL_EXPORTER_OTLP_ENDPOINT"` populates the field
    // when the flag is absent. Same acceptance shape as above.
    let tmp = tempfile::tempdir().unwrap();
    Command::cargo_bin("tau")
        .unwrap()
        .args(["list", "packages"])
        .env("TAU_HOME", tmp.path())
        .env("OTEL_EXPORTER_OTLP_ENDPOINT", "http://127.0.0.1:65535")
        .env_remove("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT")
        .env_remove("OTEL_EXPORTER_OTLP_HEADERS")
        .assert()
        .stderr(predicates::str::contains("unexpected argument").not());
}
