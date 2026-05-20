# Tau target triple registry — design

**Date:** 2026-05-19
**Status:** Approved
**Authors:** Claude (Opus 4.7)
**Tracking:** ROADMAP Phase 2 §B
**Successor ADR:** 0034 (to be added by the implementation plan)

## 1. Background

Phase 2 §A (`tau check`, PR #161) closed the validation surface. Phase 2 §C (`tau build --target <triple>`) will produce content-hashed deployment bundles pinning a target. §D guarantees forward-compat for the capability vocabulary in those bundles. Both §C and §D need a **stable, parseable, structural** identifier for what a bundle is built for.

Today the codebase has informal target names only in prose (`docs/explanation/tau-as-language.md`):

- `linux-native-strict`, `linux-native-light` — 3-part `<platform>-<adapter>-<tier>`
- `container-podman` — 2-part `<adapter>-<engine>`, no tier
- `remote-vercel`, `wasi-p2` — 2-part `<adapter>-<variant>`, no tier/platform

These shapes are inconsistent. §B's job is to formalize the convention, codify the first set of triples, and provide enough machinery for §C to consume.

## 2. Goal

A code-callable target triple registry that:

1. Defines a stable, structural identifier for tau deployment targets.
2. Codifies v1's supported targets and reserved namespaces.
3. Surfaces the registry through three user-facing affordances: `tau target list`, `tau target show <triple>`, and `tau check --target <triple>`.
4. Sets up §C/§D for clean extension without renaming any triple shipped in §B.

**Non-goals:**

- No `tau build --target` (that's §C).
- No bundle format (§C).
- No new sandbox adapters (those land independently).
- No forward-compat machinery for `CapabilityShape` vocabulary versioning (§D).

## 3. Naming convention

Bazel-inspired: a triple is fundamentally a **constraint set on three orthogonal axes**, displayed as a canonical short name. The structural form is the source of truth; the string form is for ergonomics, parsing, and bundle metadata.

### 3.1 Axes (the three constraint settings)

| Axis | Variants | Source of truth |
|---|---|---|
| `Platform` | `Linux`, `Darwin`, `Windows`, `Any` | new enum in `tau-ports::target` |
| `AdapterFamily` | `Native`, `Container`, `Remote`, `Wasi`, `Passthrough` | new enum in `tau-ports::target`; aligns with `tau-runtime::sandbox::registry::RegistryKind` |
| `SandboxTier` | `Strict`, `Light`, `None` | existing, in `tau-ports` |

The `Wasi` family is reserved (no `Wasi` variant in `RegistryKind` today; added when §G ships).

### 3.2 Canonical display

```text
<platform>-<adapter>-<tier>
```

with `passthrough` as the single special case — a one-segment name representing `(Platform::Any, AdapterFamily::Passthrough, SandboxTier::None)`.

Format rules (the parser):
- Lowercase ASCII letters and hyphens only.
- Exactly 1 or 3 hyphen-separated segments.
- Single-segment: must be in the whitelist (currently `{passthrough}`).
- Three-segment: each segment maps to one Variant; unknown → parse error with the list of valid values for that position.

### 3.3 Why this shape (vs alternatives)

- **vs. ad-hoc whitelist** (`container-podman`, `remote-vercel`, `wasi-p2`): mixed segment shapes mean every parser site has to enumerate cases. The struct form makes adapter ↔ triple matching a struct comparison, not a string match.
- **vs. LLVM-style `<arch>-<vendor>-<os>-<isolation>`** (e.g., `aarch64-apple-darwin-strict`): plugin binaries are pre-built per-arch and selected at install time; arch is not a tau-level concern. Bundling arch into the triple would double the triple count without serving §C.
- **vs. Bazel's open constraint system** (arbitrary `(setting, value)` pairs): tau's domain has 3 fixed axes today and a fourth coming in §D (`capability_vocab_version`). Three fixed fields stay readable and serializable.

## 4. v1 registry

### 4.1 Triples that ship (status `Available`)

| Triple | Platform | AdapterFamily | SandboxTier | Required shapes |
|---|---|---|---|---|
| `linux-native-strict` | Linux | Native | Strict | `fs.r`, `fs.w`, `exec`, `net.http` |
| `linux-native-light` | Linux | Native | Light | `fs.r`, `fs.w`, `exec`, `net.http` |
| `linux-container-strict` | Linux | Container | Strict | `fs.r`, `fs.w`, `exec`, `net.http` |
| `darwin-native-strict` | Darwin | Native | Strict | `fs.r`, `fs.w`, `exec`, `net.http` |
| `passthrough` | Any | Passthrough | None | all-shapes |

"Required shapes" = the capability shapes the target's adapter MUST enforce; bundles compiled for this triple may NOT require any shape outside this set. (Aligned with the existing adapter registry's `shapes_supported_fn` values.)

`AgentSpawn` is in `all-shapes` (Passthrough) but not in the strict/light entries because no current adapter enforces it at the sandbox layer (it's an in-process capability). When §G ships an isolating runtime for agent-spawn, the shape lists may grow — that's a forward-compat addition, not a rename.

### 4.2 Reserved (status `Reserved`)

| Triple / namespace | Reason |
|---|---|
| `windows-native-strict` | `tau-sandbox-windows` is scaffold-only per ADR-0023; probe returns `Unavailable`. Triple parses + validates; `tau check --target` emits an adapter-unavailable Warning. |
| `linux-remote-*`, `darwin-remote-*`, `any-remote-*` | Remote adapter family is registered but no concrete provider has shipped. Triples with the `Remote` family but no live adapter parse + are reserved by namespace; no individual entries ship in v1. |
| `linux-wasi-*`, `any-wasi-*` | Wasi adapter family doesn't exist as a `RegistryKind` yet. Whole namespace reserved; no entries ship. |

### 4.3 Status discipline

```rust
pub enum TripleStatus {
    Available,
    Reserved { reason: &'static str },
}
```

`Reserved` triples surface in `tau target list` (so users know the name is taken). `tau target show <reserved>` works. `tau check --target <reserved>` runs validation and emits a Warning that no working adapter exists.

## 5. Code surface

### 5.1 `tau-ports::target` module

```rust
pub mod target;

// target/mod.rs
pub use platform::Platform;
pub use adapter_family::AdapterFamily;
pub use triple::TargetTriple;
pub use profile::{TargetCapabilityProfile, TripleStatus};
pub use registry::{REGISTRY, lookup, list_available, list_all};

// target/platform.rs
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Linux,
    Darwin,
    Windows,
    Any,
}

impl Platform { pub fn as_str(&self) -> &'static str; }
impl FromStr for Platform { type Err = ParseError; ... }
impl Display for Platform { ... }

// target/adapter_family.rs
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AdapterFamily {
    Native,
    Container,
    Remote,
    Wasi,
    Passthrough,
}
// FromStr / Display / as_str — same shape as Platform.

// target/triple.rs
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TargetTriple {
    pub platform: Platform,
    pub adapter_family: AdapterFamily,
    pub tier: tau_ports::SandboxTier,
}

impl TargetTriple {
    pub const PASSTHROUGH: Self = TargetTriple {
        platform: Platform::Any,
        adapter_family: AdapterFamily::Passthrough,
        tier: SandboxTier::None,
    };
}
impl FromStr for TargetTriple { type Err = ParseError; ... }
impl Display for TargetTriple { ... }
impl TryFrom<String> for TargetTriple { ... }
impl From<TargetTriple> for String { ... }

// target/profile.rs
#[derive(Debug, Clone)]
pub struct TargetCapabilityProfile {
    pub triple: TargetTriple,
    pub required_shapes: CapabilityShapeSet,
    pub status: TripleStatus,
}

#[derive(Debug, Clone)]
pub enum TripleStatus {
    Available,
    Reserved { reason: &'static str },
}

// target/registry.rs
//
// `TargetCapabilityProfile` cannot be `const`-constructed because
// `CapabilityShapeSet` owns a `Vec` (the `Custom { name: String }`
// variant prevents `const fn` constructors). The static registry
// therefore stores entries with a function-pointer constructor for
// the shape set — same pattern as
// `tau-runtime::sandbox::registry::AdapterRegistration::shapes_supported_fn`.

pub struct TargetTripleEntry {
    pub triple: TargetTriple,
    pub shapes_fn: fn() -> CapabilityShapeSet,
    pub status: TripleStatus,
}

impl TargetTripleEntry {
    /// Materialise the full profile (allocates the shape set).
    pub fn profile(&self) -> TargetCapabilityProfile { ... }
}

pub static REGISTRY: &[TargetTripleEntry] = &[
    TargetTripleEntry {
        triple: TargetTriple {
            platform: Platform::Linux,
            adapter_family: AdapterFamily::Native,
            tier: SandboxTier::Strict,
        },
        shapes_fn: || fs_rw_exec_net(),
        status: TripleStatus::Available,
    },
    // ... 4 more Available entries + 1 Reserved (windows-native-strict)
];

pub fn lookup(triple: &TargetTriple) -> Option<&'static TargetTripleEntry>;
pub fn list_available() -> impl Iterator<Item = &'static TargetTripleEntry>;
pub fn list_all() -> impl Iterator<Item = &'static TargetTripleEntry>;

// target/parse.rs (errors)
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum ParseError {
    #[error("empty triple")]
    Empty,
    #[error("triple has {0} segments; expected 1 or 3")]
    WrongSegmentCount(usize),
    #[error("unknown single-segment triple `{0}`; expected one of: passthrough")]
    UnknownSpecial(String),
    #[error("unknown platform `{0}`; expected one of: linux, darwin, windows, any")]
    UnknownPlatform(String),
    #[error("unknown adapter family `{0}`; expected one of: native, container, remote, wasi, passthrough")]
    UnknownAdapterFamily(String),
    #[error("unknown tier `{0}`; expected one of: strict, light, none")]
    UnknownTier(String),
    #[error("invalid character `{0}` in triple; only lowercase ASCII letters and hyphens allowed")]
    InvalidChar(char),
}
```

### 5.2 `tau-runtime::sandbox::target_match` module

```rust
// tau-runtime/src/sandbox/target_match.rs
use tau_ports::target::{TargetTriple, TargetTripleEntry, AdapterFamily, Platform};
use crate::sandbox::registry::{AdapterRegistration, REGISTRY as ADAPTER_REGISTRY, RegistryKind};

/// Does the given adapter registration satisfy the triple's constraints?
///
/// Requires:
/// - Adapter's `platforms` set includes triple's platform (or triple is `Platform::Any`).
/// - Adapter's `RegistryKind` maps to triple's `AdapterFamily`.
/// - Adapter's `tiers_supported` contains triple's tier.
///
/// Shape coverage is NOT checked here — that's a separate "can this adapter
/// enforce the triple's required shapes?" check via `adapter.shapes_supported_fn`.
pub fn adapter_satisfies(adapter: &AdapterRegistration, triple: &TargetTriple) -> bool;

/// Find the first adapter registration that satisfies the triple.
///
/// Used by `tau check --target` to map a triple to an adapter for the
/// "is this triple's adapter Available locally?" probe. Returns the
/// adapter registration (not an instantiated `SandboxAdapter`); the
/// caller probes if needed.
pub fn registration_for_triple(triple: &TargetTriple) -> Option<&'static AdapterRegistration>;

/// Map `RegistryKind` → `AdapterFamily`.
fn kind_to_family(kind: RegistryKind) -> AdapterFamily {
    match kind {
        RegistryKind::Native => AdapterFamily::Native,
        RegistryKind::Container => AdapterFamily::Container,
        RegistryKind::Remote => AdapterFamily::Remote,
        RegistryKind::Passthrough => AdapterFamily::Passthrough,
    }
}
```

Pure functions. No async (probe is left to the caller). Unit-tested inline.

### 5.3 `tau-cli::cmd::target` subcommand

```text
crates/tau-cli/src/cmd/target/
├── mod.rs        # CLI dispatch + TargetArgs
├── list.rs       # `tau target list [--all]`  (default: Available only)
├── show.rs       # `tau target show <triple>` (full profile + suggested-fix on parse error)
└── render.rs     # human/JSON rendering helpers shared by list+show
```

#### 5.3.1 `tau target list`

```text
$ tau target list
linux-native-strict    Available  fs.r, fs.w, exec, net.http
linux-native-light     Available  fs.r, fs.w, exec, net.http
linux-container-strict Available  fs.r, fs.w, exec, net.http
darwin-native-strict   Available  fs.r, fs.w, exec, net.http
passthrough            Available  fs.r, fs.w, exec, net.http, agent.spawn

$ tau target list --all
linux-native-strict     Available  fs.r, fs.w, exec, net.http
...
windows-native-strict   Reserved   (scaffold; probe Unavailable)
```

`--json` flag emits one JSONL object per triple (matches `tau check` JSON pattern).

#### 5.3.2 `tau target show <triple>`

```text
$ tau target show linux-native-strict
linux-native-strict
  status:        Available
  platform:      linux
  adapter:       native
  tier:          strict
  shapes:        fs.r, fs.w, exec, net.http
  adapter local: Available (kind: native, priority: 100)

$ tau target show unknown-name
error: unknown triple `unknown-name`
       could not parse: triple has 2 segments; expected 1 or 3
       did you mean: linux-native-strict? (Levenshtein distance 8)
```

Suggestion logic reuses `cmd/skill/levenshtein.rs` (already in tree from Skills-3, PR #66).

#### 5.3.3 Error rendering

Parse errors surface through `cmd/error_render.rs` (existing helper from tau-pkg) — same render pattern as `RequiresToolsBareStringRejected` etc.

### 5.4 `tau check --target <triple>` integration

New CLI flag on `tau check` (and any per-category invocation like `tau check sandbox --target X`). Behavior:

1. Parse `<triple>` via `TargetTriple::from_str`. Parse error → exit 64 (usage).
2. Look up profile via `tau_ports::target::lookup(&triple)`. Unknown → exit 64 with suggestion (re-use show's renderer).
3. Run categories with the target in scope:
   - `sandbox` category: validate plugins' required shapes ⊆ target profile's `required_shapes`. Error finding per violation. Validate project's `required_tier` ≤ target's tier. Error finding on violation. Adapter is determined by `registration_for_triple(&triple)`; if no local registration → Warning finding `target reserved (no working adapter exists)` for `Reserved` triples, OR Warning `target's adapter not registered locally` for `Available` triples where no compatible adapter is in `ADAPTER_REGISTRY`. If the adapter IS registered but its probe returns Unavailable, also Warning.
   - Other categories (`config`, `lockfile`, `packages`, `plugins`, `skills`): unchanged behavior; the target is informational only for these.
4. Exit code follows the existing `tau check` policy (0 clean / 2 fixable bug / 3 needs-setup / 64 usage / 70 internal).

The sandbox category receives the target via a new optional field on `CheckCtx`:

```rust
pub struct CheckCtx {
    // existing fields
    pub project_root: PathBuf,
    pub scope: Scope,
    pub project: Option<ProjectConfig>,
    pub fast: bool,
    // new:
    pub target: Option<TargetTriple>,
}
```

`run_sandbox` branches:
- If `ctx.target.is_some()`: use target's profile + matching adapter (from `registration_for_triple`) instead of `resolve_sandbox_check_adapter(...)`.
- If `ctx.target.is_none()`: existing behavior (resolve local adapter from `[sandbox]` requirements).

The existing `check_plugin_sandbox` helper (from PR #173) takes `Option<&SandboxAdapter>` — an *instantiated* adapter. The `--target` path validates shapes against the target's profile **without** instantiating an adapter (the adapter may not be available locally; the point of `--target` is to validate at compile-time against a possibly-non-local target). So `--target` uses a sibling helper instead:

- Add `check_plugin_sandbox_against_profile(plugin_id, manifest_path, profile) -> SandboxPluginOutcome` to `resolve_helpers.rs`. Reads the manifest, validates declared shapes ⊆ `profile.required_shapes`. Same outcome enum as the existing helper.
- `cmd/check/categories/sandbox.rs::run_sandbox` picks the helper based on `ctx.target`:
  - `ctx.target.is_some()` → `check_plugin_sandbox_against_profile`
  - `ctx.target.is_none()` → existing `check_plugin_sandbox` (local adapter path)

### 5.5 ROADMAP + docs

- `ROADMAP.md`: flip §B to ✅, add Phase 2 progress table mirroring Phase 1's table.
- `docs/reference/target-triples.md`: Diátaxis reference page with the full registry, parse rules, example use.
- `docs/explanation/tau-as-language.md`: update the §B paragraph to point at the new ADR and reference page.

## 6. CLI surface summary

| Command | New / Changed | Purpose |
|---|---|---|
| `tau target list [--all] [--json]` | new | enumerate registry |
| `tau target show <triple> [--json]` | new | inspect one triple |
| `tau check --target <triple>` | changed (new flag) | validate against target instead of local adapter |
| `tau check sandbox --target <triple>` | changed (flag propagates) | category-level form of the above |

No changes to `tau resolve --check-sandbox`. That command exists for project-side install validation; bundle-level target validation lives on `tau check`.

## 7. Risks

| Risk | Mitigation |
|---|---|
| Shape lists in registry drift from adapter `shapes_supported_fn` | Add a regression test: every Available triple's `required_shapes` ⊆ at least one adapter's `shapes_supported_fn()` output. Test lives in `tau-runtime` (where the join makes sense). |
| Parser strictness rejects names a user expects to work (e.g. `linux_native_strict`) | Parser is strict; error message lists allowed characters. The Levenshtein-suggestion code path handles typos. No fuzzy parsing — strict-by-design. |
| §C/§D need a serde-stable triple representation | Triple is `serde(try_from = "String", into = "String")`. Bundles serialize the string form; struct is implementation detail. |
| `Reserved` triples confuse users in `--target` mode | `tau target show <reserved>` clearly labels Reserved + reason. `tau check --target <reserved>` emits a clear Warning. |
| Adding a new triple in §C/later breaks existing bundles | Adding a triple is additive (new entry in `REGISTRY`). Renaming or removing an Available triple is forbidden (constitution-level rule documented in the ADR). |

## 8. Testing strategy

- **Unit tests inline** in `tau-ports::target`:
  - Round-trip parse → display for every Available triple
  - Parse error for each `ParseError` variant
  - Levenshtein-style suggestion sanity (5 closest matches)
  - `lookup` returns the expected entry for each Available triple; `None` for unknown
- **Unit tests inline** in `tau-runtime::sandbox::target_match`:
  - `adapter_satisfies` matrix: every (adapter, triple) pair in the cross-product yields the expected boolean
  - `registration_for_triple` returns the right registration for each Available triple
  - Cross-check: every Available triple's required_shapes ⊆ its matched adapter's shapes_supported_fn output
- **Integration tests** in `tau-cli/tests/`:
  - `cmd_target_list.rs`: human + JSON output snapshot
  - `cmd_target_show.rs`: success + parse-error + Reserved triple snapshots
  - `cmd_check_target.rs`: `tau check --target linux-native-strict` against a fixture project with a plugin needing a shape outside the target's set → Error finding
  - `cmd_check_target.rs`: `tau check --target windows-native-strict` → Warning finding (Reserved)
  - `cmd_check_target.rs`: `tau check --target bogus-triple` → exit 64 with suggestion

No new e2e tests; no new CI required-checks needed.

## 9. Out of scope

- `tau build --target` (Phase 2 §C).
- Bundle format (Phase 2 §C).
- Capability vocab versioning / forward-compat for `CapabilityShape` (Phase 2 §D).
- Adding new sandbox adapters.
- Cross-arch concerns (plugins handle their own per-arch distribution).
- Adding `Remote` or `Wasi` triples — namespace reserved, no individual entries ship.
- Engine selection inside `linux-container-strict` (stays in `[sandbox.container].engine` config; not part of the triple).

## 10. ADR

ADR-0034 codifies:
- The 3-axis structural form as the source of truth.
- The canonical short-name display rules + single-segment specials.
- The v1 list of Available triples + the `windows-native-strict`/`remote-*`/`wasi-*` reservations.
- The stability discipline: Available triples are immutable once shipped; additions are forward-compatible; deletions and renames are constitution-forbidden.
- Forward-compat hook: §D may add a `capability_vocab_version` field; old bundles parse with default; new field surfaces only when needed.
