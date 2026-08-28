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
    GetNamedSecurityInfoW, SetEntriesInAclW, SetNamedSecurityInfoW, ACCESS_MODE, EXPLICIT_ACCESS_W,
    GRANT_ACCESS, REVOKE_ACCESS, SE_FILE_OBJECT, TRUSTEE_IS_GROUP, TRUSTEE_IS_SID, TRUSTEE_W,
};
use windows::Win32::Security::Isolation::{
    CreateAppContainerProfile, DeleteAppContainerProfile, DeriveAppContainerSidFromAppContainerName,
};
use windows::Win32::Security::{
    FreeSid, ACL, CONTAINER_INHERIT_ACE, DACL_SECURITY_INFORMATION, OBJECT_INHERIT_ACE,
    PSECURITY_DESCRIPTOR, PSID,
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
/// and permission bitmask `mask`, then merge it into `path`'s existing
/// DACL via a `GetNamedSecurityInfoW` read + `SetEntriesInAclW` merge +
/// `SetNamedSecurityInfoW` write-back.
///
/// # Fix round 1: read-modify-write, not replace
///
/// An earlier version of this function passed `OldAcl = None` to
/// `SetEntriesInAclW`, which builds an ACL containing *only* the new
/// entry; `SetNamedSecurityInfoW` then writes that as `path`'s **entire**
/// DACL. That was a critical bug on both call paths:
/// - `grant_access` would strip every other DACL entry (owner, SYSTEM,
///   Everyone, …) from `path`, denying everyone but the AppContainer SID.
/// - `revoke_access` — called on every `CapabilityHandle` drop — would
///   build a `REVOKE_ACCESS` entry against an empty base ACL (nothing to
///   revoke from), so `SetEntriesInAclW` returns a valid but **empty**
///   (0-ACE) DACL, and `SetNamedSecurityInfoW` writes that as an
///   effective deny-all DACL — permanently locking the path after every
///   cleanup.
///
/// This version reads `path`'s current DACL first via
/// `GetNamedSecurityInfoW` and passes it as `SetEntriesInAclW`'s `OldAcl`
/// so the new entry merges into the existing DACL instead of replacing
/// it; `revoke_access` now removes only the AppContainer SID's own ACE,
/// leaving every other entry on the path untouched.
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

        // Read the existing DACL. `old_dacl` points *inside* the `sd`
        // security-descriptor buffer Win32 allocates here — it stays
        // valid only as long as `sd` is alive, so `sd` must not be freed
        // until after the `SetEntriesInAclW` call below has consumed it.
        let mut old_dacl: *mut ACL = std::ptr::null_mut();
        let mut sd = PSECURITY_DESCRIPTOR::default();
        let rc = GetNamedSecurityInfoW(
            PCWSTR(path_w.as_ptr()),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            None,
            None,
            Some(&mut old_dacl),
            None,
            &mut sd,
        );
        if rc.is_err() {
            return Err(std::io::Error::other(format!(
                "GetNamedSecurityInfoW({path}): {rc:?}"
            )));
        }

        let mut new_dacl: *mut ACL = std::ptr::null_mut();
        let rc = SetEntriesInAclW(Some(&[ea]), Some(old_dacl as *const ACL), &mut new_dacl);
        if !sd.0.is_null() {
            LocalFree(Some(HLOCAL(sd.0)));
        }
        if rc.is_err() {
            // MSDN: on failure, *newAcl is undefined — only free it if
            // the call happened to leave a non-null value behind.
            if !new_dacl.is_null() {
                LocalFree(Some(HLOCAL(new_dacl as *mut _)));
            }
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
            LocalFree(Some(HLOCAL(new_dacl as *mut _)));
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

/// Human-readable dump of `path`'s DACL: one `[type/flags/mask/sid]`
/// group per ACE, in order.
///
/// Diagnostic only — nothing in the adapter's behaviour depends on it.
///
/// # Why (#622 FAILURE B)
///
/// `install_rust_cargo_acceptance` grants read on `$RUSTUP_HOME`, the
/// grant call returns success, `covers-rustc=true`, and yet
/// `CreateProcess` of `<rustup>\toolchains\…\bin\rustc.exe` answers
/// `Access is denied`. Every hypothesis left standing is about whether
/// the inheritable ACE actually **landed on that file** and with what
/// rights — a question no exit code can answer but that the file's own
/// DACL answers directly. `AceFlags & 0x10` (`INHERITED_ACE`) says
/// whether the ACE arrived by propagation; `Mask` says whether
/// `FILE_EXECUTE` (0x20) and `FILE_READ_DATA` (0x1) survived it.
///
/// Errors are returned rather than panicking: a diagnostic must never
/// be the reason a test fails.
pub(crate) fn describe_dacl(path: &str) -> std::io::Result<String> {
    use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows::Win32::Security::{
        AclSizeInformation, GetAce, GetAclInformation, ACCESS_ALLOWED_ACE, ACL_SIZE_INFORMATION,
    };
    let path_w = wide(path);
    unsafe {
        let mut dacl: *mut ACL = std::ptr::null_mut();
        let mut sd = PSECURITY_DESCRIPTOR::default();
        let rc = GetNamedSecurityInfoW(
            PCWSTR(path_w.as_ptr()),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            None,
            None,
            Some(&mut dacl),
            None,
            &mut sd,
        );
        if rc.is_err() {
            return Err(std::io::Error::other(format!(
                "GetNamedSecurityInfoW({path}): {rc:?}"
            )));
        }

        let mut out = String::new();
        if dacl.is_null() {
            // A NULL DACL means "everyone, everything" — worth saying
            // out loud rather than rendering as an empty ACE list.
            out.push_str("<null-dacl>");
        } else {
            let mut info = ACL_SIZE_INFORMATION::default();
            let sized = GetAclInformation(
                dacl as *const ACL,
                &mut info as *mut _ as *mut core::ffi::c_void,
                std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
                AclSizeInformation,
            );
            if sized.is_err() {
                out.push_str("<GetAclInformation failed>");
            }
            for i in 0..info.AceCount {
                let mut ace: *mut core::ffi::c_void = std::ptr::null_mut();
                if GetAce(dacl as *const ACL, i, &mut ace).is_err() || ace.is_null() {
                    out.push_str("[ace?] ");
                    continue;
                }
                // Only ACCESS_ALLOWED_ACE (0) and ACCESS_DENIED_ACE (1)
                // are decoded: they share the `header, mask, SidStart`
                // layout, so `SidStart` really is a SID. Object/callback
                // ACE types interpose extra fields there, and handing
                // `ConvertSidToStringSidW` a non-SID would have it walk
                // a bogus SubAuthorityCount — so those are reported by
                // their numeric type instead of being mis-decoded.
                let a = &*(ace as *const ACCESS_ALLOWED_ACE);
                let sid_str = if a.Header.AceType <= 1 {
                    let sid = PSID(&a.SidStart as *const u32 as *mut core::ffi::c_void);
                    let mut s = PWSTR::null();
                    if ConvertSidToStringSidW(sid, &mut s).is_ok() {
                        let v = s.to_string().unwrap_or_else(|_| "<utf16>".to_string());
                        let _ = LocalFree(HLOCAL(s.as_ptr() as *mut _));
                        v
                    } else {
                        "<sid?>".to_string()
                    }
                } else {
                    "<undecoded-ace-type>".to_string()
                };
                out.push_str(&format!(
                    "[type={} flags=0x{:02x} mask=0x{:08x} sid={sid_str}] ",
                    a.Header.AceType, a.Header.AceFlags, a.Mask
                ));
            }
        }
        if !sd.0.is_null() {
            LocalFree(HLOCAL(sd.0));
        }
        Ok(out)
    }
}

/// String form ("S-1-15-2-…") of an AppContainer profile's package SID.
/// Called from `pipe_proxy::spawn_pipe_proxy` to build the pipe's DACL
/// (the "container part" ACE).
pub(crate) fn sid_string(profile: &AppContainerSid) -> std::io::Result<String> {
    use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
    let psid = sid_for(&profile.profile_name)?;
    let mut s = PWSTR::null();
    let r = unsafe { ConvertSidToStringSidW(psid, &mut s) };
    unsafe { FreeSid(psid) };
    r.map_err(|e| std::io::Error::other(format!("ConvertSidToStringSidW: {e}")))?;
    let out =
        unsafe { s.to_string() }.map_err(|e| std::io::Error::other(format!("sid utf16: {e}")))?;
    unsafe {
        let _ = LocalFree(HLOCAL(s.as_ptr() as *mut _));
    }
    Ok(out)
}

/// String SID of the current process token's user. Used as the
/// "user part" ACE of the egress pipe's DACL (an AppContainer access
/// check requires BOTH a user-part and a container-part grant; Everyone
/// must not be used — it would admit any non-AppContainer local process).
pub(crate) fn current_user_sid_string() -> std::io::Result<String> {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER};
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)
            .map_err(|e| std::io::Error::other(format!("OpenProcessToken: {e}")))?;
        let mut len = 0u32;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut len);
        let mut buf = vec![0u8; len as usize];
        let r = GetTokenInformation(
            token,
            TokenUser,
            Some(buf.as_mut_ptr() as *mut _),
            len,
            &mut len,
        );
        let _ = windows::Win32::Foundation::CloseHandle(token);
        r.map_err(|e| std::io::Error::other(format!("GetTokenInformation: {e}")))?;
        let tu = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut s = PWSTR::null();
        ConvertSidToStringSidW(tu.User.Sid, &mut s)
            .map_err(|e| std::io::Error::other(format!("ConvertSidToStringSidW: {e}")))?;
        let out = s
            .to_string()
            .map_err(|e| std::io::Error::other(format!("sid utf16: {e}")))?;
        let _ = LocalFree(HLOCAL(s.as_ptr() as *mut _));
        Ok(out)
    }
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

        // Regression guard for the replace-vs-merge DACL bug: if
        // revoke_access ever again writes an empty (deny-all) DACL
        // instead of merging the revoke into the existing one, this
        // directory becomes inaccessible and removal fails here instead
        // of silently leaking a permanently locked temp dir.
        std::fs::remove_dir_all(&dir).expect("temp dir still removable after revoke");
    }
}
