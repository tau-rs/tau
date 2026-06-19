# β.7.5 PR-E1 — no_std link foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the `tau-wasm-guest` `no_std` cdylib actually **link** `tau-runtime-core` for `wasm32-wasip2` with **zero `std`**, so PR-E2 can drive `run_ir` in-guest behind the locked 3-import determinism boundary.

**Architecture:** The guest is `#![no_std]` with its own `#[global_allocator]`/`#[panic_handler]`. Linking `tau-runtime-core` currently fails (`duplicate lang item 'panic_impl'`) because `serde`/`serde_json` (default `std`), `url`→`icu`, `serde_yaml`, `globset`, `jsonschema`, and `uuid/v4`→`getrandom` are pulled into the guest's dependency graph and drag in `std`. This PR cuts each std vector via Cargo feature surgery across `tau-domain`, `tau-ports`, `tau-ir`, `tau-runtime-core`, fixes the cargo workspace-inheritance quirk that silently drops `default-features = false`, and converts the CI "no-std guard" from a compile check into a real **std-free link** gate.

**Tech Stack:** Rust, `wasm32-wasip2`, cargo features/resolver-v2, `wit-bindgen` 0.58, `dlmalloc`.

## Global Constraints

- **CARGO RULES (CLAUDE.md):** every cargo command sets `CARGO_INCREMENTAL=0`, `CARGO_TARGET_DIR=target/agent-<role>` (use `target/agent-impl` for this work; `target/agent-wasm-guest` for the wasm link gate), `-p <crate>`, and a `timeout` (build 180s, test 300s, clippy 240s). Never bare `cargo`.
- **Host behavior unchanged.** All host-side crates (`tau-cli`, `tau-runtime-tokio`, `tau-pkg`, …) must build and test exactly as before. The std-only deps (`url`, `serde_yaml`, `globset`, `jsonschema`, `serde_json/std`) stay enabled on the host via feature unification; only the **guest's wasm subtree** loses them.
- **The link gate is the test.** The deliverable is verified by: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-wasm-guest cargo build -p tau-wasm-guest --target wasm32-wasip2 --release` producing a `.wasm` with no `std` link error.
- **No new default features on the host.** Every feature added here defaults ON for host members (so nothing regresses) and is the thing the guest opts OUT of.
- **Determinism boundary unchanged:** still exactly the 3 `tau:run/host` WIT imports. This PR adds no host imports.
- `rustup target add wasm32-wasip2` must be present (already installed this environment; CI installs it).

---

## File structure

- `crates/tau-wasm-guest/Cargo.toml` — add the (correctly-declared) `tau-runtime-core`/`tau-ir`/`serde_json` wasm deps that force the link.
- `crates/tau-wasm-guest/src/guest.rs` — temporary decode call to force monomorphization/link (reverted to a real-but-minimal decode at the end; full `run_ir` wiring is PR-E2).
- `crates/tau-domain/Cargo.toml` — `url` → optional behind new `package-source` feature; `serde_yaml` → split into new `skill-md` feature; `serde_json` and `base64` → alloc-only.
- `crates/tau-domain/src/package/source.rs`, `src/lib.rs`, `src/package/manifest.rs`, `src/package/skill.rs` — `#[cfg(feature = ...)]` gating of the `Url`-bearing and yaml-parsing code.
- `crates/tau-ports/Cargo.toml`, `crates/tau-ir/Cargo.toml` — `serde_json` → alloc-only.
- `crates/tau-runtime-core/Cargo.toml` — `globset` → optional behind existing `capability-override`; drop `uuid` `v4` feature; depend on `tau-domain` without `package-source`/`skill-md`.
- `.github/workflows/ci.yml` — the `runtime-core-no-std` job's guest step becomes the authoritative std-free **link** gate (already runs `--release`; add an assertion / comment that this is a *link* not *compile* gate, and keep `-p tau-wasm-guest`).

