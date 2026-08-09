# Windows AppContainer Adapter (Phase 2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn `tau-sandbox-windows` from a Phase-1 stub into a truthful Strict-tier adapter that enforces filesystem + process isolation via Windows AppContainer, so `resolve_adapter(Strict)` succeeds on Windows and the 10 gated install-path Tier-2 tests un-gate.

**Architecture:** Camp-2 exec-wrapper (matches tau's Linux/macOS model). `wrap_spawn` creates a per-spawn AppContainer profile, grants FS ACLs to its SID, and rebuilds the `&mut Command` to run the target *through* a new stateless helper `tau-appcontainer-launcher.exe` that does `CreateProcessAsUserW` + `SECURITY_CAPABILITIES` + a `KILL_ON_JOB_CLOSE` job object. Network egress is deferred and fails closed. `plugin_host`, the mcp transport, and the install path are untouched.

**Tech Stack:** Rust (stable), the `windows` crate (Win32 FFI, target-gated), `tau-ports` capability-gate traits, `tokio`, `cargo nextest`, GitHub Actions `windows-latest`.

**Reference spec:** `docs/superpowers/specs/2026-08-09-sandbox-windows-appcontainer-phase2-design.md`

## Global Constraints

- **CARGO RULES (mandatory).** Every cargo command: `timeout <N> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-<role> cargo <cmd> -p <crate>`. Timeouts: test 300, build/check 180, clippy 240, fmt 30. Never bare `cargo`, never `--workspace`, always `-p`. Use `target/agent-sandbox-win` as the target dir for this plan's local commands.
- **CI-only iteration for Win32 runtime.** AppContainer cannot run on macOS/Linux/Wine. Pure-logic code (`profile.rs`, launcher arg-parsing) unit-tests on any host; all `cfg(target_os = "windows")` runtime behavior is verified only on `windows-latest` CI (~5–7 min/cycle). Local pre-flight for cfg-gating: `cargo check --target x86_64-pc-windows-gnu -p tau-sandbox-windows`.
- **Rust stable / MSRV-locked.** No nightly features (rules out `CommandExt::raw_attribute`). Pin the `windows` crate to a fixed minor; verify the version resolves under the workspace MSRV before committing PR1 Task 1.
- **`#[non_exhaustive]` discipline.** All public capability-gate types are `#[non_exhaustive]`; construct via provided constructors, never struct literals across crates.
- **Network is out of scope.** The adapter must FAIL CLOSED on any HTTP-capability plan; `NetworkHttp` is dropped from `supported_shapes`. Do not add proxy/loopback code — that is the follow-on EPIC.
- **Probe truthfulness ordering.** `probe()` stays `Unavailable` until enforcement is implemented AND proven green on Windows CI (end of PR2). Only PR3 flips it to `Available`.
- **rustfmt is a separate required gate.** Run `cargo fmt --check` before every push; clippy/nextest green ≠ fmt-clean.
- **Remote is `tau-rs/tau`.** PRs and `gh api` target `tau-rs/tau`, not `LEBOCQTitouan/tau`.
- **Commits:** conventional, imperative, scoped. Use `git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit` to avoid lefthook identity corruption.

---

# PR1 — `tau-appcontainer-launcher` helper binary

**Deliverable:** a standalone `tau-appcontainer-launcher.exe` that launches a target program inside an AppContainer, proven by a Windows integration test. No runtime wiring yet. Probe still `Unavailable`; nothing in production selects this.

## Task 1: Add the `windows` dependency and declare the launcher bin

**Files:**
- Modify: `crates/tau-sandbox-windows/Cargo.toml`

**Interfaces:**
- Produces: a `[[bin]]` named `tau-appcontainer-launcher` (test wiring later uses `env!("CARGO_BIN_EXE_tau-appcontainer-launcher")`); the `windows` crate available under `cfg(target_os = "windows")`.

- [ ] **Step 1: Add the target-gated `windows` dep + bin declaration**

Edit `crates/tau-sandbox-windows/Cargo.toml`. Replace the Phase-2 placeholder comment (lines 20–23) with a real dep block, and declare the bin:

```toml
[[bin]]
name = "tau-appcontainer-launcher"
path = "src/bin/tau-appcontainer-launcher.rs"

# Real Win32 AppContainer + ACL + process-creation calls. Target-gated so
# the crate still builds on Linux/macOS (pure-logic modules only).
[target.'cfg(target_os = "windows")'.dependencies.windows]
version = "0.58"
features = [
    "Win32_Foundation",
    "Win32_Security",
    "Win32_Security_Authorization",
    "Win32_Security_Isolation",
    "Win32_System_Threading",
    "Win32_System_JobObjects",
    "Win32_System_Console",
    "Win32_System_Memory",
]
```

- [ ] **Step 2: Verify the version resolves under MSRV**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-sandbox-win cargo tree -p tau-sandbox-windows -i windows --target x86_64-pc-windows-msvc`
Expected: resolves to a single `windows 0.58.x`. If it fails the MSRV floor, step down the minor (0.57, 0.56…) until `cargo +<msrv> check` accepts it, and record the chosen version in the spec's Component map.

- [ ] **Step 3: Create a placeholder bin so the manifest is valid**

Create `crates/tau-sandbox-windows/src/bin/tau-appcontainer-launcher.rs`:

```rust
//! AppContainer launcher — placeholder; real logic lands in Task 2/3.
fn main() {
    std::process::exit(2);
}
```

- [ ] **Step 4: Verify the crate still builds off-Windows**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-sandbox-win cargo check -p tau-sandbox-windows`
Expected: PASS (the `windows` dep is target-gated, so macOS build ignores it).

- [ ] **Step 5: Verify cfg-gated cross-compile picks up `windows`**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-sandbox-win cargo check --target x86_64-pc-windows-gnu -p tau-sandbox-windows`
Expected: PASS (or a clean `windows`-crate compile; if the gnu target isn't installed, note it and rely on CI's `cargo-check-windows`).

- [ ] **Step 6: Commit**

```bash
git add crates/tau-sandbox-windows/Cargo.toml crates/tau-sandbox-windows/src/bin/tau-appcontainer-launcher.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(sandbox-windows): add windows dep + declare launcher bin"
```

## Task 2: Launcher argument parsing (pure, testable on any host)

**Files:**
- Create: `crates/tau-sandbox-windows/src/launcher_args.rs`
- Modify: `crates/tau-sandbox-windows/src/lib.rs` (add `pub mod launcher_args;` — pure, not cfg-gated)

**Interfaces:**
- Produces: `pub struct LauncherArgs { pub profile: String, pub caps: Vec<String>, pub program: std::ffi::OsString, pub args: Vec<std::ffi::OsString> }` and `pub fn parse_launcher_args(argv: impl Iterator<Item = std::ffi::OsString>) -> Result<LauncherArgs, String>`. The Win32 `main` (Task 3) consumes this.

- [ ] **Step 1: Write the failing tests**

Create `crates/tau-sandbox-windows/src/launcher_args.rs` with only the tests + type stubs:

```rust
//! Pure arg parsing for `tau-appcontainer-launcher`. No Win32; unit-tested
//! on any host. CLI contract:
//!   tau-appcontainer-launcher --profile <name> [--cap <sid>]... -- <prog> <arg>...
use std::ffi::OsString;

/// Parsed launcher invocation.
#[derive(Debug, PartialEq, Eq)]
pub struct LauncherArgs {
    /// AppContainer profile name (the adapter already created it).
    pub profile: String,
    /// Well-known capability SID names (empty in Phase 2 — net deferred).
    pub caps: Vec<String>,
    /// The real program to run inside the AppContainer.
    pub program: OsString,
    /// Arguments to the real program.
    pub args: Vec<OsString>,
}

/// Parse launcher argv (excluding argv[0]).
pub fn parse_launcher_args(
    argv: impl Iterator<Item = OsString>,
) -> Result<LauncherArgs, String> {
    unimplemented!()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn os(v: &[&str]) -> Vec<OsString> { v.iter().map(OsString::from).collect() }

    #[test]
    fn parses_profile_and_program() {
        let a = parse_launcher_args(os(&["--profile", "tau-sbx-1", "--", "cargo", "build"]).into_iter()).unwrap();
        assert_eq!(a.profile, "tau-sbx-1");
        assert!(a.caps.is_empty());
        assert_eq!(a.program, OsString::from("cargo"));
        assert_eq!(a.args, os(&["build"]));
    }

    #[test]
    fn collects_repeated_caps() {
        let a = parse_launcher_args(os(&["--profile", "p", "--cap", "A", "--cap", "B", "--", "prog"]).into_iter()).unwrap();
        assert_eq!(a.caps, vec!["A".to_string(), "B".to_string()]);
        assert_eq!(a.program, OsString::from("prog"));
    }

    #[test]
    fn missing_profile_is_error() {
        let e = parse_launcher_args(os(&["--", "prog"]).into_iter()).unwrap_err();
        assert!(e.contains("profile"), "got {e}");
    }

    #[test]
    fn missing_program_is_error() {
        let e = parse_launcher_args(os(&["--profile", "p", "--"]).into_iter()).unwrap_err();
        assert!(e.contains("program"), "got {e}");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-sandbox-win cargo nextest run -p tau-sandbox-windows launcher_args`
Expected: FAIL (`unimplemented!`).

- [ ] **Step 3: Implement `parse_launcher_args`**

Replace the `unimplemented!()` body:

```rust
    let mut profile: Option<String> = None;
    let mut caps: Vec<OsString> = Vec::new();
    let mut caps_str: Vec<String> = Vec::new();
    let mut it = argv;
    while let Some(a) = it.next() {
        if a == "--" {
            let program = it.next().ok_or_else(|| "missing program after --".to_string())?;
            let rest: Vec<OsString> = it.collect();
            let profile = profile.ok_or_else(|| "missing --profile".to_string())?;
            return Ok(LauncherArgs { profile, caps: caps_str, program, args: rest });
        } else if a == "--profile" {
            profile = Some(it.next().ok_or("--profile needs a value")?.to_string_lossy().into_owned());
        } else if a == "--cap" {
            let c = it.next().ok_or("--cap needs a value")?;
            caps_str.push(c.to_string_lossy().into_owned());
            caps.push(c);
        } else {
            return Err(format!("unexpected arg: {}", a.to_string_lossy()));
        }
    }
    Err("missing -- separator / program".to_string())
```

Add `pub mod launcher_args;` to `lib.rs` (after `pub use profile::...`; not cfg-gated).

- [ ] **Step 4: Run tests to verify they pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-sandbox-win cargo nextest run -p tau-sandbox-windows launcher_args`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/tau-sandbox-windows/src/launcher_args.rs crates/tau-sandbox-windows/src/lib.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(sandbox-windows): launcher arg parser (pure)"
```

## Task 3: Launcher Win32 body — `CreateProcessAsUserW` inside an AppContainer

**Files:**
- Modify: `crates/tau-sandbox-windows/src/bin/tau-appcontainer-launcher.rs`

**Interfaces:**
- Consumes: `tau_sandbox_windows::launcher_args::parse_launcher_args`, and (for SID derivation) the `windows` crate's `Win32::Security::Isolation::DeriveAppContainerSidFromAppContainerName`.
- Produces: an executable that runs `<program> <args>` inside the named AppContainer, inherits stdio, and exits with the child's exit code. **This body is `cfg(target_os = "windows")`; on other targets `main` prints an error and exits 2 so the bin still compiles cross-platform.**

> **CI-only verification.** There is no local red/green loop for this task; it is proven by Task 4's Windows integration test. Locally, only `cargo check --target x86_64-pc-windows-gnu` applies. Verify every `windows`-crate symbol/signature against docs.rs for the pinned version — the sequence below is correct but exact type names (e.g. `HANDLE` vs `HANDLE(0)`, `BOOL`, `PWSTR`) must match the crate version.

- [ ] **Step 1: Replace the placeholder bin with the real launcher**

```rust
//! AppContainer launcher: runs a target program inside a named AppContainer
//! via CreateProcessAsUserW, inheriting stdio, holding a KILL_ON_JOB_CLOSE
//! job so the child dies with the launcher. See the Phase-2 spec.

use tau_sandbox_windows::launcher_args::parse_launcher_args;

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("tau-appcontainer-launcher is Windows-only");
    std::process::exit(2);
}

#[cfg(target_os = "windows")]
fn main() {
    let parsed = match parse_launcher_args(std::env::args_os().skip(1)) {
        Ok(p) => p,
        Err(e) => { eprintln!("launcher: {e}"); std::process::exit(2); }
    };
    match win::run(parsed) {
        Ok(code) => std::process::exit(code),
        Err(e) => { eprintln!("launcher: {e}"); std::process::exit(3); }
    }
}

#[cfg(target_os = "windows")]
mod win {
    use tau_sandbox_windows::launcher_args::LauncherArgs;
    use std::os::windows::ffi::OsStrExt;
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
    use windows::Win32::Security::Isolation::DeriveAppContainerSidFromAppContainerName;
    use windows::Win32::Security::{PSID, SECURITY_CAPABILITIES, SID_AND_ATTRIBUTES};
    use windows::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE};
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, SetInformationJobObject,
        JobObjectExtendedLimitInformation, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows::Win32::System::Threading::{
        CreateProcessAsUserW, DeleteProcThreadAttributeList, GetExitCodeProcess,
        InitializeProcThreadAttributeList, ResumeThread, UpdateProcThreadAttribute,
        WaitForSingleObject, CREATE_SUSPENDED, EXTENDED_STARTUPINFO_PRESENT,
        LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION, PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
        STARTUPINFOEXW, STARTF_USESTDHANDLES, INFINITE,
    };

    fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
        s.encode_wide().chain(std::iter::once(0)).collect()
    }

    pub fn run(a: LauncherArgs) -> Result<i32, String> {
        unsafe {
            // 1. Derive the AppContainer SID from the (already-created) profile name.
            let profile_w = wide(std::ffi::OsStr::new(&a.profile));
            let sid: PSID = DeriveAppContainerSidFromAppContainerName(PCWSTR(profile_w.as_ptr()))
                .map_err(|e| format!("DeriveAppContainerSid: {e}"))?;

            // 2. SECURITY_CAPABILITIES (Phase 2: no capability SIDs; net deferred).
            //    If a.caps is non-empty, build a SID_AND_ATTRIBUTES array here.
            let caps_array: Vec<SID_AND_ATTRIBUTES> = Vec::new(); // net deferred → empty
            let sec_caps = SECURITY_CAPABILITIES {
                AppContainerSid: sid,
                Capabilities: if caps_array.is_empty() { std::ptr::null_mut() } else { caps_array.as_ptr() as *mut _ },
                CapabilityCount: caps_array.len() as u32,
                Reserved: 0,
            };

            // 3. Proc-thread attribute list carrying the SECURITY_CAPABILITIES.
            let mut size: usize = 0;
            let _ = InitializeProcThreadAttributeList(LPPROC_THREAD_ATTRIBUTE_LIST(std::ptr::null_mut()), 1, 0, &mut size);
            let mut attr_buf = vec![0u8; size];
            let attr_list = LPPROC_THREAD_ATTRIBUTE_LIST(attr_buf.as_mut_ptr() as *mut _);
            InitializeProcThreadAttributeList(attr_list, 1, 0, &mut size)
                .map_err(|e| format!("InitializeProcThreadAttributeList: {e}"))?;
            UpdateProcThreadAttribute(
                attr_list, 0,
                PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize,
                Some(&sec_caps as *const _ as *const core::ffi::c_void),
                std::mem::size_of::<SECURITY_CAPABILITIES>(),
                None, None,
            ).map_err(|e| format!("UpdateProcThreadAttribute: {e}"))?;

            // 4. STARTUPINFOEXW with inherited stdio handles.
            let mut si = STARTUPINFOEXW::default();
            si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
            si.lpAttributeList = attr_list;
            si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
            si.StartupInfo.hStdInput = GetStdHandle(STD_INPUT_HANDLE).map_err(|e| e.to_string())?;
            si.StartupInfo.hStdOutput = GetStdHandle(STD_OUTPUT_HANDLE).map_err(|e| e.to_string())?;
            si.StartupInfo.hStdError = GetStdHandle(STD_ERROR_HANDLE).map_err(|e| e.to_string())?;

            // 5. Build the command line: "program" arg1 arg2 (quote per Win32 rules).
            let mut cmdline: Vec<u16> = build_command_line(&a);

            // 6. Create suspended, assign to a KILL_ON_JOB_CLOSE job, resume.
            let mut pi = PROCESS_INFORMATION::default();
            CreateProcessAsUserW(
                HANDLE::default(),                 // hToken = NULL → caller's token, AppContainer applied via attrs
                PCWSTR::null(),
                PWSTR(cmdline.as_mut_ptr()),
                None, None,
                true,                              // bInheritHandles
                EXTENDED_STARTUPINFO_PRESENT | CREATE_SUSPENDED,
                None, PCWSTR::null(),
                &si.StartupInfo, &mut pi,
            ).map_err(|e| format!("CreateProcessAsUserW: {e}"))?;

            let job = CreateJobObjectW(None, PCWSTR::null()).map_err(|e| format!("CreateJobObject: {e}"))?;
            let mut jinfo = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            jinfo.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(job, JobObjectExtendedLimitInformation,
                &jinfo as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32)
                .map_err(|e| format!("SetInformationJobObject: {e}"))?;
            AssignProcessToJobObject(job, pi.hProcess).map_err(|e| format!("AssignProcessToJobObject: {e}"))?;
            ResumeThread(pi.hThread);

            // 7. Wait, propagate exit code.
            if WaitForSingleObject(pi.hProcess, INFINITE) != WAIT_OBJECT_0 {
                return Err("WaitForSingleObject failed".into());
            }
            let mut code: u32 = 0;
            GetExitCodeProcess(pi.hProcess, &mut code).map_err(|e| e.to_string())?;

            DeleteProcThreadAttributeList(attr_list);
            let _ = CloseHandle(pi.hThread);
            let _ = CloseHandle(pi.hProcess);
            // NB: intentionally keep `job` alive until here; dropping/closing it
            // would kill the child. Leaking it at process exit is fine (launcher
            // exits immediately after).
            Ok(code as i32)
        }
    }

    fn build_command_line(a: &LauncherArgs) -> Vec<u16> {
        // Minimal Win32 command-line quoting: wrap each token containing a
        // space/quote in double quotes and escape embedded quotes/backslashes.
        fn quote(s: &std::ffi::OsStr) -> String {
            let s = s.to_string_lossy();
            if !s.is_empty() && !s.contains([' ', '\t', '"']) { return s.into_owned(); }
            let mut out = String::from("\"");
            let mut backslashes = 0;
            for c in s.chars() {
                match c {
                    '\\' => { backslashes += 1; out.push('\\'); }
                    '"' => { for _ in 0..=backslashes { out.push('\\'); } backslashes = 0; out.push('"'); }
                    _ => { backslashes = 0; out.push(c); }
                }
            }
            for _ in 0..backslashes { out.push('\\'); }
            out.push('"');
            out
        }
        let mut parts = vec![quote(&a.program)];
        parts.extend(a.args.iter().map(|x| quote(x)));
        let joined = parts.join(" ");
        std::ffi::OsString::from(joined).encode_wide().chain(std::iter::once(0)).collect()
    }
}
```

- [ ] **Step 2: Local cfg-gate check (off-Windows)**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-sandbox-win cargo check -p tau-sandbox-windows`
Expected: PASS (non-Windows `main` compiles; `win` module cfg-gated out).

