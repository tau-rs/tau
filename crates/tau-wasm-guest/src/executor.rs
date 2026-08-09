//! Single-threaded no_std executor. Guest futures are backed by synchronous
//! host imports + in-process `RefCell` state, so they never register a real
//! wake; a no-op waker with a busy-poll loop drives them to completion.

extern crate alloc;

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

/// Drain a stream to completion, invoking `f` on each item as it arrives.
pub fn for_each_stream<S: Stream, F: FnMut(S::Item)>(stream: S, mut f: F) {
    let mut stream = pin!(stream);
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    loop {
        match stream.as_mut().poll_next(&mut cx) {
            Poll::Ready(Some(item)) => f(item),
            Poll::Ready(None) => return,
            Poll::Pending => core::hint::spin_loop(),
        }
    }
}
