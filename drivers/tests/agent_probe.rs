//! Availability (ADR-0013 §6): the version pin is a registration error, a
//! logged-out CLI is not, and a refused `send` re-probes exactly once.
//!
//! The "CLI" here is a shell script, because what is under test is what the
//! driver does with an exit status and a line of output — never a
//! credential, which the driver has no way to read (ADR-0013 §8).

#![cfg(all(feature = "agent", unix))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

#[path = "common/agent.rs"]
mod agent;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use tau_drivers::agent::{
    probe_login, probe_version, AgentConfig, Availability, ConfigError, LoginOutput, Probe, Verdict,
};

/// Writes an executable script into `dir` and returns its path.
fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// A stand-in `claude`: `--version` prints a version, `auth status` prints
/// the JSON #130 recorded, and a missing marker file means logged out.
const CLI: &str = r#"
case "$1" in
  --version) echo "2.1.272 (Claude Code)"; exit 0 ;;
  auth)
    if [ -f "$TAU_TEST_LOGIN" ]; then
      echo '{"loggedIn":true,"authMethod":"claude.ai","email":"someone@example.com"}'
      exit 0
    fi
    echo "Error checking login status: No such file or directory (os error 2)" >&2
    exit 1 ;;
esac
exit 3
"#;

fn config(dir: &Path, marker: &Path) -> AgentConfig {
    let mut config = agent::adr_config(dir);
    config.binary = script(dir, "claude", CLI);
    config.env = vec![
        ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
        ("TAU_TEST_LOGIN".to_owned(), marker.display().to_string()),
    ];
    config
}

/// `claude auth status` carries the user's email, organisation id and name.
/// One field ever reaches a reply, never the document (ADR-0013 §2, §8).
fn mode_from_login(out: &LoginOutput) -> Option<String> {
    out.stdout
        .contains("\"authMethod\":\"claude.ai\"")
        .then(|| "none".to_owned())
}

fn probe() -> Probe {
    Probe {
        version_args: vec!["--version".to_owned()],
        login_args: vec!["auth".to_owned(), "status".to_owned()],
        mode_from_login,
    }
}

#[test]
fn the_version_is_read_and_a_pin_that_does_not_match_is_a_config_error() {
    let dir = agent::Temp::new("version");
    let marker = dir.path().join("logged-in");
    let mut config = config(dir.path(), &marker);
    assert_eq!(
        probe_version(&config, &probe()).unwrap(),
        "2.1.272 (Claude Code)"
    );

    config.expect_version = Some("2.1.272".to_owned());
    assert!(
        probe_version(&config, &probe()).is_ok(),
        "the pin is a substring match"
    );

    config.expect_version = Some("2.2".to_owned());
    let err = probe_version(&config, &probe()).unwrap_err();
    assert!(matches!(err, ConfigError::Version { .. }), "{err}");
    assert!(err.to_string().contains("2.1.272"), "{err}");
}

#[test]
fn a_binary_that_is_not_there_is_a_config_error_not_a_reply() {
    let dir = agent::Temp::new("missing");
    let marker = dir.path().join("logged-in");
    let mut config = config(dir.path(), &marker);
    config.binary = dir.path().join("no-such-cli");
    let err = probe_version(&config, &probe()).unwrap_err();
    assert!(matches!(err, ConfigError::Binary { .. }), "{err}");
    assert!(err.to_string().contains("no-such-cli"), "{err}");
}

#[test]
fn logged_out_is_a_verdict_carrying_the_clis_own_words() {
    let dir = agent::Temp::new("logged-out");
    let marker = dir.path().join("logged-in");
    let config = config(dir.path(), &marker);

    let Verdict::Unavailable { message } = probe_login(&config, &probe()) else {
        panic!("the marker is absent, so the CLI is logged out")
    };
    // The ADR's own error message, from #130's `codex login status` run:
    // the command, the exit status, and what the CLI said.
    assert!(
        message.starts_with("claude auth status: exit 1: "),
        "{message}"
    );
    assert!(message.contains("No such file or directory"), "{message}");

    // Construction does not fail over it: a login is a runtime state a
    // human changes, not a configuration error.
    config.check().unwrap();
}

#[test]
fn the_mode_is_one_field_and_never_the_document() {
    let dir = agent::Temp::new("mode");
    let marker = dir.path().join("logged-in");
    std::fs::write(&marker, "").unwrap();
    let config = config(dir.path(), &marker);

    let verdict = probe_login(&config, &probe());
    assert_eq!(verdict.mode(), Some("none"), "`init.apiKeySource`'s value");
    assert!(verdict.is_ready());
    assert!(
        !format!("{verdict:?}").contains("example.com"),
        "the probe's document holds an email; a reply is a blob the log \
         keeps forever, so only the one field crosses"
    );
}

#[test]
fn an_unavailable_verdict_reprobes_once_per_send_and_a_ready_one_never_does() {
    let dir = agent::Temp::new("reprobe");
    let marker = dir.path().join("logged-in");
    let config = config(dir.path(), &marker);
    let probe = probe();

    let availability = Availability::new(probe_login(&config, &probe));
    assert!(
        !availability.verdict().is_ready(),
        "logged out at construction"
    );

    // Still logged out: refused again, and the verdict is refreshed rather
    // than stale.
    assert!(!availability.check(&config, &probe).is_ready());

    // The human logs in mid-run: the next `send` probes and finds it.
    std::fs::write(&marker, "").unwrap();
    assert!(availability.check(&config, &probe).is_ready());

    // And now it stops probing: a ready verdict costs no subprocess, which
    // is why removing the marker changes nothing until a run says otherwise.
    std::fs::remove_file(&marker).unwrap();
    assert!(availability.check(&config, &probe).is_ready());

    // A run whose own events name an authentication failure flips it back —
    // `codex`'s 401 loop, a `claude` result that says so.
    availability.fail("stream error: 401 Unauthorized");
    let verdict = availability.verdict();
    assert!(!verdict.is_ready());
    assert_eq!(
        verdict,
        Verdict::Unavailable {
            message: "stream error: 401 Unauthorized".to_owned()
        }
    );
    // The driver never logs in, and never retries in a loop: one probe.
    assert!(!availability.check(&config, &probe).is_ready());
}
