//! Snapshot tests for `--help` text on every subcommand and the top-level CLI.
//! Catches accidental help-text regressions.

use assert_cmd::Command;

fn capture_help(args: &[&str]) -> String {
    let output = Command::cargo_bin("tau")
        .unwrap()
        .args(args)
        .output()
        .expect("binary runs");
    // clap's `--help` exits with status 0 and writes to stdout.
    //
    // Normalize platform variations so the same .snap files are valid
    // everywhere:
    //   1. CRLF → LF — Windows writes `\r\n`, macOS/Linux write `\n`.
    //   2. `tau.exe` → `tau` — clap derives the binary name from
    //      `argv[0]`, so `Usage:` shows `tau.exe` on Windows.
    String::from_utf8(output.stdout)
        .expect("utf8")
        .replace("\r\n", "\n")
        .replace("tau.exe", "tau")
}

#[test]
fn snapshot_top_level_help() {
    let s = capture_help(&["--help"]);
    insta::assert_snapshot!("top_level_help", s);
}

#[test]
fn snapshot_init_help() {
    let s = capture_help(&["init", "--help"]);
    insta::assert_snapshot!("init_help", s);
}

#[test]
fn snapshot_install_help() {
    let s = capture_help(&["install", "--help"]);
    insta::assert_snapshot!("install_help", s);
}

#[test]
fn snapshot_list_help() {
    let s = capture_help(&["list", "--help"]);
    insta::assert_snapshot!("list_help", s);
}

#[test]
fn snapshot_run_help() {
    let s = capture_help(&["run", "--help"]);
    insta::assert_snapshot!("run_help", s);
}

#[test]
fn snapshot_chat_help() {
    let s = capture_help(&["chat", "--help"]);
    insta::assert_snapshot!("chat_help", s);
}

#[test]
fn snapshot_check_help() {
    let s = capture_help(&["check", "--help"]);
    insta::assert_snapshot!("check_help", s);
}

#[test]
fn snapshot_build_help() {
    let s = capture_help(&["build", "--help"]);
    insta::assert_snapshot!("build_help", s);
}

#[test]
fn snapshot_resolve_help() {
    let s = capture_help(&["resolve", "--help"]);
    insta::assert_snapshot!("resolve_help", s);
}

#[test]
fn snapshot_plugin_help() {
    let s = capture_help(&["plugin", "--help"]);
    insta::assert_snapshot!("plugin_help", s);
}

#[test]
fn snapshot_plugin_describe_help() {
    let s = capture_help(&["plugin", "describe", "--help"]);
    insta::assert_snapshot!("plugin_describe_help", s);
}

#[test]
fn snapshot_plugin_run_help() {
    let s = capture_help(&["plugin", "run", "--help"]);
    insta::assert_snapshot!("plugin_run_help", s);
}

#[test]
fn snapshot_plugin_protocol_help() {
    let s = capture_help(&["plugin", "protocol", "--help"]);
    insta::assert_snapshot!("plugin_protocol_help", s);
}

#[test]
fn snapshot_plugin_protocol_decode_help() {
    let s = capture_help(&["plugin", "protocol", "decode", "--help"]);
    insta::assert_snapshot!("plugin_protocol_decode_help", s);
}

#[test]
fn snapshot_uninstall_help() {
    let s = capture_help(&["uninstall", "--help"]);
    insta::assert_snapshot!("uninstall_help", s);
}

#[test]
fn snapshot_verify_help() {
    let s = capture_help(&["verify", "--help"]);
    insta::assert_snapshot!("verify_help", s);
}

#[test]
fn snapshot_update_help() {
    let s = capture_help(&["update", "--help"]);
    insta::assert_snapshot!("update_help", s);
}

#[test]
fn snapshot_session_help() {
    insta::assert_snapshot!(capture_help(&["session", "--help"]));
}

#[test]
fn snapshot_session_list_help() {
    insta::assert_snapshot!(capture_help(&["session", "list", "--help"]));
}

#[test]
fn snapshot_session_show_help() {
    insta::assert_snapshot!(capture_help(&["session", "show", "--help"]));
}

#[test]
fn snapshot_session_delete_help() {
    insta::assert_snapshot!(capture_help(&["session", "delete", "--help"]));
}

#[test]
fn snapshot_session_export_help() {
    insta::assert_snapshot!(capture_help(&["session", "export", "--help"]));
}
