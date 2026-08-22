//! THROWAWAY SPIKE (issue #622) — do not ship, delete after the spike
//! PR reports its CI results.
//!
//! Host half of the Windows egress spike: each test creates a real
//! AppContainer profile, runs `tau-spike-net-probe` inside it through
//! `tau-appcontainer-launcher`, and asserts the DESIGN HYPOTHESIS from
//! the 2026-08-22 brainstorm (issue #622). A red test here is not a
//! product bug — it is a refuted hypothesis, and the `SPIKE ...` markers
//! in the failure output are the measurement.
//!
//! Hypotheses under test:
//! - H1: loopback works between two processes in the SAME AppContainer
//!   (`spike_h1_*`) while container→host loopback stays blocked
//!   (`spike_h1_control_*`).
//! - H2: a named pipe whose DACL grants the container's package SID is
//!   openable from inside (`spike_h2_*`), and the package-SID ACE is
//!   load-bearing (`spike_h2_control_*`). Plain and `LOCAL\` pipe names
//!   are probed separately (NPFS historically restricts AppContainers
//!   to `\\.\pipe\LOCAL\...`).
//! - H3 (ADR-0067 item-2 premise): a leaf-only ACL grant on a nested
//!   file is NOT readable without ancestor FILE_TRAVERSE grants.
#![cfg(all(target_os = "windows", feature = "integration-tests"))]
// Spike-scoped: creating a named pipe with a custom DACL is Win32 FFI.
#![allow(unsafe_code)]

use std::os::windows::io::{FromRawHandle, OwnedHandle};
use std::process::{Command, Output};

use tau_sandbox_windows::test_support;

// ---------------------------------------------------------------------
// Harness helpers
// ---------------------------------------------------------------------

fn unique(tag: &str) -> String {
    format!("tau-spike-{tag}-{}", std::process::id())
}

/// Run the probe inside a fresh AppContainer profile named `profile`
/// (already created by the caller) and return the launcher's output.
///
/// The probe executable itself gets a read+execute ACL grant for the
/// container SID first — without it the image load inside the container
/// is expected to fail, which would shadow every probe.
fn run_probe(profile: &str, probe_args: &[&str]) -> Output {
    let probe_exe = env!("CARGO_BIN_EXE_tau-spike-net-probe");
    test_support::grant_read(profile, probe_exe).expect("grant read+execute on probe exe");
    let out = Command::new(env!("CARGO_BIN_EXE_tau-appcontainer-launcher"))
        .args(["--profile", profile, "--", probe_exe])
        .args(probe_args)
        .output()
        .expect("spawn launcher");
    test_support::revoke_read(profile, probe_exe).ok();
    out
}

fn render(out: &Output) -> String {
    format!(
        "exit={:?}\n--- stdout ---\n{}\n--- stderr ---\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    )
}

// ---------------------------------------------------------------------
// H1 — same-container loopback
// ---------------------------------------------------------------------

/// H1: a listener on 127.0.0.1 inside the AppContainer is reachable
/// from a sibling process in the same container (same package SID).
#[test]
fn spike_h1_same_container_loopback_roundtrip() {
    let profile = unique("h1");
    test_support::create_profile(&profile).expect("create profile");
    let out = run_probe(&profile, &["loopback-serve"]);
    test_support::delete_profile(&profile).ok();
    assert_eq!(
        out.status.code(),
        Some(0),
        "H1 REFUTED — same-container loopback failed:\n{}",
        render(&out)
    );
}

/// H1 control: the container must NOT reach a host-side loopback
/// listener. If this fails, the CI runner has a loopback exemption and
/// every other H1/H2 result is suspect.
#[test]
fn spike_h1_control_host_loopback_blocked() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("host bind");
    let port = listener.local_addr().expect("addr").port();
    let profile = unique("h1ctl");
    test_support::create_profile(&profile).expect("create profile");
    let out = run_probe(&profile, &["connect", &port.to_string()]);
    test_support::delete_profile(&profile).ok();
    assert_ne!(
        out.status.code(),
        Some(0),
        "CONTROL BROKEN — container reached a HOST loopback listener \
         (runner has a loopback exemption?):\n{}",
        render(&out)
    );
}