---

## Task 1: Establish the failing std-free link gate (TDD red)

**Files:**
- Modify: `crates/tau-wasm-guest/Cargo.toml:17-19`
- Modify: `crates/tau-wasm-guest/src/guest.rs:62-69`

**Interfaces:**
- Consumes: `tau_ir::from_canonical_bytes(&[u8]) -> Result<IrModule, serde_json::Error>` (re-exported at `tau_ir` crate root).
- Produces: a guest that references `tau-runtime-core` + `tau-ir`, so the wasm link exercises their full graph.

- [ ] **Step 1: Add the deps that force the link.** Declare them so `default-features = false` actually sticks (the workspace-inheritance form silently drops it — verified). Use the workspace form but ALSO set `default-features = false` explicitly; if a later link still shows `jsonschema`/default features, fall back to direct `path =` deps (proven to make `default-features = false` stick).

```toml
[target.'cfg(target_arch = "wasm32")'.dependencies]
wit-bindgen = { workspace = true }
dlmalloc    = { workspace = true }
tau-ir = { workspace = true, default-features = false }
tau-runtime-core = { workspace = true, default-features = false }
serde_json = { workspace = true, default-features = false, features = ["alloc"] }
```

- [ ] **Step 2: Reference the graph from `run`** (replace the hardcoded body):

```rust
impl Guest for Component {
    fn run(_prompt: String) -> Result<String, String> {
        // PR-E1: force the tau-runtime-core graph into the link.
        // Real run_ir wiring is PR-E2.
        match tau_ir::from_canonical_bytes(b"{}") {
            Ok(_) => Ok("{}".to_string()),
            Err(e) => Err(e.to_string()),
        }
    }
}
```

- [ ] **Step 3: Run the link gate; verify it FAILS.**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-wasm-guest cargo build -p tau-wasm-guest --target wasm32-wasip2 --release 2>&1 | tail -20`
Expected: `error[E0152]: found duplicate lang item 'panic_impl' ... first defined in 'std' (which 'serde' depends on)`.

- [ ] **Step 4: Commit (red checkpoint).**

```bash
git add crates/tau-wasm-guest
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "test(β.7.5): guest links tau-runtime-core — red (std leak)"
```

---

## Task 2: Cut `serde_json/std` — alloc-only in tau-ir, tau-ports, tau-domain

**Files:**
- Modify: `crates/tau-ir/Cargo.toml:15`
- Modify: `crates/tau-ports/Cargo.toml:32`
- Modify: `crates/tau-domain/Cargo.toml:27`

**Interfaces:**
- Consumes: nothing new.
- Produces: `serde_json` resolves to its `alloc` feature (no `std`) in the guest subtree; host keeps `std` via unification with `tau-cli`/etc.

- [ ] **Step 1: tau-ir** — change `serde_json = { workspace = true }` to:

```toml
serde_json = { workspace = true, default-features = false, features = ["alloc"] }
```

- [ ] **Step 2: tau-ports** — change `serde_json = "1"` to:

```toml
serde_json = { version = "1", default-features = false, features = ["alloc"] }
```

- [ ] **Step 3: tau-domain** — change `serde_json = { version = "1", features = ["float_roundtrip"] }` to:

```toml
serde_json = { version = "1", default-features = false, features = ["alloc", "float_roundtrip"] }
```

- [ ] **Step 4: Verify host crates still build** (feature unification restores `std`):

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-ir -p tau-ports -p tau-domain`
Expected: clean.

