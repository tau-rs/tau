# Install Diagnostics → `tracing` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Route the install/build path's human-facing lifecycle messages and capability advisories through structured `tracing` (target `tau_pkg::install`, package fields) instead of `eprintln!`, so they honor `RUST_LOG`, JSON sinks, and structured capture — matching the runtime's plugin-host pattern.

**Architecture:** Replace three `eprintln!` call sites in `crates/tau-pkg/src/install.rs` with `tracing` macros: the build-start lifecycle message → `info!`, the two capability advisories (`warn_unknown_kind`, `warn_non_namespaced_custom_capabilities`) → `warn!`. All carry `target: "tau_pkg::install"` and a `package` field (plus `version`/`kind`/`capability` as relevant). The raw cargo stdout/stderr streaming (`eprint!` at lines 652/656) stays untouched — build output must keep streaming, not be buried inside structured events. A capturing `tracing_subscriber::Layer` in unit tests asserts target, level, fields, and `RUST_LOG`-style `EnvFilter` honoring.

**Tech Stack:** Rust, `tracing`, `tracing-subscriber` (dev-dependency, `env-filter` + `registry` features), `toml` (manifest construction in tests).

---

## Background / context for the implementer

- **The finding (audit O2, design facet D10):** install/build writes to stderr via `eprintln!`; runtime uses structured `tracing`. The two cannot be filtered/correlated/captured uniformly — `RUST_LOG` controls one and not the other. Security-relevant capability advisories are among the `eprintln!` lines, so they cannot be elevated, suppressed, or shipped to a structured sink.
- **Reference pattern to follow:** `crates/tau-runtime-tokio/src/plugin_host/process.rs:201-279`, e.g.:
  ```rust
  tracing::debug!(
      target: "tau_runtime_tokio::plugin_host",
      plugin = plugin_name.as_str(),
      binary_path = ?binary_path,
      "plugin.spawning"
  );
  ```
  Use a crate-scoped `target:` string + structured fields + a terse static message.
- **Capturing-subscriber test template:** `crates/tau-runtime-tokio/tests/tracing_emission.rs` (a `Layer` recording events into an `Arc<Mutex<Vec<…>>>`, installed via `tracing_subscriber::registry().with(layer).set_default()`). We adapt a smaller version that also records `target`, `level`, and the `package` field.
- **`tracing` is already a normal dependency** of `tau-pkg` (`crates/tau-pkg/Cargo.toml:28`). Only the **dev-dependency** `tracing-subscriber` needs adding.
- **The three call sites (current):**
  - `crates/tau-pkg/src/install.rs:628-633` — build-start `eprintln!` inside `build_rust_cargo_plugin`.
  - `crates/tau-pkg/src/install.rs:731-735` — `warn_unknown_kind` `eprintln!`.
  - `crates/tau-pkg/src/install.rs:745-751` — `warn_non_namespaced_custom_capabilities` `eprintln!`.
- **KEEP UNCHANGED:** `crates/tau-pkg/src/install.rs:652` and `:656` (`eprint!("{s}")` streaming raw cargo stdout/stderr). The brief is explicit: raw cargo output keeps streaming.
- **No behavior change:** only the diagnostic channel moves. Same conditions, same data, no new errors, no removed warnings.
- **Cargo rules (CLAUDE.md):** every cargo invocation is `timeout <n> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo <cmd> -p tau-pkg`. Prefer `cargo nextest run` for tests, `cargo test --doc` for doctests. Never bare `cargo`.
- **Coordination:** sessions 41 (install/bundle) and 44 (cargo-target-dir) also touch `install.rs`. Our edits are localized to the three call sites + two function signatures (build path) + a new `#[cfg(test)]` block. Rebase on `origin/main` before pushing if those landed first.

## File structure

- **Modify:** `crates/tau-pkg/Cargo.toml` — add `tracing-subscriber` to `[dev-dependencies]`.
- **Modify:** `crates/tau-pkg/src/install.rs` — three call sites + thread package name/version into the build path + new `#[cfg(test)]` test module (or extend the existing `mod tests` at line 895).

