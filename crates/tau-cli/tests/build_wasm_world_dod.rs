//! EPIC 3.2 DoD: the wasm guest is compiled against a capability-EXACT WIT
//! world — an ungranted capability's WASI interface is absent from the world
//! *text* fed to (and accepted by) the guest's `wit_bindgen::generate!`, not
//! just declared but unused. Builds the wasm guest, so it is #[ignore]d like
//! the other guest-build tests (run with --run-ignored).
//!
//! Two fixtures, built sequentially in one test (not two parallel tests) since
//! the guest build shares `target/tau-build-wasm`:
//!   - `net-http`: grants `net.http` only → world text has `wasi:http`, no `wasi:filesystem`.
//!   - `trivial`:  host-only, no caps    → world text has no `wasi:` at all.
//!
//! The primary assertions are on the cap-derived WORLD TEXT
//! (`wasm_world_for_project`'s output, the same bytes written to
//! `TAU_WORLD_WIT` and consumed by `wit_bindgen::generate!` in the guest) —
//! that is the layer EPIC 3.2's guarantee (A3) actually lives at, and tying
//! the assertion to a `guest build succeeded` fact (not just a unit test of
//! the generator in isolation) proves the world is both capability-exact AND
//! a compile-valid no_std bindgen world. The secondary `wit_component::decode`
//! checks on the *compiled component's* actual imports are kept as a 3.4
//! regression guard (see the comment in the test body below) — today
//! they're vacuous (wasm-ld DCE drops every WASI import, granted or not,
//! because no guest source calls WASI directly yet), but they'll become
//! meaningful once 3.4 wires the guest to call granted WASI interfaces.

use std::path::PathBuf;
use std::process::Command;

use tau_cli::cmd::build_wasm::{lower_to_wasm_ir, wasm_world_for_project};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/wasm-build")
        .join(name)
}

/// Build the guest for a fixture and return `(world_text, imported_interfaces)`:
/// the cap-derived WIT world text fed to `TAU_WORLD_WIT` (successfully
/// compiled against, since this only returns after `out.status.success()`),
/// and the component's imported interface package-ids (e.g.
/// "wasi:http/types@0.2.3") as actually decoded from the built wasm.
fn build_and_decode(fixture_name: &str) -> (String, Vec<String>) {
    let (_module, bytes) = lower_to_wasm_ir(&fixture(fixture_name)).unwrap();
    let world = wasm_world_for_project(&fixture(fixture_name)).unwrap();
    let ir = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(ir.path(), &bytes).unwrap();
    let wit = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(wit.path(), world.as_bytes()).unwrap();

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let out = Command::new(env!("CARGO"))
        .current_dir(&root)
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
        .env("CARGO_TARGET_DIR", root.join("target/tau-build-wasm"))
        .env("TAU_IR_BYTES", ir.path())
        .env("TAU_WORLD_WIT", wit.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "guest build failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    let wasm_path = stdout
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|m| m["reason"] == "compiler-artifact")
        .filter(|m| {
            m["target"]["name"]
                .as_str()
                .is_some_and(|n| n == "tau-wasm-guest" || n == "tau_wasm_guest")
        })
        .flat_map(|m| {
            m["filenames"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|f| f.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .find(|f| f.ends_with(".wasm"))
        .unwrap();
    let wasm = std::fs::read(&wasm_path).unwrap();
    // wit_component::decode of a component yields (Resolve, WorldId).
    let (resolve, world_id) = match wit_component::decode(&wasm).expect("decode component") {
        wit_component::DecodedWasm::Component(resolve, world) => (resolve, world),
        _ => panic!("expected a component, got a wit package"),
    };
    let imports: Vec<String> = resolve.worlds[world_id]
        .imports
        .keys()
        .filter_map(|k| match k {
            wit_parser::WorldKey::Interface(id) => resolve.id_of(*id),
            _ => None,
        })
        .collect();
    (world, imports)
}

#[test]
#[ignore = "builds the wasm32-wasip2 guest; run with --run-ignored"]
fn dod_guest_compiles_against_cap_exact_world() {
    // net-http grants net.http only.
    let (net_world, net_imports) = build_and_decode("net-http");
    // Primary DoD (A3): the world text fed to and accepted by the guest's
    // `wit_bindgen::generate!` is capability-exact, proven against a
    // successful no_std guest compile.
    assert!(
        net_world.contains("import wasi:http/outgoing-handler@0.2.3;"),
        "net granted → wasi:http in the compiled-against world:\n{net_world}"
    );
    assert!(
        !net_world.contains("wasi:filesystem"),
        "fs UNGRANTED → absent from the world the guest is compiled against (DoD):\n{net_world}"
    );

    // trivial grants nothing → no wasi in the world text at all.
    let (triv_world, triv_imports) = build_and_decode("trivial");
    assert!(
        !triv_world.contains("wasi:"),
        "no caps → no wasi in the compiled-against world:\n{triv_world}"
    );

    // Secondary, 3.4-forward regression guard: today these are vacuous
    // (wasm-ld's unreachable-import elimination drops every WASI import from
    // the compiled component, granted or not, because no guest source calls
    // a `wasi:http`/`wasi:filesystem` function directly yet — all host
    // interaction routes through the single `tau:host/host` import, see
    // `host_ports.rs`). Once 3.4 wires the guest to actually call granted
    // WASI interfaces, these start distinguishing "absent because ungranted"
    // from "absent because unused", so they're kept (not deleted) as a
    // forward-looking tripwire.
    assert!(
        !net_imports
            .iter()
            .any(|i| i.starts_with("wasi:filesystem/")),
        "fs UNGRANTED → wasi:filesystem must be absent from the compiled component's \
         actual imports (3.4-forward regression guard): {net_imports:?}"
    );
    assert!(
        !triv_imports.iter().any(|i| i.starts_with("wasi:")),
        "no caps → no wasi imports in the compiled component (3.4-forward regression \
         guard): {triv_imports:?}"
    );
}