- [ ] **Step 5: Re-run the link gate; expect the next std-puller** (now `url`/`serde_yaml` via `tau-domain`, not `serde_json`).

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-wasm-guest cargo build -p tau-wasm-guest --target wasm32-wasip2 --release 2>&1 | tail -20`
Expected: still fails, but the error now blames `url`/`icu` or `serde_yaml` (std), not `serde_json`. (If it still blames `serde_json`, the workspace-inheritance quirk is re-enabling default features — switch the guest's deps to direct `path =` form per Task 1 Step 1.)

- [ ] **Step 6: Commit.**

```bash
git add crates/tau-ir/Cargo.toml crates/tau-ports/Cargo.toml crates/tau-domain/Cargo.toml
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "refactor(β.7.5): serde_json alloc-only in tau-ir/ports/domain (no_std link)"
```

---

## Task 3: Cut `url`, `serde_yaml`, and std-`base64` from tau-domain's no_std path

`url` (→`idna`/`icu`, std-only) is used ONLY by `PackageSource::Git(url::Url)` (`package/source.rs:111`) which the interpreter never reads (it only calls `PackageManifest::capabilities()`; the baked IR contains no manifest). `serde_yaml` is used ONLY by `parse_skill_md` (`package/skill.rs:239`), host-side skill loading. `base64` is needed for `Value::Bytes` (reachable) but is no_std-capable.

**Files:**
- Modify: `crates/tau-domain/Cargo.toml` (`[dependencies]` + `[features]`)
- Modify: `crates/tau-domain/src/lib.rs:44` (`pub use url::Url;`)
- Modify: `crates/tau-domain/src/package/source.rs` (the `Url` variant + parse)
- Modify: `crates/tau-domain/src/package/manifest.rs` (the `source` field)
- Modify: `crates/tau-domain/src/package/skill.rs:200-249` (`parse_skill_md`)

**Interfaces:**
- Consumes: nothing new.
- Produces: tau-domain features `package-source` (gates `url` + `PackageSource::Git` + `PackageManifest.source`) and `skill-md` (gates `serde_yaml` + `parse_skill_md`), both default-ON for host; `base64` alloc-only. `tau-runtime-core` will depend on tau-domain WITHOUT these two features (Task 4).

- [ ] **Step 1: tau-domain Cargo.toml — make `url`/`serde_yaml` optional, `base64` alloc-only, add features.**

```toml
url        = { workspace = true, optional = true }
base64     = { workspace = true, optional = true, default-features = false, features = ["alloc"] }
serde_yaml = { version = "0.9", optional = true }
```

```toml
[features]
# core serde derives on the runtime vocabulary (no_std-safe). Pulls base64
# for Value::Bytes wire encoding (alloc-only) but NOT url/serde_yaml.
serde         = ["dep:serde", "dep:base64", "uuid/serde", "semver/serde"]
# host-only: PackageSource::Git embeds url::Url (std-only via idna/icu).
package-source = ["dep:url", "url/serde"]
# host-only: SKILL.md YAML frontmatter parsing (serde_yaml is std-only).
skill-md       = ["dep:serde_yaml", "serde"]
```

(Adjust the exact prior `serde` feature contents from `crates/tau-domain/Cargo.toml:22` — drop `dep:serde_yaml`, `url/serde` from `serde`; move them to the new features. Keep `uuid/serde`, `semver/serde`.)

- [ ] **Step 2: Add `package-source` + `skill-md` to the host default feature set** so host members are unchanged. If tau-domain has no `default`, add:

```toml
default = ["package-source", "skill-md"]
```
Then ensure `tau-runtime-core` and other wasm-subtree crates pull tau-domain with `default-features = false` (Task 4). Host crates that need package-source/skill-md (`tau-pkg`, `tau-cli`) already get them via default; verify after.

- [ ] **Step 3: Gate `url` in source.rs.** Wrap the `Url(url::Url)` variant and its parse arm:

```rust
#[cfg(feature = "package-source")]
Url(url::Url),
```
and the `url::Url::parse` arm at `source.rs:163` in `#[cfg(feature = "package-source")]`, with a `#[cfg(not(feature = "package-source"))]` fallback that rejects/ignores git-url sources (the guest never constructs these).