---

## Task 1: Add `tracing-subscriber` dev-dependency

**Files:**
- Modify: `crates/tau-pkg/Cargo.toml` (the `[dev-dependencies]` table, currently around lines 24-32)

- [ ] **Step 1: Add the dev-dependency**

In `crates/tau-pkg/Cargo.toml`, under `[dev-dependencies]`, add:

```toml
# Capturing subscriber for the install-diagnostics tracing tests
# (asserts target/level/fields + RUST_LOG-style EnvFilter honoring).
tracing-subscriber = { workspace = true, features = ["env-filter", "registry"] }
```

(The workspace root pins `tracing-subscriber = { version = "0.3", features = ["env-filter"] }`; re-declaring `registry` here is additive and matches how `tau-workflow` / `tau-observe` opt into extra features.)

- [ ] **Step 2: Verify it resolves**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-pkg --tests`
Expected: compiles (no test added yet); confirms the feature set resolves.

- [ ] **Step 3: Commit**

```bash
git add crates/tau-pkg/Cargo.toml
git -c user.name="Test User" -c user.email="test@example.com" \
  commit -m "test(tau-pkg): add tracing-subscriber dev-dep for install-diagnostics tests"
```

---

## Task 2: Failing-first test — `warn_unknown_kind` emits a structured `warn!`

**Files:**
- Modify: `crates/tau-pkg/src/install.rs` — add a `#[cfg(test)]` module `diagnostics_tracing_tests` near the bottom of the file (after the existing `mod tests` at line 895, or as a sibling test module). Place a shared capture helper here that Tasks 3-4 reuse.

- [ ] **Step 1: Write the failing test (capture helper + first assertion)**

Append to `crates/tau-pkg/src/install.rs`:

```rust
#[cfg(test)]
mod diagnostics_tracing_tests {
    use std::sync::{Arc, Mutex};

    use tau_domain::{PackageManifest, UncheckedManifest};
    use tracing::field::{Field, Visit};
    use tracing::{Event, Level, Subscriber};
    use tracing_subscriber::layer::Context;
    use tracing_subscriber::prelude::*;
    use tracing_subscriber::{EnvFilter, Layer};

    /// One captured `tracing` event: its target, level, and the value of
    /// the `package` field (if present).
    #[derive(Clone, Debug)]
    struct Captured {
        target: String,
        level: Level,
        package: Option<String>,
        message: Option<String>,
    }

    #[derive(Default, Clone)]
    struct CaptureLayer(Arc<Mutex<Vec<Captured>>>);

    impl<S: Subscriber> Layer<S> for CaptureLayer {
        fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
            let mut v = FieldVisitor::default();
            event.record(&mut v);
            self.0
                .lock()
                .expect("capture mutex poisoned")
                .push(Captured {
                    target: event.metadata().target().to_string(),
                    level: *event.metadata().level(),
                    package: v.package,
                    message: v.message,
                });
        }
    }

    /// Extracts the `package` field and the event `message` (recorded by
    /// `tracing` under the reserved `message` field name). Both arrive via
    /// `record_str` or `record_debug` depending on the recorder; accept
    /// both and strip the `Debug` quotes.
    #[derive(Default)]
    struct FieldVisitor {
        package: Option<String>,
        message: Option<String>,
    }

    impl FieldVisitor {
        fn set(&mut self, name: &str, value: String) {
            match name {
                "package" => self.package = Some(value),
                "message" => self.message = Some(value),
                _ => {}
            }
        }
    }

    impl Visit for FieldVisitor {
        fn record_str(&mut self, field: &Field, value: &str) {
            self.set(field.name(), value.to_string());
        }
        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            let raw = format!("{value:?}");
            self.set(field.name(), raw.trim_matches('"').to_string());
        }
    }

    /// Parse + validate a manifest from a TOML body (mirrors the install
    /// pipeline's `read_manifest`, minus disk I/O).
    fn manifest_from_toml(body: &str) -> PackageManifest {
        toml::from_str::<UncheckedManifest>(body)
            .expect("test manifest TOML parses")
            .validate()
            .expect("test manifest validates")
    }

    /// Minimal valid manifest with an arbitrary `kind` and capability list
    /// spliced in. `kind` and `caps_toml` are caller-controlled so each
    /// test drives a specific warn path.
    fn manifest_toml(kind: &str, caps_toml: &str) -> String {
        format!(
            "name = \"acme-tool\"\n\
             version = \"1.2.3\"\n\
             description = \"a tool\"\n\
             authors = [\"Acme <acme@example.com>\"]\n\
             source = \"https://example.com/acme/tool.git\"\n\
             kind = \"{kind}\"\n\
             dependencies = []\n\
             {caps_toml}\n"
        )
    }

    #[test]
    fn warn_unknown_kind_emits_structured_warn() {
        let captured = CaptureLayer::default();
        let _guard = tracing_subscriber::registry()
            .with(captured.clone())
            .set_default();

        // `weird-kind` is not in the canonical kinds list → warn path fires.
        let manifest = manifest_from_toml(&manifest_toml("weird-kind", "capabilities = []"));
        super::warn_unknown_kind(&manifest);

        let events = captured.0.lock().expect("capture mutex poisoned").clone();
        let warn = events
            .iter()
            .find(|e| e.level == Level::WARN && e.target == "tau_pkg::install")
            .unwrap_or_else(|| panic!("no WARN @ tau_pkg::install; captured = {events:?}"));
        assert_eq!(
            warn.package.as_deref(),
            Some("acme-tool"),
            "warn event must carry the package-name field; captured = {events:?}"
        );
    }
}
```

