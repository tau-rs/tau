//! The echo driver: the minimal border guard.
//!
//! Returns every request payload unchanged and reports one `tokens` per byte.
//! It exists so the walking skeleton has a world to talk to that is entirely
//! inside the test, and so a run's accounting is checkable by hand.

use crate::abi::{Consumption, DimKey};
use crate::kernel::{BoxFuture, Delivery};

use super::Driver;

/// Echoes requests back; bills one token per byte.
#[derive(Debug, Default)]
pub struct EchoDriver {
    _private: (),
}

impl EchoDriver {
    /// A new echo driver.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl Driver for EchoDriver {
    fn handle(&mut self, request: Delivery) -> BoxFuture<(Vec<u8>, Consumption)> {
        Box::pin(async move {
            let bytes = u64::try_from(request.payload.len()).unwrap_or(u64::MAX);
            let consumed = Consumption::from_dims([(DimKey::Tokens, bytes)]);
            (request.payload, consumed)
        })
    }
}
