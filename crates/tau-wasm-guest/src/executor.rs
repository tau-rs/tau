//! Single-threaded no_std executor. Guest futures are backed by synchronous
//! host imports + in-process `RefCell` state, so they never register a real
//! wake; a no-op waker with a busy-poll loop drives them to completion.

extern crate alloc;

use alloc::vec::Vec;
use core::future::Future;
use core::pin::pin;
use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

use futures_core::Stream;

static VTABLE: RawWakerVTable = RawWakerVTable::new(
    |_| RawWaker::new(core::ptr::null(), &VTABLE), // clone
    |_| {},                                        // wake
    |_| {},                                        // wake_by_ref
    |_| {},                                        // drop
);

fn noop_waker() -> Waker {
    // SAFETY: every vtable fn ignores the data pointer and is a pure no-op.
    unsafe { Waker::from_raw(RawWaker::new(core::ptr::null(), &VTABLE)) }
}

/// Drive a future to completion on the single guest thread.
pub fn block_on<F: Future>(fut: F) -> F::Output {
    let mut fut = pin!(fut);
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    loop {
        match fut.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return v,
            Poll::Pending => core::hint::spin_loop(),
        }
    }
}

/// Drain a stream to completion, collecting every item.
pub fn collect_stream<S: Stream>(stream: S) -> Vec<S::Item> {
    let mut stream = pin!(stream);
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    let mut out = Vec::new();
    loop {
        match stream.as_mut().poll_next(&mut cx) {
            Poll::Ready(Some(item)) => out.push(item),
            Poll::Ready(None) => return out,
            Poll::Pending => core::hint::spin_loop(),
        }
    }
}