- [ ] **Step 3: Local Windows cross-check**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-sandbox-win cargo check --target x86_64-pc-windows-gnu -p tau-sandbox-windows`
Expected: PASS, or a list of `windows`-crate signature mismatches to fix against docs.rs. Iterate until clean. (If the gnu toolchain is unavailable locally, rely on CI's `cargo-check-windows` job and Task 4.)

- [ ] **Step 4: Commit**

```bash
git add crates/tau-sandbox-windows/src/bin/tau-appcontainer-launcher.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(sandbox-windows): launcher runs target in AppContainer via CreateProcessAsUserW"
```

## Task 4: Windows integration test for the launcher (CI-proven)

**Files:**
- Create: `crates/tau-sandbox-windows/tests/launcher_integration.rs`
- Modify: `crates/tau-sandbox-windows/src/acl.rs` (make `create_appcontainer_profile`/`delete_appcontainer_profile` real *for the test to have a profile to use* — minimal: real `CreateAppContainerProfile`/`DeleteAppContainerProfile`, still stub ACLs). *(If you prefer to keep all ACL work in PR2 Task 5, use `CreateAppContainerProfile` inline in the test instead and skip this modify.)*

**Interfaces:**
- Consumes: `env!("CARGO_BIN_EXE_tau-appcontainer-launcher")`, a real AppContainer profile.

> This is the first CI-only proof. It runs only on `windows-latest` with `--features integration-tests`.

- [ ] **Step 1: Write the integration test**

```rust
//! Windows-only: proves the launcher runs a target inside an AppContainer.
#![cfg(all(target_os = "windows", feature = "integration-tests"))]

