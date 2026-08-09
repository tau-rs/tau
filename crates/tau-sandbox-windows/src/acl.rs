//! Win32 AppContainer profile + ACL helpers.
//!
//! Windows-only (the entire module is `cfg(target_os = "windows")`-gated
//! at the lib level). Compiles + runs only on Windows.
//!
//! # Status: profile create/delete real, ACLs still stub
//!
//! `create_appcontainer_profile` / `delete_appcontainer_profile` call the
//! real Win32 `CreateAppContainerProfile` / `DeleteAppContainerProfile`
//! APIs (see the spec at
//! `docs/superpowers/specs/2026-08-09-sandbox-windows-appcontainer-phase2-design.md`).
//! `grant_access` / `revoke_access` remain **stub implementations** that
//! return placeholder values without calling Win32 — the real
//! `SetEntriesInAclW` / `SetNamedSecurityInfoW` integration lands in
//! PR2 (Task 5).
//!
//! Until PR2 lands, `wrap_spawn` succeeds (returns a `SandboxHandle`
//! whose drop revokes nothing) but the plugin is **not actually
//! sandboxed** on Windows. The `Sandbox::probe` documents this by
//! returning a non-Strict tier when PR2's enforcement is missing.
//!
//! This module is already `cfg(target_os = "windows")`-gated at the lib
//! level, and its job is inherently unsafe FFI (Win32 AppContainer
//! profile APIs) — scope the workspace's `unsafe_code = "warn"` opt-out
//! locally rather than disabling it crate-wide.
#![allow(unsafe_code)]

use windows::core::PCWSTR;
use windows::Win32::Foundation::ERROR_ALREADY_EXISTS;
use windows::Win32::Security::FreeSid;
use windows::Win32::Security::Isolation::{CreateAppContainerProfile, DeleteAppContainerProfile};

/// Encode a Rust string as a null-terminated UTF-16 buffer suitable for
/// a `PCWSTR` Win32 argument.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Indicates which kind of access an ACL grant or revoke should target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AccessKind {
    /// File-read access on the path.
    #[allow(dead_code)] // exercised only on the Windows runtime path
    Read,
    /// File-read + file-write access on the path.
    #[allow(dead_code)] // exercised only on the Windows runtime path
    Write,
}

/// Owned wrapper around an AppContainer SID.
///
/// In the Phase 2 implementation this owns a Win32 PSID allocated by
/// `DeriveAppContainerSidFromAppContainerName`; here it just stores the
/// profile name so subsequent stub calls can identify the SID.
#[derive(Debug, Clone)]
pub(crate) struct AppContainerSid {
    #[allow(dead_code)] // Phase 2 will use this to format the SID for ACL ops
    pub(crate) profile_name: String,
}

/// Create (or reuse) an AppContainer profile named `name`, returning an
/// `AppContainerSid` carrying the profile name.
///
/// Calls the real `CreateAppContainerProfile`. `ERROR_ALREADY_EXISTS` is
/// treated as success (the profile is idempotently reusable — callers
/// that want a fresh profile per spawn already pick a unique name). Any
/// other Win32 failure is surfaced as an `io::Error` so callers don't
/// silently proceed without a profile.
///
/// The PSID Win32 allocates for the new profile is freed immediately
/// (via `FreeSid`) since `AppContainerSid` only needs the profile name —
/// ACL grants (PR2) re-derive the SID from the name when needed.
pub(crate) fn create_appcontainer_profile(name: &str) -> std::io::Result<AppContainerSid> {
    let n = wide(name);
    let display = wide(name);
    let desc = wide("tau sandbox");
    unsafe {
        match CreateAppContainerProfile(
            PCWSTR(n.as_ptr()),
            PCWSTR(display.as_ptr()),
            PCWSTR(desc.as_ptr()),
            None,
        ) {
            Ok(psid) => {
                FreeSid(psid);
            }
            Err(e) if e.code() == windows::core::HRESULT::from_win32(ERROR_ALREADY_EXISTS.0) => {
                // Idempotent: the profile already exists, proceed.
            }
            Err(e) => {
                return Err(std::io::Error::other(format!(
                    "CreateAppContainerProfile({name}): {e}"
                )));
            }
        }
    }
    Ok(AppContainerSid {
        profile_name: name.to_string(),
    })
}

/// Delete the AppContainer profile named `name`.
///
/// Calls the real `DeleteAppContainerProfile`. Failure (e.g. the profile
/// doesn't exist, or is still in use by a running process) is surfaced
/// as an `io::Error`; callers that treat deletion as best-effort cleanup
/// (e.g. `CapabilityHandle` drop) already discard the result.
pub(crate) fn delete_appcontainer_profile(name: &str) -> std::io::Result<()> {
    let n = wide(name);
    unsafe { DeleteAppContainerProfile(PCWSTR(n.as_ptr())) }
        .map_err(|e| std::io::Error::other(format!("DeleteAppContainerProfile({name}): {e}")))
}

/// Stub: no-op.
///
/// PR2 calls `SetEntriesInAclW` + `SetNamedSecurityInfoW` to add a
/// `GRANT_ACCESS` entry on the path's DACL targeting the AppContainer SID.
pub(crate) fn grant_access(
    _sid: &AppContainerSid,
    _path: &str,
    _kind: AccessKind,
) -> std::io::Result<()> {
    Ok(())
}

/// Stub: no-op.
///
/// PR2 calls `SetEntriesInAclW` + `SetNamedSecurityInfoW` to remove
/// the entry added by [`grant_access`].
pub(crate) fn revoke_access(
    _sid: &AppContainerSid,
    _path: &str,
    _kind: AccessKind,
) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unique-per-test profile name so parallel `cargo test` runs on the
    /// same Windows machine don't collide on a shared AppContainer profile.
    fn unique_profile(tag: &str) -> String {
        format!("tau-acl-test-{tag}-{}", std::process::id())
    }

    #[test]
    fn create_returns_named_sid() {
        let name = unique_profile("create");
        let sid = create_appcontainer_profile(&name).expect("create");
        assert_eq!(sid.profile_name, name);
        delete_appcontainer_profile(&name).expect("cleanup");
    }

    #[test]
    fn create_is_idempotent_on_already_exists() {
        let name = unique_profile("idempotent");
        create_appcontainer_profile(&name).expect("first create");
        // ERROR_ALREADY_EXISTS must be swallowed, not surfaced as an error.
        create_appcontainer_profile(&name).expect("second create should be idempotent");
        delete_appcontainer_profile(&name).expect("cleanup");
    }

    #[test]
    fn delete_nonexistent_profile_errors() {
        let name = unique_profile("nonexistent");
        let err = delete_appcontainer_profile(&name)
            .expect_err("deleting a profile that was never created must fail");
        assert!(
            err.to_string().contains("DeleteAppContainerProfile"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn grant_revoke_are_noops() {
        let name = unique_profile("grant-revoke");
        let sid = create_appcontainer_profile(&name).unwrap();
        grant_access(&sid, "C:\\path", AccessKind::Read).expect("grant");
        revoke_access(&sid, "C:\\path", AccessKind::Read).expect("revoke");
        delete_appcontainer_profile(&name).expect("cleanup");
    }
}