- [ ] **Step 2: Run the test to verify it FAILS**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg diagnostics_tracing_tests::warn_unknown_kind_emits_structured_warn`
Expected: FAIL — the assertion panics with "no WARN @ tau_pkg::install" because `warn_unknown_kind` currently uses `eprintln!`, which emits no `tracing` event.

(If it fails to compile because `warn_unknown_kind` is private — it is in the same file, so `super::warn_unknown_kind` resolves. If `validate()` rejects an empty/odd field, adjust the TOML body to match `read_manifest`'s minimal accepted shape from the `read_manifest` doctest.)

---

## Task 3: Make Task 2 pass — convert `warn_unknown_kind` to `warn!`

**Files:**
- Modify: `crates/tau-pkg/src/install.rs:712-737` (`warn_unknown_kind`)

- [ ] **Step 1: Replace the `eprintln!` with `tracing::warn!`**

In `warn_unknown_kind`, replace the `eprintln!` block (lines 730-736):

```rust
    if !known_kinds.contains(&kind_str) {
        eprintln!(
            "warning: package {} has unknown kind {:?}; tau-runtime will treat it as opaque",
            manifest.name(),
            kind_str,
        );
    }
```

with:

```rust
    if !known_kinds.contains(&kind_str) {
        tracing::warn!(
            target: "tau_pkg::install",
            package = %manifest.name(),
            kind = kind_str,
            "package declares unknown kind; tau-runtime will treat it as opaque",
        );
    }
```

Also update the function's doc comment first line (line 705) from `/// Warn (eprintln) if …` to `/// Warn (via `tracing`) if …` for accuracy.

- [ ] **Step 2: Run the test to verify it PASSES**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg diagnostics_tracing_tests::warn_unknown_kind_emits_structured_warn`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/tau-pkg/src/install.rs
git -c user.name="Test User" -c user.email="test@example.com" \
  commit -m "fix(tau-pkg): route unknown-kind advisory through tracing::warn (O2/D10)"
```

---

## Task 4: Test + convert `warn_non_namespaced_custom_capabilities`

**Files:**
- Modify: `crates/tau-pkg/src/install.rs` — add a test to `diagnostics_tracing_tests`
- Modify: `crates/tau-pkg/src/install.rs:739-754` (`warn_non_namespaced_custom_capabilities`)

