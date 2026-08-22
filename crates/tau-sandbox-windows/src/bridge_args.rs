//! Pure arg parsing for `tau-net-bridge-win`. No Win32; unit-tested on
//! any host. CLI contract:
//!   tau-net-bridge-win --pipe <name> -- <prog> <arg>...
use std::ffi::OsString;

/// Parsed bridge invocation.
#[derive(Debug, PartialEq, Eq)]
pub struct BridgeArgs {
    /// Bare pipe name (no `\\.\pipe\` prefix).
    pub pipe: String,
    /// The real program to run (the plugin / cargo).
    pub program: OsString,
    /// Arguments to the real program.
    pub args: Vec<OsString>,
}

/// Parse bridge argv (excluding argv[0]).
pub fn parse_bridge_args(argv: impl Iterator<Item = OsString>) -> Result<BridgeArgs, String> {
    let mut pipe: Option<String> = None;
    let mut it = argv;
    while let Some(a) = it.next() {
        if a == "--" {
            let program = it
                .next()
                .ok_or_else(|| "missing program after --".to_string())?;
            let args: Vec<OsString> = it.collect();
            let pipe = pipe.ok_or_else(|| "missing --pipe".to_string())?;
            return Ok(BridgeArgs {
                pipe,
                program,
                args,
            });
        } else if a == "--pipe" {
            pipe = Some(
                it.next()
                    .ok_or("--pipe needs a value")?
                    .to_string_lossy()
                    .into_owned(),
            );
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
    fn parses_pipe_and_program() {
        let a =
            parse_bridge_args(os(&["--pipe", "tau-proxy-1-0", "--", "cargo", "build"]).into_iter())
                .unwrap();
        assert_eq!(a.pipe, "tau-proxy-1-0");
        assert_eq!(a.program, OsString::from("cargo"));
        assert_eq!(a.args, os(&["build"]));
    }

    #[test]
    fn missing_pipe_is_error() {
        let e = parse_bridge_args(os(&["--", "prog"]).into_iter()).unwrap_err();
        assert!(e.contains("pipe"), "got {e}");
    }

    #[test]
    fn missing_program_is_error() {
        let e = parse_bridge_args(os(&["--pipe", "p", "--"]).into_iter()).unwrap_err();
        assert!(e.contains("program"), "got {e}");
    }
}
