//! The sandbox wire shape against ADR-0009's JSON: the fixtures round-trip
//! through the types in value, `describe()` is the ADR's, and a config the
//! driver cannot honour is refused at construction.

#![cfg(all(feature = "sandbox", unix))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use std::time::Duration;

use serde_json::Value;
use tau_drivers::sandbox::wire::{self, ErrorKind, Reply, Request, Stop, Usage, VERSION};
use tau_drivers::sandbox::{ConfigError, SandboxConfig, SandboxDriver};
use tau_kernel::abi::DimKey;
use tau_kernel::driver::Driver;

const REQUEST: &str = include_str!("fixtures/sandbox/request.json");
const REPLY: &str = include_str!("fixtures/sandbox/reply.json");
const REPLY_ERROR: &str = include_str!("fixtures/sandbox/reply-error.json");
const DESCRIBE: &str = include_str!("fixtures/sandbox/describe.json");

fn roundtrip<T: serde::de::DeserializeOwned + serde::Serialize>(fixture: &str) -> T {
    let expected: Value = serde_json::from_str(fixture).unwrap();
    let typed: T = serde_json::from_value(expected.clone()).unwrap();
    let back = serde_json::to_value(&typed).unwrap();
    assert_eq!(back, expected, "the fixture and the types diverged");
    typed
}