use std::process::Command;

/// The launcher runs a benign target and forwards its exit code + stdout.
#[test]
fn launcher_runs_target_and_forwards_exit_and_stdout() {
    // Create a real AppContainer profile for the run (unique name).
    let profile = format!("tau-test-{}", std::process::id());
    tau_sandbox_windows::test_support::create_profile(&profile).expect("create profile");

    let out = Command::new(env!("CARGO_BIN_EXE_tau-appcontainer-launcher"))
        .args(["--profile", &profile, "--", "cmd", "/C", "echo hello & exit 7"])
        .output()
        .expect("spawn launcher");

    tau_sandbox_windows::test_support::delete_profile(&profile).ok();

    assert!(String::from_utf8_lossy(&out.stdout).contains("hello"), "stdout: {:?}", out.stdout);
    assert_eq!(out.status.code(), Some(7), "exit code should propagate");
}
```

- [ ] **Step 2: Expose minimal test support**

Add to `crates/tau-sandbox-windows/src/lib.rs` (Windows-only), a thin `test_support` module wrapping `acl::create_appcontainer_profile`/`delete_appcontainer_profile` so tests can create/delete a profile without exposing the whole `acl` module:

```rust
#[cfg(all(target_os = "windows", feature = "integration-tests"))]
pub mod test_support {
    //! Windows-only test helpers, gated behind `integration-tests`.
    pub fn create_profile(name: &str) -> std::io::Result<()> {
        crate::acl::create_appcontainer_profile(name).map(|_| ())
    }
    pub fn delete_profile(name: &str) -> std::io::Result<()> {
        crate::acl::delete_appcontainer_profile(name)
    }
}
```

- [ ] **Step 3: Make `create/delete_appcontainer_profile` real (minimal)**

In `acl.rs`, replace the two profile stubs with real Win32 (leave `grant_access`/`revoke_access` as stubs until PR2 Task 5):

```rust
use windows::core::PCWSTR;
use windows::Win32::Security::Isolation::{CreateAppContainerProfile, DeleteAppContainerProfile};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub(crate) fn create_appcontainer_profile(name: &str) -> std::io::Result<AppContainerSid> {
    let n = wide(name);
    let display = wide(name);
    let desc = wide("tau sandbox");
    unsafe {
        // Idempotent: if it already exists, proceed (ERROR_ALREADY_EXISTS is fine).
        let _ = CreateAppContainerProfile(PCWSTR(n.as_ptr()), PCWSTR(display.as_ptr()),
                                          PCWSTR(desc.as_ptr()), None);
    }
    Ok(AppContainerSid { profile_name: name.to_string() })
}

