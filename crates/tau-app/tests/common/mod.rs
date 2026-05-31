//! Shared test harness for Layer 2 serve-mode tests.
//!
//! Constructs a `Dispatcher` backed by in-memory channels and a minimal
//! Runtime (MockLlmBackend). No subprocess, no real LLM — fast and
//! deterministic.

#![allow(dead_code)]

use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tau_app::serve::{CancelRegistry, Dispatcher, HandshakeState, Inbound, Outbound, Project};
use tau_ports::fixtures::MockLlmBackend;
use tokio::sync::mpsc;
use tokio::task::LocalSet;

pub struct Harness {
    pub in_tx: mpsc::Sender<Inbound>,
    pub out_rx: mpsc::Receiver<Outbound>,
    /// Exposed so tests can pre-register in-flight tokens (e.g. concurrency tests).
    pub cancel_reg: CancelRegistry,
    /// Keeps the dispatcher thread alive until Harness is dropped.
    /// Public so shutdown tests can `.join()` on it after sending Eof.
    pub dispatcher_thread: std::thread::JoinHandle<()>,
}

impl Harness {
    /// Build a Harness from a fixture project directory with `max_concurrent = 8`.
    pub async fn new(fixture_dir: PathBuf) -> Self {
        Self::with_options(fixture_dir, 8).await
    }

    /// Build a Harness with a custom `max_concurrent` cap.
    ///
    /// Used by concurrency tests to set a tight cap (e.g. 1) and observe
    /// the `-32004 SERVER_BUSY` error when the cap is reached.
    pub async fn with_options(fixture_dir: PathBuf, max_concurrent: usize) -> Self {
        // tau_pkg::Scope::resolve (called transitively by Project::load) reads
        // $HOME to find the user scope. GitHub Actions Windows runners don't
        // set $HOME (Windows uses %USERPROFILE%), so we set it explicitly for
        // tests. Safe to set unconditionally: if HOME is already set, this is
        // a no-op; if it's unset (Windows CI), we point it at USERPROFILE or
        // a tempdir.
        ensure_home_env();

        let (in_tx, in_rx) = mpsc::channel::<Inbound>(32);
        let (out_tx, out_rx) = mpsc::channel::<Outbound>(64);

        let project = Arc::new(Project::load(&fixture_dir).await.expect("load fixture"));

        // Build a minimal Runtime with a MockLlmBackend.
        // Runtime::builder().build() returns NoLlmBackend if empty, so we
        // wire in a mock backend that is never actually called in handshake tests.
        let backend = MockLlmBackend::new("mock-llm");
        let runtime = Arc::new(
            tau_runtime_tokio::Runtime::builder()
                .with_llm_backend(backend)
                .build()
                .expect("build runtime"),
        );

        // Shared cancel_reg: exposed on Harness so tests can pre-register
        // in-flight tokens to simulate a saturated concurrency cap.
        let cancel_reg = CancelRegistry::default();

        let dispatcher = Dispatcher {
            project,
            runtime,
            handshake: HandshakeState::default(),
            cancel_reg: cancel_reg.clone(),
            max_concurrent,
            out_tx,
        };

        // `LocalSet` is !Send, so it cannot be spawned with tokio::spawn.
        // Instead, run the dispatcher on a dedicated OS thread that owns a
        // current_thread tokio runtime + LocalSet — the same topology
        // `lifecycle::run` uses in production (local_set.run_until).
        let thread = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build current_thread rt for dispatcher thread");
            let local_set = LocalSet::new();
            rt.block_on(local_set.run_until(async move {
                let _ = dispatcher.run(in_rx).await;
            }));
        });

        Self {
            in_tx,
            out_rx,
            cancel_reg,
            dispatcher_thread: thread,
        }
    }

    /// Perform a successful handshake. Must be called before any runtime.*
    /// method. Returns the handshake response (panics if none arrives).
    pub async fn handshake(&mut self) {
        self.send_raw(
            r#"{"jsonrpc":"2.0","id":0,"method":"meta.handshake","params":{"protocol_version":1}}"#,
        )
        .await;
        let resp = self.recv().await.expect("handshake response");
        assert_eq!(
            resp["result"]["protocol_version"], 1,
            "handshake failed: {resp}"
        );
    }

    /// Send a raw JSON line to the dispatcher.
    pub async fn send_raw(&self, line: &str) {
        let v: Value = serde_json::from_str(line).expect("test json");
        let _ = self.in_tx.send(Inbound::Json(v)).await;
    }

    /// Send an `Inbound::Eof` to signal clean shutdown.
    pub async fn send_eof(&self) {
        let _ = self.in_tx.send(Inbound::Eof).await;
    }

    /// Receive the next outbound message, with a 500 ms timeout.
    pub async fn recv(&mut self) -> Option<Value> {
        match tokio::time::timeout(Duration::from_millis(500), self.out_rx.recv()).await {
            Ok(Some(out)) => serde_json::to_value(&out).ok(),
            _ => None,
        }
    }

    /// Receive the next outbound message, with a custom timeout.
    pub async fn recv_timeout(&mut self, ms: u64) -> Option<Value> {
        match tokio::time::timeout(Duration::from_millis(ms), self.out_rx.recv()).await {
            Ok(Some(out)) => serde_json::to_value(&out).ok(),
            _ => None,
        }
    }
}

/// Ensure tau's scope-global lookup succeeds and parallel tests don't
/// race on the default `~/.tau/config.toml`.
///
/// `tau_pkg::Scope::global` reads `$TAU_HOME` → `$XDG_DATA_HOME/tau` →
/// `$HOME/.tau`, and writes a default `config.toml` into the resolved
/// directory if missing. On Windows, two parallel tests racing on the
/// same path produce "Access is denied" (os error 5).
///
/// We point `$TAU_HOME` at a dedicated process-local tempdir,
/// initialized exactly once. All tests in this binary share it; the
/// directory + default config.toml are created on first call, so no
/// later test ever has to write them.
fn ensure_home_env() {
    use std::sync::OnceLock;
    static TAU_HOME_INIT: OnceLock<()> = OnceLock::new();
    TAU_HOME_INIT.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("tau-serve-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create tau_home tempdir");
        // Pre-create the default config.toml so concurrent tests
        // don't race on writing it.
        let cfg = dir.join("config.toml");
        if !cfg.exists() {
            let _ = std::fs::write(&cfg, "");
        }
        std::env::set_var("TAU_HOME", dir);
    });
}
