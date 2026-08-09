//! AppContainer launcher: runs a target program inside a named AppContainer
//! via CreateProcessW + PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
//! inheriting stdio, holding a KILL_ON_JOB_CLOSE job so the child dies with
//! the launcher. See the Phase-2 spec.
//!
//! This bin is a separate crate root, so `tau_sandbox_windows`'s lib-level
//! lints don't reach it. The Windows path below is inherently unsafe FFI
//! (Win32 process-creation + job-object APIs); scope the workspace's
//! `unsafe_code = "warn"` opt-out to the `win` module only.
#![cfg_attr(target_os = "windows", allow(unsafe_code))]

#[cfg(target_os = "windows")]
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
        Err(e) => {
            eprintln!("launcher: {e}");
            std::process::exit(2);
        }
    };
    match win::run(parsed) {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("launcher: {e}");
            std::process::exit(3);
        }
    }
}

#[cfg(target_os = "windows")]
mod win {
    use std::os::windows::ffi::OsStrExt;
    use tau_sandbox_windows::launcher_args::LauncherArgs;
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, LocalFree, HLOCAL, WAIT_OBJECT_0};
    use windows::Win32::Security::Isolation::DeriveAppContainerSidFromAppContainerName;
    use windows::Win32::Security::{PSID, SECURITY_CAPABILITIES, SID_AND_ATTRIBUTES};
    use windows::Win32::System::Console::{
        GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows::Win32::System::Threading::{
        CreateProcessW, DeleteProcThreadAttributeList, GetExitCodeProcess,
        InitializeProcThreadAttributeList, ResumeThread, TerminateProcess,
        UpdateProcThreadAttribute, WaitForSingleObject, CREATE_SUSPENDED,
        EXTENDED_STARTUPINFO_PRESENT, INFINITE, LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION,
        PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, STARTF_USESTDHANDLES, STARTUPINFOEXW,
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
            let caps_array: Vec<SID_AND_ATTRIBUTES> = Vec::new(); // net deferred -> empty
            let sec_caps = SECURITY_CAPABILITIES {
                AppContainerSid: sid,
                Capabilities: if caps_array.is_empty() {
                    std::ptr::null_mut()
                } else {
                    caps_array.as_ptr() as *mut _
                },
                CapabilityCount: caps_array.len() as u32,
                Reserved: 0,
            };

            // 3. Proc-thread attribute list carrying the SECURITY_CAPABILITIES.
            let mut size: usize = 0;
            // First call is expected to fail with ERROR_INSUFFICIENT_BUFFER; it
            // still writes the required buffer size into `size`, which is the
            // documented two-call pattern for this API.
            let _ = InitializeProcThreadAttributeList(
                LPPROC_THREAD_ATTRIBUTE_LIST(std::ptr::null_mut()),
                1,
                0,
                &mut size,
            );
            let mut attr_buf = vec![0u8; size];
            let attr_list = LPPROC_THREAD_ATTRIBUTE_LIST(attr_buf.as_mut_ptr() as *mut _);
            InitializeProcThreadAttributeList(attr_list, 1, 0, &mut size)
                .map_err(|e| format!("InitializeProcThreadAttributeList: {e}"))?;
            UpdateProcThreadAttribute(
                attr_list,
                0,
                PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize,
                Some(&sec_caps as *const _ as *const core::ffi::c_void),
                std::mem::size_of::<SECURITY_CAPABILITIES>(),
                None,
                None,
            )
            .map_err(|e| format!("UpdateProcThreadAttribute: {e}"))?;

            // 4. STARTUPINFOEXW with inherited stdio handles.
            let mut si = STARTUPINFOEXW::default();
            si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
            si.lpAttributeList = attr_list;
            si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
            si.StartupInfo.hStdInput = GetStdHandle(STD_INPUT_HANDLE).map_err(|e| e.to_string())?;
            si.StartupInfo.hStdOutput =
                GetStdHandle(STD_OUTPUT_HANDLE).map_err(|e| e.to_string())?;
            si.StartupInfo.hStdError = GetStdHandle(STD_ERROR_HANDLE).map_err(|e| e.to_string())?;

            // 5. Build the command line: "program" arg1 arg2 (quote per Win32 rules).
            let mut cmdline: Vec<u16> = build_command_line(&a);

            // 6. Create suspended. No token is passed: CreateProcessAsUserW
            //    requires a *valid* primary token and NULL is not a
            //    documented-valid value for it, so the AppContainer is
            //    applied purely via the PROC_THREAD_ATTRIBUTE_SECURITY_
            //    CAPABILITIES attribute on the plain CreateProcessW path
            //    (same parameter list minus the leading hToken).
            let mut pi = PROCESS_INFORMATION::default();
            CreateProcessW(
                PCWSTR::null(),
                PWSTR(cmdline.as_mut_ptr()),
                None,
                None,
                true, // bInheritHandles
                EXTENDED_STARTUPINFO_PRESENT | CREATE_SUSPENDED,
                None,
                PCWSTR::null(),
                &si.StartupInfo,
                &mut pi,
            )
            .map_err(|e| format!("CreateProcessW: {e}"))?;

            // The derived SID's memory only needs to stay alive through the
            // CreateProcessW call above (the OS reads SECURITY_CAPABILITIES,
            // including AppContainerSid, while building the child's
            // AppContainer token during process creation). Free it now per
            // MSDN (DeriveAppContainerSidFromAppContainerName's PSID must be
            // released with LocalFree).
            let _ = LocalFree(HLOCAL(sid.0));

            // From here on the child exists (suspended). Any failure in this
            // block must not leak a suspended zombie process: terminate +
            // close both handles before propagating the error.
            let result: Result<i32, String> = (|| {
                let job = CreateJobObjectW(None, PCWSTR::null())
                    .map_err(|e| format!("CreateJobObject: {e}"))?;
                let mut jinfo = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                jinfo.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    &jinfo as *const _ as *const core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
                .map_err(|e| format!("SetInformationJobObject: {e}"))?;
                AssignProcessToJobObject(job, pi.hProcess)
                    .map_err(|e| format!("AssignProcessToJobObject: {e}"))?;
                // ResumeThread returns the previous suspend count, or
                // u32::MAX on failure -- it does not return a Result.
                if ResumeThread(pi.hThread) == u32::MAX {
                    return Err("ResumeThread failed".to_string());
                }

                // 7. Wait, propagate exit code.
                if WaitForSingleObject(pi.hProcess, INFINITE) != WAIT_OBJECT_0 {
                    return Err("WaitForSingleObject failed".into());
                }
                let mut code: u32 = 0;
                GetExitCodeProcess(pi.hProcess, &mut code).map_err(|e| e.to_string())?;
                // NB: intentionally keep `job` alive until here; dropping/
                // closing it would kill the child. Leaking it at process
                // exit is fine (launcher exits immediately after).
                Ok(code as i32)
            })();

            if result.is_err() {
                // Best-effort: the child may already be gone (e.g. the wait
                // failed for a reason other than the process itself), so
                // ignore TerminateProcess's own error.
                let _ = TerminateProcess(pi.hProcess, 1);
            }
            DeleteProcThreadAttributeList(attr_list);
            let _ = CloseHandle(pi.hThread);
            let _ = CloseHandle(pi.hProcess);
            result
        }
    }

    fn build_command_line(a: &LauncherArgs) -> Vec<u16> {
        // Minimal Win32 command-line quoting: wrap each token containing a
        // space/quote in double quotes and escape embedded quotes/backslashes.
        fn quote(s: &std::ffi::OsStr) -> String {
            let s = s.to_string_lossy();
            if !s.is_empty() && !s.contains([' ', '\t', '"']) {
                return s.into_owned();
            }
            let mut out = String::from("\"");
            let mut backslashes = 0;
            for c in s.chars() {
                match c {
                    '\\' => {
                        backslashes += 1;
                        out.push('\\');
                    }
                    '"' => {
                        for _ in 0..=backslashes {
                            out.push('\\');
                        }
                        backslashes = 0;
                        out.push('"');
                    }
                    _ => {
                        backslashes = 0;
                        out.push(c);
                    }
                }
            }
            for _ in 0..backslashes {
                out.push('\\');
            }
            out.push('"');
            out
        }
        let mut parts = vec![quote(&a.program)];
        parts.extend(a.args.iter().map(|x| quote(x)));
        let joined = parts.join(" ");
        std::ffi::OsString::from(joined)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }
}
