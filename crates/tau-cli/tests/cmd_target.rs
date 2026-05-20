//! Integration tests for `tau target list` and `tau target show`.

use std::process::Command;

fn tau_bin() -> std::path::PathBuf {
    assert_cmd::cargo::cargo_bin("tau")
}

#[test]
fn list_default_shows_only_available() {
    let out = Command::new(tau_bin())
        .args(["target", "list"])
        .output()
        .expect("spawn");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).expect("utf8");
    assert!(stdout.contains("linux-native-strict"));
    assert!(stdout.contains("passthrough"));
    assert!(
        !stdout.contains("windows-native-strict"),
        "Reserved should be hidden by default"
    );
}

#[test]
fn list_all_includes_reserved() {
    let out = Command::new(tau_bin())
        .args(["target", "list", "--all"])
        .output()
        .expect("spawn");
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).expect("utf8");
    assert!(stdout.contains("windows-native-strict"));
}

#[test]
fn list_json_emits_one_event_per_triple() {
    let out = Command::new(tau_bin())
        .args(["target", "list", "--all", "--json"])
        .output()
        .expect("spawn");
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).expect("utf8");
    let lines: Vec<&str> = stdout.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(
        lines.len(),
        6,
        "expected 6 entries (5 Available + 1 Reserved), got {} — stdout: {stdout}",
        lines.len()
    );
    for l in &lines {
        let v: serde_json::Value = serde_json::from_str(l).expect("each line is JSON");
        assert_eq!(v["event"], "target");
        assert!(v["triple"].is_string());
    }
}

#[test]
fn show_known_triple_prints_matrix() {
    let out = Command::new(tau_bin())
        .args(["target", "show", "linux-native-strict"])
        .output()
        .expect("spawn");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).expect("utf8");
    assert!(stdout.contains("linux-native-strict"));
    assert!(stdout.contains("status:"));
    assert!(stdout.contains("platform: linux"));
    assert!(stdout.contains("adapter:  native"));
    assert!(stdout.contains("tier:     strict"));
}

#[test]
fn show_reserved_triple_includes_reason() {
    let out = Command::new(tau_bin())
        .args(["target", "show", "windows-native-strict"])
        .output()
        .expect("spawn");
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).expect("utf8");
    assert!(stdout.contains("Reserved"));
    assert!(stdout.contains("scaffold"));
}

#[test]
fn show_bogus_triple_exits_64_with_suggestion() {
    let out = Command::new(tau_bin())
        .args(["target", "show", "linux-native-strikt"]) // typo
        .output()
        .expect("spawn");
    assert_eq!(out.status.code(), Some(64));
    // Suggestion can land on either stdout (output.human) or stderr (output.error).
    let stderr = String::from_utf8(out.stderr).expect("utf8");
    let stdout = String::from_utf8(out.stdout).expect("utf8");
    let combined = format!("{stderr}{stdout}");
    assert!(
        combined.contains("did you mean")
            || combined.contains("could not parse")
            || combined.contains("unknown triple"),
        "expected error or suggestion in output. combined: {combined}"
    );
}

#[test]
fn show_unknown_but_parsing_triple_exits_64() {
    let out = Command::new(tau_bin())
        .args(["target", "show", "darwin-container-strict"]) // parses, not registered
        .output()
        .expect("spawn");
    assert_eq!(out.status.code(), Some(64));
}
