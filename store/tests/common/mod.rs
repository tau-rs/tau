//! One scenario, run over any store: a root, a child, the echo driver, and
//! a payload at every put site the §1 table names except the hook ones.
//! The tests shred parts of it and ask what is still readable.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, dead_code)]

use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use tau_kernel::abi::{AgentId, BlobRef, Budget, DimKey, DriverId, Endpoint, Name, Namespace};
use tau_kernel::blob::{Blobs, Memory};
use tau_kernel::driver::echo::EchoDriver;
use tau_kernel::kernel::{AbortHandle, BoxFuture, Kernel};
use tau_kernel::log::{Entry, Log};
use tau_kernel::syscall::{program, Match, WaitFor};
use tau_store::Disk;
use tempfile::TempDir;

pub(crate) const CHILD_MSG: &[u8] = b"child asks the echo";
pub(crate) const ROOT_MSG: &[u8] = b"root asks the echo";
pub(crate) const CHILD_RESULT: &[u8] = b"what the child made";
pub(crate) const ROOT_RESULT: &[u8] = b"what the root made";

/// A `Write` the test can read back after the kernel is done with it.
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

pub(crate) fn echo_id() -> DriverId {
    DriverId::new(Name::new("echo").unwrap())
}

/// The two stores the port has, each fresh. The `TempDir` keeps the disk
/// one alive for as long as the test holds it.
pub(crate) fn stores() -> Vec<(&'static str, Box<dyn Blobs>, Option<TempDir>)> {
    let dir = tempfile::tempdir().unwrap();
    let disk = Disk::open(dir.path()).unwrap();
    vec![
        ("Memory", Box::new(Memory::new()), None),
        ("Disk", Box::new(disk), Some(dir)),
    ]
}

/// A finished run of the scenario.
pub(crate) struct Run {
    pub(crate) kernel: Arc<Kernel>,
    pub(crate) root: AgentId,
    pub(crate) child: AgentId,
    /// What the root's `wait` handed it: the child's result reference.
    pub(crate) handed_up: BlobRef,
    pub(crate) sink: SharedBuf,
}

/// Root spawns a child; the child sends `CHILD_MSG` to the echo, reads the
/// reply and exits with `child_result`; the root waits for it, sends
/// `ROOT_MSG` to the echo, reads the reply and exits with `root_result`.
pub(crate) async fn run(blobs: Box<dyn Blobs>, child_result: &[u8], root_result: &[u8]) -> Run {
    run_with(blobs, child_result, root_result, true).await
}

/// [`run`], with the child's echo round trip optional: a child that sends
/// nothing and exits with nothing puts nothing.
pub(crate) async fn run_with(
    blobs: Box<dyn Blobs>,
    child_result: &[u8],
    root_result: &[u8],
    child_sends: bool,
) -> Run {
    let sink = SharedBuf::default();
    let log = Log::with_sink(sink.clone()).unwrap();
    let kernel = Kernel::boot_with(log, tokio_spawner, blobs);
    let echo = kernel
        .register_driver(
            echo_id(),
            EchoDriver::new(),
            Budget::from_dims([(DimKey::Tokens, 64)]),
        )
        .unwrap();
    let ns = Namespace::from_caps([echo]);
    let child_ns = ns.clone();
    let handed = Arc::new(Mutex::new(None));
    let handed_out = Arc::clone(&handed);
    let child_result = child_result.to_vec();
    let root_result = root_result.to_vec();
    let root = kernel
        .spawn_root(
            program(move |root| async move {
                let child = root
                    .spawn(
                        program(move |child| async move {
                            if child_sends {
                                let corr = child.send(echo, CHILD_MSG).unwrap();
                                let reply = child.recv(Match::Corr(corr)).await.unwrap();
                                assert_eq!(child.read(reply.payload).unwrap(), CHILD_MSG);
                            }
                            child.exit(&child_result)
                        }),
                        child_ns,
                        Budget::from_dims([(DimKey::Tokens, 200), (DimKey::Calls, 2)]),
                    )
                    .unwrap();
                let done = root.wait(WaitFor::Child(child)).await.unwrap();
                handed_out.lock().unwrap().replace(done.result().unwrap());
                let corr = root.send(echo, ROOT_MSG).unwrap();
                let reply = root.recv(Match::Corr(corr)).await.unwrap();
                assert_eq!(root.read(reply.payload).unwrap(), ROOT_MSG);
                root.exit(&root_result)
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
    let child = kernel
        .state()
        .agents()
        .find_map(|(id, a)| (a.parent == Some(root)).then_some(id))
        .unwrap();
    let handed_up = handed.lock().unwrap().unwrap();
    Run {
        kernel,
        root,
        child,
        handed_up,
        sink,
    }
}

/// Every payload reference the log carries, with its owner, per the
/// ADR-0012 §1 table. The empty reference is not content and is left out.
pub(crate) fn payloads(entries: &[Entry]) -> Vec<(AgentId, BlobRef)> {
    let mut out = Vec::new();
    for entry in entries {
        match entry {
            Entry::Sent { msg, .. } => {
                let Endpoint::Agent { id } = msg.from else {
                    panic!("a request is sent by an agent");
                };
                out.push((id, msg.payload));
            }
            Entry::Replied { msg, to } | Entry::Emitted { msg, to, .. } => {
                out.push((*to, msg.payload));
            }
            Entry::Exited { agent, result, .. } => out.push((*agent, *result)),
            Entry::Cancelled { agent, reason, .. } => out.push((*agent, *reason)),
            Entry::Verdicts { subject, roll, .. } => {
                for (_, ruling) in roll {
                    match ruling {
                        tau_kernel::abi::Ruling::Deny(blob)
                        | tau_kernel::abi::Ruling::Failed { error: blob, .. } => {
                            out.push((*subject, *blob));
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    out.retain(|(_, blob)| *blob != BlobRef::EMPTY);
    out
}

impl Run {
    /// The references `owner`'s entries carry.
    pub(crate) fn refs_of(&self, owner: AgentId) -> Vec<BlobRef> {
        payloads(&self.kernel.entries())
            .into_iter()
            .filter_map(|(o, blob)| (o == owner).then_some(blob))
            .collect()
    }

    /// Asserts every reference of `owner` reads as `Some` (or `None`).
    pub(crate) fn assert_readable(&self, owner: AgentId, readable: bool, why: &str) {
        let refs = self.refs_of(owner);
        assert!(!refs.is_empty(), "{why}: {owner} has payloads in the log");
        for blob in refs {
            assert_eq!(
                self.kernel.read(blob).is_some(),
                readable,
                "{why}: {owner}'s {blob}"
            );
        }
    }
}