pub(crate) fn delete_appcontainer_profile(name: &str) -> std::io::Result<()> {
    let n = wide(name);
    unsafe { let _ = DeleteAppContainerProfile(PCWSTR(n.as_ptr())); }
    Ok(())
}
```

- [ ] **Step 4: Push and verify on Windows CI**

Local check: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-sandbox-win cargo check --target x86_64-pc-windows-gnu -p tau-sandbox-windows --features integration-tests`
Then push the PR1 branch and confirm `cargo-check-windows` (Tier 0) is green. Full runtime proof of this test happens once PR2 Task 8 adds `--features integration-tests` to the Tier-2 Windows job; until then, run it via a temporary `workflow_dispatch` or a scratch CI job. Note in the PR body that Task 4's *runtime* assertion is verified in the Tier-2 run.

- [ ] **Step 5: Commit + open PR1**

```bash
git add crates/tau-sandbox-windows/tests/launcher_integration.rs crates/tau-sandbox-windows/src/lib.rs crates/tau-sandbox-windows/src/acl.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "test(sandbox-windows): windows integration test for launcher"
timeout 30 env CARGO_TARGET_DIR=target/agent-sandbox-win cargo fmt --check
git push -u origin HEAD
gh pr create --base main --repo tau-rs/tau --title "feat(sandbox-windows): AppContainer launcher (Phase 2, PR1/3)" --body "First of 3 PRs graduating the Windows sandbox. Adds tau-appcontainer-launcher + pure arg parser + real profile create/delete + a Windows integration test. Probe stays Unavailable; nothing selects this adapter yet. See docs/superpowers/specs/2026-08-09-sandbox-windows-appcontainer-phase2-design.md."
```

---

# PR2 — real enforcement (probe stays `Unavailable`)

**Deliverable:** `wrap_spawn` grants real FS ACLs and rebuilds the command through the launcher; HTTP plans fail closed; enforcement is proven by `strict_integration.rs` on Windows CI. Probe still `Unavailable`, so production behavior on Windows is unchanged. **A green Windows integration run here is the gate for PR3.**

## Task 5: Real ACL grant/revoke in `acl.rs`

**Files:**
- Modify: `crates/tau-sandbox-windows/src/acl.rs`

**Interfaces:**
- Consumes: the `AppContainerSid` (holds `profile_name`), `AccessKind`.
- Produces: `grant_access(&AppContainerSid, path, AccessKind) -> io::Result<()>` and `revoke_access(...)` that add/remove a DACL entry for the AppContainer SID on `path`.

> CI-only verification via Task 7. Locally only the cross-check applies.

- [ ] **Step 1: Implement `grant_access` / `revoke_access`**

