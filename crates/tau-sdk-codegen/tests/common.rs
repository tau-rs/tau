//! Shared test harness: lower a ProjectConfig to canonical IR bytes and run
//! a Python authoring script through `python3`.
#![allow(dead_code)] // used by multiple integration test files

use std::path::Path;
use std::process::Command;

/// The native-tool content-hash cache used by every fixture: a deterministic
/// hash seeded by the first byte of the symbolic name (matches the pattern in
/// tau-ts-extract's conformance tests).
fn caches() -> tau_ir_lower::Caches<'static> {
    tau_ir_lower::Caches {
        native_tool: &|fn_name: &str| {
            let seed = fn_name.as_bytes().first().copied().unwrap_or(1);
            Some([seed; 32])
        },
        mcp_contract: &|_| None,
        skill: &|_| None,
        prompt_file: &|_| Ok(Vec::new()),
    }
}

/// Lower a parsed ProjectConfig to canonical IR bytes.
pub fn lower_config_bytes(cfg: &tau_pkg::project::ProjectConfig) -> Vec<u8> {
    let target = tau_ports::target::TargetTriple::PASSTHROUGH;
    let module = tau_ir_lower::lower_project(cfg, &target, &caches())
        .expect("lowering must succeed")
        .module;
    tau_ir::canonical::to_canonical_bytes(&module)
}

/// Parse TOML text and lower it to canonical IR bytes.
pub fn lower_toml_bytes(toml: &str) -> Vec<u8> {
    let cfg = tau_pkg::project::ProjectConfig::parse_str(toml).expect("parse tau.toml");
    lower_config_bytes(&cfg)
}

/// True if `python3` is on PATH.
pub fn python3_available() -> bool {
    Command::new("python3")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Run a Python authoring script; return its stdout as a String, or `None` if
/// `python3` is unavailable. `pythonpath` is prepended to PYTHONPATH so the
/// script can `import tau_sdk`. Panics if python3 is present but the script
/// exits non-zero.
pub fn run_python_toml(script: &Path, pythonpath: Option<&Path>) -> Option<String> {
    if !python3_available() {
        return None;
    }
    let mut cmd = Command::new("python3");
    cmd.arg(script);
    if let Some(pp) = pythonpath {
        cmd.env("PYTHONPATH", pp);
    }
    let out = cmd.output().expect("spawn python3");
    assert!(
        out.status.success(),
        "python3 {} failed:\n{}",
        script.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    Some(String::from_utf8(out.stdout).expect("python stdout is utf8"))
}
