//! Registry of cancellation tokens for in-flight requests.

use super::protocol::RequestId;
use dashmap::DashMap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Thread-safe registry of `RequestId → CancellationToken`. Per spec §8.2.
///
/// ```
/// use tau_app::serve::{CancelRegistry, RequestId};
///
/// let reg = CancelRegistry::default();
/// assert!(reg.is_empty());
///
/// let tok = reg.register(RequestId::Int(1));
/// assert_eq!(reg.len(), 1);
/// assert!(!tok.is_cancelled());
///
/// let cancelled = reg.cancel(&RequestId::Int(1));
/// assert!(cancelled);
/// assert!(tok.is_cancelled());
/// assert!(reg.is_empty());
/// ```
#[derive(Debug, Default, Clone)]
pub struct CancelRegistry {
    map: Arc<DashMap<RequestId, CancellationToken>>,
}

impl CancelRegistry {
    /// Register a new token for `id`. Returns a clone of the token the
    /// caller should `.cancelled()` on. If an entry already exists for
    /// this id (concurrent request id reuse — protocol violation by
    /// client), the old token is replaced and returned.
    ///
    /// ```
    /// use tau_app::serve::{CancelRegistry, RequestId};
    ///
    /// let reg = CancelRegistry::default();
    /// let tok = reg.register(RequestId::Str("req-1".into()));
    /// assert_eq!(reg.len(), 1);
    /// assert!(!tok.is_cancelled());
    /// ```
    pub fn register(&self, id: RequestId) -> CancellationToken {
        let tok = CancellationToken::new();
        self.map.insert(id, tok.clone());
        tok
    }

    /// Look up and cancel `id`. Returns `true` if found.
    ///
    /// ```
    /// use tau_app::serve::{CancelRegistry, RequestId};
    ///
    /// let reg = CancelRegistry::default();
    /// // Unknown id returns false.
    /// assert!(!reg.cancel(&RequestId::Int(999)));
    ///
    /// let tok = reg.register(RequestId::Int(1));
    /// assert!(reg.cancel(&RequestId::Int(1)));
    /// assert!(tok.is_cancelled());
    /// ```
    pub fn cancel(&self, id: &RequestId) -> bool {
        if let Some((_, tok)) = self.map.remove(id) {
            tok.cancel();
            true
        } else {
            false
        }
    }

    /// Remove an entry without cancelling (called when request completes
    /// normally).
    ///
    /// ```
    /// use tau_app::serve::{CancelRegistry, RequestId};
    ///
    /// let reg = CancelRegistry::default();
    /// let tok = reg.register(RequestId::Int(2));
    /// reg.forget(&RequestId::Int(2));
    /// // Token is not cancelled — request completed successfully.
    /// assert!(!tok.is_cancelled());
    /// assert!(reg.is_empty());
    /// ```
    pub fn forget(&self, id: &RequestId) {
        self.map.remove(id);
    }

    /// Cancel all entries (used during graceful shutdown).
    ///
    /// ```
    /// use tau_app::serve::{CancelRegistry, RequestId};
    ///
    /// let reg = CancelRegistry::default();
    /// let a = reg.register(RequestId::Int(1));
    /// let b = reg.register(RequestId::Int(2));
    /// reg.cancel_all();
    /// assert!(a.is_cancelled());
    /// assert!(b.is_cancelled());
    /// assert_eq!(reg.len(), 0);
    /// ```
    pub fn cancel_all(&self) {
        for entry in self.map.iter() {
            entry.value().cancel();
        }
        self.map.clear();
    }

    /// Current number of in-flight entries.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_then_cancel() {
        let reg = CancelRegistry::default();
        let tok = reg.register(RequestId::Int(1));
        assert!(!tok.is_cancelled());
        assert!(reg.cancel(&RequestId::Int(1)));
        assert!(tok.is_cancelled());
    }

    #[test]
    fn cancel_unknown_returns_false() {
        let reg = CancelRegistry::default();
        assert!(!reg.cancel(&RequestId::Int(999)));
    }

    #[test]
    fn forget_does_not_cancel() {
        let reg = CancelRegistry::default();
        let tok = reg.register(RequestId::Int(2));
        reg.forget(&RequestId::Int(2));
        assert!(!tok.is_cancelled());
    }

    #[test]
    fn cancel_all_cancels_everything() {
        let reg = CancelRegistry::default();
        let a = reg.register(RequestId::Int(1));
        let b = reg.register(RequestId::Int(2));
        reg.cancel_all();
        assert!(a.is_cancelled());
        assert!(b.is_cancelled());
        assert_eq!(reg.len(), 0);
    }
}
