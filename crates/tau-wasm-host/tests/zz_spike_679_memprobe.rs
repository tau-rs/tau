//! THROWAWAY SPIKE HARNESS — issue #679. NOT production code. Delete after the
//! measurement lands. Not wired into CI, `#[ignore]`d, and named `zz_` so it
//! sorts last and is obvious in a file listing.
//!
//! Measures the tau wasm host-port boundary:
//!   N1  shipped-component size as a function of what the baked IR makes
//!       reachable (the published 15.3 KiB floor is an EMPTY-IR build that
//!       dead-code-eliminates the whole runtime, `serde_json` included).
//!   N2  peak guest linear memory for one run, as a function of conversation
//!       depth x declared tool count / schema size, plus the raw JSON payload
//!       bytes that actually cross `complete` on the deepest turn.
//!
//! Run:
//!   timeout 1800 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-jsonsize \
//!     cargo test -p tau-wasm-host --test zz_spike_679_memprobe -- --ignored --nocapture

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use tau_ports::llm::{CompletionResponse, StopReason, ToolUse};
use wasmtime::component::{Component, HasSelf, Linker};
use wasmtime::{Config, Engine, ResourceLimiter, Store};
use wasmtime_wasi::{ResourceTable, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

wasmtime::component::bindgen!({
    path: "../../wit/tau-host.wit",
    world: "runner",
});

use tau::host::host;

// ---------------------------------------------------------------------------
// Fixture generation: a parameterised tau.toml lowered to canonical IR bytes.
// ---------------------------------------------------------------------------

/// Build a project whose entry agent declares `n_tools` native tools, each
/// carrying a JSON-Schema blob padded to roughly `schema_pad` bytes, and lower
/// it to canonical IR. Tool 0 is always `read_temp` so the cassette's tool
/// calls dispatch against a real `tau-native-tools` body.
fn spike_ir_bytes(n_tools: usize, schema_pad: usize, max_turns: u32) -> Vec<u8> {
    let mut toml = format!(
        r#"
packages = ["anthropic"]

[project]
name = "spike-679"
version = "0.1.0"

[models.claude]
backend = "anthropic"
model = "claude-sonnet-4-6"

[agents.main]
display_name = "Main"
package = "spike-679@^0.1"
model = "claude"
max_turns = {max_turns}
"#
    );

    if n_tools > 0 {
        let refs: Vec<String> = (0..n_tools).map(|i| format!("\"{}\"", tool_name(i))).collect();
        toml.push_str(&format!("tool_refs = [{}]\n", refs.join(", ")));
    }

    toml.push_str(
        r#"
[agents.main.prompt]
system = "You are a spike agent."
"#,
    );

    for i in 0..n_tools {
        // Pad the schema with a `description` string so the JSON Schema that
        // crosses the boundary has a controlled byte size.
        let pad = "x".repeat(schema_pad);
        toml.push_str(&format!(
            r#"
[tools.{name}]
native = "{native}"
description = "Spike tool {i}."
capabilities = []
input_schema = {{ type = "object", properties = {{ arg = {{ type = "string", description = "{pad}" }} }} }}
"#,
            name = tool_name(i),
            native = native_name(i),
        ));
    }

    let config = tau_pkg::project::ProjectConfig::parse_str(&toml).expect("spike fixture parses");
    let target: tau_ports::target::TargetTriple = "any-wasi-strict".parse().unwrap();
    let caches = tau_ir_lower::Caches {
        // Non-zero: `[0u8; 32]` is the resolve-stage sentinel that typecheck
        // rejects as `UnknownNativeTool`.
        native_tool: &|_| Some([1u8; 32]),
        mcp_contract: &|_| None,
        skill: &|_| None,
        prompt_file: &|_| Ok(Vec::new()),
    };
    let module = tau_ir_lower::lower_project(&config, &target, &caches)
        .expect("spike fixture lowers")
        .module;
    tau_ir::to_canonical_bytes(&module)
}

fn tool_name(i: usize) -> String {
    if i == 0 {
        "read_temp".to_string()
    } else {
        format!("spike_tool_{i}")
    }
}

fn native_name(i: usize) -> String {
    if i == 0 {
        "ReadTemp".to_string()
    } else {
        format!("SpikeTool{i}")
    }
}

// ---------------------------------------------------------------------------
// Guest build (lifted verbatim from tests/roundtrip.rs).
// ---------------------------------------------------------------------------

fn build_guest_component(ir_bytes: Option<&[u8]>) -> Vec<u8> {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crate is two levels below the workspace root")
        .to_path_buf();

    let guest_target_dir = workspace_root.join("target/spike-679-guest");

    let mut cmd = Command::new(env!("CARGO"));
    cmd.current_dir(&workspace_root)
        .args([
            "build",
            "-p",
            "tau-wasm-guest",
            "--target",
            "wasm32-wasip2",
            "--release",
            "--message-format=json",
        ])
        .env("CARGO_INCREMENTAL", "0")
        .env("CARGO_TARGET_DIR", &guest_target_dir);

    let _scratch;
    if let Some(bytes) = ir_bytes {
        let f = tempfile::NamedTempFile::new().expect("ir scratch");
        std::fs::write(f.path(), bytes).expect("write ir scratch");
        cmd.env("TAU_IR_BYTES", f.path());
        _scratch = Some(f);
    } else {
        _scratch = None;
    }

    let output = cmd.output().expect("failed to spawn cargo to build the guest");
    assert!(
        output.status.success(),
        "guest build failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("cargo json output is utf-8");
    let wasm_path = stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|msg| msg["reason"] == "compiler-artifact")
        .filter(|msg| {
            msg["target"]["name"]
                .as_str()
                .is_some_and(|name| name == "tau-wasm-guest" || name == "tau_wasm_guest")
        })
        .flat_map(|msg| {
            msg["filenames"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|f| f.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .find(|f| f.ends_with(".wasm"))
        .expect("no .wasm artifact in cargo output");

    std::fs::read(&wasm_path).expect("failed to read the built guest component")
}

/// Declared initial linear-memory size of the component's core module, in
/// bytes. `memory_growing` never fires if the guest fits inside its initial
/// allocation, so this is the floor every measured peak sits on top of.
fn declared_initial_memory_bytes(wasm: &[u8]) -> Option<usize> {
    let f = tempfile::NamedTempFile::new().ok()?;
    std::fs::write(f.path(), wasm).ok()?;
    let out = Command::new("wasm-tools")
        .arg("print")
        .arg(f.path())
        .output()
        .ok()?;
    let wat = String::from_utf8_lossy(&out.stdout).into_owned();
    for line in wat.lines() {
        let t = line.trim();
        if t.starts_with("(memory ") {
            let pages: usize = t
                .split_whitespace()
                .last()?
                .trim_end_matches(')')
                .parse()
                .ok()?;
            return Some(pages * 64 * 1024);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Instrumented host: peak linear memory + per-crossing payload bytes.
// ---------------------------------------------------------------------------

#[derive(Default, Debug, Clone)]
struct Probe {
    /// High-water mark of guest linear memory in bytes (wasm memory never
    /// shrinks, so this is the provisioning number a device must budget).
    peak_memory: usize,
    /// Byte length of every `request-json` the guest pushed across `complete`.
    request_bytes: Vec<usize>,
    /// Byte length of every `CompletionResponse` JSON returned to the guest.
    response_bytes: Vec<usize>,
    /// Byte length of every `event-json` pushed across `emit-event`.
    event_bytes: Vec<usize>,
}

struct SpikeState {
    table: ResourceTable,
    wasi: WasiCtx,
    cassette: Vec<String>,
    next: usize,
    probe: Arc<Mutex<Probe>>,
    peak_memory: usize,
}

impl ResourceLimiter for SpikeState {
    fn memory_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> std::result::Result<bool, wasmtime::Error> {
        // `current` matters as much as `desired`: the FIRST call reports the
        // module's declared initial size, which no other hook exposes.
        let hi = desired.max(_current);
        if hi > self.peak_memory {
            self.peak_memory = hi;
            self.probe.lock().unwrap().peak_memory = hi;
        }
        Ok(true)
    }

    fn table_growing(
        &mut self,
        _current: usize,
        _desired: usize,
        _maximum: Option<usize>,
    ) -> std::result::Result<bool, wasmtime::Error> {
        Ok(true)
    }
}

impl WasiView for SpikeState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl host::Host for SpikeState {
    fn complete(&mut self, request_json: String) -> Result<String, String> {
        self.probe
            .lock()
            .unwrap()
            .request_bytes
            .push(request_json.len());
        let resp = self
            .cassette
            .get(self.next)
            .cloned()
            .unwrap_or_else(|| self.cassette.last().cloned().expect("non-empty cassette"));
        self.next += 1;
        self.probe.lock().unwrap().response_bytes.push(resp.len());
        Ok(resp)
    }

    fn now_millis(&mut self) -> u64 {
        0
    }

    fn next_u64(&mut self) -> u64 {
        0
    }

    fn emit_event(&mut self, event_json: String) {
        self.probe.lock().unwrap().event_bytes.push(event_json.len());
    }
}

fn run_with_probe(wasm: &[u8], prompt: &str, cassette: Vec<String>) -> (Result<String, String>, Probe) {
    let mut config = Config::new();
    config.cranelift_nan_canonicalization(true);
    config.wasm_relaxed_simd(false);
    let engine = Engine::new(&config).expect("engine");
    let component = Component::new(&engine, wasm).expect("component loads");

    let mut linker: Linker<SpikeState> = Linker::new(&engine);
    wasmtime_wasi::p2::add_to_linker_sync(&mut linker).expect("wasi linker");
    Runner::add_to_linker::<_, HasSelf<SpikeState>>(&mut linker, |s| s).expect("host linker");

    let probe = Arc::new(Mutex::new(Probe::default()));
    let state = SpikeState {
        table: ResourceTable::new(),
        wasi: WasiCtxBuilder::new().build(),
        cassette,
        next: 0,
        probe: Arc::clone(&probe),
        peak_memory: 0,
    };

    let mut store = Store::new(&engine, state);
    store.limiter(|s| s as &mut dyn ResourceLimiter);

    let runner = Runner::instantiate(&mut store, &component, &linker).expect("instantiate");
    let out = match runner.call_run(&mut store, prompt) {
        Ok(inner) => inner,
        Err(trap) => Err(format!("trap: {trap}")),
    };

    let p = probe.lock().unwrap().clone();
    (out, p)
}

// ---------------------------------------------------------------------------
// Cassette: `turns - 1` tool-calling responses then one EndTurn.
// ---------------------------------------------------------------------------

fn cassette(turns: usize, text_bytes: usize) -> Vec<String> {
    let text = "y".repeat(text_bytes);
    let mut out = Vec::with_capacity(turns);
    for i in 0..turns.saturating_sub(1) {
        let resp = CompletionResponse::new(
            text.clone(),
            vec![ToolUse::new(
                format!("call-{i}"),
                "read_temp".into(),
                serde_json::from_str("{}").unwrap(),
            )],
            StopReason::ToolUse,
            None,
        );
        out.push(serde_json::to_string(&resp).expect("cassette entry serialises"));
    }
    let last = CompletionResponse::new(text, vec![], StopReason::EndTurn, None);
    out.push(serde_json::to_string(&last).expect("cassette entry serialises"));
    out
}

// ---------------------------------------------------------------------------
// N1 — static: shipped component size vs what the baked IR makes reachable.
// ---------------------------------------------------------------------------

#[test]
#[ignore = "spike #679: builds several wasm32-wasip2 guests"]
fn n1_component_size_grid() {
    let cases: Vec<(&str, Option<Vec<u8>>)> = vec![
        ("empty IR (published 15.3 KiB floor)", None),
        ("IR, 0 tools", Some(spike_ir_bytes(0, 0, 64))),
        ("IR, 1 tool, 0 B schema pad", Some(spike_ir_bytes(1, 0, 64))),
        ("IR, 4 tools, 256 B schema pad", Some(spike_ir_bytes(4, 256, 64))),
        ("IR, 16 tools, 256 B schema pad", Some(spike_ir_bytes(16, 256, 64))),
    ];

    println!("\n=== N1: shipped component size ===");
    println!("| variant | IR bytes | component bytes | KiB |");
    println!("|---|--:|--:|--:|");
    for (label, ir) in &cases {
        let wasm = build_guest_component(ir.as_deref());
        let ir_len = ir.as_ref().map_or(0, |b| b.len());
        println!(
            "| {label} | {ir_len} | {} | {:.1} |",
            wasm.len(),
            wasm.len() as f64 / 1024.0
        );
    }
}

/// Persist the reference fixture (4 tools, 256 B schema pad) so the
/// `serde_json`-stub differential can be rebuilt by hand:
///
///   TAU_IR_BYTES=target/spike-679/ir-4tools.bin \
///   env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/spike-679-guest \
///     cargo build -p tau-wasm-guest --target wasm32-wasip2 --release
#[test]
#[ignore = "spike #679: writes the reference IR + component to target/spike-679/"]
fn n1b_dump_reference_artifacts() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .join("target/spike-679");
    std::fs::create_dir_all(&dir).expect("mkdir");

    let ir = spike_ir_bytes(4, 256, 64);
    std::fs::write(dir.join("ir-4tools.bin"), &ir).expect("write ir");

    let wasm = build_guest_component(Some(&ir));
    std::fs::write(dir.join("guest-4tools.wasm"), &wasm).expect("write wasm");
    println!("ir {} B, component {} B -> {}", ir.len(), wasm.len(), dir.display());
}

// ---------------------------------------------------------------------------
// N2 — dynamic: peak guest linear memory across turns x tools.
// ---------------------------------------------------------------------------

#[test]
#[ignore = "spike #679: builds several wasm32-wasip2 guests"]
fn n2_peak_memory_grid() {
    // ~500 B of assistant text per turn: a short-but-real agent reply.
    const TEXT_BYTES: usize = 500;
    const SCHEMA_PAD: usize = 256;
    let tool_counts = [1usize, 4, 16];
    let turn_counts = [1usize, 2, 4, 8, 16, 32];

    println!("\n=== N2: peak guest linear memory (bytes) ===");
    println!("text/turn = {TEXT_BYTES} B, schema pad = {SCHEMA_PAD} B/tool");

    for &tools in &tool_counts {
        let ir = spike_ir_bytes(tools, SCHEMA_PAD, 64);
        let wasm = build_guest_component(Some(&ir));
        let initial = declared_initial_memory_bytes(&wasm).unwrap_or(0);
        println!(
            "\n--- {tools} tool(s); component {} B; IR {} B; declared initial memory {initial} B ({:.1} KiB) ---",
            wasm.len(),
            ir.len(),
            initial as f64 / 1024.0
        );
        println!("| turns | peak linear mem B | peak KiB | complete calls | last req JSON B | total req JSON B | total event JSON B |");
        println!("|--:|--:|--:|--:|--:|--:|--:|");
        for &turns in &turn_counts {
            let (out, p) = run_with_probe(&wasm, "start", cassette(turns, TEXT_BYTES));
            if let Err(e) = &out {
                println!("| {turns} | RUN ERROR: {e} | | | | | |");
                continue;
            }
            let last_req = p.request_bytes.last().copied().unwrap_or(0);
            let total_req: usize = p.request_bytes.iter().sum();
            let total_ev: usize = p.event_bytes.iter().sum();
            let peak = p.peak_memory.max(initial);
            println!(
                "| {turns} | {peak} | {:.1} | {} | {last_req} | {total_req} | {total_ev} |",
                peak as f64 / 1024.0,
                p.request_bytes.len()
            );
        }
    }

    // Second sweep: hold the shape fixed and vary per-turn CONTENT size. This
    // is what separates "JSON is a constant-factor tax" from "the history is
    // inherently too big" — if peak tracks payload with a slope near 1, the
    // data is the problem, not the encoding.
    println!("\n=== N2b: peak memory vs per-turn content size (4 tools, 8 turns) ===");
    let ir = spike_ir_bytes(4, SCHEMA_PAD, 64);
    let wasm = build_guest_component(Some(&ir));
    let initial = declared_initial_memory_bytes(&wasm).unwrap_or(0);
    println!("declared initial memory {initial} B");
    println!("| text B/turn | peak linear mem B | peak KiB | last req JSON B | total req JSON B |");
    println!("|--:|--:|--:|--:|--:|");
    for &text in &[100usize, 500, 2000, 8000] {
        let (out, p) = run_with_probe(&wasm, "start", cassette(8, text));
        if let Err(e) = &out {
            println!("| {text} | RUN ERROR: {e} | | | |");
            continue;
        }
        let peak = p.peak_memory.max(initial);
        let last_req = p.request_bytes.last().copied().unwrap_or(0);
        let total_req: usize = p.request_bytes.iter().sum();
        println!(
            "| {text} | {peak} | {:.1} | {last_req} | {total_req} |",
            peak as f64 / 1024.0
        );
    }
}
