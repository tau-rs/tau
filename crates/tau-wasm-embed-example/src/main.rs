//! tau-wasm-embed-example — EPIC 7.2 Variant A reference host.
//!
//! A "product" that embeds tau as a *component*: it loads a workflow built
//! with `tau build --target wasm` from disk (workflow-as-data — the product
//! binary never changes when the workflow does), supplies the four host
//! ports via [`EmbedPorts`], and prints every `RunEvent` live as a JSON
//! line. Offline out of the box: the LLM port answers with a canned reply.
#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tau_runtime_core::stream::RunEvent;
use tau_wasm_host::embed::{
    run_component_with_ports, CompletionRequest, CompletionResponse, EmbedPorts, StopReason,
};

const USAGE: &str = "usage: tau-wasm-embed-example <component.wasm> [prompt]";

/// The product's port surface: echo LLM, real wall clock, clock-seeded
/// entropy, and a live event sink. A real product supplies its inference
/// client (credentials stay host-side) and OS entropy here.
struct ProductPorts {
    entropy: AtomicU64,
    events: Arc<AtomicUsize>,
    completed: Arc<AtomicBool>,
}

fn wall_clock_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl EmbedPorts for ProductPorts {
    fn complete(&mut self, _req: CompletionRequest) -> Result<CompletionResponse, String> {
        Ok(CompletionResponse::new(
            "tau-wasm-embed-example reply".to_string(),
            Vec::new(),
            StopReason::EndTurn,
            None,
        ))
    }

    fn now_millis(&mut self) -> u64 {
        wall_clock_millis()
    }

    fn next_u64(&mut self) -> u64 {
        // xorshift64* — NOT cryptographic; a real product supplies OS entropy.
        let mut x = self.entropy.load(Ordering::Relaxed);
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.entropy.store(x, Ordering::Relaxed);
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn on_event(&mut self, event_json: &str) {
        // Prove the typed contract: every line is a deserializable RunEvent
        // (via the product's own tau-runtime-core dep — exactly how a real
        // product consumes the stream).
        match serde_json::from_str::<RunEvent>(event_json) {
            Ok(event) => {
                if matches!(event, RunEvent::RunCompleted { .. }) {
                    self.completed.store(true, Ordering::Relaxed);
                }
                self.events.fetch_add(1, Ordering::Relaxed);
                println!("{event_json}");
            }
            Err(err) => eprintln!("unparseable RunEvent ({err}): {event_json}"),
        }
    }
}

fn parse_args(args: Vec<String>) -> Result<(PathBuf, String), String> {
    let mut it = args.into_iter();
    let component = it.next().ok_or(USAGE)?;
    let prompt = it
        .next()
        .unwrap_or_else(|| "hello from the product".to_string());
    if it.next().is_some() {
        return Err(USAGE.to_string());
    }
    Ok((PathBuf::from(component), prompt))
}

fn main() -> ExitCode {
    let (component, prompt) = match parse_args(std::env::args().skip(1).collect()) {
        Ok(parsed) => parsed,
        Err(usage) => {
            eprintln!("{usage}");
            return ExitCode::from(2);
        }
    };
    let bytes = match std::fs::read(&component) {
        Ok(bytes) => bytes,
        Err(err) => {
            eprintln!("failed to read {}: {err}", component.display());
            return ExitCode::FAILURE;
        }
    };

    let events = Arc::new(AtomicUsize::new(0));
    let completed = Arc::new(AtomicBool::new(false));
    let ports = ProductPorts {
        entropy: AtomicU64::new(wall_clock_millis() | 1),
        events: Arc::clone(&events),
        completed: Arc::clone(&completed),
    };

    // No capabilities granted: the workflow gets no fs/net, whatever it asks
    // for. A real product passes the caps its governance approved.
    match run_component_with_ports(&bytes, &prompt, Box::new(ports), &[], Path::new(".")) {
        Ok(_sentinel) => {
            let seen = events.load(Ordering::Relaxed);
            if completed.load(Ordering::Relaxed) {
                println!("run completed: {seen} events");
                ExitCode::SUCCESS
            } else {
                eprintln!("run ended without RunCompleted ({seen} events)");
                ExitCode::FAILURE
            }
        }
        Err(err) => {
            eprintln!("embedding failed: {err}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_args;

    #[test]
    fn component_path_is_required() {
        assert!(parse_args(vec![]).is_err());
    }

    #[test]
    fn prompt_defaults_when_omitted() {
        let (path, prompt) = parse_args(vec!["wf.wasm".to_string()]).unwrap();
        assert_eq!(path.to_str(), Some("wf.wasm"));
        assert_eq!(prompt, "hello from the product");
    }

    #[test]
    fn extra_args_are_rejected() {
        let args = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        assert!(parse_args(args).is_err());
    }
}
