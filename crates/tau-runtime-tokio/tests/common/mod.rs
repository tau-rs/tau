//! Shared scaffolding for tau-runtime integration tests.
//!
//! Cargo treats every `*.rs` file directly under `tests/` as its own
//! integration-test binary. To share helpers across them without
//! producing a phantom `common` test binary, the module lives at
//! `tests/common/mod.rs` (NOT `tests/common.rs`) and is pulled into
//! each test file via `mod common;`. See:
//! <https://doc.rust-lang.org/book/ch11-03-test-organization.html#submodules-in-integration-tests>.
//!
//! Each helper centralizes one canonical fixture so the per-task tests
//! read as straight-line scenarios (build LLM, build runtime, run,
//! assert) without per-file boilerplate.

#![allow(dead_code, unused_imports)] // Integration test helpers — used selectively per test file.

pub mod mock_llm;
pub use mock_llm::{MockLlmBackend, MockTurn};

use std::str::FromStr;

use tau_domain::{
    Address, AgentDefinition, AgentId, AgentInstanceId, Message, MessagePayload, PackageId,
    PackageManifest, PackageName, UncheckedManifest, Value, Version,
};
use tau_ports::fixtures::make_tool_spec;
use tau_ports::ToolSpec;

/// Build a minimal default `AgentDefinition` for tests.
///
/// `id`, `display_name`, `package` and `llm_backend` are validated via
/// the canonical constructors. `system_prompt` and `config` start
/// empty — callers can layer on `with_system_prompt` if needed.
pub fn agent_def(
    id: &str,
    display_name: &str,
    package_id: &str,
    llm_backend_name: &str,
) -> AgentDefinition {
    let package = parse_package_id(package_id);
    AgentDefinition::new(
        AgentId::from_str(id).expect("valid agent id"),
        display_name.to_string(),
        package,
        PackageName::from_str(llm_backend_name).expect("valid llm backend name"),
    )
}

/// Parse `<name>@<version>` into a `PackageId`. `PackageId` is
/// `#[non_exhaustive]` (struct-literal construction blocked from
/// outside `tau-domain`), so we go through the `PackageId::new`
/// constructor with `PackageName::from_str` + `Version::parse`.
fn parse_package_id(s: &str) -> PackageId {
    let (name, version) = s
        .split_once('@')
        .expect("package id must use <name>@<version> form");
    PackageId::new(
        PackageName::from_str(name).expect("valid package name"),
        Version::parse(version).expect("valid version"),
    )
}

/// Build a `PackageManifest` with no capabilities. Used by the
/// happy-path tests.
pub fn manifest_with_no_capabilities() -> PackageManifest {
    manifest_from_toml(
        r#"
            name = "test-pkg"
            version = "0.1.0"
            description = "test package"
            authors = []
            source = "https://example.com/test.git"
            kind = "tool"
            dependencies = []
            capabilities = []
        "#,
    )
}

/// Validate an inline manifest TOML body and return the
/// `PackageManifest`. Panics on parse or validation failure — these
/// fixtures are author-controlled, so any failure is a test bug.
pub fn manifest_from_toml(toml_body: &str) -> PackageManifest {
    let unchecked: UncheckedManifest =
        toml::from_str(toml_body).expect("test manifest TOML must parse");
    unchecked
        .validate()
        .expect("test manifest must satisfy validation")
}

/// Mint a fresh `Address::Agent(...)` for tests that need to fabricate
/// prior conversation history. The run loop generates its own per-run
/// `AgentInstanceId` internally; this helper is for *fixture* messages
/// that pre-date the run (e.g. REPL history threaded into
/// [`Runtime::run_with_history`]).
pub fn agent_address() -> Address {
    Address::Agent(AgentInstanceId::new())
}

/// Build a fresh user-authored `Message` with the given text payload.
/// `recipient` is a freshly minted `Address::User` standing in for the
/// agent — the run loop assigns its own `AgentInstanceId` internally.
pub fn user_message(content: &str) -> Message {
    Message::new(
        Address::User,
        Address::User, // recipient is overwritten by the runtime; placeholder only.
        MessagePayload::Text {
            content: content.to_string(),
        },
    )
}

/// Build a `RunOptions` with `MockClock` and `DeterministicRandom` injected.
///
/// Integration tests must use this (or set `clock`/`random` manually)
/// because `run_streaming_inner` panics when `RunOptions.clock` or
/// `RunOptions.random` is `None` — that contract is enforced since Task
/// 3.7 routed all id-minting through the port helpers.
pub fn run_options() -> tau_runtime_tokio::RunOptions {
    use std::sync::Arc;
    use tau_ports::{DeterministicRandom, MockClock};
    let mut opts = tau_runtime_tokio::RunOptions::default();
    opts.clock = Some(Arc::new(MockClock::new()));
    opts.random = Some(Arc::new(DeterministicRandom::seeded(42)));
    opts
}

/// Build a minimal `ToolSpec` for a mock tool. `input_schema` is an
/// empty object — the run loop's `deserialize_tool_args` is a
/// passthrough at v0.1, so the schema is unused at runtime.
pub fn empty_tool_spec(name: &str) -> ToolSpec {
    make_tool_spec(
        name.to_string(),
        format!("mock {name} tool"),
        Value::Object(Default::default()),
    )
}
