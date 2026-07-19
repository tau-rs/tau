//! `tau build --emit-trigger=systemd` writes scheduler descriptors next to
//! the bundle for a project's cron triggers.

use assert_cmd::Command;

/// Build an isolated `TAU_HOME` tempdir under `scratch` with the
/// `config.toml` pre-created so parallel tests don't race on its
/// initial write.
fn make_tau_home(scratch: &std::path::Path) -> std::path::PathBuf {
    let home = scratch.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let cfg = home.join("config.toml");
    if !cfg.exists() {
        std::fs::write(&cfg, "").unwrap();
    }
    home
}

#[test]
fn emit_trigger_systemd_writes_units() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let tau_home = make_tau_home(root);

    std::fs::write(
        root.join("tau.toml"),
        r#"packages = ["anthropic"]

[project]
name = "trig"
version = "0.1.0"

[models]
default = { backend = "anthropic", model = "claude-haiku-4-5" }

[agents.summarizer]
display_name = "S"
package = "p@^0.1"
model = "default"

[agents.summarizer.prompt]
system = "hi"

[trigger.nightly]
kind = "cron"
agent = "summarizer"
schedule = "0 3 * * *"
"#,
    )
    .unwrap();
    std::fs::write(
        root.join("tau.lock"),
        "schema_version = 6\ngenerated_by_tau_version = \"0.1.0\"\ngenerated_at = \"2024-01-01T00:00:00Z\"\n",
    )
    .unwrap();

    Command::cargo_bin("tau")
        .unwrap()
        .args(["build", "--no-governance", "--emit-trigger", "systemd"])
        .current_dir(root)
        .env("TAU_HOME", &tau_home)
        .assert()
        .success();

    assert!(
        root.join("tau-nightly.service").exists(),
        "service unit missing; dir contents: {:?}",
        std::fs::read_dir(root)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.file_name()))
            .collect::<Vec<_>>(),
    );
    let timer = std::fs::read_to_string(root.join("tau-nightly.timer")).unwrap();
    assert!(
        timer.contains("OnCalendar=*-*-* 03:00:00"),
        "timer content did not contain expected OnCalendar entry; got:\n{timer}",
    );
}

#[test]
fn emit_trigger_k8s_writes_cronjob_manifest() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let tau_home = make_tau_home(root);

    std::fs::write(
        root.join("tau.toml"),
        r#"packages = ["anthropic"]

[project]
name = "ktrig"
version = "0.1.0"

[models]
default = { backend = "anthropic", model = "claude-haiku-4-5" }

[agents.worker]
display_name = "W"
package = "p@^0.1"
model = "default"

[agents.worker.prompt]
system = "go"

[trigger.hourly]
kind = "cron"
agent = "worker"
schedule = "0 * * * *"
"#,
    )
    .unwrap();
    std::fs::write(
        root.join("tau.lock"),
        "schema_version = 6\ngenerated_by_tau_version = \"0.1.0\"\ngenerated_at = \"2024-01-01T00:00:00Z\"\n",
    )
    .unwrap();

    Command::cargo_bin("tau")
        .unwrap()
        .args(["build", "--no-governance", "--emit-trigger", "k8s"])
        .current_dir(root)
        .env("TAU_HOME", &tau_home)
        .assert()
        .success();

    assert!(
        root.join("tau-hourly.cronjob.yaml").exists(),
        "k8s CronJob manifest missing; dir contents: {:?}",
        std::fs::read_dir(root)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.file_name()))
            .collect::<Vec<_>>(),
    );
}

#[test]
fn emit_trigger_no_cron_triggers_exits_success_with_note() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let tau_home = make_tau_home(root);

    // Project with only a manual trigger — no cron → descriptors are empty
    // but the build still succeeds.
    std::fs::write(
        root.join("tau.toml"),
        r#"packages = ["anthropic"]

[project]
name = "notrig"
version = "0.1.0"

[models]
default = { backend = "anthropic", model = "claude-haiku-4-5" }

[agents.solo]
display_name = "Solo"
package = "p@^0.1"
model = "default"

[agents.solo.prompt]
system = "hi"

[trigger.on_demand]
kind = "manual"
agent = "solo"
"#,
    )
    .unwrap();
    std::fs::write(
        root.join("tau.lock"),
        "schema_version = 6\ngenerated_by_tau_version = \"0.1.0\"\ngenerated_at = \"2024-01-01T00:00:00Z\"\n",
    )
    .unwrap();

    Command::cargo_bin("tau")
        .unwrap()
        .args(["build", "--no-governance", "--emit-trigger", "systemd"])
        .current_dir(root)
        .env("TAU_HOME", &tau_home)
        .assert()
        .success()
        .stderr(predicates::str::contains("No cron triggers to emit"));

    // No descriptor files should be written.
    assert!(
        !root.join("tau-on_demand.service").exists(),
        "unexpected service unit for a manual trigger",
    );
}
