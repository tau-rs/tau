# EPIC 5.6 — Browser capabilities profile + published wasm bundle-size number

Status: accepted (2026-08-21)
Slice: EPIC 5 story 5.6 (`docs/superpowers/plans/vision-roadmap.md:186`)
Depends on: 5.4 typed consumers (`@tau/embed-js`, `@tau/react`, `@tau/angular`,
`streaming-demo`) — merged (#547 / #552 / #554 / #560 / #562 / #579).

## Problem

A tau workflow compiles to a `wasm32-wasip2` component that the 5.4 SDKs run in
a browser via `jco`. Two questions have no published answer today:

1. **Which capabilities work in a browser host?** The capability model
   (`tau_domain::Capability` → `tau_ports::target::Disposition`) already knows
   how each capability maps to WASI, but no page tells an author what survives
   the browser's WASI-0.2 (`jco`) substrate versus the wasmtime host.
2. **How big is the bundle a browser downloads?** No number is measured or
   published anywhere; the roadmap names `wasm-metadce` as the intended tool.

This slice publishes both — a **browser capabilities profile** (a reference
table) and a **CI-measured bundle-size number** — with the measurement
reproducible locally and in CI (`local == CI == docs`).

## Non-goals

- No new target-registry entry, no `AdapterFamily`/`Disposition` code change.
  The profile is *derived* from the existing model, not a new enforcement
  surface. (A future `any-browser-strict` triple, if ever needed, is a separate
  ADR — flagged in "Future work".)
- No `tau-pkg` / `tau-ir` core edits (Lane-independent per the 5.x plan).
- No size *optimisation* work (size-tuned profile, `wasm-opt` in the build
  pipeline). We measure and publish; tuning is future work.

## Design

### 1. Browser capabilities profile

The profile is the existing per-capability `Disposition`
(`crates/tau-ports/src/target/wasi_map.rs:57`) *projected onto a browser host*.
Each capability gets a browser status:

- **Available** — works in the browser with no host-side loss.
- **Emulated** — works only through a browser shim whose semantics differ from
  the native/wasmtime host (documented divergence).
- **Forbidden** — cannot be expressed in a browser; build-time error on wasm.

| Capability | `Disposition` | Browser status | Rationale |
|---|---|---|---|
| `task_list`, `plan` | `InGuest` | Available | Enforced inside the guest; no host surface. |
| `agent.spawn`, `skill.spawn` | `InGuest` | Available | Recursion runs entirely in-guest. |
| `net.http` | `Wasi` | Emulated | `wasi:http` → browser `fetch`; CORS + no raw sockets + preflight; live calls need a JSPI (`jco --async-mode jspi`) build (the 5.4 demo uses a synchronous cassette). |
| `fs.read`, `fs.write` | `Wasi` | Emulated | `wasi:filesystem` → OPFS / in-memory; no host FS; preopen scoping differs from wasmtime. |
| `fs.exec` | `Unsupported` | Forbidden | No exec surface (already a build-time error on wasm). |
| `process.spawn` | `Unsupported` | Forbidden | No OS processes in a browser. |
| `custom.*` | `HostMediated` | Forbidden (by default) | Needs a host mediator outside the WASI ABI; no browser default. |
| clock, random (ports, not caps) | — | Available | `Date.now()` / `crypto.getRandomValues`. |

This is the same table wasmtime enforces; the browser column records **where a
browser's `jco` WASI-0.2 implementation diverges** (flagged at
`ROADMAP.md:682`). "Emulated" is the honest status for `net.http` and `fs.*`:
the *capability* is grantable, but the browser substrate constrains it.

### 2. Bundle-size measurement

The shipped artifact is a **WASI-0.2 component** (magic `0061 736d 0d00 0100`),
built by the existing CI link-gate:

```
cargo build -p tau-wasm-guest --target wasm32-wasip2 --release
```

with the committed empty-cap baseline world (`wit-baseline/runner.wit`) and the
built-in default IR — i.e. the **minimal guest floor**, deterministic with no
env vars (this is what CI already builds at `ci.yml:258`).

**Key constraint discovered:** Binaryen (`wasm-metadce`, `wasm-opt`) **cannot
parse a component** ([binaryen#6728](https://github.com/WebAssembly/binaryen/issues/6728)).
So the pipeline is:

1. **Shipped size** = component `.wasm` byte size, measured with `wasm-tools`
   (which *does* understand components, and is already tau's wasm toolchain).
2. **Tree-shaken floor** = extract the core module
   (`wasm-tools component unbundle`), run Binaryen `wasm-metadce` rooted at the
   component's real exports (`run`, `cabi_realloc`, `cabi_post_run`, `memcmp`,
   `memory`), then `wasm-opt -Oz`. This is the honest "via `wasm-metadce`"
   number: it demonstrates how much of the shipped bytes are custom
   sections + already-dead code the browser loader never needs.

Measured (baseline empty-cap guest, `wasm32-wasip2`, default release profile):

| Number | Bytes | KiB | Tool |
|---|---|---|---|
| Shipped component (browser download) | 15,686 | 15.3 | `wasm-tools` |
| Core module after `wasm-metadce` | 10,651 | 10.4 | Binaryen `wasm-metadce` |
| + `wasm-opt -Oz` | 9,878 | 9.6 | Binaryen |

The published headline number is the **shipped component: 15.3 KiB** — the real
browser download for the minimal guest. The metadce floor (10.4 KiB) is
published alongside as the tree-shaking demonstration the roadmap asked for.
Realistic workflows scale above this floor with their baked IR + declared tools
(the 5.4 `streaming-demo` fixture is ~1.6 MB); the floor is the deterministic,
CI-pinned number.

### 3. `local == CI == docs`

- **`scripts/wasm-guest-size.sh`** encapsulates the whole pipeline (build →
  `wasm-tools` size → unbundle → `wasm-metadce` → `wasm-opt`), prints a table,
  and enforces a **budget** (fail if the shipped component exceeds a generous
  ceiling — a regression tripwire, not a tuning target). Callers supply
  `CARGO_TARGET_DIR` per the workspace CARGO RULES.
- **CI** (`ci.yml`, the existing wasm-guest job) installs `wasm-tools` +
  `binaryen`, runs the script, and appends the table to `$GITHUB_STEP_SUMMARY`
  so every PR publishes the number. The budget check makes size regressions
  visible.
- **Docs** — `docs/reference/browser-capabilities.md` (new, under `# Reference`,
  added to `SUMMARY.md`) publishes the profile table + the numbers + the
  `binaryen#6728` caveat + the reproduce command.

## Testing

- `scripts/wasm-guest-size.sh` runs end-to-end locally and in CI; its budget
  check is the regression gate.
- `mdbook build` clean (new page in `SUMMARY.md`, no broken links).
- No Rust source changes → no new unit tests; the script + CI step are the test.

## Future work

- A first-class `any-browser-strict` target triple + `AdapterFamily::Wasi`
  browser adapter (separate ADR) would turn "Emulated/Forbidden" from a doc
  table into a build-time `tau check --target any-browser-strict` gate.
- A size-tuned release profile (`opt-level="z"`, `lto`, `strip`) + `wasm-opt`
  in `tau build wasm` would lower the shipped number toward the metadce floor.
- JSPI (`jco --async-mode jspi`) build path for live browser `net.http`.
