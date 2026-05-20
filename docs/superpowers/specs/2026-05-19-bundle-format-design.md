# Tau bundle format (Phase 2 §C.1) — design

**Date:** 2026-05-19
**Status:** Approved
**Authors:** Claude (Opus 4.7)
**Tracking:** ROADMAP Phase 2 §C — decomposed into §C.1 (this spec), §C.2 (`tau build`), §C.3 (`tau run --bundle`)
**Successor ADR:** 0035 (to be added by the implementation plan)

## 1. Background

Phase 2 §A (`tau check`, PR #161) and §B (target triple registry, PR #190) shipped. §C produces the deployment artifact — a content-hashed bundle that pins everything a target host needs to execute a tau workflow reproducibly.

Per ROADMAP, §C is estimated at ~6 weeks. To stay shippable, it decomposes into three sub-PRs:

| Sub-PR | Scope | Status |
|---|---|---|
| **§C.1** | Bundle format (this spec) — schema + serde + self-hash integrity check. Pure data crate. No CLI. | this spec |
| §C.2 | `tau build --target <triple>` — producer. Composes lockfile + tree_hash + compute_effective into a bundle file. | future |
| §C.3 | `tau run --bundle <file>` — consumer. Loads + verifies + runs. | future |

§C.4 (self-contained bundles with plugin binaries) is **deferred indefinitely** per the brainstorm decision: ship reference-only first, add binary embedding only when an air-gap or remote-runner use case materialises. The format chosen here does not preclude a future `binaries:` section.

## 2. Goal

Define the on-disk schema for a tau bundle + verification of its self-integrity.

**Non-goals:**

- No CLI verb. `tau build` is §C.2; `tau run --bundle` is §C.3.
- No cross-machine reproducibility verification (that's §E). The self-hash proves "this byte stream wasn't tampered with"; it does NOT prove "this byte stream is what re-building from the same inputs would produce."
- No bundle signing. Phase 3+.
- No plugin binary embedding. Reference-only per the brainstorm.

## 3. Strategic decisions (locked from brainstorm)

1. **Reference-only.** Bundles pin source URLs + tree hashes; binaries are fetched from the source at deploy time. Self-contained mode is a future opt-in flag, not part of v1.
2. **Single TOML file** with `.tau` extension. No directory tree, no tar wrapper. Smallest deployable unit; trivially git-committable, hashable, pipeable.
3. **schema_version starts at 1.** Forward-compat hook: new fields land as additive optional in v1.x; major schema breaks bump to 2 with loud-and-clear mismatch errors.

## 4. Bundle schema (v1)

Canonical field order (top-level table → arrays preserved in declaration order):

```toml
schema_version = 1

[bundle]
sha256       = "<64 hex chars>"  # self-hash; see §5
created_at   = "2026-05-19T13:42:11Z"  # RFC 3339 UTC; informational
tau_version  = "0.1.0"
target       = "linux-native-strict"   # TargetTriple display form

[project]
name              = "support-bot"
version           = "0.3.2"
tau_toml_sha256   = "<64 hex chars>"   # SHA-256 of source tau.toml bytes

[[packages]]                           # one per resolved package (mirrors lockfile)
name              = "tau-plugin-fs-read"
version           = "0.2.1"
source            = "git+https://github.com/example/fs-read.git#tag=v0.2.1"
tree_sha256       = "<64 hex chars>"   # from tau-pkg::tree_hash
binary_sha256     = "<64 hex chars>"   # Some for plugin packages, omitted otherwise
required_shapes   = ["FilesystemRead"] # from package manifest

[[agents]]
id                      = "researcher"
backend                 = { kind = "ollama", model = "llama3.1:8b" }
system_prompt_sha256    = "<64 hex chars>"  # hash of prompt text
required_tools          = ["tau-plugin-fs-read"]

[agents.effective_capabilities]        # one table per agent — output of compute_effective()
allow_fs_read = ["/data/**", "/etc/agent/**"]
deny_fs_read  = ["/data/secrets/**"]
# ... per-shape allow/deny lists. Empty fields omitted.
```

### 4.1 Required fields

Every field above without an explicit "omitted otherwise" qualifier is **required**. Parser rejects missing fields with a typed error variant.

### 4.2 Optional fields

- `[[packages]].binary_sha256`: only for plugin packages (those with a binary artifact). Data-only packages omit it.
- `[[packages]].required_shapes`: empty array if the package declares none.
- `[agents.effective_capabilities]`: this table is OMITTED entirely if the agent's resolved grant set is empty (after `compute_effective`). When present, only the non-empty per-shape allow/deny lists are emitted.
- `[bundle].created_at`: required in v1. (Could become optional in a future "reproducible-build" mode; that's §E.)

### 4.3 Forward-compat reservations

- New top-level tables MAY be added in v1.x and are ignored by older parsers, EXCEPT that a parser whose major schema_version is older than the bundle's must error out loudly (`UnsupportedSchemaVersion`).
- New per-package or per-agent fields MAY be added in v1.x as optional with defaults; missing-from-bundle = default value.
- New CapabilityShape variants ride on §D's forward-compat machinery; this spec doesn't touch them.
- A `[binaries]` section is reserved for the future self-contained mode. Producers in v1 MUST NOT emit it; consumers in v1 ignore it (forward-compat).

## 5. Self-hash methodology

The `bundle.sha256` field is the SHA-256 of the canonical-TOML serialization of the bundle **with the `bundle.sha256` field set to the empty string** (`""`). Algorithm:

**Computing the hash (producer):**
1. Build `BundleManifest` in memory with `bundle.sha256 = ""`.
2. Serialize to canonical TOML bytes.
3. Compute SHA-256 of those bytes → hex string.
4. Store the hex string in `bundle.sha256`.
5. Re-serialize the manifest. The output file's bytes are the bundle.

**Verifying the hash (consumer):**
1. Read the bundle file. Parse it.
2. Save the value of `bundle.sha256` (let's call it `claimed`).
3. Set `bundle.sha256 = ""` on the parsed struct.
4. Serialize to canonical TOML bytes.
5. Compute SHA-256 of those bytes → hex string (`computed`).
6. Compare `claimed == computed`. Mismatch → `BundleIntegrityError::HashMismatch`.

The zero-the-field trick (vs. line-removal) keeps the invariant simple: the same canonical-TOML emitter is used at both ends, and the only difference is the value of one string field.

### 5.1 Canonical TOML

Stable byte-for-byte output for the same `BundleManifest` value. Rules:

- Top-level tables in the fixed order declared above (`schema_version`, `[bundle]`, `[project]`, `[[packages]]`, `[[agents]]`).
- Within a table, fields in the order declared above.
- Arrays of tables (`[[packages]]`, `[[agents]]`) in the order the producer emitted them. The producer (§C.2) is responsible for choosing a deterministic order; the format is order-preserving.
- Inline tables (`backend = { kind = "...", model = "..." }`) use TOML's standard inline syntax.
- Arrays of strings (e.g., `required_shapes`) keep their producer-given order; the format does NOT sort them.
- String values are emitted with `"..."` (double-quoted), with TOML's standard escaping.
- No comments, no trailing whitespace, single `\n` line endings.

Implementation: a `to_canonical_toml(&BundleManifest) -> String` method that writes fields manually in fixed order rather than relying on `toml::to_string`'s arbitrary serializer order. Unit test asserts: take a fully-populated manifest → serialize → parse → serialize again → byte-identical.

## 6. Code surface

### 6.1 `tau-pkg::bundle` module

```rust
// crates/tau-pkg/src/bundle/mod.rs
pub mod manifest;
pub mod canonical;
pub mod hash;
pub mod error;

pub use manifest::{
    BundleManifest, BundleMeta, ProjectInfo, BundlePackage, BundleAgent,
    BackendRef, BundleEffectiveCapabilities,
};
pub use canonical::to_canonical_toml;
pub use hash::{compute_self_hash, verify_self_hash};
pub use error::{BundleParseError, BundleIoError, BundleIntegrityError};
```

### 6.2 Structs (full surface)

```rust
// crates/tau-pkg/src/bundle/manifest.rs

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BundleManifest {
    pub schema_version: u32,
    pub bundle: BundleMeta,
    pub project: ProjectInfo,
    #[serde(default)]
    pub packages: Vec<BundlePackage>,
    #[serde(default)]
    pub agents: Vec<BundleAgent>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BundleMeta {
    pub sha256: String,
    pub created_at: String,           // RFC 3339; kept as String to avoid chrono dep
    pub tau_version: String,
    pub target: tau_ports::target::TargetTriple,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProjectInfo {
    pub name: String,
    pub version: semver::Version,
    pub tau_toml_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BundlePackage {
    pub name: String,
    pub version: semver::Version,
    pub source: tau_domain::PackageSource,
    pub tree_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary_sha256: Option<String>,
    #[serde(default)]
    pub required_shapes: Vec<tau_domain::CapabilityShape>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BundleAgent {
    pub id: tau_domain::AgentId,
    pub backend: BackendRef,
    pub system_prompt_sha256: String,
    #[serde(default)]
    pub required_tools: Vec<String>,
    /// One table per agent (not an array). Omitted entirely when the
    /// agent has no resolved grants.
    #[serde(default, skip_serializing_if = "BundleEffectiveCapabilities::is_empty")]
    pub effective_capabilities: BundleEffectiveCapabilities,
}

/// Backend identification carried in the bundle. Mirrors what tau.toml's
/// `[[agents.backend]]` block holds today.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BackendRef {
    pub kind: String,            // e.g. "ollama", "anthropic", "openai", "stub"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    // Forward-compat: arbitrary extra fields preserved as-is.
    #[serde(flatten)]
    pub extra: std::collections::BTreeMap<String, toml::Value>,
}

/// Per-agent serialized form of `compute_effective`'s output. One entry
/// per capability shape with non-empty allow OR deny lists.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub struct BundleEffectiveCapabilities {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow_fs_read: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny_fs_read: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow_fs_write: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny_fs_write: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow_exec: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny_exec: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow_net_http: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny_net_http: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow_agent_spawn: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny_agent_spawn: Vec<String>,
}
```

Notes:
- `version: semver::Version` — `semver` is already a workspace dep used by lockfile.
- `tau_domain::PackageSource` — already exists in tau-domain.
- `tau_ports::target::TargetTriple` — from §B.
- `BackendRef.extra: BTreeMap<String, toml::Value>` — forward-compat catch-all for backend-specific keys (e.g., `anthropic` may carry `api_base_url`; `ollama` may carry `host`). Producers don't need to enumerate every backend's options.
- `BundleEffectiveCapabilities` is a single table per agent holding all per-shape allow/deny lists combined. The struct exposes an `is_empty()` predicate; when true, `skip_serializing_if` omits the table entirely.

### 6.3 API

```rust
// crates/tau-pkg/src/bundle/mod.rs (entry points)

impl BundleManifest {
    /// Parse a bundle from a TOML string.
    pub fn parse_str(s: &str) -> Result<Self, BundleParseError> { ... }

    /// Read + parse a bundle from a file.
    pub fn from_path(p: &std::path::Path) -> Result<Self, BundleIoError> { ... }

    /// Serialize the manifest to canonical TOML bytes.
    pub fn to_canonical_toml(&self) -> String { ... }

    /// Compute the self-hash (does not mutate the manifest).
    pub fn compute_self_hash(&self) -> String { ... }

    /// Verify the manifest's `bundle.sha256` field matches the
    /// recomputed self-hash.
    pub fn verify_self_hash(&self) -> Result<(), BundleIntegrityError> { ... }
}
```

### 6.4 Errors

```rust
// crates/tau-pkg/src/bundle/error.rs

#[derive(Debug, thiserror::Error)]
pub enum BundleParseError {
    #[error("TOML parse error: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("unsupported schema version {found}; this tau binary supports v1.x only")]
    UnsupportedSchemaVersion { found: u32 },
    #[error("invalid target triple `{0}`: {1}")]
    InvalidTarget(String, tau_ports::target::ParseError),
    #[error("invalid semver `{0}`: {1}")]
    InvalidVersion(String, semver::Error),
    // Specific missing-field errors added as discovered during implementation.
}

#[derive(Debug, thiserror::Error)]
pub enum BundleIoError {
    #[error("could not read bundle at {path}: {source}")]
    Read { path: std::path::PathBuf, source: std::io::Error },
    #[error(transparent)]
    Parse(#[from] BundleParseError),
}

#[derive(Debug, thiserror::Error)]
pub enum BundleIntegrityError {
    #[error("bundle self-hash mismatch: claimed {claimed}, computed {computed}")]
    HashMismatch { claimed: String, computed: String },
    #[error("bundle.sha256 field is empty or missing")]
    HashFieldEmpty,
}
```

## 7. Testing strategy

Unit tests inline in `tau-pkg::bundle`:

1. **Round-trip**: build a fully-populated `BundleManifest` → `to_canonical_toml` → `parse_str` → assert equal. Then serialize again → assert byte-identical to the first emission.
2. **Self-hash compute + verify**: build a manifest with sha256 = "" → `compute_self_hash` → store on manifest → `verify_self_hash` returns Ok.
3. **Self-hash mismatch detection**: tamper with one byte (e.g., change a package version) post-hash → `verify_self_hash` returns `HashMismatch`.
4. **Schema version forward-compat**: parse a manifest with `schema_version = 2` → `UnsupportedSchemaVersion` error.
5. **Schema version backward-compat additive**: parse a v1 manifest that includes an unknown future field (e.g., a `[binaries]` table at top level) → succeeds, field ignored.
6. **`binary_sha256` optionality**: parse manifests with and without the field → both succeed; data-only package omits it correctly.
7. **Per-shape effective_capabilities**: 5 shapes × allow/deny → all 10 fields serialize/deserialize cleanly when populated; omit-when-empty rule respected.
8. **TargetTriple round-trip in `target` field**: parse "linux-native-strict" through serde → BundleMeta.target equals the expected struct.
9. **PackageSource round-trip**: git URL with `#tag=` fragment round-trips.
10. **Realistic fixture**: a hand-written `fixtures/support-bot.tau` checked into the test tree → `from_path` → `verify_self_hash` returns Ok. Updates to the spec require updating the fixture's `bundle.sha256` field; this is intentional friction to catch unintended schema drift.

Sample size: ~10-15 unit tests.

## 8. Dependency additions

- `sha2` already exists as a workspace dep (used by tree_hash).
- `toml` already exists.
- `serde` / `serde::Serialize`/`Deserialize` already in use.
- `semver` already in use.

No new `Cargo.toml` deps in `tau-pkg`.

## 9. ADR

ADR-0035 codifies:
- The reference-only-for-v1 strategic decision (per brainstorm).
- The single-TOML-file shape.
- The schema (top-level layout; the field list itself is allowed to evolve in v1.x).
- The self-hash methodology (zero-the-field trick).
- Forward-compat reservations (v1.x additive, v2+ loud-mismatch).
- A `[binaries]` section is reserved for a future self-contained mode but MUST NOT be emitted by v1 producers.
- Bundle file extension: `.tau`. (Reserves the extension. Producers MUST use it; consumers MAY accept other extensions for explicit `--bundle <path>` invocations.)

## 10. Risks

| Risk | Mitigation |
|---|---|
| Canonical TOML emitter drifts from `toml` crate's default emitter, causing surprising hash mismatches when a future maintainer "cleans up" the serializer | Round-trip byte-identity test is the regression gate. The hand-written canonical emitter is small (~60 LOC); changes go through that test. |
| `BundleManifest` struct grows unwieldy as §C.2 (producer) populates it from many sources | The struct is a passive data carrier. The composition logic lives in §C.2. Keep `tau-pkg::bundle` pure. |
| Forward-compat trap: a v1.1 bundle that uses a new optional field can't parse on a v1.0 consumer because the field isn't optional | Every new field added in v1.x carries `#[serde(default)]`. The pattern is documented in this spec; reviewers enforce it on PRs touching the schema. |
| `created_at` ruins reproducibility | Documented: reproducibility ≠ content-addressing. §E will address reproducibility via a separate `--source-date-epoch`-style mechanism or by recommending an externally-controlled timestamp source. |

## 11. Out of scope

- `tau build --target` — Phase 2 §C.2.
- `tau run --bundle` — Phase 2 §C.3.
- Self-contained bundles with plugin binaries — explicit deferral.
- Cross-machine reproducibility — Phase 2 §E.
- Bundle signing — Phase 3+.
- Adding any new CLI subcommand.
- Modifying lockfile format.
- Changing `compute_effective` or `tree_hash` signatures.
