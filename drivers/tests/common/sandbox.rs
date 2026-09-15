//! What the sandbox tests share: a config that points at the shim cargo
//! built for this crate and at `/bin/sh`, a kernel boot, and a driver
//! wrapper that records every reply — including the ones the kernel
//! dead-letters after a cancel.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tau_drivers::sandbox::wire::Reply;
use tau_drivers::sandbox::{SandboxConfig, SandboxDriver};
use tau_kernel::abi::{Budget, Consumption, Corr, DimKey, DriverId, Name, Namespace};
use tau_kernel::driver::{Driver, ToolSchema};
use tau_kernel::kernel::{AbortHandle, BoxFuture, Delivery, Kernel};
use tau_kernel::log::Log;
use tau_kernel::reducer::Agent;
use tau_kernel::syscall::{program, Match};

/// The shim cargo built alongside this test binary.
pub(crate) const SHIM: &str = env!("CARGO_BIN_EXE_tau-sandbox-shim");

pub(crate) fn tokio_spawner(fut: BoxFuture<()>) -> AbortHandle {
    let task = tokio::spawn(fut);
    Box::new(move || task.abort())
}

pub(crate) fn sh_id() -> DriverId {
    DriverId::new(Name::new("sh").unwrap())
}

/// `/bin/sh` running `main.sh`, under `cpu` seconds and `wall`, with the
/// shim this build produced.
pub(crate) fn config(cpu: u32, wall: Duration) -> SandboxConfig {
    let mut c = SandboxConfig::new(["/bin/sh"], "main.sh", cpu, wall);
    c.shim = Some(SHIM.into());
    c
}

pub(crate) fn driver(config: SandboxConfig) -> SandboxDriver {
    SandboxDriver::new(config).unwrap()
}

pub(crate) fn request(code: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({ "code": code })).unwrap()
}

/// A driver wrapper that remembers every reply it produced, by corr.
#[derive(Clone)]
pub(crate) struct Recorded<D> {
    pub(crate) inner: D,
    pub(crate) replies: Arc<Mutex<Vec<(Corr, Reply, Consumption)>>>,
}

impl<D: Driver> Recorded<D> {
    pub(crate) fn new(inner: D) -> Self {
        Self {
            inner,
            replies: Arc::default(),
        }
    }

    pub(crate) fn replies(&self) -> Vec<(Corr, Reply, Consumption)> {
        self.replies.lock().unwrap().clone()
    }
}

impl<D: Driver> Driver for Recorded<D> {
    fn handle(&self, request: Delivery) -> BoxFuture<(Vec<u8>, Consumption)> {
        let corr = request.corr;
        let fut = self.inner.handle(request);
        let sink = Arc::clone(&self.replies);
        Box::pin(async move {
            let (bytes, consumed) = fut.await;
            let reply: Reply = serde_json::from_slice(&bytes).expect("a sandbox reply");
            sink.lock().unwrap().push((corr, reply, consumed.clone()));
            (bytes, consumed)
        })
    }

    fn describe(&self) -> Option<ToolSchema> {
        self.inner.describe()
    }

    fn abandon(&self, corr: Corr) {
        self.inner.abandon(corr);
    }
}

/// Runs one request through a real kernel: register, `send`, `recv`,
/// exit with the reply bytes. Returns the reply and the root's record
/// after settlement.
pub(crate) async fn run_one(driver: SandboxDriver, payload: Vec<u8>) -> (Reply, Agent) {
    let ceiling = driver.ceiling();
    let kernel = Kernel::boot(Log::in_memory(), tokio_spawner);
    let cap = kernel.register_driver(sh_id(), driver, ceiling).unwrap();
    let ns = Namespace::from_caps([cap]);
    let root = kernel
        .spawn_root(
            program(move |root| async move {
                let corr = root.send(cap, &payload).unwrap();
                let reply = root.recv(Match::Corr(corr)).await.unwrap();
                let bytes = root.read(reply.payload).unwrap();
                root.exit(&bytes)
            }),
            ns,
            Budget::from_dims([(DimKey::ComputeMs, 60_000), (DimKey::Calls, 2)]),
        )
        .unwrap();
    kernel.drained().await.unwrap();
    kernel.shutdown();
    let outcome = kernel.claim(root).unwrap();
    let tau_kernel::reducer::Outcome::Exited(blob) = outcome else {
        panic!("the root exited on its own: {outcome:?}");
    };
    let bytes = kernel.read(blob).unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    let reply: Reply = serde_json::from_value(value.clone())
        .unwrap_or_else(|e| panic!("not a sandbox reply: {e}\n{value:#}"));
    let record = kernel.state().agent(root).unwrap().clone();
    assert!(record.reserved.is_empty(), "settled at the reply");
    (reply, record)
}

/// Whether a process with this pid exists (a reaped zombie does not).
pub(crate) fn alive(pid: i32) -> bool {
    nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok()
}

/// Polls `check` with short sleeps until it holds or ~5 s pass.
pub(crate) async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    for _ in 0..250 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for: {what}");
}

/// A unique directory under the system temp dir, for files a run must
/// leave behind for the test to read.
pub(crate) fn outside_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("tau-sandbox-test-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