Replace the two stubs. Derive the SID from the profile name, build an `EXPLICIT_ACCESS_W`, merge it into the path's existing DACL with `SetEntriesInAclW`, and write it back with `SetNamedSecurityInfoW`:

```rust
use windows::core::PCWSTR;
use windows::Win32::Foundation::LocalFree;
use windows::Win32::Security::Authorization::{
    SetEntriesInAclW, SetNamedSecurityInfoW, EXPLICIT_ACCESS_W, GRANT_ACCESS, REVOKE_ACCESS,
    SET_ACCESS, SE_FILE_OBJECT, TRUSTEE_W, TRUSTEE_IS_SID, TRUSTEE_IS_GROUP,
};
use windows::Win32::Security::Isolation::DeriveAppContainerSidFromAppContainerName;
use windows::Win32::Security::{ACL, DACL_SECURITY_INFORMATION, PSID};
use windows::Win32::Storage::FileSystem::{
    FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_GENERIC_EXECUTE,
};
use windows::Win32::System::SystemServices::{
    SUB_CONTAINERS_AND_OBJECTS_INHERIT, CONTAINER_INHERIT_ACE, OBJECT_INHERIT_ACE,
};

fn sid_for(profile: &str) -> std::io::Result<PSID> {
    let w: Vec<u16> = profile.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        DeriveAppContainerSidFromAppContainerName(PCWSTR(w.as_ptr()))
            .map_err(|e| std::io::Error::other(format!("derive sid: {e}")))
    }
}

fn access_mask(kind: AccessKind) -> u32 {
    match kind {
        AccessKind::Read => (FILE_GENERIC_READ | FILE_GENERIC_EXECUTE).0,
        AccessKind::Write => (FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_GENERIC_EXECUTE).0,
    }
}

fn set_entry(sid: PSID, path: &str, mode: windows::Win32::Security::Authorization::ACCESS_MODE, mask: u32) -> std::io::Result<()> {
    let path_w: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let mut ea = EXPLICIT_ACCESS_W::default();
        ea.grfAccessPermissions = mask;
        ea.grfAccessMode = mode;
        ea.grfInheritance = (CONTAINER_INHERIT_ACE.0 | OBJECT_INHERIT_ACE.0) as u32;
        ea.Trustee = TRUSTEE_W {
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_GROUP,
            ptstrName: windows::core::PWSTR(sid.0 as *mut u16),
            ..Default::default()
        };
        let mut new_dacl: *mut ACL = std::ptr::null_mut();
        // Merge into the existing DACL (None = start from empty on revoke path use current).
        let rc = SetEntriesInAclW(Some(&[ea]), None, &mut new_dacl);
        if rc.is_err() { return Err(std::io::Error::other(format!("SetEntriesInAclW: {rc:?}"))); }
        let rc = SetNamedSecurityInfoW(
            PCWSTR(path_w.as_ptr()), SE_FILE_OBJECT, DACL_SECURITY_INFORMATION,
            None, None, Some(new_dacl as *const ACL), None,
        );
        if !new_dacl.is_null() { LocalFree(windows::Win32::Foundation::HLOCAL(new_dacl as *mut _)); }
        if rc.is_err() { return Err(std::io::Error::other(format!("SetNamedSecurityInfo: {rc:?}"))); }
    }
    Ok(())
}

pub(crate) fn grant_access(sid: &AppContainerSid, path: &str, kind: AccessKind) -> std::io::Result<()> {
    let s = sid_for(&sid.profile_name)?;
    set_entry(s, path, SET_ACCESS, access_mask(kind))
}

pub(crate) fn revoke_access(sid: &AppContainerSid, path: &str, kind: AccessKind) -> std::io::Result<()> {
    let s = sid_for(&sid.profile_name)?;
    set_entry(s, path, REVOKE_ACCESS, access_mask(kind))
}
```

> Verify against docs.rs for the pinned `windows` version: the exact module for `SUB_CONTAINERS_AND_OBJECTS_INHERIT`/`CONTAINER_INHERIT_ACE`, whether `FILE_GENERIC_*` are `FILE_ACCESS_RIGHTS` (`.0` to get the u32), and `SetEntriesInAclW`'s slice-vs-count signature. Fix mismatches from the cross-check.

- [ ] **Step 2: Local cross-check**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-sandbox-win cargo check --target x86_64-pc-windows-gnu -p tau-sandbox-windows`
Expected: PASS after resolving signature mismatches.

- [ ] **Step 3: Commit**

```bash
git add crates/tau-sandbox-windows/src/acl.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(sandbox-windows): real Win32 ACL grant/revoke for AppContainer SID"
```

## Task 6: `wrap_spawn` rebuilds the command through the launcher; drop `NetworkHttp`

**Files:**
- Modify: `crates/tau-sandbox-windows/src/lib.rs`
- Modify: `crates/tau-sandbox-windows/src/spawn.rs` (repurpose `register_appcontainer_for_command` into the command-rebuild helper, or inline it and delete the stub)

**Interfaces:**
- Consumes: `acl::{create_appcontainer_profile, grant_access, revoke_access, delete_appcontainer_profile}`, `build_appcontainer_caps`.
- Produces: `wrap_spawn` that (1) refuses HTTP plans, (2) grants ACLs, (3) rebuilds `*cmd` to prepend the launcher, (4) returns a `CapabilityHandle` that revokes ACLs + deletes the profile on drop. `supported_shapes()` no longer includes `NetworkHttp`.

- [ ] **Step 1: Drop `NetworkHttp` from `supported_shapes` and update its unit test**

In `lib.rs`, remove `set.insert(tau_domain::CapabilityShape::NetworkHttp);` from `supported_shapes`. Update `supported_shapes_includes_all` → rename to `supported_shapes_is_fs_and_exec` and assert `NetworkHttp` is **absent**:

```rust
    #[test]
    fn supported_shapes_is_fs_and_exec() {
        let s = WindowsSandbox::new("windows");
        let supported = s.supported_shapes();
        assert!(supported.contains(&tau_domain::CapabilityShape::FilesystemRead));
        assert!(supported.contains(&tau_domain::CapabilityShape::FilesystemWrite));
        assert!(supported.contains(&tau_domain::CapabilityShape::ProcessExec));
        assert!(!supported.contains(&tau_domain::CapabilityShape::NetworkHttp),
            "network is deferred (fail-closed) in Phase 2");
    }
```

Also update `validate_plan_accepts_known_shapes` to drop the `net.http` capability from its plan (that shape is now unsupported and would be rejected).

- [ ] **Step 2: Run the pure tests to verify the shape change**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-sandbox-win cargo nextest run -p tau-sandbox-windows`
Expected: PASS (the updated `supported_shapes` + `validate_plan` tests; `validate_plan_rejects_unsupported_shape` now also covers net implicitly).

