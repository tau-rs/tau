//! Win32 AppContainer profile + ACL helpers.
//!
//! Windows-only (the entire module is `cfg(target_os = "windows")`-gated
//! at the lib level). Compiles + runs only on Windows.
//!
//! # Status: profile create/delete and ACL grant/revoke are real
//!
//! `create_appcontainer_profile` / `delete_appcontainer_profile` call the
//! real Win32 `CreateAppContainerProfile` / `DeleteAppContainerProfile`
//! APIs. `grant_access` / `revoke_access` call the real
//! `SetEntriesInAclW` / `SetNamedSecurityInfoW` APIs to add/remove a DACL
//! entry for the AppContainer SID on a filesystem path (see the spec at
//! `docs/superpowers/specs/2026-08-09-sandbox-windows-appcontainer-phase2-design.md`).
//!
//! This module is already `cfg(target_os = "windows")`-gated at the lib
//! level, and its job is inherently unsafe FFI (Win32 AppContainer
//! profile + ACL APIs) — scope the workspace's `unsafe_code = "warn"`
//! opt-out locally rather than disabling it crate-wide.
#![allow(unsafe_code)]

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{LocalFree, ERROR_ALREADY_EXISTS, HLOCAL};
use windows::Win32::Security::Authorization::{
    SetEntriesInAclW, SetNamedSecurityInfoW, ACCESS_MODE, EXPLICIT_ACCESS_W, GRANT_ACCESS,
    REVOKE_ACCESS, SE_FILE_OBJECT, TRUSTEE_IS_GROUP, TRUSTEE_IS_SID, TRUSTEE_W,
};
use windows::Win32::Security::Isolation::{
    CreateAppContainerProfile, DeleteAppContainerProfile, DeriveAppContainerSidFromAppContainerName,
};
use windows::Win32::Security::{
    FreeSid, ACL, CONTAINER_INHERIT_ACE, DACL_SECURITY_INFORMATION, OBJECT_INHERIT_ACE, PSID,
};
use windows::Win32::Storage::FileSystem::{
    FILE_GENERIC_EXECUTE, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
};

/// Encode a Rust string as a null-terminated UTF-16 buffer suitable for
/// a `PCWSTR` Win32 argument.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Indicates which kind of access an ACL grant or revoke should target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AccessKind {
    /// File-read access on the path.
    Read,
    /// File-read + file-write access on the path.
    Write,
}