// ---------------------------------------------------------------------
// H2 — named pipe with package-SID DACL
// ---------------------------------------------------------------------

/// H2a: SID-granted pipe, plain `\\.\pipe\<name>` namespace.
#[test]
fn spike_h2a_pipe_plain_name_sid_dacl_opens() {
    pipe_positive_probe("h2a", |name| name.to_string());
}

/// H2b: SID-granted pipe under `\\.\pipe\LOCAL\<name>`.
#[test]
fn spike_h2b_pipe_local_name_sid_dacl_opens() {
    pipe_positive_probe("h2b", |name| format!(r"LOCAL\{name}"));
}

fn pipe_positive_probe(tag: &str, pipe_name: impl Fn(&str) -> String) {
    let profile = unique(tag);
    test_support::create_profile(&profile).expect("create profile");
    let sid = win::sid_string(&profile);
    // Everyone satisfies the token's user part; the package SID ACE
    // satisfies the AppContainer part. Both are required.
    let sddl = format!("D:P(A;;GA;;;WD)(A;;GA;;;{sid})");
    let name = pipe_name(&unique(&format!("{tag}-pipe")));
    let server = win::create_pipe(&name, &sddl);
    // Echo server: pipe I/O blocks, so serve on a thread. nextest runs
    // each test in its own process, so a thread stuck in
    // ConnectNamedPipe (client denied) dies with the test process.
    std::thread::spawn(move || win::serve_echo(server));
    let out = run_probe(&profile, &["pipe-open", &name]);
    test_support::delete_profile(&profile).ok();
    assert_eq!(
        out.status.code(),
        Some(0),
        "H2({tag}) REFUTED — SID-granted pipe not usable from container:\n{}",
        render(&out)
    );
}

/// H2 control: without the package-SID ACE the open must be denied,
/// proving the per-spawn SID grant is what admits the container (and
/// that a pipe is NOT wide-open to arbitrary AppContainers).
#[test]
fn spike_h2_control_pipe_without_sid_ace_denied() {
    let profile = unique("h2ctl");
    test_support::create_profile(&profile).expect("create profile");
    // Everyone only — no package/capability ACE.
    let name = unique("h2ctl-pipe");
    let _server = win::create_pipe(&name, "D:P(A;;GA;;;WD)");
    let out = run_probe(&profile, &["pipe-open", &name]);
    test_support::delete_profile(&profile).ok();
    assert_ne!(
        out.status.code(),
        Some(0),
        "CONTROL BROKEN — container opened a pipe with NO package-SID ACE:\n{}",
        render(&out)
    );
}

// ---------------------------------------------------------------------
// H3 — leaf-only ACL reachability (ADR-0067 item-2 premise)
// ---------------------------------------------------------------------

/// H3: ADR-0067 asserts a leaf-only grant on `a/b/c/leaf.txt` is NOT
/// readable without FILE_TRAVERSE on the ancestors. AppContainer tokens
/// may retain SeChangeNotifyPrivilege (bypass traverse checking), which
/// would make ancestor grants unnecessary. Round 1 only asserted "the
/// probe ran", but nextest hides passing tests' stdout, so the
/// measurement was invisible. Round 2 asserts the ADR's premise
/// (read DENIED): pass = premise confirmed (item 2 needs traverse
/// grants); fail = the failure output shows the read succeeded, i.e.
/// bypass-traverse holds and item 2 is tests-only.
#[test]
fn spike_h3_leaf_only_grant_nested_read_measurement() {
    let profile = unique("h3");
    test_support::create_profile(&profile).expect("create profile");
    let dir = std::env::temp_dir().join(unique("h3-nested")).join("a/b/c");
    std::fs::create_dir_all(&dir).expect("mkdirs");
    let leaf = dir.join("leaf.txt");
    std::fs::write(&leaf, "spike").expect("write leaf");
    let leaf_str = leaf.to_str().expect("utf8 path");
    test_support::grant_read(&profile, leaf_str).expect("grant leaf");
    let out = run_probe(&profile, &["read-file", leaf_str]);
    test_support::delete_profile(&profile).ok();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("SPIKE probe=read-file"),
        "H3 probe never ran:\n{}",
        render(&out)
    );
    assert_ne!(
        out.status.code(),
        Some(0),
        "H3 REFUTED — leaf-only grant WAS readable at a nested path \
         (bypass-traverse holds; ADR-0067 item 2 becomes tests-only):\n{}",
        render(&out)
    );
}