- [ ] **Step 3: Rewrite `wrap_spawn_windows` to rebuild the command**

Replace the body of `wrap_spawn_windows` (lib.rs:156–243). Keep profile creation + ACL grants + the fail-closed HTTP refusal; replace the `spawn::register_appcontainer_for_command(...)` line with a real command rebuild (darwin idiom):

```rust
    // Fail closed on network — egress is a deferred follow-on EPIC.
    if caps.has_http {
        return Err(CapabilityError::Unsupported {
            what: "Network(Http) on Windows: egress not yet supported (deferred follow-on)".to_string(),
        });
    }

    // Rebuild the command to run the target THROUGH the launcher, which does
    // CreateProcessAsUserW inside the AppContainer. Mirrors the darwin
    // sandbox-exec rebuild (`*cmd = Command::new(...)`).
    let launcher = std::env::var_os("TAU_APPCONTAINER_LAUNCHER_PATH")
        .unwrap_or_else(|| std::ffi::OsString::from("tau-appcontainer-launcher"));
    let orig_program = cmd.get_program().to_os_string();
    let orig_args: Vec<std::ffi::OsString> = cmd.get_args().map(|a| a.to_os_string()).collect();
    let orig_envs: Vec<(std::ffi::OsString, Option<std::ffi::OsString>)> = cmd
        .get_envs().map(|(k, v)| (k.to_os_string(), v.map(|x| x.to_os_string()))).collect();
    let orig_cwd = cmd.get_current_dir().map(|p| p.to_path_buf());

    *cmd = Command::new(launcher);
    cmd.arg("--profile").arg(&profile_name);
    // caps.capability_sids would be added here once net lands; empty in Phase 2.
    cmd.arg("--").arg(orig_program).args(orig_args);
    for (k, v) in orig_envs {
        match v { Some(val) => { cmd.env(k, val); } None => { cmd.env_remove(k); } }
    }
    if let Some(dir) = orig_cwd { cmd.current_dir(dir); }
```

Change the earlier `let app_sid = acl::create_appcontainer_profile(...)` error mapping to keep using `WrapFailed`, unchanged. Remove the now-unused `proxy_handle`/`register_appcontainer_for_command` lines. Keep the `CapabilityHandle::new(move || { revoke...; delete... })` cleanup exactly as-is.

- [ ] **Step 4: Delete the obsolete spawn stub**

Remove `crates/tau-sandbox-windows/src/spawn.rs` and its `#[cfg(target_os = "windows")] mod spawn;` line in `lib.rs` (the command rebuild replaces it). If you prefer to keep the file, gut it to a doc-only module — but deletion is cleaner (no consumers remain).

- [ ] **Step 5: Local cross-check**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-sandbox-win cargo check --target x86_64-pc-windows-gnu -p tau-sandbox-windows`
Expected: PASS. Also `cargo check -p tau-sandbox-windows` (macOS) PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/tau-sandbox-windows/src/lib.rs
git rm crates/tau-sandbox-windows/src/spawn.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(sandbox-windows): wrap_spawn rebuilds cmd via launcher; drop NetworkHttp (fail-closed)"
```

## Task 7: `strict_integration.rs` — enforcement proof (CI)

**Files:**
- Create: `crates/tau-sandbox-windows/tests/strict_integration.rs`

**Interfaces:**
- Consumes: `WindowsSandbox`, `TAU_APPCONTAINER_LAUNCHER_PATH = env!("CARGO_BIN_EXE_tau-appcontainer-launcher")`, `CapabilityPlan`.

> Runs only on `windows-latest` with `--features integration-tests`. This is the gate for PR3.

- [ ] **Step 1: Write the enforcement tests**

```rust
//! Windows-only enforcement proof for the AppContainer adapter.
#![cfg(all(target_os = "windows", feature = "integration-tests"))]

use serde_json::json;
use std::process::Command;
use tau_ports::{CapabilityPlan, ProcessCapabilityGate};
use tau_sandbox_windows::WindowsSandbox;

fn plan(caps: serde_json::Value) -> CapabilityPlan {
    serde_json::from_value(json!({ "capabilities": caps, "context": null, "limits": null })).unwrap()
}

fn with_launcher(mut c: Command) -> Command {
    c.env("TAU_APPCONTAINER_LAUNCHER_PATH", env!("CARGO_BIN_EXE_tau-appcontainer-launcher"));
    c
}

/// A denied path is unreadable from inside the AppContainer (empty plan).
#[test]
fn empty_plan_denies_arbitrary_read() {
    // Write a secret to a temp file NOT granted to the container.
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret.txt");
    std::fs::write(&secret, b"topsecret").unwrap();

    let sandbox = WindowsSandbox::new("native");
    let mut cmd = with_launcher(Command::new("cmd"));
    cmd.args(["/C", &format!("type \"{}\"", secret.display())]);
    cmd.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());

    let rt = tokio::runtime::Runtime::new().unwrap();
    let _handle = rt.block_on(sandbox.wrap_spawn(&plan(json!([])), &mut cmd)).expect("wrap");
    let out = cmd.output().expect("spawn");
    // AppContainer has no ACL grant on the temp dir → read denied.
    assert!(!String::from_utf8_lossy(&out.stdout).contains("topsecret"),
        "secret leaked: {:?}", out.stdout);
}

/// A granted path IS readable; a sibling is NOT.
#[test]
fn granted_path_readable_sibling_denied() {
    let dir = tempfile::tempdir().unwrap();
    let granted = dir.path().join("granted"); std::fs::create_dir_all(&granted).unwrap();
    let ok = granted.join("ok.txt"); std::fs::write(&ok, b"visible").unwrap();
    let sibling = dir.path().join("other.txt"); std::fs::write(&sibling, b"hidden").unwrap();

    let sandbox = WindowsSandbox::new("native");
    let mut cmd = with_launcher(Command::new("cmd"));
    cmd.args(["/C", &format!("type \"{}\" & type \"{}\"", ok.display(), sibling.display())]);
    cmd.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());

    let rt = tokio::runtime::Runtime::new().unwrap();
    let p = plan(json!([{ "kind": "fs.read", "paths": [granted.to_string_lossy()] }]));
    let _handle = rt.block_on(sandbox.wrap_spawn(&p, &mut cmd)).expect("wrap");
    let out = cmd.output().expect("spawn");
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("visible"), "granted path should be readable: {s}");
    assert!(!s.contains("hidden"), "sibling should be denied: {s}");
}

/// HTTP plans fail closed.
#[test]
fn http_plan_is_refused() {
    let sandbox = WindowsSandbox::new("native");
    let mut cmd = with_launcher(Command::new("cmd"));
    let p = plan(json!([{ "kind": "net.http", "hosts": ["example.com"], "methods": ["GET"] }]));
    let rt = tokio::runtime::Runtime::new().unwrap();
    let err = rt.block_on(sandbox.wrap_spawn(&p, &mut cmd)).expect_err("must refuse http");
    let msg = format!("{err:?}");
    assert!(msg.contains("egress") || msg.contains("ShapeUnsupported"), "got {msg}");
}
```

