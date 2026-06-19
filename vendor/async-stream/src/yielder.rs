use core::future::Future;
use core::marker::PhantomData;
use core::pin::Pin;
use core::task::{Context, Poll};

#[derive(Debug)]
pub struct Sender<T> {
    _p: PhantomData<fn(T) -> T>,
}

#[derive(Debug)]
pub struct Receiver<T> {
    _p: PhantomData<T>,
}

pub(crate) struct Enter<'a, T> {
    _rx: &'a mut Receiver<T>,
    prev: *mut (),
}

// Note: It is considered unsound for anyone other than our macros to call
// this function. This is a private API intended only for calls from our
// macros, and users should never call it, but some people tend to
// misinterpret it as fine to call unless it is marked unsafe.
#[doc(hidden)]
pub unsafe fn pair<T>() -> (Sender<T>, Receiver<T>) {
    let tx = Sender { _p: PhantomData };
    let rx = Receiver { _p: PhantomData };
    (tx, rx)
}

// Tracks the pointer to `Option<T>`.
//
// Upstream backs this with a std `thread_local!`. We keep that exactly on std
// hosts (correct under the multi-threaded tokio executor — each thread gets its
// own slot), and use a single-threaded global on wasm32, where wasip2 has no
// threads and `thread_local!` (a std macro) is unavailable.
#[cfg(not(target_arch = "wasm32"))]
mod store {
    use core::cell::Cell;
    use core::ptr;

    std::thread_local!(static STORE: Cell<*mut ()> = const { Cell::new(ptr::null_mut()) });

    pub(crate) fn with<R>(f: impl FnOnce(&Cell<*mut ()>) -> R) -> R {
        STORE.with(f)
    }
}

#[cfg(target_arch = "wasm32")]
mod store {
    use core::cell::Cell;
    use core::ptr;

    struct SyncCell(Cell<*mut ()>);
    // SAFETY: wasm32 (wasip2) is single-threaded — there is no concurrent
    // access to this global. async-stream only touches the slot within a
    // single synchronous `poll_next` call on the one and only thread.
    unsafe impl Sync for SyncCell {}

    static STORE: SyncCell = SyncCell(Cell::new(ptr::null_mut()));

    pub(crate) fn with<R>(f: impl FnOnce(&Cell<*mut ()>) -> R) -> R {
        f(&STORE.0)
    }
}

// ===== impl Sender =====

impl<T> Sender<T> {
    pub fn send(&mut self, value: T) -> impl Future<Output = ()> {
        Send { value: Some(value) }
    }
}

struct Send<T> {
    value: Option<T>,
}

impl<T> Unpin for Send<T> {}

impl<T> Future for Send<T> {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
        if self.value.is_none() {
            return Poll::Ready(());
        }

        store::with(|cell| {
            let ptr = cell.get() as *mut Option<T>;
            let option_ref = unsafe { ptr.as_mut() }.expect("invalid usage");

            if option_ref.is_none() {
                *option_ref = self.value.take();
            }

            Poll::Pending
        })
    }
}

// ===== impl Receiver =====

impl<T> Receiver<T> {
    pub(crate) fn enter<'a>(&'a mut self, dst: &'a mut Option<T>) -> Enter<'a, T> {
        let prev = store::with(|cell| {
            let prev = cell.get();
            cell.set(dst as *mut _ as *mut ());
            prev
        });

        Enter { _rx: self, prev }
    }
}

// ===== impl Enter =====

impl<'a, T> Drop for Enter<'a, T> {
    fn drop(&mut self) {
        store::with(|cell| cell.set(self.prev));
    }
}
