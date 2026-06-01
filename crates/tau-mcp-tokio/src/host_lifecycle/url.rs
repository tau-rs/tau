//! MCP URL discriminator.
//!
//! Per the β.3 design doc §3, the `[tools.<name>] mcp = "..."` field
//! discriminates transport by URL scheme:
//!
//! - `stdio:<command>` → subprocess MCP server (PR-2)
//! - `http://...` / `https://...` → Streamable HTTP (PR-3)
//!
//! Any other scheme is rejected with `UrlParseError::UnsupportedScheme`.

use crate::host_lifecycle::error::UrlParseError;

/// Parsed MCP server URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpUrl {
    /// Subprocess MCP server. The vec is the command argv.
    Stdio {
        /// argv to spawn (first element is the binary).
        cmd: Vec<String>,
    },
}

/// Parse an MCP URL string into a typed `McpUrl`.
///
/// Currently accepts only `stdio:<command>` — HTTP variants land in PR-3.
pub fn parse_url(s: &str) -> Result<McpUrl, UrlParseError> {
    let s = s.trim();
    if s.is_empty() {
        return Err(UrlParseError::Empty);
    }
    if let Some(rest) = s.strip_prefix("stdio:") {
        let rest = rest.trim();
        if rest.is_empty() {
            return Err(UrlParseError::EmptyStdioCommand);
        }
        // Shell-split the command. v0 uses naive whitespace splitting —
        // future may grow to handle quoted args, but real MCP server
        // commands (`npx --yes @modelcontextprotocol/server-weather`,
        // `uvx mcp-server-fetch`) don't need quoting.
        let cmd = rest.split_whitespace().map(String::from).collect();
        return Ok(McpUrl::Stdio { cmd });
    }
    // PR-3 will add http/https arms here.
    let scheme = s.split(':').next().unwrap_or("").to_string();
    Err(UrlParseError::UnsupportedScheme { scheme })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_stdio() {
        let url = parse_url("stdio:npx --yes weather").expect("parse");
        match url {
            McpUrl::Stdio { cmd } => {
                assert_eq!(cmd, vec!["npx", "--yes", "weather"]);
            }
        }
    }

    #[test]
    fn empty_url_rejected() {
        assert!(matches!(parse_url(""), Err(UrlParseError::Empty)));
        assert!(matches!(parse_url("   "), Err(UrlParseError::Empty)));
    }

    #[test]
    fn empty_stdio_command_rejected() {
        assert!(matches!(
            parse_url("stdio:"),
            Err(UrlParseError::EmptyStdioCommand)
        ));
        assert!(matches!(
            parse_url("stdio:   "),
            Err(UrlParseError::EmptyStdioCommand)
        ));
    }

    #[test]
    fn http_rejected_in_pr2_with_correct_scheme() {
        let err = parse_url("https://mcp.example.com").expect_err("should reject in PR-2");
        match err {
            UrlParseError::UnsupportedScheme { scheme } => {
                assert_eq!(scheme, "https");
            }
            other => panic!("expected UnsupportedScheme, got {other:?}"),
        }
    }

    #[test]
    fn unknown_scheme_rejected() {
        let err = parse_url("ws://example.com").expect_err("should reject");
        match err {
            UrlParseError::UnsupportedScheme { scheme } => {
                assert_eq!(scheme, "ws");
            }
            other => panic!("expected UnsupportedScheme, got {other:?}"),
        }
    }
}
