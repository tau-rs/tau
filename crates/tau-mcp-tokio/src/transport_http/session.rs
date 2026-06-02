//! `Mcp-Session-Id` header tracker.
//!
//! Per MCP spec rev 2025-03-26: the Streamable HTTP transport assigns a
//! session ID on the initialize response (HTTP response header
//! `Mcp-Session-Id`). The client must include that header on every
//! subsequent request. We track it in interior-mutable storage so the
//! `McpHttpServer`'s `Transport::send_message` can attach it without
//! needing `&mut self`.

use std::sync::Mutex;

/// HTTP header name MCP uses for session IDs.
pub const MCP_SESSION_ID_HEADER: &str = "Mcp-Session-Id";

/// Interior-mutable session-ID tracker.
#[derive(Debug, Default)]
pub struct SessionState {
    id: Mutex<Option<String>>,
}

impl SessionState {
    /// Construct with no session ID yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the current session ID (None until initialize response sets it).
    pub fn get(&self) -> Option<String> {
        self.id
            .lock()
            .expect("session state mutex poisoned")
            .clone()
    }

    /// Set the session ID. Idempotent: re-setting to the same value is
    /// a no-op; setting to a DIFFERENT non-None value while one is
    /// already pinned is logged + ignored (the first one wins, per
    /// MCP's "single session per HTTP transport" guarantee).
    pub fn set(&self, new_id: String) {
        let mut guard = self.id.lock().expect("session state mutex poisoned");
        match &*guard {
            None => *guard = Some(new_id),
            Some(existing) if existing == &new_id => {}
            Some(existing) => {
                tracing::warn!(
                    existing = %existing,
                    attempted = %new_id,
                    "ignoring conflicting Mcp-Session-Id; first-wins"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_with_no_id() {
        let s = SessionState::new();
        assert_eq!(s.get(), None);
    }

    #[test]
    fn set_then_get() {
        let s = SessionState::new();
        s.set("abc-123".into());
        assert_eq!(s.get(), Some("abc-123".into()));
    }

    #[test]
    fn re_set_same_id_idempotent() {
        let s = SessionState::new();
        s.set("abc".into());
        s.set("abc".into());
        assert_eq!(s.get(), Some("abc".into()));
    }

    #[test]
    fn first_wins_on_conflict() {
        let s = SessionState::new();
        s.set("first".into());
        s.set("second".into());
        assert_eq!(s.get(), Some("first".into()));
    }
}