- [ ] **Step 1: Write the failing test**

Add to the `diagnostics_tracing_tests` module:

```rust
    #[test]
    fn warn_non_namespaced_capability_emits_structured_warn() {
        let captured = CaptureLayer::default();
        let _guard = tracing_subscriber::registry()
            .with(captured.clone())
            .set_default();

        // A capability whose `kind` has no dot deserializes to
        // `Capability::Custom { name: "mytool" }` → warn path fires.
        let caps = "[[capabilities]]\nkind = \"mytool\"";
        let manifest = manifest_from_toml(&manifest_toml("tool", caps));
        super::warn_non_namespaced_custom_capabilities(&manifest);

        let events = captured.0.lock().expect("capture mutex poisoned").clone();
        let warn = events
            .iter()
            .find(|e| e.level == Level::WARN && e.target == "tau_pkg::install")
            .unwrap_or_else(|| panic!("no WARN @ tau_pkg::install; captured = {events:?}"));
        assert_eq!(
            warn.package.as_deref(),
            Some("acme-tool"),
            "warn event must carry the package-name field; captured = {events:?}"
        );
    }
```

- [ ] **Step 2: Run to verify it FAILS**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg diagnostics_tracing_tests::warn_non_namespaced_capability_emits_structured_warn`
Expected: FAIL — "no WARN @ tau_pkg::install" (still `eprintln!`).

- [ ] **Step 3: Replace the `eprintln!` with `tracing::warn!`**

In `warn_non_namespaced_custom_capabilities`, replace the `eprintln!` block (lines 745-751):

```rust
            if !name.contains('.') {
                eprintln!(
                    "warning: package {} declares Capability::Custom {{ name = {:?} }} \
                     without a dot-namespaced name; consider e.g. \"vendor.feature.action\"",
                    manifest.name(),
                    name,
                );
            }
```

with:

```rust
            if !name.contains('.') {
                tracing::warn!(
                    target: "tau_pkg::install",
                    package = %manifest.name(),
                    capability = %name,
                    "Capability::Custom name is not dot-namespaced; \
                     consider e.g. \"vendor.feature.action\"",
                );
            }
```

Also update the function's doc comment first line (line 739) from `/// Warn (eprintln) on …` to `/// Warn (via `tracing`) on …`.

- [ ] **Step 4: Run to verify it PASSES**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg diagnostics_tracing_tests::warn_non_namespaced_capability_emits_structured_warn`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-pkg/src/install.rs
git -c user.name="Test User" -c user.email="test@example.com" \
  commit -m "fix(tau-pkg): route non-namespaced-capability advisory through tracing::warn (O2/D10)"
```

---

## Task 5: Test `RUST_LOG`-style filtering honors the warn level

**Files:**
- Modify: `crates/tau-pkg/src/install.rs` — add a test to `diagnostics_tracing_tests`

This is the deterministic proof the brief asks for: the advisory now honors `RUST_LOG`. `EnvFilter::new("error")` emulates `RUST_LOG=error`; a `warn!` event must be suppressed under it and present under `warn`.

- [ ] **Step 1: Write the test**

Add to `diagnostics_tracing_tests`:

```rust
    #[test]
    fn warn_paths_honor_rust_log_filtering() {
        // Below threshold: RUST_LOG=error suppresses the warn event.
        {
            let captured = CaptureLayer::default();
            let _guard = tracing_subscriber::registry()
                .with(EnvFilter::new("error"))
                .with(captured.clone())
                .set_default();
            let manifest = manifest_from_toml(&manifest_toml("weird-kind", "capabilities = []"));
            super::warn_unknown_kind(&manifest);
            let events = captured.0.lock().expect("capture mutex poisoned").clone();
            assert!(
                !events
                    .iter()
                    .any(|e| e.target == "tau_pkg::install" && e.level == Level::WARN),
                "RUST_LOG=error must suppress the warn advisory; captured = {events:?}"
            );
        }
        // At threshold: RUST_LOG=warn lets it through.
        {
            let captured = CaptureLayer::default();
            let _guard = tracing_subscriber::registry()
                .with(EnvFilter::new("warn"))
                .with(captured.clone())
                .set_default();
            let manifest = manifest_from_toml(&manifest_toml("weird-kind", "capabilities = []"));
            super::warn_unknown_kind(&manifest);
            let events = captured.0.lock().expect("capture mutex poisoned").clone();
            assert!(
                events
                    .iter()
                    .any(|e| e.target == "tau_pkg::install" && e.level == Level::WARN),
                "RUST_LOG=warn must admit the warn advisory; captured = {events:?}"
            );
        }
    }
```

