#![no_std]
#![warn(missing_debug_implementations, rust_2018_idioms, unreachable_pub)]

//! Asynchronous stream of elements.
//!
//! Vendored no_std fork of `async-stream` 0.3.6 — see the crate's `Cargo.toml`
//! header for why. The public API (`stream!` / `try_stream!`) is identical to
//! upstream; only the internals were ported to `core::` + a target-gated
//! yielder STORE.
//!
//! Provides two macros, `stream!` and `try_stream!`, allowing the caller to
//! define asynchronous streams of elements. These are implemented using `async`
//! & `await` notation.
//!
//! The `stream!` macro returns an anonymous type implementing the [`Stream`]
//! trait. The `Item` associated type is the type of the values yielded from the
//! stream. The `try_stream!` also returns an anonymous type implementing the
//! [`Stream`] trait, but the `Item` associated type is `Result<T, Error>`. The
//! `try_stream!` macro supports using `?` notation as part of the
//! implementation.
//!
//! [`Stream`]: https://docs.rs/futures-core/*/futures_core/stream/trait.Stream.html

// The yielder uses a real `thread_local!` on std hosts (correct under the
// multi-threaded tokio executor) and a single-threaded global on wasm32
// (wasip2 is single-threaded). `thread_local!` lives in `std`.
#[cfg(not(target_arch = "wasm32"))]
extern crate std;

mod async_stream;
mod next;
mod yielder;

/// Asynchronous stream
///
/// See [crate](index.html) documentation for more details.
#[macro_export]
macro_rules! stream {
    ($($tt:tt)*) => {
        $crate::__private::stream_inner!(($crate) $($tt)*)
    }
}

/// Asynchronous fallible stream
///
/// See [crate](index.html) documentation for more details.
#[macro_export]
macro_rules! try_stream {
    ($($tt:tt)*) => {
        $crate::__private::try_stream_inner!(($crate) $($tt)*)
    }
}

// Not public API.
#[doc(hidden)]
pub mod __private {
    pub use crate::async_stream::AsyncStream;
    pub use crate::next::next;
    pub use async_stream_impl::{stream_inner, try_stream_inner};
    pub mod yielder {
        pub use crate::yielder::pair;
    }
}