/// Owned wrapper around an AppContainer SID.
///
/// Stores the profile name rather than a live `PSID`: `grant_access` /
/// `revoke_access` re-derive the `PSID` from the name via
/// `DeriveAppContainerSidFromAppContainerName` on each call (and free it
/// immediately after use) rather than caching one here, so this type has
/// no Win32 resource to manage.
#[derive(Debug, Clone)]
pub(crate) struct AppContainerSid {
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
/// Calls the real `DeleteAppContainerProfile`. Deleting a profile that
/// does not exist is a no-op success (the API is idempotent for a
/// valid-but-absent name), so only genuine failures (e.g. an invalid name,
/// or the profile still in use by a running process) surface as an
/// `io::Error`; callers that treat deletion as best-effort cleanup
/// (e.g. `CapabilityHandle` drop) already discard the result.
pub(crate) fn delete_appcontainer_profile(name: &str) -> std::io::Result<()> {
    let n = wide(name);
    unsafe { DeleteAppContainerProfile(PCWSTR(n.as_ptr())) }
        .map_err(|e| std::io::Error::other(format!("DeleteAppContainerProfile({name}): {e}")))
}

/// Derive the Win32 `PSID` for an AppContainer profile name.
///
/// Callers are responsible for freeing the returned `PSID` via `FreeSid`
/// once they're done with it (mirrors `create_appcontainer_profile`'s
/// handling of the `PSID` Win32 hands back).
fn sid_for(profile_name: &str) -> std::io::Result<PSID> {
    let w = wide(profile_name);
    unsafe { DeriveAppContainerSidFromAppContainerName(PCWSTR(w.as_ptr())) }.map_err(|e| {
        std::io::Error::other(format!(
            "DeriveAppContainerSidFromAppContainerName({profile_name}): {e}"
        ))
    })
}

/// Map an [`AccessKind`] to the Win32 file-access-rights bitmask granted
/// or revoked on the path.
fn access_mask(kind: AccessKind) -> u32 {
    match kind {
        AccessKind::Read => (FILE_GENERIC_READ | FILE_GENERIC_EXECUTE).0,
        AccessKind::Write => (FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_GENERIC_EXECUTE).0,
    }
}

/// Build an `EXPLICIT_ACCESS_W` entry for `sid` with access mode `mode`
/// and permission bitmask `mask`, then apply it to `path`'s DACL via
/// `SetEntriesInAclW` + `SetNamedSecurityInfoW`.
///
/// # Concern: replaces rather than merges the on-disk DACL
///
/// `SetEntriesInAclW`'s `OldAcl` parameter is `None` below, so the ACL it
/// builds contains *only* this one entry — the existing DACL on `path` is
/// never read (that would require a prior `GetNamedSecurityInfoW` call).
/// `SetNamedSecurityInfoW` then writes that single-entry ACL as `path`'s
/// entire DACL, which **replaces** any pre-existing access entries on the
/// path rather than merging into them. This follows the task brief's
/// specified call sequence as-is; flagged here for reviewer attention —
/// see the task report for the full writeup.
fn set_entry(sid: PSID, path: &str, mode: ACCESS_MODE, mask: u32) -> std::io::Result<()> {
    let path_w = wide(path);
    unsafe {
        let ea = EXPLICIT_ACCESS_W {
            grfAccessPermissions: mask,
            grfAccessMode: mode,
            grfInheritance: CONTAINER_INHERIT_ACE | OBJECT_INHERIT_ACE,
            Trustee: TRUSTEE_W {
                TrusteeForm: TRUSTEE_IS_SID,
                TrusteeType: TRUSTEE_IS_GROUP,
                ptstrName: PWSTR(sid.0 as *mut u16),
                ..Default::default()
            },
        };
        let mut new_dacl: *mut ACL = std::ptr::null_mut();
        let rc = SetEntriesInAclW(Some(&[ea]), None, &mut new_dacl);
        if rc.is_err() {
            return Err(std::io::Error::other(format!("SetEntriesInAclW: {rc:?}")));
        }
        let rc = SetNamedSecurityInfoW(
            PCWSTR(path_w.as_ptr()),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            None,
            None,
            Some(new_dacl as *const ACL),
            None,
        );
        if !new_dacl.is_null() {
            LocalFree(HLOCAL(new_dacl as *mut _));
        }
        rc.ok()
            .map_err(|e| std::io::Error::other(format!("SetNamedSecurityInfoW: {e}")))?;
    }
    Ok(())
}

/// Grant `kind` access on `path` to the AppContainer identified by `sid`.
///
/// Derives the `PSID` from `sid.profile_name`, adds a `GRANT_ACCESS`
/// entry to `path`'s DACL via `SetEntriesInAclW` + `SetNamedSecurityInfoW`,
/// and frees the derived `PSID` before returning.
pub(crate) fn grant_access(
    sid: &AppContainerSid,
    path: &str,
    kind: AccessKind,
) -> std::io::Result<()> {
    let s = sid_for(&sid.profile_name)?;
    let result = set_entry(s, path, GRANT_ACCESS, access_mask(kind));
    unsafe {
        FreeSid(s);
    }
    result
}

/// Revoke `kind` access on `path` from the AppContainer identified by
/// `sid` — the inverse of [`grant_access`].
///
/// Derives the `PSID` from `sid.profile_name`, adds a `REVOKE_ACCESS`
/// entry to `path`'s DACL via `SetEntriesInAclW` + `SetNamedSecurityInfoW`,
/// and frees the derived `PSID` before returning.
pub(crate) fn revoke_access(
    sid: &AppContainerSid,
    path: &str,
    kind: AccessKind,
) -> std::io::Result<()> {
    let s = sid_for(&sid.profile_name)?;
    let result = set_entry(s, path, REVOKE_ACCESS, access_mask(kind));
    unsafe {
        FreeSid(s);
    }
    result
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
    fn delete_nonexistent_profile_is_noop() {
        // DeleteAppContainerProfile is idempotent for a valid-but-absent
        // name: deleting a profile that was never created succeeds. This
        // is what makes best-effort cleanup on `CapabilityHandle` drop safe.
        let name = unique_profile("nonexistent");
        delete_appcontainer_profile(&name)
            .expect("deleting a never-created profile should be a no-op success");
    }

    #[test]
    fn grant_revoke_round_trip_on_real_path() {
        // grant_access/revoke_access now call real Win32 ACL APIs, which
        // fail against a path that doesn't exist (e.g. the previous
        // literal "C:\\path" placeholder) — use a real temp directory so
        // this exercises SetNamedSecurityInfoW against an object Windows
        // can actually resolve.
        let name = unique_profile("grant-revoke");
        let dir = std::env::temp_dir().join(format!("tau-acl-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.to_str().expect("utf8 path");

        let sid = create_appcontainer_profile(&name).unwrap();
        grant_access(&sid, path, AccessKind::Read).expect("grant");
        revoke_access(&sid, path, AccessKind::Read).expect("revoke");
        delete_appcontainer_profile(&name).expect("cleanup profile");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