- [ ] **Step 2: Add `tempfile` to dev-deps**

In `crates/tau-sandbox-windows/Cargo.toml` `[dev-dependencies]`, add `tempfile = "3"`.

- [ ] **Step 3: Local cross-check (compile only)**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-sandbox-win cargo check --target x86_64-pc-windows-gnu -p tau-sandbox-windows --features integration-tests --tests`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/tau-sandbox-windows/tests/strict_integration.rs crates/tau-sandbox-windows/Cargo.toml
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "test(sandbox-windows): AppContainer FS-enforcement + fail-closed proof"
```

## Task 8: Wire the Windows integration tests into Tier-2 CI

**Files:**
- Modify: `.github/workflows/tier2.yml` (the `nextest-windows` job)

**Interfaces:**
- Produces: the `nextest / windows` job runs with `--features integration-tests` so Tasks 4 + 7 actually execute.

- [ ] **Step 1: Add the feature flag to the Windows nextest run**

In `tier2.yml`, the `nextest-windows` job's run step, change:

```yaml
    - run: cargo nextest run --profile ci --workspace --all-targets --no-fail-fast --retries 2
```
to:
```yaml
    - run: cargo nextest run --profile ci --workspace --all-targets --features integration-tests --no-fail-fast --retries 2
```

> If enabling `integration-tests` workspace-wide pulls in other crates' integration suites that shouldn't run on Windows, scope instead with a second step: `cargo nextest run --profile ci -p tau-sandbox-windows --features integration-tests --no-fail-fast --retries 2`. Decide based on the first CI run; document the choice in the PR body.

- [ ] **Step 2: Push PR2 and verify on Windows CI**

```bash
timeout 30 env CARGO_TARGET_DIR=target/agent-sandbox-win cargo fmt --check
git add .github/workflows/tier2.yml
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "ci(tier2): run tau-sandbox-windows integration tests on windows"
git push -u origin HEAD
gh pr create --base main --repo tau-rs/tau --title "feat(sandbox-windows): AppContainer enforcement (Phase 2, PR2/3)" --body "Real FS ACLs + launcher command-rebuild + fail-closed network; strict_integration.rs proves isolation on windows CI. Probe stays Unavailable (no production behavior change). Gate for PR3."
gh pr merge <PR#> --squash --delete-branch --auto
```

- [ ] **Step 3: Confirm the Windows enforcement run is green (the PR3 gate)**

Apply the `full-matrix` label to run Tier 2, then:
Run: `gh pr checks <PR#> --repo tau-rs/tau` and inspect the `nextest / windows` job per-job status. Pull raw logs if needed: `gh api repos/tau-rs/tau/actions/jobs/<id>/logs`. All three `strict_integration.rs` tests + the launcher test must pass. **Do not start PR3 until this is green.**

---

# PR3 — flip the switch + un-gate

**Deliverable:** `probe → Available{Strict}`, registry routes to Windows, target-triple graduates, the 10 tests un-gate, ADRs updated. Needs the `full-matrix` label.

## Task 9: Flip the probe to `Available { tier: Strict }`

**Files:**
- Modify: `crates/tau-sandbox-windows/src/lib.rs` (`run_probe`)

**Interfaces:**
- Produces: `probe()` returns `Available { tier: Strict, .. }` on Windows.

- [ ] **Step 1: Rewrite `run_probe`**

```rust
async fn run_probe() -> CapabilityProbe {
    if !cfg!(target_os = "windows") {
        return CapabilityProbe::Unavailable { reason: "not running on Windows".to_string() };
    }
    CapabilityProbe::Available {
        tier: tau_ports::CapabilityTier::Strict,
        details: "AppContainer (FS + process isolation); network egress deferred (fail-closed)".to_string(),
    }
}
```

- [ ] **Step 2: Cross-check**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-sandbox-win cargo check --target x86_64-pc-windows-gnu -p tau-sandbox-windows`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/tau-sandbox-windows/src/lib.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(sandbox-windows): probe Available{Strict} (FS+exec; net deferred)"
```

## Task 10: Route the registry to Windows

**Files:**
- Modify: `crates/tau-runtime-tokio/src/process_gate/registry.rs` (the `Native` `AdapterRegistration.platforms`; the `native_is_linux_and_darwin` test)

**Interfaces:**
- Consumes: `PlatformSet`.
- Produces: `Native` registration includes Windows so `resolve_adapter` routes to `WindowsSandbox` on Windows.

- [ ] **Step 1: Update the failing registry test first**

In `registry.rs`, find `native_is_linux_and_darwin` and change it to assert Windows is now included (rename to `native_includes_windows`):

```rust
    #[test]
    fn native_includes_windows() {
        let reg = REGISTRY.iter().find(|r| matches!(r.kind, RegistryKind::Native)).unwrap();
        assert!(reg.platforms.includes("linux"));
        assert!(reg.platforms.includes("macos"));
        assert!(reg.platforms.includes("windows"));
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-sandbox-win cargo nextest run -p tau-runtime-tokio native_includes_windows`
Expected: FAIL (Windows not yet included).

- [ ] **Step 3: Change the `Native` platform set**

Change the `Native` entry's `platforms:` from `PlatformSet::LinuxAndDarwin` to a set that includes Windows. If `PlatformSet::Multi` already means "linux+macos+windows" (Container uses it), reuse it; otherwise add a `PlatformSet` variant covering all three native hosts. Keep priority 100.

- [ ] **Step 4: Run it to verify it passes**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-sandbox-win cargo nextest run -p tau-runtime-tokio native_includes_windows`
Expected: PASS. Also run the resolver test module to catch fallout: `... cargo nextest run -p tau-runtime-tokio process_gate`.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-runtime-tokio/src/process_gate/registry.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(process-gate): route Native adapter to Windows"
```

## Task 11: Graduate the target-triple `windows-native-strict` to Available

**Files:**
- Modify: `crates/tau-ports/src/target/registry.rs` (the `windows-native-strict` entry, ~line 149)

**Interfaces:**
- Produces: `windows-native-strict` `TripleStatus::Available` (closes the `host()` divergence gap — default `tau build` and `--target windows-native-strict` now agree on Windows).