- [ ] **Step 2: Run to verify it PASSES** (it should — the warn path already emits via tracing after Task 3)

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg diagnostics_tracing_tests::warn_paths_honor_rust_log_filtering`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/tau-pkg/src/install.rs
git -c user.name="Test User" -c user.email="test@example.com" \
  commit -m "test(tau-pkg): assert install advisories honor RUST_LOG filtering (O2)"
```

---

## Task 6: Convert the build-start lifecycle message to `tracing::info!`

The build-start `eprintln!` is a lifecycle message, not raw cargo output, so it moves to `tracing`. It needs the package name/version, which `build_rust_cargo_plugin` does not currently receive — thread them in from `build_plugin_if_needed`, which holds the full `manifest`.

**Files:**
- Modify: `crates/tau-pkg/src/install.rs:584-585` (call to `build_rust_cargo_plugin` inside `build_plugin_if_needed`)
- Modify: `crates/tau-pkg/src/install.rs:598-633` (`build_rust_cargo_plugin` signature + the `eprintln!`)

- [ ] **Step 1: Add `PackageName`/`Version` to the existing `use` line**

`crates/tau-pkg/src/install.rs:35` currently:

```rust
use tau_domain::{kinds, Capability, PackageName, PackageSource, PluginKind, Version};
```

`PackageName` and `Version` are already imported. No change needed — confirm they're present (they are used elsewhere in the file). Skip if already imported.

- [ ] **Step 2: Thread name/version into the build path**

In `build_plugin_if_needed` (line 584-585), change the `RustCargo` arm:

```rust
        PluginKind::RustCargo => build_rust_cargo_plugin(plugin_manifest, package_dir, options),
```

to:

```rust
        PluginKind::RustCargo => build_rust_cargo_plugin(
            plugin_manifest,
            manifest.name(),
            manifest.version(),
            package_dir,
            options,
        ),
```

Change the `build_rust_cargo_plugin` signature (line 598-602):

```rust
fn build_rust_cargo_plugin(
    plugin_manifest: &tau_domain::PluginManifest,
    package_dir: &Path,
    options: &BuildOptions,
) -> Result<Option<LockedPlugin>, InstallError> {
```

to:

```rust
fn build_rust_cargo_plugin(
    plugin_manifest: &tau_domain::PluginManifest,
    package_name: &PackageName,
    package_version: &Version,
    package_dir: &Path,
    options: &BuildOptions,
) -> Result<Option<LockedPlugin>, InstallError> {
```

- [ ] **Step 3: Replace the build-start `eprintln!` with `tracing::info!`**

Replace lines 628-633:

```rust
    eprintln!(
        "  building {bin} ({kind}) in {dir}...",
        bin = plugin_manifest.bin,
        kind = plugin_manifest.kind,
        dir = package_dir.display(),
    );
```

with:

```rust
    tracing::info!(
        target: "tau_pkg::install",
        package = %package_name,
        version = %package_version,
        bin = %plugin_manifest.bin,
        kind = %plugin_manifest.kind,
        dir = %package_dir.display(),
        "building plugin binary",
    );
```

**Do NOT touch lines 652 / 656** (`eprint!("{s}")` for raw cargo stdout/stderr) — those keep streaming per the brief.