// ---------------------------------------------------------------------
// Win32 spike plumbing (pipe with custom DACL, SID stringification)
// ---------------------------------------------------------------------

mod win {
    use super::*;
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::{LocalFree, ERROR_PIPE_CONNECTED, HANDLE, HLOCAL};
    use windows::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        SDDL_REVISION_1,
    };
    use windows::Win32::Security::Isolation::DeriveAppContainerSidFromAppContainerName;
    use windows::Win32::Security::{FreeSid, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
    use windows::Win32::Storage::FileSystem::{
        FlushFileBuffers, ReadFile, WriteFile, PIPE_ACCESS_DUPLEX,
    };
    use windows::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
        PIPE_TYPE_BYTE, PIPE_WAIT,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// String SID (`S-1-15-2-...`) for an AppContainer profile name.
    pub fn sid_string(profile: &str) -> String {
        unsafe {
            let w = wide(profile);
            let psid = DeriveAppContainerSidFromAppContainerName(PCWSTR(w.as_ptr()))
                .expect("DeriveAppContainerSidFromAppContainerName");
            let mut s = PWSTR::null();
            let r = ConvertSidToStringSidW(psid, &mut s);
            FreeSid(psid);
            r.expect("ConvertSidToStringSidW");
            let out = s.to_string().expect("utf16 sid");
            let _ = LocalFree(HLOCAL(s.as_ptr() as *mut _));
            out
        }
    }

    /// Create `\\.\pipe\<name>` (single instance, byte mode) with the
    /// DACL described by `sddl`. Returns an owned handle; dropping it
    /// closes the pipe.
    pub fn create_pipe(name: &str, sddl: &str) -> OwnedHandle {
        unsafe {
            let sddl_w = wide(sddl);
            let mut sd = PSECURITY_DESCRIPTOR::default();
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(sddl_w.as_ptr()),
                SDDL_REVISION_1,
                &mut sd,
                None,
            )
            .expect("ConvertStringSecurityDescriptorToSecurityDescriptorW");
            let sa = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: sd.0,
                bInheritHandle: false.into(),
            };
            let path = wide(&format!(r"\\.\pipe\{name}"));
            let h = CreateNamedPipeW(
                PCWSTR(path.as_ptr()),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                1,    // single instance
                4096, // out buffer
                4096, // in buffer
                0,    // default timeout
                Some(&sa),
            );
            let _ = LocalFree(HLOCAL(sd.0));
            assert!(!h.is_invalid(), "CreateNamedPipeW failed");
            OwnedHandle::from_raw_handle(h.0 as _)
        }
    }

    /// Blocking echo server: wait for a client, read "ping", answer
    /// "pong". Runs on a throwaway thread; if the client never connects
    /// (denied), the thread stays parked in ConnectNamedPipe and dies
    /// with the per-test process.
    pub fn serve_echo(pipe: OwnedHandle) {
        use std::os::windows::io::AsRawHandle;
        unsafe {
            let h = HANDLE(pipe.as_raw_handle());
            if let Err(e) = ConnectNamedPipe(h, None) {
                // Client connected between CreateNamedPipe and here.
                if e.code() != ERROR_PIPE_CONNECTED.to_hresult() {
                    eprintln!("serve_echo: ConnectNamedPipe: {e}");
                    return;
                }
            }
            let mut buf = [0u8; 4];
            let mut read = 0u32;
            if ReadFile(h, Some(&mut buf), Some(&mut read), None).is_err() || &buf != b"ping" {
                eprintln!("serve_echo: bad read ({read} bytes: {buf:?})");
                return;
            }
            let mut written = 0u32;
            let _ = WriteFile(h, Some(b"pong"), Some(&mut written), None);
            let _ = FlushFileBuffers(h);
            let _ = DisconnectNamedPipe(h);
        }
    }
}