- [ ] **Step 1: Find and update the relevant unit test**

Locate the target-registry test that asserts `windows-native-strict` is `Reserved` (grep `Reserved` / `windows-native-strict` in `crates/tau-ports/src/target/`). Flip it to assert `Available`. If there is a test enumerating the Available set (memory notes "5 Available + 1 Reserved"), update the counts to 6 Available / 0 Reserved.

- [ ] **Step 2: Run it to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-sandbox-win cargo nextest run -p tau-ports target::registry`
Expected: FAIL.

- [ ] **Step 3: Change the status**

Change the `windows-native-strict` entry from `TripleStatus::Reserved { reason: "..." }` to `TripleStatus::Available` (match the shape the other native-strict triples use).

- [ ] **Step 4: Run to verify pass + check for host()-path fallout**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-sandbox-win cargo nextest run -p tau-ports`
Expected: PASS. Also grep for tests referencing `passthrough`-substituted-for-`host()` (per `project_windows_host_reserved_gap`, PR #251 swapped `host()` for `passthrough` in `resolve_target_accepts_available_triple` / `build_with_available_target_succeeds`) — those can now use `host()` again, but changing them is optional; leaving them is fine.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-ports/src/target/registry.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(target): graduate windows-native-strict Reserved->Available"
```

## Task 12: Un-gate the 10 install-path tests

**Files:**
- Modify: `crates/tau-cli/tests/cmd_install.rs` (×2), `cmd_list.rs` (×2), `cmd_uninstall.rs` (×2), `cmd_update.rs` (×4)

**Interfaces:**
- Produces: the 10 tests run on Windows.

- [ ] **Step 1: Remove every `#[cfg_attr(windows, ignore = "…Phase-2 stub")]`**

Delete the 3-line `#[cfg_attr(windows, ignore = "tau install needs a Strict sandbox adapter; Windows adapter is a Phase-2 stub")]` attribute above each of the 10 tests (locations in the spec's "The 10 gated tests" section). Also delete the now-stale explanatory comment block above `install_local_file_url_writes_to_global_scope` (cmd_install.rs:24–29) since it no longer applies.

- [ ] **Step 2: Local compile check (macOS — behavior unchanged there)**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-sandbox-win cargo nextest run -p tau-cli install_local_file_url_writes_to_global_scope`
Expected: PASS on macOS (these already run on non-Windows).

- [ ] **Step 3: Commit**

```bash
git add crates/tau-cli/tests/cmd_install.rs crates/tau-cli/tests/cmd_list.rs crates/tau-cli/tests/cmd_uninstall.rs crates/tau-cli/tests/cmd_update.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "test(cli): un-gate 10 Windows install-path tests (adapter now Available)"
```

## Task 13: ADR-0066 + supersede ADR-0023

**Files:**
- Create: `docs/decisions/0066-sandbox-windows-appcontainer-phase2.md`
- Modify: `docs/decisions/0023-sandbox-windows-scaffold.md` (status line)

**Interfaces:** documentation only.

- [ ] **Step 1: Write ADR-0066**

Record: the launcher (Camp-2 exec-wrapper) decision and why not the Chromium broker model; network deferral + fail-closed (`NetworkHttp` dropped, AppContainer-loopback finding); the 3-PR phasing; registry + target-triple flips; reference the spec and the egress follow-on. Follow `docs/decisions/template.md`. Use `(1)`/`(2)` not `1.`/`2.` in any Mermaid labels.

- [ ] **Step 2: Mark ADR-0023 superseded**

In `0023-sandbox-windows-scaffold.md`, change the status line to: `**Status:** Accepted (scaffold); Phase 2 superseded by [ADR-0066](0066-sandbox-windows-appcontainer-phase2.md)`.

- [ ] **Step 3: Verify docs build (if touching linked pages)**

ADRs under `docs/decisions/` are not part of the mdBook `SUMMARY.md` tree unless referenced; if you add a SUMMARY entry, run `cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build` and `rm -rf docs/book`. Otherwise skip.

- [ ] **Step 4: Commit, push PR3, enroll auto-merge, verify full-matrix**

```bash
timeout 30 env CARGO_TARGET_DIR=target/agent-sandbox-win cargo fmt --check
git add docs/decisions/0066-sandbox-windows-appcontainer-phase2.md docs/decisions/0023-sandbox-windows-scaffold.md
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "docs(adr): ADR-0066 Windows AppContainer Phase 2; supersede 0023 Phase 2"
git push -u origin HEAD
gh pr create --base main --repo tau-rs/tau --title "feat(sandbox-windows): graduate adapter + un-gate tests (Phase 2, PR3/3)" --body "Flip probe Available{Strict}, route registry to Windows, graduate windows-native-strict target triple (closes host() gap), un-gate 10 install-path tests, ADR-0066. Requires full-matrix. Closes issue #530's Windows install-path gates."
gh pr merge <PR#> --squash --delete-branch --auto
```
Apply the `full-matrix` label. Confirm `nextest / windows` is green with the 10 tests running (per-job check; raw logs via `gh api repos/tau-rs/tau/actions/jobs/<id>/logs`).

---

## Self-Review (completed against the spec)

- **Spec coverage:** launcher (T2/T3) ✓; FS ACLs (T5) ✓; command-rebuild + drop NetworkHttp + fail-closed (T6) ✓; enforcement proof (T7) ✓; CI wiring (T8) ✓; probe flip (T9) ✓; registry flip (T10) ✓; target-triple + host() gap (T11) ✓; un-gate 10 tests (T12) ✓; ADR-0066 + supersede 0023 (T13) ✓; probe-truthfulness ordering (probe flip isolated to PR3, gated on PR2 green) ✓.
- **Placeholder scan:** Win32 FFI steps carry real code with exact `windows`-crate symbols + an explicit "verify signatures against docs.rs / the cross-check loop" note (unavoidable for CI-only-verifiable FFI), not vague "implement X" — acceptable and flagged.
- **Type consistency:** `LauncherArgs`/`parse_launcher_args`, `AppContainerSid`/`AccessKind`, `create/delete_appcontainer_profile`, `grant/revoke_access`, `TAU_APPCONTAINER_LAUNCHER_PATH`, `WindowsSandbox` used consistently across tasks.
- **Known open verification points (call out in PR bodies):** exact `windows 0.58` symbol paths (`SUB_CONTAINERS_AND_OBJECTS_INHERIT`, `FILE_GENERIC_*.0`, `SetEntriesInAclW` signature, `SECURITY_CAPABILITIES` field types) resolve on the cross-check/CI loop; whether `PlatformSet::Multi` already covers all three native hosts (T10); whether `--features integration-tests` should be workspace-wide or crate-scoped on the Windows job (T8).