- [ ] **Step 4: Gate the re-export** at `lib.rs:44`:

```rust
#[cfg(feature = "package-source")]
pub use url::Url;
```

- [ ] **Step 5: Gate `PackageManifest.source`.** If `manifest.rs` has a mandatory `source: PackageSource` field, make it `#[cfg(feature = "package-source")]` with a `#[cfg(not)]` builder default, OR confirm `PackageSource` itself compiles without the `Url` variant (other variants like `Path`/`Registry` remain) so the field stays but cannot hold a `Url` in no_std. Prefer the latter (smaller serde-shape impact). Verify `PackageManifest::capabilities()` is unaffected.

- [ ] **Step 6: Gate `parse_skill_md`** at `skill.rs:200` behind `#[cfg(feature = "skill-md")]` (and its `serde_yaml` use). Host keeps it via default.

- [ ] **Step 7: Verify host builds + tests unchanged.**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-domain`
Expected: all pass (skill-md + package-source tests still run under default features).

- [ ] **Step 8: Verify tau-domain builds no_std-clean for wasm with the reduced feature set:**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-wasm-guest cargo build -p tau-domain --target wasm32-wasip2 --no-default-features --features serde`
Expected: builds without `url`/`serde_yaml`/`icu` in the graph.

- [ ] **Step 9: Commit.**

```bash
git add crates/tau-domain
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "refactor(β.7.5): gate url/serde_yaml behind host features; base64 alloc-only"
```

---

## Task 4: Cut `globset` + `uuid/v4`(getrandom) from tau-runtime-core; wire reduced tau-domain features

`globset` is a NON-optional dep used only by `skill_resolve::apply_scope_paths` under the `capability-override` feature — make it optional so `--no-default-features` drops it. `uuid_v4` (`ids.rs:31`) builds via `Uuid::from_bytes(bytes)` from the injected `RandomSource` and never calls `Uuid::new_v4()`, so the `v4` feature (→`getrandom`) is unnecessary.

**Files:**
- Modify: `crates/tau-runtime-core/Cargo.toml:12,23,31,39-49`

**Interfaces:**
- Consumes: tau-domain `serde` feature only (no `package-source`/`skill-md`).
- Produces: `tau-runtime-core --no-default-features` has no `globset`/`jsonschema`/`getrandom`/`url`/`serde_yaml`/`serde_json-std` in its graph.

- [ ] **Step 1: `globset` → optional, gated by `capability-override`:**

```toml
globset = { workspace = true, optional = true }
```
```toml
capability-override = ["dep:globset"]
```

- [ ] **Step 2: Drop the `v4` feature from `uuid`** (verify no `Uuid::new_v4()` call remains: `grep -rn "new_v4" crates/tau-runtime-core/src`):

```toml
uuid = { workspace = true }
```
(`Uuid::from_bytes` needs no feature.) If `grep` finds a real `new_v4()`, refactor it to route through `ids::uuid_v4`.

- [ ] **Step 3: tau-domain dep — drop the host-only features for core:**

```toml
tau-domain = { workspace = true, default-features = false, features = ["serde"] }
```
(Already `default-features = false, features = ["serde"]` at line 12 — confirm it does NOT add `package-source`/`skill-md`; with Task 3's split it won't.)

- [ ] **Step 4: Verify host runtime-core builds + tests with default features:**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core`
Expected: all pass (capability-override on by default → globset present).

- [ ] **Step 5: Re-run the link gate. Resolve residual std-pullers iteratively** (DISCOVERY step — re-link, read the error, gate the named crate). Known candidates and their fixes:
  - `getrandom` still present → check `cargo tree -p tau-wasm-guest --target wasm32-wasip2 -i getrandom@0.3.4`; trace the puller (likely `uuid` or `ulid`) and drop its rng feature.
  - `async-stream` link issue → verify it builds no_std; if not, it's used only by `stream::run_streaming_inner` — already in the wasip2 graph, should link with alloc.
  - `chrono` → already `default-features = false, features = ["alloc","serde"]`, no_std-clean.

Run after each fix: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-wasm-guest cargo build -p tau-wasm-guest --target wasm32-wasip2 --release 2>&1 | tail -25`
Expected (final): **a `.wasm` is produced, no link error.**

