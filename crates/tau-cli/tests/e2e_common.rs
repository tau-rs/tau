//! Shared helpers for `tau serve` e2e tests.
//!
//! Each e2e test file includes this via `#[path = "e2e_common.rs"] mod e2e_common;`.

#![allow(dead_code)]

/// Ensure tau's global scope can be resolved without racing.
///
/// `tau_pkg::Scope::global` reads `$TAU_HOME` and creates a default
/// `config.toml` in it if missing. Parallel tests writing the same
/// path on Windows produce "Access is denied". We point `$TAU_HOME`
/// at a dedicated process-local tempdir initialized once, with the
/// config.toml pre-created so no test races on writing it.
pub fn ensure_home_env() {
    use std::sync::OnceLock;
    static TAU_HOME_INIT: OnceLock<()> = OnceLock::new();
    TAU_HOME_INIT.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("tau-serve-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create tau_home tempdir");
        let cfg = dir.join("config.toml");
        if !cfg.exists() {
            let _ = std::fs::write(&cfg, "");
        }
        std::env::set_var("TAU_HOME", dir);
    });
}

/// Kill the serve child and drain whatever it wrote to stderr, formatted for
/// a panic message.
///
/// When `tau serve` dies during startup (e.g. it can't resolve a sandbox
/// adapter), the parent test otherwise only sees a missing ready-marker or an
/// empty stdout line — the child's actual error is discarded. This surfaces
/// that error so CI shows *why* serve failed instead of a bare assertion.
///
/// Reads `child.stderr` if it is still owned by the `Child` (the smoke /
/// streaming tests take only `stdout` into a reader, leaving `stderr`
/// available here). Tests that consume `stderr` themselves collect their own
/// lines instead and pass them to a panic directly.
pub fn drain_child_stderr(child: &mut std::process::Child) -> String {
    use std::io::Read;
    let _ = child.kill();
    let mut err = String::new();
    if let Some(mut e) = child.stderr.take() {
        let _ = e.read_to_string(&mut err);
    }
    if err.trim().is_empty() {
        "\n--- tau serve child stderr: (empty or already consumed) ---".to_string()
    } else {
        format!("\n--- tau serve child stderr ---\n{err}\n--- end child stderr ---")
    }
}
