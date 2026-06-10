//! Smoke test: tau-ts-extract crate is reachable and its entrypoint exists.

#[test]
fn extract_project_entrypoint_exists() {
    use std::path::Path;
    let src = "// empty TS file";
    let path = Path::new("/tmp/test.ts");
    let result = tau_ts_extract::extract_project(src, path);
    // Phase 1: any result (Ok or Err) means the symbol is reachable.
    let _ = result;
}
