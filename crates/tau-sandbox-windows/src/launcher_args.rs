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
pub fn parse_launcher_args(argv: impl Iterator<Item = OsString>) -> Result<LauncherArgs, String> {
    let mut profile: Option<String> = None;
    let mut caps: Vec<OsString> = Vec::new();
    let mut caps_str: Vec<String> = Vec::new();
    let mut it = argv;
    while let Some(a) = it.next() {
        if a == "--" {
            let program = it
                .next()
                .ok_or_else(|| "missing program after --".to_string())?;
            let rest: Vec<OsString> = it.collect();
            let profile = profile.ok_or_else(|| "missing --profile".to_string())?;
            return Ok(LauncherArgs {
                profile,
                caps: caps_str,
                program,
                args: rest,
            });
        } else if a == "--profile" {
            profile = Some(
                it.next()
                    .ok_or("--profile needs a value")?
                    .to_string_lossy()
                    .into_owned(),
            );
        } else if a == "--cap" {
            let c = it.next().ok_or("--cap needs a value")?;
            caps_str.push(c.to_string_lossy().into_owned());
            caps.push(c);
        } else {
            return Err(format!("unexpected arg: {}", a.to_string_lossy()));
        }
    }
    Err("missing -- separator / program".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn os(v: &[&str]) -> Vec<OsString> {
        v.iter().map(OsString::from).collect()
    }

    #[test]
    fn parses_profile_and_program() {
        let a = parse_launcher_args(
            os(&["--profile", "tau-sbx-1", "--", "cargo", "build"]).into_iter(),
        )
        .unwrap();
        assert_eq!(a.profile, "tau-sbx-1");
        assert!(a.caps.is_empty());
        assert_eq!(a.program, OsString::from("cargo"));
        assert_eq!(a.args, os(&["build"]));
    }

    #[test]
    fn collects_repeated_caps() {
        let a = parse_launcher_args(
            os(&["--profile", "p", "--cap", "A", "--cap", "B", "--", "prog"]).into_iter(),
        )
        .unwrap();
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
