//! One scenario over any store, and the `tau` binary as a subprocess. The
//! tests write the scenario's log to a file and point the binary at it and
//! at the store directory, which is what an operator does.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, dead_code)]

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};

use tau_kernel::abi::{AgentId, Budget, DimKey, DriverId, Name, Namespace};
use tau_kernel::blob::Blobs;
use tau_kernel::driver::echo::EchoDriver;
use tau_kernel::kernel::{AbortHandle, BoxFuture, Kernel};
use tau_kernel::log::Log;
use tau_kernel::syscall::{program, Match, WaitFor};
use tokio::sync::Notify;

pub(crate) const CHILD_MSG: &[u8] = b"child asks the echo";
pub(crate) const CHILD_RESULT: &[u8] = b"what the child made";
pub(crate) const ROOT_MSG: &[u8] = b"root asks the echo";
pub(crate) const ROOT_RESULT: &[u8] = b"what the root made";

/// A `Write` the test can read back while and after the kernel writes it.
#[derive(Clone, Default)]
pub(crate) struct SharedBuf(Arc<Mutex<Vec<u8>>>);

impl SharedBuf {
    pub(crate) fn contents(&self) -> Vec<u8> {
        self.0.lock().unwrap().clone()
    }
}

impl Write for SharedBuf {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(crate) fn tokio_spawner(fut: BoxFuture<()>) -> AbortHandle {
    let task = tokio::spawn(fut);
    Box::new(move || task.abort())
}

/// A run of the scenario: the kernel, its two agents, and the log sink.
pub(crate) struct Run {
    pub(crate) kernel: Arc<Kernel>,
    pub(crate) root: AgentId,
    pub(crate) child: AgentId,
    pub(crate) sink: SharedBuf,
}

impl Run {
    /// The log as written so far, as a file under cargo's temp dir.
    pub(crate) fn write_log(&self, name: &str) -> PathBuf {
        scratch(name, &self.sink.contents())
    }
}

fn boot(blobs: Box<dyn Blobs>) -> (Arc<Kernel>, SharedBuf, tau_kernel::abi::Capability) {
    let sink = SharedBuf::default();
    let kernel = Kernel::boot_with(Log::with_sink(sink.clone()).unwrap(), tokio_spawner, blobs);
    let echo = kernel
        .register_driver(
            DriverId::new(Name::new("echo").unwrap()),
            EchoDriver::new(),
            Budget::from_dims([(DimKey::Tokens, 64)]),
        )
        .unwrap();
    (kernel, sink, echo)
}

fn child_of(kernel: &Kernel, root: AgentId) -> AgentId {
    kernel
        .state()
        .agents()
        .find_map(|(id, a)| (a.parent == Some(root)).then_some(id))
        .unwrap()
}

/// Root spawns a child that exits with `CHILD_RESULT`; the root waits for
/// it, echoes `ROOT_MSG`, and exits with `ROOT_RESULT`. Finished and shut
/// down on return.
pub(crate) async fn finished(blobs: Box<dyn Blobs>) -> Run {
    let (kernel, sink, echo) = boot(blobs);
    let ns = Namespace::from_caps([echo]);
    let child_ns = ns.clone();
    let root = kernel
        .spawn_root(
            program(move |root| async move {
                let child = root
                    .spawn(
                        program(move |child| async move { child.exit(CHILD_RESULT) }),
                        child_ns,
                        Budget::from_dims([(DimKey::Tokens, 100)]),
                    )
                    .unwrap();
                let done = root.wait(WaitFor::Child(child)).await.unwrap();
                assert_eq!(
                    root.read(done.result().unwrap()).as_deref(),
                    Some(CHILD_RESULT)
                );
                let corr = root.send(echo, ROOT_MSG).unwrap();
                let _ = root.recv(Match::Corr(corr)).await.unwrap();
                root.exit(ROOT_RESULT)
            }),
            ns,
            Budget::from_dims([
                (DimKey::Tokens, 1_000),
                (DimKey::Calls, 10),
                (DimKey::Depth, 1),
            ]),
        )
        .unwrap();
    kernel.drained().await.unwrap();
    kernel.shutdown();
    let child = child_of(&kernel, root);
    Run {
        kernel,
        root,
        child,
        sink,
    }
}

/// Root spawns a child that echoes `CHILD_MSG` (so it owns a payload) and
/// then blocks on a `recv` only a cancel will answer. Returned while the
/// child is live; `finish` cancels it and drains.
pub(crate) async fn live(blobs: Box<dyn Blobs>) -> Run {
    let (kernel, sink, echo) = boot(blobs);
    let ns = Namespace::from_caps([echo]);
    let child_ns = ns.clone();
    let parked = Arc::new(Notify::new());
    let parked_in = Arc::clone(&parked);
    let root = kernel
        .spawn_root(
            program(move |root| async move {
                let child = root
                    .spawn(
                        program(move |child| async move {
                            let corr = child.send(echo, CHILD_MSG).unwrap();
                            let _ = child.recv(Match::Corr(corr)).await.unwrap();
                            parked_in.notify_one();
                            let _ = child.recv(Match::Any).await;
                            child.exit(b"never")
                        }),
                        child_ns,
                        Budget::from_dims([(DimKey::Tokens, 200), (DimKey::Calls, 2)]),
                    )
                    .unwrap();
                let _ = root.wait(WaitFor::Child(child)).await;
                root.exit(ROOT_RESULT)
            }),
            ns,
            Budget::from_dims([
                (DimKey::Tokens, 1_000),
                (DimKey::Calls, 10),
                (DimKey::Depth, 1),
            ]),
        )
        .unwrap();
    parked.notified().await;
    let child = child_of(&kernel, root);
    Run {
        kernel,
        root,
        child,
        sink,
    }
}

impl Run {
    /// Cancels the parked child of [`live`], drains, and shuts down.
    pub(crate) async fn finish(&self) {
        self.kernel
            .cancel_from_harness(self.child, &tau_kernel::syscall::CancelMode::immediate())
            .unwrap();
        self.kernel.drained().await.unwrap();
        self.kernel.shutdown();
    }
}

/// A scratch file under cargo's per-crate temp dir; the name keeps parallel
/// tests apart.
pub(crate) fn scratch(name: &str, contents: &[u8]) -> PathBuf {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    fs::write(&path, contents).unwrap();
    path
}

pub(crate) fn tau(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tau"))
        .args(args)
        .output()
        .expect("the tau binary runs")
}

pub(crate) fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

pub(crate) fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

pub(crate) fn path(p: &Path) -> &str {
    p.to_str().unwrap()
}

/// The `hash=` field of the summary line, the last one on stdout.
pub(crate) fn replay_hash(log: &Path) -> String {
    let out = tau(&["replay", path(log)]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    text.lines()
        .last()
        .and_then(|line| line.split("hash=").nth(1))
        .unwrap_or_else(|| panic!("no hash on the last line: {text}"))
        .trim()
        .to_owned()
}