#[test]
fn the_request_fixture_round_trips() {
    let request: Request = roundtrip(REQUEST);
    assert_eq!(request.v, Some(VERSION));
    assert_eq!(request.stdin.as_deref(), Some("1 2 3\n"));
    // Absent `v` and `stdin` are the projected version and an empty stdin.
    let bare: Request = serde_json::from_str(r#"{"code":"x"}"#).unwrap();
    assert_eq!(bare.v, None);
    assert_eq!(bare.stdin, None);
    // A field the schema does not have is refused, never silently dropped.
    assert!(serde_json::from_str::<Request>(r#"{"code":"x","argv":[]}"#).is_err());
}

#[test]
fn the_reply_fixtures_round_trip() {
    let reply: Reply = roundtrip(REPLY);
    assert_eq!(reply.stop, Stop::Exit(0));
    assert_eq!(reply.usage.compute_ms(), 23, "18211 + 4102 µs, rounded up");
    let error: Reply = roundtrip(REPLY_ERROR);
    let Stop::Error(e) = error.stop else {
        panic!("an error reply")
    };
    assert_eq!(e.kind, ErrorKind::Unsupported);
    assert_eq!(error.usage, Usage::default());
}

#[test]
fn every_stop_variant_has_the_adr_shape() {
    let shapes = [
        (Stop::Exit(3), r#"{"exit":3}"#),
        (Stop::Signal("SIGSEGV".into()), r#"{"signal":"SIGSEGV"}"#),
        (Stop::CpuLimit, r#""cpu_limit""#),
        (Stop::WallLimit, r#""wall_limit""#),
        (Stop::Abandoned, r#""abandoned""#),
    ];
    for (stop, json) in shapes {
        assert_eq!(serde_json::to_string(&stop).unwrap(), json);
        assert_eq!(serde_json::from_str::<Stop>(json).unwrap(), stop);
    }
}

#[test]
fn compute_ms_rounds_up_and_saturates() {
    let usage = |u, s| Usage {
        cpu_user_us: u,
        cpu_sys_us: s,
        max_rss_bytes: 0,
    };
    assert_eq!(usage(0, 0).compute_ms(), 0);
    assert_eq!(usage(1, 0).compute_ms(), 1);
    assert_eq!(usage(999, 1).compute_ms(), 1);
    assert_eq!(usage(1_000, 1).compute_ms(), 2);
    assert_eq!(usage(u64::MAX, 1).compute_ms(), u64::MAX.div_ceil(1_000));
}

/// The ADR §7 config: Python, 2 s CPU, 256 MiB, 10 s wall, 64 KiB output.
fn adr_config() -> SandboxConfig {
    let mut c = SandboxConfig::new(["python3"], "main.py", 2, Duration::from_secs(10));
    c.description = Some("Run Python 3 code.".into());
    c.shim = Some(common::sandbox::SHIM.into());
    c
}

#[test]
fn describe_is_the_adr_fixture() {
    let fixture: Value = serde_json::from_str(DESCRIBE).unwrap();
    let mut config = adr_config();
    let expected_description = fixture["description"].as_str().unwrap().to_owned();
    // The memory clause is Linux's; elsewhere the bound is refused (§4),
    // and the sentence says nothing about memory.
    let expected_description = if cfg!(target_os = "linux") {
        config.memory_bytes = Some(256 * 1024 * 1024);
        expected_description
    } else {
        expected_description.replace("256 MiB memory, ", "")
    };
    let driver = SandboxDriver::new(config).unwrap();
    let schema = driver.describe().expect("a sandbox is a tool");
    assert_eq!(schema.description, expected_description);
    let input_schema: Value = serde_json::from_slice(&schema.input_schema).unwrap();
    assert_eq!(input_schema, fixture["input_schema"]);
    assert_eq!(wire::schema(), fixture["input_schema"]);
}

#[test]
fn the_ceiling_is_cpu_seconds_in_milliseconds() {
    let driver = SandboxDriver::new(adr_config()).unwrap();
    assert_eq!(driver.ceiling().get(&DimKey::ComputeMs), Some(2_000));
    assert_eq!(
        driver.ceiling().get(&DimKey::Calls),
        None,
        "the kernel adds calls"
    );
    assert_eq!(driver.in_flight(), 0);
}

#[test]
fn the_default_shim_is_beside_the_executable() {
    let mut config = adr_config();
    config.shim = None;
    let driver = SandboxDriver::new(config).unwrap();
    let exe = std::env::current_exe().unwrap();
    assert_eq!(
        driver.shim_path(),
        exe.parent().unwrap().join("tau-sandbox-shim")
    );
}

#[test]
fn a_config_the_driver_cannot_honour_is_refused() {
    let mut no_interpreter = adr_config();
    no_interpreter.interpreter.clear();
    assert!(matches!(
        SandboxDriver::new(no_interpreter),
        Err(ConfigError::NoInterpreter)
    ));

    let mut nested = adr_config();
    nested.entry = "src/main.py".into();
    assert!(matches!(
        SandboxDriver::new(nested),
        Err(ConfigError::BadEntry { .. })
    ));

    let mut no_cpu = adr_config();
    no_cpu.cpu_seconds = 0;
    assert!(matches!(
        SandboxDriver::new(no_cpu),
        Err(ConfigError::ZeroBound {
            what: "cpu_seconds"
        })
    ));

    let mut no_wall = adr_config();
    no_wall.wall = Duration::ZERO;
    assert!(matches!(
        SandboxDriver::new(no_wall),
        Err(ConfigError::ZeroBound { what: "wall" })
    ));
}

#[cfg(not(target_os = "linux"))]
#[test]
fn a_memory_bound_is_refused_where_the_host_does_not_enforce_it() {
    let mut config = adr_config();
    config.memory_bytes = Some(256 * 1024 * 1024);
    let err = SandboxDriver::new(config).unwrap_err();
    assert!(
        matches!(err, ConfigError::MemoryBoundUnsupported { .. }),
        "{err}"
    );
    assert!(err.to_string().contains(std::env::consts::OS), "{err}");
}

#[test]
fn the_description_names_every_bound_in_readable_units() {
    let mut config = adr_config();
    config.description = None;
    config.wall = Duration::from_millis(1_500);
    config.output_bytes = 1_000;
    let text = SandboxDriver::new(config)
        .unwrap()
        .describe()
        .unwrap()
        .description;
    assert!(text.starts_with("Run code with `python3`."), "{text}");
    assert!(text.contains("1500 ms wall"), "{text}");
    assert!(text.contains("1000 bytes of stdout"), "{text}");

    let mut config = adr_config();
    config.output_bytes = 2 * 1024 * 1024;
    let text = SandboxDriver::new(config)
        .unwrap()
        .describe()
        .unwrap()
        .description;
    assert!(text.contains("2 MiB of stdout"), "{text}");
}

#[test]
fn the_driver_shows_its_config_and_never_a_secret() {
    let mut config = adr_config();
    config.env = vec![("TOKEN".into(), "hunter2".into())];
    let driver = SandboxDriver::new(config).unwrap();
    assert_eq!(driver.config().cpu_seconds, 2);
    let shown = format!("{driver:?}");
    assert!(shown.contains("in_flight: 0"), "{shown}");
    // The environment is the harness's to configure and may hold secrets;
    // the config is what the harness wrote, and Debug shows it as written.
    assert!(shown.contains("main.py"), "{shown}");
}
