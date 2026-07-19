# D8-B — IR format load gate (design)

**Date:** 2026-07-19
**Status:** Approved, pre-implementation
**Crate:** `tau-ir`
**ir_format impact:** none (stays `v2.4.0`)

## Context

`tau_ir::from_canonical_bytes` (`crates/tau-ir/src/canonical.rs`) is today a
bare `serde_json::from_slice::<IrModule>` returning `serde_json::Error` on
failure. There is **no version check anywhere on the load path** — a bundle
whose `ir_format` does not structurally match the running `tau` fails with a
raw serde error ("missing field", "unknown variant"), not an actionable one.

This gate is the enabling prerequisite for the **D9-C** canonicalization flip
(`ir_format 3.0.0`). The land order is fixed: D8-B must merge **before** the
3.0.0 flip, because it provides the clean *"major mismatch — rebuild with a
matching tau"* error that a `v2.x` tau should show when handed a `v3.0.0`
bundle. (Note: an *old* tau reading a 3.0.0 bundle json-decodes fine but fails
the existing hash-verify with a divergence error — failing, not silent. D8-B
does not change that path; it improves the `tau-ir` decode surface.)

## Decision: major-only gate

The gate rejects **iff the major version differs**. Within a single major,
anything decodes — honoring the existing semver contract in
`module.rs:16-23` ("MINOR for additive changes … PATCH for spec-only edits").

Rejected alternative — *strict / no forward-compat within a major* (also
reject same-major newer-minor) — was declined: it throws away the
forward-compatibility every minor bump has been engineered for (each is
documented "additive / byte-stable when absent"), and its only payoff is a
marginally friendlier error for a case that already works. Hard rejection is
reserved for genuine breaking changes (major), which is exactly the D9-C
`v3.0.0` case this gate exists to catch.

### Gate matrix (this tau emits `v2.4.0`)

| Bundle `ir_format` | major cmp | Result |
|---|---|---|
| `v2.4.0` | 2 == 2 | `Ok` (exact) |
| `v2.3.0` | 2 == 2 | `Ok` (older minor; additive fields absent) |
| `v2.5.0` (future D6-B) | 2 == 2 | `Ok` (newer minor; unknown additive fields ignored) |
| `v3.0.0` (future D9-C) | 2 ≠ 3 | `Err(FormatMajorMismatch)` |
| `v1.0.0` | 2 ≠ 1 | `Err(FormatMajorMismatch)` |
| `"garbage"`, `""`, `"v.4"` | — | `Err(FormatUnparseable)` |
| valid version, malformed body | — | `Err(Decode)` |

## Two-phase decode

```
from_canonical_bytes(bytes)
  PHASE 1 (peek):  serde_json::from_slice::<VersionPeek>  → read ir_format
       serde err ─────────────────────────────────► Err(Decode)
       parse major(bundle) / major(current)
       unparseable bundle version ─────────────────► Err(FormatUnparseable)
       major(bundle) != major(current) ────────────► Err(FormatMajorMismatch)
  PHASE 2 (full):  serde_json::from_slice::<IrModule>
       serde err ─────────────────────────────────► Err(Decode)
       ok ─────────────────────────────────────────► Ok(IrModule)
```

Phase 1 decodes only `{"ir_format": "..."}` (serde ignores every other key), so
the *version* is trusted before the full shape is. A genuinely-different
`v3.0.0` shape therefore fails with `FormatMajorMismatch` instead of whatever
serde throws mid-`IrModule`.

## Components

1. **`IrFormatVersion::major(&self) -> Result<u64, ()>`** (`module.rs`) — strip
   an optional leading `v`, parse the integer before the first `.`. Plus a
   `CURRENT_MAJOR: u64` associated constant (parsed from `CURRENT`, or a pinned
   literal `2` kept in sync by a test asserting they agree).
2. **`VersionPeek`** (private, `canonical.rs`) —
   `#[derive(Deserialize)] struct VersionPeek { ir_format: IrFormatVersion }`.
   Phase-1 target; shape-independent because serde ignores unknown fields.
3. **`from_canonical_bytes`** — rewritten two-phase as above.
   `to_canonical_bytes` is **unchanged** (its `expect` stays; D9-C changes it).
4. **`IrError`** (`error.rs`) gains three variants, all `String`/alloc payloads
   (keeps the crate `no_std`; `serde_json::Error` is never carried by value):
   - `Decode(String)` — wraps a stringified serde error.
   - `FormatMajorMismatch { bundle: String, current: String, bundle_major: u64 }`.
   - `FormatUnparseable { value: String }`.

## Type signatures

```rust
// error.rs
pub enum IrError {
    /* …existing… */
    #[error("canonical IR is not valid JSON: {0}")]
    Decode(String),
    #[error("IR format major {bundle_major} is incompatible with this tau \
             (emits {current}); rebuild with a matching tau")]
    FormatMajorMismatch { bundle: String, current: String, bundle_major: u64 },
    #[error("IR format version {value:?} is not a valid vMAJOR.MINOR.PATCH string")]
    FormatUnparseable { value: String },
}

// module.rs
impl IrFormatVersion {
    pub const CURRENT_MAJOR: u64 = 2;
    pub fn major(&self) -> Result<u64, ()>;
}

// canonical.rs
pub fn from_canonical_bytes(bytes: &[u8]) -> Result<IrModule, IrError>;
```

## Caller impact

| Caller | file | change |
|---|---|---|
| `tau run --bundle` | `tau-cli/src/cmd/run.rs:114` | error arm now `IrError` (was `serde_json::Error`); the later `ir_format = %ir.ir_format` log is unaffected |
| wasm guest | `tau-wasm-guest/src/guest.rs:104` | `.map_err(|e| e.to_string())` still compiles (`IrError: Display`) |
| conformance | `tau-ir-conformance/src/bundle_mode.rs:169` | error match arm updated to `IrError` |

## Testing (`nextest -p tau-ir`)

- **`major()` truth table:** `v2.4.0→2`, `2.4.0→2`, `v10.0.0→10`, `""→Err`,
  `"garbage"→Err`, `"v.4"→Err`.
- **`CURRENT_MAJOR` agreement:** assert `IrFormatVersion::current().major() ==
  Ok(CURRENT_MAJOR)` so a future `CURRENT` bump can't silently desync.
- **Gate matrix:** serialize a real `IrModule`, edit the `ir_format` string in
  the JSON (`serde_json::Value` surgery, so no real v3 shape needed) to
  `v3.0.0` / `v1.0.0` / `v2.9.9` / `"vX"`, assert the expected `IrError`.
- **Forward-compat:** `v2.5.0` and `v2.3.0` edited bundles both decode `Ok`.
- **Unknown-additive-field tolerance:** inject `"assets": {...}` alongside
  `ir_format: v2.5.0`, assert `Ok` (serde ignores it) — the D6-B forward-compat
  guarantee under test.
- **Body-decode error:** valid `ir_format`, corrupt body → `Err(Decode)`.
- **Existing round-trip tests** in `canonical.rs` keep passing (same-version).

## Scope / non-goals

- **No `ir_format` bump.** Read-path behavior only; canonical *output* bytes are
  byte-identical. No schema regen, no golden changes, `CURRENT` stays `v2.4.0`.
- **No canonicalization change** — that is D9-C.
- **No ADR of its own** — enabling refactor; a one-line forward-reference goes
  in the D9-C ADR when it lands.
