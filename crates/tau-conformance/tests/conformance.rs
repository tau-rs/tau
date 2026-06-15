//! β.6 conformance gate tests.
//! (a) dev == golden — LIVE.
//! (b) dev == wasm — #[ignore] until β.7.5.

use std::path::{Path, PathBuf};

use tau_conformance::{
    differ,
    event::ConformanceEvent,
    profile::{DevProfile, Profile, WasmProfile},
    scenario::Scenario,
    CONFORMANCE_EVENT_VERSION,
};

fn golden_path(dir: &Path) -> PathBuf {
    dir.join("expected_events.json")
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Golden {
    version: u32,
    events: Vec<ConformanceEvent>,
}

#[tokio::test(flavor = "current_thread")]
async fn fan_monitor_dev_matches_golden() {
    let s = Scenario::load(Scenario::fixture_dir("fan_monitor")).expect("load fixture");
    let actual = DevProfile.run(&s).await.expect("dev profile runs");

    if std::env::var("TAU_CONFORMANCE_BLESS").is_ok() {
        let g = Golden {
            version: CONFORMANCE_EVENT_VERSION,
            events: actual.clone(),
        };
        std::fs::write(
            golden_path(&s.dir),
            serde_json::to_string_pretty(&g).unwrap(),
        )
        .unwrap();
        eprintln!("blessed {} events", actual.len());
        return;
    }

    let raw = std::fs::read_to_string(golden_path(&s.dir)).expect("golden exists (bless first)");
    let golden: Golden = serde_json::from_str(&raw).expect("golden parses");
    assert_eq!(
        golden.version, CONFORMANCE_EVENT_VERSION,
        "golden version stale — re-bless with TAU_CONFORMANCE_BLESS=1"
    );
    if let Some(d) = differ::diff(&golden.events, &actual) {
        panic!("dev profile diverged from golden:\n{}", d.report);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn dev_profile_is_deterministic() {
    let s = Scenario::load(Scenario::fixture_dir("fan_monitor")).expect("load");
    let a = DevProfile.run(&s).await.expect("run 1");
    let b = DevProfile.run(&s).await.expect("run 2");
    assert!(
        differ::diff(&a, &b).is_none(),
        "dev profile is nondeterministic"
    );
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "TODO(β.7.5): WasmProfile needs `tau build wasm`; see ADR-0046"]
async fn fan_monitor_dev_matches_wasm() {
    let s = Scenario::load(Scenario::fixture_dir("fan_monitor")).expect("load");
    let dev = DevProfile.run(&s).await.expect("dev runs");
    let wasm = WasmProfile.run(&s).await.expect("wasm runs"); // unimplemented! until β.7.5
    if let Some(d) = differ::diff(&dev, &wasm) {
        panic!("dev vs wasm divergence:\n{}", d.report);
    }
}
