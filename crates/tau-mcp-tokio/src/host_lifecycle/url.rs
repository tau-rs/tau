//! MCP URL discriminator.
//!
//! Per the β.3 design doc §3, the `[tools.<name>] mcp = "..."` field
//! discriminates transport by URL scheme:
//!
//! - `stdio:<command>` → subprocess MCP server (PR-2)
//! - `http://...` / `https://...` → Streamable HTTP (PR-3)
//! - `cassette:<path>` → JSONL cassette replay (PR-6)
//!
//! Any other scheme is rejected with `UrlParseError::UnsupportedScheme`.

use std::path::PathBuf;

use crate::host_lifecycle::error::UrlParseError;

/// Parsed MCP server URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpUrl {
    /// Subprocess MCP server. The vec is the command argv.
    Stdio {
        /// argv to spawn (first element is the binary).
        cmd: Vec<String>,
    },
    /// Plain-HTTP Streamable MCP server (accepted but should warn at
    /// build time per spec §3).
    Http {
        /// Validated URL with a host component.
        url: url::Url,
    },
    /// HTTPS Streamable MCP server.
    Https {
        /// Validated URL with a host component.
        url: url::Url,
    },
    /// JSONL cassette replay — reads from a local file path.
    Cassette {
        /// Path to the JSONL cassette file.
        path: PathBuf,
    },
}

/// Parse an MCP URL string into a typed `McpUrl`.
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
        let cmd = rest.split_whitespace().map(String::from).collect();
        return Ok(McpUrl::Stdio { cmd });
    }
    if let Some(rest) = s.strip_prefix("cassette:") {
        let rest = rest.trim();
        if rest.is_empty() {
            return Err(UrlParseError::EmptyCassettePath);
        }
        return Ok(McpUrl::Cassette {
            path: PathBuf::from(rest),
        });
    }
    if s.starts_with("http://") || s.starts_with("https://") {
        let url = url::Url::parse(s).map_err(|e| UrlParseError::UnsupportedScheme {
            scheme: format!("invalid URL: {e}"),
        })?;
        if url.host().is_none() {
            return Err(UrlParseError::UnsupportedScheme {
                scheme: "http(s) URL has no host".to_string(),
            });
        }
        return match url.scheme() {
            "http" => Ok(McpUrl::Http { url }),
            "https" => Ok(McpUrl::Https { url }),
            other => Err(UrlParseError::UnsupportedScheme {
                scheme: other.to_string(),
            }),
        };
    }
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
            other => panic!("expected Stdio, got {other:?}"),
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
    fn https_accepted() {
        let url = parse_url("https://mcp.example.com").expect("parse");
        match url {
            McpUrl::Https { url } => {
                assert_eq!(url.host_str(), Some("mcp.example.com"));
            }
            other => panic!("expected Https, got {other:?}"),
        }
    }

    #[test]
    fn http_accepted() {
        let url = parse_url("http://localhost:8080/mcp").expect("parse");
        match url {
            McpUrl::Http { url } => {
                assert_eq!(url.host_str(), Some("localhost"));
                assert_eq!(url.port(), Some(8080));
            }
            other => panic!("expected Http, got {other:?}"),
        }
    }

    #[test]
    fn ws_rejected() {
        let err = parse_url("ws://example.com").expect_err("should reject");
        match err {
            UrlParseError::UnsupportedScheme { scheme } => {
                assert_eq!(scheme, "ws");
            }
            other => panic!("expected UnsupportedScheme, got {other:?}"),
        }
    }

    #[test]
    fn file_rejected() {
        let err = parse_url("file:///etc/passwd").expect_err("should reject");
        match err {
            UrlParseError::UnsupportedScheme { scheme } => {
                assert_eq!(scheme, "file");
            }
            other => panic!("expected UnsupportedScheme, got {other:?}"),
        }
    }

    #[test]
    fn http_without_host_rejected() {
        let err = parse_url("http://").expect_err("should reject");
        assert!(matches!(err, UrlParseError::UnsupportedScheme { .. }));
    }

    #[test]
    fn cassette_relative_path_parses() {
        let url = parse_url("cassette:./fixtures/weather.jsonl").expect("parse");
        match url {
            McpUrl::Cassette { path } => {
                assert_eq!(path, std::path::PathBuf::from("./fixtures/weather.jsonl"));
            }
            other => panic!("expected Cassette, got {other:?}"),
        }
    }

    #[test]
    fn cassette_absolute_path_parses() {
        let url = parse_url("cassette:/tmp/x.jsonl").expect("parse");
        match url {
            McpUrl::Cassette { path } => {
                assert_eq!(path, std::path::PathBuf::from("/tmp/x.jsonl"));
            }
            other => panic!("expected Cassette, got {other:?}"),
        }
    }

    #[test]
    fn cassette_empty_path_rejected() {
        let err = parse_url("cassette:").expect_err("should reject");
        match err {
            UrlParseError::EmptyCassettePath => {}
            other => panic!("expected EmptyCassettePath, got {other:?}"),
        }
        assert!(matches!(
            parse_url("cassette:   "),
            Err(UrlParseError::EmptyCassettePath)
        ));
    }

    #[test]
    fn cassette_path_trimmed() {
        let url = parse_url("cassette:   ./x.jsonl   ").expect("parse");
        match url {
            McpUrl::Cassette { path } => {
                assert_eq!(path, std::path::PathBuf::from("./x.jsonl"));
            }
            other => panic!("expected Cassette, got {other:?}"),
        }
    }
}
