use std::path::PathBuf;

fn guest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Every vendored WASI package must be pinned to WASI_VERSION so the closure
/// table (wit_world.rs) and the vendored .wit cannot drift apart.
#[test]
fn vendored_wasi_versions_match_pin() {
    let pin = format!("@{}", tau_ports::target::wasi_map::WASI_VERSION); // "@0.2.3"
    let deps = guest_dir().join("wit/deps");
    let mut checked = 0usize;
    for entry in walk_wit(&deps) {
        let text = std::fs::read_to_string(&entry).unwrap();
        for line in text.lines() {
            let l = line.trim_start();
            if l.starts_with("package wasi:") {
                assert!(l.contains(&pin), "unpinned package in {}: {l}", entry.display());
                checked += 1;
            }
        }
    }
    assert!(checked >= 4, "expected >=4 vendored wasi packages, found {checked}");
}

/// The committed baseline MUST be byte-identical to the empty-cap generator
/// output, so the fallback world CI compiles cannot drift from generate_world.
#[test]
fn baseline_equals_empty_generate_world() {
    let baseline = std::fs::read_to_string(guest_dir().join("wit-baseline/runner.wit"))
        .expect("baseline present");
    let generated = tau_ports::target::wit_world::generate_world(&[]).unwrap();
    assert_eq!(baseline, generated);
}

fn walk_wit(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() { out.extend(walk_wit(&p)); }
        else if p.extension().is_some_and(|x| x == "wit") { out.push(p); }
    }
    out
}