- [ ] **Step 6: Assert it is a real component** (catches the `_rdl_*` LTO/cabi_realloc class of bug under `--release`):

Run: `wasm-tools component wit target/agent-wasm-guest/wasm32-wasip2/release/tau_wasm_guest.wasm | grep -i "tau:run"`
Expected: lists the `tau:run` world.

- [ ] **Step 7: Commit.**

```bash
git add crates/tau-runtime-core/Cargo.toml
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "refactor(β.7.5): globset optional + drop uuid/v4 — guest links std-free"
```

---

## Task 5: Lock the link gate in CI; restore guest to a clean minimal decode

**Files:**
- Modify: `.github/workflows/ci.yml:365-377` (the `runtime-core-no-std` job)
- Modify: `crates/tau-wasm-guest/src/guest.rs` (keep the real `from_canonical_bytes(b"{}")` decode — it's a genuine, minimal use of the now-linked graph; full `run_ir` is PR-E2)

**Interfaces:**
- Consumes: the green link from Task 4.
- Produces: a CI gate that fails if any future change reintroduces a `std` leak into the guest.

- [ ] **Step 1: Strengthen the CI comment + step** so the guest build is documented as the authoritative **std-free link** gate (the lib-compile step above it only proves compilation on a std target). The existing `cargo build -p tau-wasm-guest --target wasm32-wasip2 --release` step already IS the link gate — add a `wasm-tools component wit` assertion step mirroring Task 4 Step 6, and a comment noting that a `duplicate lang item panic_impl` here means a dep regained `std`.

- [ ] **Step 2: Full workspace check — nothing regressed on host:**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check --workspace` is FORBIDDEN by CARGO RULES (no `--workspace`). Instead check the touched host crates individually:
`for c in tau-domain tau-ports tau-ir tau-runtime-core tau-runtime-tokio tau-pkg tau-cli; do timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p $c || break; done`
Expected: all clean.

- [ ] **Step 3: Run the conformance + key suites that exercise runtime-core unchanged:**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core -p tau-ir-conformance -p tau-conformance`
Expected: green (golden unchanged; this PR touches no runtime logic).

- [ ] **Step 4: Commit + push + open PR.**

```bash
git add .github/workflows/ci.yml crates/tau-wasm-guest
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "ci(β.7.5): guest wasm build is the std-free link gate (PR-E1)"
git push -u origin bake-wasm-ir-pr-e
gh pr create --base main --title "feat(β.7.5): no_std link foundation — guest links tau-runtime-core std-free (PR-E1)" --body "<see PR body below>"
```

---

## Self-review notes

- **Spec coverage:** This PR is the prerequisite the spec's §4.2/§6.2 assumed was complete after PR-A but wasn't (PR-A delivered a no_std *compile* on a std target, not a std-free *link*). It implements the actual D1 "fully-linked guest" precondition. PR-E2 (separate plan) does §9 baking + in-guest `run_ir` + `tau-wasm-host::run_component` wiring (needs PR-D #367 merged first).
- **Discovery steps:** Task 4 Step 5 is explicitly iterative (re-link → read error → gate). This is unavoidable for dependency-graph surgery; every other step is concrete.
- **Risk:** the one real unknown is whether `getrandom`/`uuid`/`ulid` produce a clean wasip2 no_std link after dropping `v4`; if `ulid` pulls `getrandom` unconditionally, gate or swap it. The link gate surfaces this immediately.
- **Host-unchanged invariant** is verified by Task 3 Step 7, Task 4 Step 4, Task 5 Steps 2–3.