- [ ] **Step 4: Verify the crate compiles and existing tests pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg`
Expected: PASS (all existing + new tests). No unused-import or signature-mismatch errors.

- [ ] **Step 5: Clippy + fmt**

Run: `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-pkg --all-targets`
Expected: no warnings.

Run: `timeout 30 env CARGO_TARGET_DIR=target/agent-impl cargo fmt -p tau-pkg`
Expected: clean (reformats if needed).

- [ ] **Step 6: Commit**

```bash
git add crates/tau-pkg/src/install.rs
git -c user.name="Test User" -c user.email="test@example.com" \
  commit -m "fix(tau-pkg): route build-start lifecycle message through tracing::info (O2/D10)"
```

---

## Task 7: Verification — capture real before/after log output

The brief requires evidence: show the advisory emitted as a structured `warn!` with the `tau_pkg::install` target and a package field, honoring `RUST_LOG`. The unit tests in Tasks 2-5 are the captured-output proof. Additionally, demonstrate the runtime behavior with a tiny throwaway harness OR rely on the test output as evidence.

- [ ] **Step 1: Run the full diagnostics test module and capture output**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg diagnostics_tracing_tests --no-capture`
Expected: 3 tests pass. Paste the run summary into the PR as the "after" evidence.

- [ ] **Step 2: Confirm no stray `eprintln!`/`println!` remain in the converted paths**

Run: `grep -n "eprintln!\|println!" crates/tau-pkg/src/install.rs`
Expected: only the doc-comment `println!` at line ~173 and NO `eprintln!` (the two `eprint!` raw-cargo-stream lines are intentionally retained — confirm they are `eprint!` not `eprintln!`).

Run: `grep -n "eprint!" crates/tau-pkg/src/install.rs`
Expected: exactly the two raw-cargo-output lines (~652, ~656).

- [ ] **Step 3: Doctest sanity (no doc examples changed, but the crate's doctests must still pass)**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-pkg --doc`
Expected: PASS.

---

## Task 8: Code review + PR

- [ ] **Step 1: Self-review scope** — `git diff origin/main` touches only `crates/tau-pkg/Cargo.toml` and `crates/tau-pkg/src/install.rs`. No behavior change beyond the diagnostic channel. The two raw-cargo `eprint!` lines are untouched.

- [ ] **Step 2: Request code review** via the `superpowers:requesting-code-review` skill.

- [ ] **Step 3: Push + open PR**

```bash
git push -u origin <branch>
gh pr create -R tau-rs/tau --base main \
  --title "fix(tau-pkg): route install diagnostics through tracing (O2/D10)" \
  --body "<cite O2 finding + D10 design facet; before/after evidence from Task 7; note raw cargo output still streams>"
```

STOP — no merge.

---

## Self-review (run before handing off)

**1. Spec coverage:**
- Build-progress + cargo lifecycle message via `tracing` → Task 6. ✅
- Capability warnings via `warn!` honoring `RUST_LOG`/JSON/capture → Tasks 3, 4, 5. ✅
- Consistent target `tau_pkg::install` + package fields → all three call sites. ✅
- Raw cargo output keeps streaming → explicitly preserved (Tasks 6/7). ✅
- Failing-first unit test for a warn path → Tasks 2, 4. ✅
- `verification-before-completion` with real captured output → Task 7. ✅
- Code review + PR citing O2/D10 → Task 8. ✅

**2. Placeholder scan:** PR body has a `<…>` placeholder (intended — filled at PR time) and `<branch>` (the actual branch name). No code-step placeholders.

**3. Type consistency:** `warn_unknown_kind`/`warn_non_namespaced_custom_capabilities` keep their `&PackageManifest` signature (tested via `super::`). `build_rust_cargo_plugin` gains `package_name: &PackageName, package_version: &Version` — the single caller in `build_plugin_if_needed` is updated in the same task. `%` formatting requires `Display`; `PackageName`/`Version` already `Display` (printed with `{}` in the current `eprintln!`s). Capture-layer field names (`package`, `message`) match the emitted field key `package` and the reserved `message` field.
