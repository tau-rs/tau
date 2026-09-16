//! `Kernel::shred` and the fold (ADR-0012 §5, §6): a shred leaves the log
//! byte-for-byte as written, and `tau replay` over that log prints the hash
//! the live kernel had. The store here is `Memory`; the same scenario runs
//! on `Disk` in `store/tests/shred.rs`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{self, Write};
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};

use tau_kernel::abi::{AgentId, Budget, DimKey, DriverId, Name, Namespace};
use tau_kernel::blob::digest;
use tau_kernel::driver::echo::EchoDriver;
use tau_kernel::kernel::{AbortHandle, BoxFuture, Kernel};
use tau_kernel::log::Log;
use tau_kernel::syscall::{program, Match, WaitFor};

/// A `Write` the test can read back after the kernel is done with it.
#[derive(Clone, Default)]
struct SharedBuf(Arc<Mutex<Vec<u8>>>);

impl Write for SharedBuf {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn tokio_spawner(fut: BoxFuture<()>) -> AbortHandle {
    let task = tokio::spawn(fut);
    Box::new(move || task.abort())
}

const CHILD_RESULT: &[u8] = b"what the child made";
const ROOT_MSG: &[u8] = b"root asks the echo";
const ROOT_RESULT: &[u8] = b"what the root made";

/// Root spawns a child that exits with `CHILD_RESULT`; the root waits for
/// it, echoes `ROOT_MSG`, and exits with `ROOT_RESULT`. Returns the kernel,
/// the child's id, and the log bytes.
async fn scenario() -> (Arc<Kernel>, AgentId, Vec<u8>) {
    let sink = SharedBuf::default();
    let kernel = Kernel::boot(Log::with_sink(sink.clone()).unwrap(), tokio_spawner);
    let echo = kernel
        .register_driver(
            DriverId::new(Name::new("echo").unwrap()),
            EchoDriver::new(),
            Budget::from_dims([(DimKey::Tokens, 64)]),
        )
        .unwrap();
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
    let child = kernel
        .state()
        .agents()
        .find_map(|(id, a)| (a.parent == Some(root)).then_some(id))
        .unwrap();
    let bytes = sink.0.lock().unwrap().clone();
    (kernel, child, bytes)
}

fn replay_hash(log: &Path) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_tau"))
        .args(["replay", log.to_str().unwrap()])
        .output()
        .expect("the tau binary runs");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.lines()
        .last()
        .and_then(|line| line.split("hash=").nth(1))
        .unwrap_or_else(|| panic!("no hash on the last line: {text}"))
        .trim()
        .to_owned()
}

#[tokio::test]
async fn a_shred_is_invisible_to_the_fold() {
    let (untouched, _, untouched_log) = scenario().await;
    let (shredded, child, shredded_log) = scenario().await;
    let result = digest(CHILD_RESULT);
    assert_eq!(shredded.read(result).as_deref(), Some(CHILD_RESULT));

    shredded.shred(child).unwrap();

    assert_eq!(shredded.read(result), None, "the child's result is gone");
    assert_eq!(
        shredded.read(digest(ROOT_MSG)).as_deref(),
        Some(ROOT_MSG),
        "the root's request is not"
    );
    assert_eq!(shredded_log, untouched_log, "the logs are the same bytes");
    assert_eq!(shredded.state_hash(), untouched.state_hash());

    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    let path = dir.join("shred-invisible-to-the-fold.log");
    std::fs::write(&path, &shredded_log).unwrap();
    assert_eq!(
        replay_hash(&path),
        untouched.state_hash().to_string(),
        "`tau replay` over the shredded run's log prints the untouched run's hash"
    );
}
