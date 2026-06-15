# β.5 Credential Provider Chain — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a Strategy + Chain `CredentialProvider` port to `tau-ports` with Env/File/Baked providers, a per-agent credential declaration, a deployment-level chain config, and a host bridge that resolves a credential through the chain and injects it into an unmodified plugin's environment — preserving every existing env-var path byte-for-byte.

**Architecture:** `tau-ports` owns the clean port (`CredentialProvider`, native `async fn in trait`), the value types (`Secret`, `CredentialId`, `CredentialRequest`, `ResolvedCredential`, `CredentialError`), the `BakedProvider`, and the reusable `CredentialChain` combinator (which mirrors the codebase's non-`Send` boxed-future dyn shim from `tau-runtime-core/src/builder.rs:84`). `tau-runtime-tokio` owns the std I/O adapters (`EnvProvider`, `FileProvider`), the `build_chain` constructor, and the resolve-then-inject wiring in `plugin_host`. `tau-pkg` owns the two config surfaces (per-agent declaration in `tau.toml`; chain order in scope/home `config.toml`), both Unchecked→validate.

**Tech Stack:** Rust (`no_std` + `alloc` in `tau-ports`), `zeroize` (secret hygiene), `thiserror` (errors), `tokio::fs` (file provider), `toml`/`serde` (config), `cargo nextest` (tests). MSRV 1.91.

**CARGO RULES:** every cargo invocation MUST be wrapped per `CLAUDE.md`. The implementing agent uses `target/agent-impl`:

    timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p <crate>

Doctests: `... cargo test --doc -p <crate>`. fmt: `... cargo fmt -p <crate>`. Never run bare `cargo`, never omit `-p`.

---

## File Structure

**PR-1 — port (`tau-ports`)**
- Create `crates/tau-ports/src/credential/mod.rs` — trait, `CredentialRequest`, `ResolvedCredential`, re-exports
- Create `crates/tau-ports/src/credential/secret.rs` — `Secret`
- Create `crates/tau-ports/src/credential/id.rs` — `CredentialId`, `InvalidCredentialId`
- Create `crates/tau-ports/src/credential/chain.rs` — `DynCredentialProvider`, `CredentialChain`
- Create `crates/tau-ports/src/credential/baked.rs` — `BakedProvider`
- Modify `crates/tau-ports/src/error.rs` — add `CredentialError`
- Modify `crates/tau-ports/src/lib.rs` — `pub mod credential;` + re-exports
- Modify `crates/tau-ports/Cargo.toml` — `zeroize` dep
- Modify `Cargo.toml` (workspace) — `zeroize` workspace dep
- Modify `docs/decisions/0046-credential-provider-chain.md` — flip Status to Accepted (already committed in spec phase)

**PR-2 — adapters + CI (`tau-runtime-tokio`)**
- Create `crates/tau-runtime-tokio/src/credentials/mod.rs` — module glue + re-exports
- Create `crates/tau-runtime-tokio/src/credentials/env.rs` — `EnvProvider`
- Create `crates/tau-runtime-tokio/src/credentials/file.rs` — `FileProvider`
- Modify `crates/tau-runtime-tokio/src/lib.rs` — `pub mod credentials;`
- Modify `.github/workflows/ci.yml` — `test-credential-chain / linux` lane

**PR-3 — config (`tau-pkg`)**
- Modify `crates/tau-pkg/src/project/project.rs` — `UncheckedAgentCredential`, `AgentCredential`, field on `UncheckedAgent`/`AgentEntry`, validation, `ProjectConfigError` variant
- Create `crates/tau-pkg/src/scope_credentials.rs` — `UncheckedCredentialsConfig`, `CredentialsChainConfig`, `ProviderConfig`, `CredentialsConfigError`, `validate`
- Modify `crates/tau-pkg/src/scope.rs` — `credentials` field on `ScopeConfig`
- Modify `crates/tau-pkg/src/lib.rs` — `pub mod scope_credentials;`

**PR-4 — host bridge (`tau-runtime-tokio`)**
- Create `crates/tau-runtime-tokio/src/credentials/build.rs` — `build_chain`
- Modify `crates/tau-runtime-tokio/src/credentials/mod.rs` — export `build_chain`
- Modify `crates/tau-runtime-tokio/src/plugin_host/process.rs` — resolve-then-inject before spawn
- Create `crates/tau-runtime-tokio/tests/credential_inject.rs` — mock-plugin integration test

**PR-5 — docs**
- Create `docs/how-to/use-mounted-secrets.md`
- Create `docs/reference/credential-providers.md`
- Modify `docs/SUMMARY.md` — list both pages
- Modify `ROADMAP.md` — β.5 check-off + migration-trigger note

---

# PR-1 — Port (`tau-ports`)

Pure port crate. No wiring. Adds the trait, value types, `BakedProvider`, and the `CredentialChain` combinator.

## Task 1.1: Add `zeroize` dependency

**Files:**
- Modify: `Cargo.toml` (workspace, `[workspace.dependencies]` near line 119)
- Modify: `crates/tau-ports/Cargo.toml`

- [ ] **Step 1: Add the workspace dependency**

In `Cargo.toml`, under `[workspace.dependencies]`, immediately after the `secrecy` line (line ~119):

```toml
zeroize             = { version = "1", default-features = false, features = ["alloc"] }
```

- [ ] **Step 2: Reference it in tau-ports**

In `crates/tau-ports/Cargo.toml`, in `[dependencies]` after the `chrono` line:

```toml
zeroize = { workspace = true }
```

- [ ] **Step 3: Verify it resolves**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-ports`
Expected: compiles (no new code yet uses zeroize, so a `unused crate` warning is fine; `check` passes).

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml Cargo.lock crates/tau-ports/Cargo.toml
git commit -m "build(tau-ports): add zeroize dependency for Secret"
```

## Task 1.2: `Secret` type

**Files:**
- Create: `crates/tau-ports/src/credential/secret.rs`
- (module not yet wired — Task 1.6 wires `pub mod credential`)

- [ ] **Step 1: Write the failing test**

Create `crates/tau-ports/src/credential/secret.rs`:

```rust
//! Resolved-secret value: redacts on `Debug`, zeroized on drop.

use alloc::vec::Vec;
use core::fmt;
use zeroize::Zeroizing;

/// A resolved credential value. Holds raw bytes (not `String`) because
/// device / secure-element keys are binary. The inner buffer is zeroized
/// on drop, and `Debug` never reveals the contents.
pub struct Secret(Zeroizing<Vec<u8>>);

impl Secret {
    /// Wrap raw bytes as a secret.
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Self(Zeroizing::new(bytes))
    }

    /// Borrow the raw secret bytes.
    pub fn expose_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Borrow the secret as UTF-8, if it is valid UTF-8 (API keys are).
    pub fn expose_str(&self) -> Result<&str, core::str::Utf8Error> {
        core::str::from_utf8(&self.0)
    }

    /// Length of the secret in bytes.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the secret is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<Vec<u8>> for Secret {
    fn from(bytes: Vec<u8>) -> Self {
        Self::from_bytes(bytes)
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;
    use alloc::vec;

    #[test]
    fn debug_redacts() {
        let s = Secret::from_bytes(b"sk-ant-supersecret".to_vec());
        assert_eq!(format!("{s:?}"), "Secret(<redacted>)");
        assert!(!format!("{s:?}").contains("supersecret"));
    }

    #[test]
    fn expose_roundtrips() {
        let s = Secret::from_bytes(b"abc123".to_vec());
        assert_eq!(s.expose_bytes(), b"abc123");
        assert_eq!(s.expose_str().unwrap(), "abc123");
        assert_eq!(s.len(), 6);
        assert!(!s.is_empty());
    }

    #[test]
    fn non_utf8_is_rejected_by_expose_str() {
        let s = Secret::from_bytes(vec![0xff, 0xfe]);
        assert!(s.expose_str().is_err());
        assert_eq!(s.expose_bytes(), &[0xff, 0xfe]);
    }
}
```

- [ ] **Step 2: Run the test — expect a build failure (module not declared yet)**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-ports`
Expected: FAIL — `secret.rs` is an orphan file (no `mod` declares it). This is intentional; Task 1.6 wires the module. Continue building the sibling files first.

(Do not commit yet — the module tree is assembled and tested together in Task 1.6.)

## Task 1.3: `CredentialId`

**Files:**
- Create: `crates/tau-ports/src/credential/id.rs`

- [ ] **Step 1: Write the type + tests**

Create `crates/tau-ports/src/credential/id.rs`:

```rust
//! Logical credential identifier, e.g. `anthropic_api_key`.

use alloc::string::String;
use core::fmt;

/// A validated logical credential id. Charset: `[a-z0-9_.-]`, non-empty.
/// Used as the lookup key a provider resolves.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CredentialId(String);

/// Error returned when a string is not a valid [`CredentialId`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidCredentialId {
    /// Human-readable reason.
    pub reason: &'static str,
}

impl CredentialId {
    /// Parse and validate a credential id.
    pub fn parse(s: impl Into<String>) -> Result<Self, InvalidCredentialId> {
        let s = s.into();
        if s.is_empty() {
            return Err(InvalidCredentialId { reason: "credential id must be non-empty" });
        }
        if !s
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '.' | '-'))
        {
            return Err(InvalidCredentialId {
                reason: "credential id must match [a-z0-9_.-]",
            });
        }
        Ok(Self(s))
    }

    /// Borrow the id as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for CredentialId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CredentialId({})", self.0)
    }
}

impl fmt::Display for CredentialId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_ids() {
        for id in ["anthropic_api_key", "openai.org", "a-b-c", "x1"] {
            assert!(CredentialId::parse(id).is_ok(), "{id} should parse");
        }
    }

    #[test]
    fn rejects_invalid_ids() {
        for id in ["", "Upper", "has space", "tab\t", "UPPER_CASE"] {
            assert!(CredentialId::parse(id).is_err(), "{id} should be rejected");
        }
    }

    #[test]
    fn as_str_roundtrips() {
        let id = CredentialId::parse("anthropic_api_key").unwrap();
        assert_eq!(id.as_str(), "anthropic_api_key");
    }
}
```

(Module wired in Task 1.6 — no separate run/commit here.)

## Task 1.4: `CredentialError`

**Files:**
- Modify: `crates/tau-ports/src/error.rs`

- [ ] **Step 1: Add the enum + test**

At the end of `crates/tau-ports/src/error.rs` (matching the existing `#[non_exhaustive]` thiserror pattern seen on `LlmError`):

```rust
/// Errors raised while resolving a credential through a provider or chain.
///
/// Chain walk semantics distinguish `Ok(None)` ("not here, try the next
/// provider") from `Err` ("this provider owns the request but failed").
/// The chain fails fast on any `Err`.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum CredentialError {
    /// The whole chain was walked and no provider held the credential.
    #[error("credential not found: {id}")]
    NotFound {
        /// The logical credential id that was not found.
        id: String,
    },
    /// A configured provider could not be reached (e.g. Vault down).
    #[error("credential provider {provider} unavailable: {reason}")]
    ProviderUnavailable {
        /// Provider name.
        provider: String,
        /// Human-readable reason.
        reason: String,
    },
    /// A provider found the credential but its content was malformed.
    #[error("credential {id} malformed: {reason}")]
    Malformed {
        /// The logical credential id.
        id: String,
        /// Human-readable reason.
        reason: String,
    },
    /// An I/O error occurred while resolving (e.g. unreadable secret file).
    #[error("credential I/O error: {reason}")]
    Io {
        /// Human-readable reason.
        reason: String,
    },
    /// An unexpected internal error.
    #[error("internal credential error: {reason}")]
    Internal {
        /// Human-readable reason.
        reason: String,
    },
}
```

(If `error.rs` does not already `use alloc::string::String;` at the top, it does — `LlmError` uses `String`. No new import needed.)

- [ ] **Step 2: Re-export from error**

This enum is re-exported via the `error` re-export line in `lib.rs` in Task 1.6.

## Task 1.5: `CredentialProvider` trait, request/response, `CredentialChain`, `BakedProvider`

**Files:**
- Create: `crates/tau-ports/src/credential/mod.rs`
- Create: `crates/tau-ports/src/credential/chain.rs`
- Create: `crates/tau-ports/src/credential/baked.rs`

- [ ] **Step 1: Write `mod.rs` (trait + value types)**

Create `crates/tau-ports/src/credential/mod.rs`:

```rust
//! Credential provider chain port (β.5).
//!
//! [`CredentialProvider`] is the Strategy: each provider knows how to
//! resolve a credential from one source (env, file, baked, …).
//! [`CredentialChain`] is the composite that walks providers in order.
//!
//! The port uses native `async fn in trait` (per ADR-0003); the
//! dyn-compatible shim lives in [`chain`] and mirrors the boxed-future
//! pattern from `tau-runtime-core/src/builder.rs`.

mod baked;
mod chain;
pub mod id;
pub mod secret;

pub use baked::BakedProvider;
pub use chain::{CredentialChain, DynCredentialProvider};
pub use id::{CredentialId, InvalidCredentialId};
pub use secret::Secret;

use alloc::string::String;

use crate::error::CredentialError;

/// What a consumer wants resolved.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct CredentialRequest {
    /// The logical credential id the chain resolves.
    pub id: CredentialId,
    /// The environment-variable name to read, used by [`crate`]'s env
    /// provider. Other providers ignore it.
    pub env_name: Option<String>,
}

impl CredentialRequest {
    /// Construct a request for the given id with no provider hints.
    pub fn new(id: CredentialId) -> Self {
        Self { id, env_name: None }
    }

    /// Attach the environment-variable name hint (for the env provider).
    pub fn with_env_name(mut self, env_name: impl Into<String>) -> Self {
        self.env_name = Some(env_name.into());
        self
    }
}

/// A successfully resolved credential.
#[non_exhaustive]
pub struct ResolvedCredential {
    /// The secret value.
    pub secret: Secret,
    /// Optional Unix-millis expiry for rotating providers. `None` = no
    /// known expiry; the consumer re-resolves past expiry.
    pub expires_at: Option<i64>,
    /// Which provider satisfied the request (for tracing/audit).
    pub source: &'static str,
}

impl ResolvedCredential {
    /// Construct a resolved credential with no expiry.
    pub fn new(secret: Secret, source: &'static str) -> Self {
        Self { secret, expires_at: None, source }
    }

    /// Set the expiry (Unix millis).
    pub fn with_expiry(mut self, expires_at: i64) -> Self {
        self.expires_at = Some(expires_at);
        self
    }
}

/// A strategy for resolving a credential from one source.
///
/// `Ok(Some(_))` = resolved. `Ok(None)` = not here, try the next
/// provider. `Err(_)` = this provider owns the request but failed.
#[allow(async_fn_in_trait)]
pub trait CredentialProvider: Send + Sync {
    /// Stable provider name (e.g. `"env"`, `"file"`, `"baked"`).
    fn name(&self) -> &str;

    /// Resolve the requested credential.
    async fn resolve(
        &self,
        req: &CredentialRequest,
    ) -> Result<Option<ResolvedCredential>, CredentialError>;
}
```

- [ ] **Step 2: Write `chain.rs` (dyn shim + combinator)**

Create `crates/tau-ports/src/credential/chain.rs`:

```rust
//! Dyn-compatible wrapper + the [`CredentialChain`] combinator.
//!
//! `CredentialProvider` uses native `async fn in trait`, which is not
//! dyn-compatible. [`DynCredentialProvider`] is the object-safe shim
//! (boxed, non-`Send` futures — matching `tau-runtime-core`'s
//! `BoxFuture` at `builder.rs:84`), with a blanket impl for every
//! `CredentialProvider`. The chain stores `Arc<dyn DynCredentialProvider>`.

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::future::Future;
use core::pin::Pin;

use super::{CredentialProvider, CredentialRequest, ResolvedCredential};
use crate::error::CredentialError;

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

/// Object-safe wrapper for [`CredentialProvider`]. Authors implement
/// `CredentialProvider`; the blanket impl below handles the dyn-cast.
pub trait DynCredentialProvider: Send + Sync {
    /// Provider name (matches [`CredentialProvider::name`]).
    fn name(&self) -> &str;

    /// Boxed-future wrapper for [`CredentialProvider::resolve`].
    fn resolve<'a>(
        &'a self,
        req: &'a CredentialRequest,
    ) -> BoxFuture<'a, Result<Option<ResolvedCredential>, CredentialError>>;
}

impl<T: CredentialProvider + 'static> DynCredentialProvider for T {
    fn name(&self) -> &str {
        CredentialProvider::name(self)
    }

    fn resolve<'a>(
        &'a self,
        req: &'a CredentialRequest,
    ) -> BoxFuture<'a, Result<Option<ResolvedCredential>, CredentialError>> {
        Box::pin(CredentialProvider::resolve(self, req))
    }
}

/// A composite provider that walks members in declared order. First
/// `Ok(Some)` wins; `Ok(None)` continues; `Err` fails fast.
pub struct CredentialChain {
    members: Vec<Arc<dyn DynCredentialProvider>>,
}

impl CredentialChain {
    /// An empty chain (resolves everything to `Ok(None)`).
    pub fn new() -> Self {
        Self { members: Vec::new() }
    }

    /// Builder-style: append a provider.
    pub fn with(mut self, provider: Arc<dyn DynCredentialProvider>) -> Self {
        self.members.push(provider);
        self
    }

    /// Append a provider in place.
    pub fn push(&mut self, provider: Arc<dyn DynCredentialProvider>) {
        self.members.push(provider);
    }

    /// Number of members.
    pub fn len(&self) -> usize {
        self.members.len()
    }

    /// Whether the chain has no members.
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }
}

impl Default for CredentialChain {
    fn default() -> Self {
        Self::new()
    }
}

impl CredentialProvider for CredentialChain {
    fn name(&self) -> &str {
        "chain"
    }

    async fn resolve(
        &self,
        req: &CredentialRequest,
    ) -> Result<Option<ResolvedCredential>, CredentialError> {
        for member in &self.members {
            match member.resolve(req).await {
                Ok(Some(resolved)) => return Ok(Some(resolved)),
                Ok(None) => continue,
                Err(err) => return Err(err), // fail-fast
            }
        }
        Ok(None)
    }
}
```

- [ ] **Step 3: Write `baked.rs`**

Create `crates/tau-ports/src/credential/baked.rs`:

```rust
//! In-memory credential provider. `no_std`-friendly; deterministic.
//! Doubles as a test provider and the seed for embedded/wasm hosts.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use super::{CredentialId, CredentialProvider, CredentialRequest, ResolvedCredential, Secret};
use crate::error::CredentialError;

/// A credential provider backed by an in-memory map.
#[derive(Default)]
pub struct BakedProvider {
    entries: BTreeMap<CredentialId, Vec<u8>>,
}

impl BakedProvider {
    /// An empty provider.
    pub fn new() -> Self {
        Self::default()
    }

    /// Builder-style: insert an entry.
    pub fn with(mut self, id: CredentialId, value: impl Into<Vec<u8>>) -> Self {
        self.entries.insert(id, value.into());
        self
    }

    /// Insert an entry in place.
    pub fn insert(&mut self, id: CredentialId, value: impl Into<Vec<u8>>) {
        self.entries.insert(id, value.into());
    }
}

impl CredentialProvider for BakedProvider {
    fn name(&self) -> &str {
        "baked"
    }

    async fn resolve(
        &self,
        req: &CredentialRequest,
    ) -> Result<Option<ResolvedCredential>, CredentialError> {
        Ok(self
            .entries
            .get(&req.id)
            .map(|v| ResolvedCredential::new(Secret::from_bytes(v.clone()), "baked")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn id(s: &str) -> CredentialId {
        CredentialId::parse(s).unwrap()
    }

    #[tokio::test]
    async fn resolves_present_key() {
        let p = BakedProvider::new().with(id("anthropic_api_key"), b"sk-ant-x".to_vec());
        let req = CredentialRequest::new(id("anthropic_api_key"));
        let got = p.resolve(&req).await.unwrap().unwrap();
        assert_eq!(got.secret.expose_bytes(), b"sk-ant-x");
        assert_eq!(got.source, "baked");
        assert_eq!(got.expires_at, None);
    }

    #[tokio::test]
    async fn absent_key_is_none() {
        let p = BakedProvider::new();
        let req = CredentialRequest::new(id("missing"));
        assert!(p.resolve(&req).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn chain_walks_to_first_match() {
        use super::super::CredentialChain;
        let empty = BakedProvider::new();
        let full = BakedProvider::new().with(id("k"), b"v".to_vec());
        let chain = CredentialChain::new()
            .with(alloc::sync::Arc::new(empty))
            .with(alloc::sync::Arc::new(full));
        let req = CredentialRequest::new(id("k"));
        let got = chain.resolve(&req).await.unwrap().unwrap();
        assert_eq!(got.secret.expose_bytes(), b"v");
    }

    #[test]
    fn touch_vec_import() {
        let _v: Vec<u8> = vec![1, 2, 3];
    }
}
```

## Task 1.6: Wire the module + re-exports, build & test

**Files:**
- Modify: `crates/tau-ports/src/lib.rs`

- [ ] **Step 1: Declare the module and re-exports**

In `crates/tau-ports/src/lib.rs`, add `pub mod credential;` after `pub mod capability_resolver;`:

```rust
pub mod capability_resolver;
pub mod credential;
```

In the re-export section, after the existing `pub use error::{...}` line, extend the error re-export to include `CredentialError` and add a credential re-export:

```rust
pub use error::{
    CapabilityError, CredentialError, KeyError, LlmError, NamespaceError, StorageError, ToolError,
};
pub use credential::{
    BakedProvider, CredentialChain, CredentialId, CredentialProvider, CredentialRequest,
    DynCredentialProvider, InvalidCredentialId, ResolvedCredential, Secret,
};
```

(If the existing `pub use error::{...}` line lists a different exact set, insert `CredentialError` alphabetically into that existing list rather than duplicating the line.)

- [ ] **Step 2: Build**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-ports`
Expected: PASS.

- [ ] **Step 3: Run the tests**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ports`
Expected: PASS — all `secret`, `id`, and `baked` tests green.

- [ ] **Step 4: Run the fixtures-feature build (CI parity)**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ports --features test-fixtures`
Expected: PASS.

- [ ] **Step 5: fmt + clippy**

Run: `timeout 30 env CARGO_TARGET_DIR=target/agent-impl cargo fmt -p tau-ports`
Run: `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-ports --all-targets`
Expected: no warnings.

- [ ] **Step 6: Flip ADR status to Accepted**

In `docs/decisions/0046-credential-provider-chain.md`, the `**Status:**` line is already `Accepted` (set during the spec phase). Confirm; no change needed.

- [ ] **Step 7: Commit**

```bash
git add crates/tau-ports/src/credential crates/tau-ports/src/lib.rs crates/tau-ports/src/error.rs
git commit -m "feat(tau-ports): CredentialProvider port + Secret + CredentialChain + BakedProvider"
```

---

# PR-2 — Adapters + CI (`tau-runtime-tokio`)

Concrete std I/O providers and the CI lane that exercises them.

## Task 2.1: `EnvProvider`

**Files:**
- Create: `crates/tau-runtime-tokio/src/credentials/mod.rs`
- Create: `crates/tau-runtime-tokio/src/credentials/env.rs`
- Modify: `crates/tau-runtime-tokio/src/lib.rs`

- [ ] **Step 1: Create the module glue**

Create `crates/tau-runtime-tokio/src/credentials/mod.rs`:

```rust
//! Concrete credential providers for the tokio host (β.5).
//!
//! The port + chain combinator live in `tau-ports`; these are the
//! std I/O adapters that need the filesystem and process environment.

mod env;
mod file;

pub use env::EnvProvider;
pub use file::FileProvider;
```

Create `crates/tau-runtime-tokio/src/credentials/env.rs`:

```rust
//! Environment-variable credential provider. This is tau's historical,
//! zero-config default: read the credential from the declared env var.

use tau_ports::credential::{
    CredentialProvider, CredentialRequest, ResolvedCredential, Secret,
};
use tau_ports::CredentialError;

/// Resolves a credential by reading the request's `env_name` from a
/// lookup function (process environment by default). An absent or empty
/// variable resolves to `Ok(None)` so the chain continues.
pub struct EnvProvider<F = fn(&str) -> Option<String>> {
    lookup: F,
}

impl EnvProvider<fn(&str) -> Option<String>> {
    /// A provider that reads the real process environment.
    pub fn from_process_env() -> Self {
        Self { lookup: |name| std::env::var(name).ok() }
    }
}

impl<F> EnvProvider<F>
where
    F: Fn(&str) -> Option<String> + Send + Sync,
{
    /// A provider that reads from a custom lookup (used in tests).
    pub fn new(lookup: F) -> Self {
        Self { lookup }
    }
}

impl<F> CredentialProvider for EnvProvider<F>
where
    F: Fn(&str) -> Option<String> + Send + Sync,
{
    fn name(&self) -> &str {
        "env"
    }

    async fn resolve(
        &self,
        req: &CredentialRequest,
    ) -> Result<Option<ResolvedCredential>, CredentialError> {
        let Some(var) = req.env_name.as_deref() else {
            return Ok(None);
        };
        match (self.lookup)(var) {
            Some(v) if !v.is_empty() => Ok(Some(ResolvedCredential::new(
                Secret::from_bytes(v.into_bytes()),
                "env",
            ))),
            _ => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tau_ports::credential::CredentialId;

    fn req(id: &str, env: &str) -> CredentialRequest {
        CredentialRequest::new(CredentialId::parse(id).unwrap()).with_env_name(env)
    }

    #[tokio::test]
    async fn resolves_present_var() {
        let p = EnvProvider::new(|n| (n == "ANTHROPIC_API_KEY").then(|| "sk-ant-z".to_string()));
        let got = p.resolve(&req("anthropic_api_key", "ANTHROPIC_API_KEY")).await.unwrap();
        assert_eq!(got.unwrap().secret.expose_str().unwrap(), "sk-ant-z");
    }

    #[tokio::test]
    async fn absent_var_is_none() {
        let p = EnvProvider::new(|_| None);
        assert!(p.resolve(&req("x", "MISSING")).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn empty_var_is_none() {
        let p = EnvProvider::new(|_| Some(String::new()));
        assert!(p.resolve(&req("x", "EMPTY")).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn no_env_hint_is_none() {
        let p = EnvProvider::new(|_| Some("v".to_string()));
        let r = CredentialRequest::new(CredentialId::parse("x").unwrap());
        assert!(p.resolve(&r).await.unwrap().is_none());
    }
}
```

- [ ] **Step 2: Declare the module**

In `crates/tau-runtime-tokio/src/lib.rs`, add after `pub mod clock;`:

```rust
pub mod credentials;
```

- [ ] **Step 3: Run the env tests**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-tokio credentials::env`
Expected: PASS (4 tests). (The `file` module is referenced by `mod.rs` but not yet created — create it in Task 2.2 before this compiles; if iterating strictly, temporarily comment `mod file;`/`pub use file::FileProvider;` then restore in 2.2. Recommended: do 2.1 + 2.2 together, then run.)

## Task 2.2: `FileProvider`

**Files:**
- Create: `crates/tau-runtime-tokio/src/credentials/file.rs`

- [ ] **Step 1: Write the provider + tests**

Create `crates/tau-runtime-tokio/src/credentials/file.rs`:

```rust
//! Mounted-secret-directory credential provider. Reads
//! `<dir>/<key_map[id]>` and trims a single trailing newline. The DoD
//! CI provider (Kubernetes / Docker secret mounts).

use std::collections::BTreeMap;
use std::path::PathBuf;

use tau_ports::credential::{
    CredentialProvider, CredentialRequest, ResolvedCredential, Secret,
};
use tau_ports::CredentialError;

/// Resolves a credential by reading a file from a secrets directory.
/// The `key_map` maps a logical credential id to a filename in `dir`.
pub struct FileProvider {
    dir: PathBuf,
    key_map: BTreeMap<String, String>,
}

impl FileProvider {
    /// Construct from a directory and an id→filename map.
    pub fn new(dir: PathBuf, key_map: BTreeMap<String, String>) -> Self {
        Self { dir, key_map }
    }
}

/// Trim a single trailing `\n` (and a preceding `\r`) — mounted secrets
/// often carry a trailing newline that is not part of the key.
fn trim_trailing_newline(mut bytes: Vec<u8>) -> Vec<u8> {
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
    }
    bytes
}

impl CredentialProvider for FileProvider {
    fn name(&self) -> &str {
        "file"
    }

    async fn resolve(
        &self,
        req: &CredentialRequest,
    ) -> Result<Option<ResolvedCredential>, CredentialError> {
        let Some(filename) = self.key_map.get(req.id.as_str()) else {
            return Ok(None);
        };
        let path = self.dir.join(filename);
        match tokio::fs::read(&path).await {
            Ok(bytes) => Ok(Some(ResolvedCredential::new(
                Secret::from_bytes(trim_trailing_newline(bytes)),
                "file",
            ))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(CredentialError::Io {
                reason: format!("{}: {e}", path.display()),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tau_ports::credential::CredentialId;

    fn key_map() -> BTreeMap<String, String> {
        let mut m = BTreeMap::new();
        m.insert("anthropic_api_key".to_string(), "anthropic-key".to_string());
        m
    }

    fn req(id: &str) -> CredentialRequest {
        CredentialRequest::new(CredentialId::parse(id).unwrap())
    }

    #[tokio::test]
    async fn reads_and_trims_newline() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("anthropic-key"), b"sk-ant-file\n").unwrap();
        let p = FileProvider::new(dir.path().to_path_buf(), key_map());
        let got = p.resolve(&req("anthropic_api_key")).await.unwrap().unwrap();
        assert_eq!(got.secret.expose_str().unwrap(), "sk-ant-file");
        assert_eq!(got.source, "file");
    }

    #[tokio::test]
    async fn unmapped_id_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let p = FileProvider::new(dir.path().to_path_buf(), key_map());
        assert!(p.resolve(&req("other_id")).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn missing_file_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let p = FileProvider::new(dir.path().to_path_buf(), key_map());
        assert!(p.resolve(&req("anthropic_api_key")).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn chain_env_then_file() {
        use std::sync::Arc;
        use tau_ports::credential::CredentialChain;
        use crate::credentials::EnvProvider;

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("anthropic-key"), b"from-file").unwrap();

        // env miss -> file hit
        let chain = CredentialChain::new()
            .with(Arc::new(EnvProvider::new(|_| None)))
            .with(Arc::new(FileProvider::new(dir.path().to_path_buf(), key_map())));
        let r = CredentialRequest::new(CredentialId::parse("anthropic_api_key").unwrap())
            .with_env_name("ANTHROPIC_API_KEY");
        let got = chain.resolve(&r).await.unwrap().unwrap();
        assert_eq!(got.secret.expose_str().unwrap(), "from-file");
        assert_eq!(got.source, "file");
    }
}
```

- [ ] **Step 2: Run both provider test modules**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-tokio credentials`
Expected: PASS — env (4) + file (4) tests green.

- [ ] **Step 3: fmt + clippy**

Run: `timeout 30 env CARGO_TARGET_DIR=target/agent-impl cargo fmt -p tau-runtime-tokio`
Run: `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-runtime-tokio --all-targets`
Expected: no warnings.

- [ ] **Step 4: Commit**

```bash
git add crates/tau-runtime-tokio/src/credentials crates/tau-runtime-tokio/src/lib.rs
git commit -m "feat(tau-runtime-tokio): Env + File credential providers"
```

## Task 2.3: CI lane

**Files:**
- Modify: `.github/workflows/ci.yml`

- [ ] **Step 1: Add the job**

In `.github/workflows/ci.yml`, after the `test-fixtures-ports` job (ends ~line 292), add a sibling job. Match the existing pinned `actions/checkout` SHA and `setup-rust` block exactly as used by `test-fixtures-ports`:

```yaml
  test-credential-chain:
    name: test-credential-chain / linux
    runs-on: ubuntu-latest
    timeout-minutes: 20
    steps:
      - uses: actions/checkout@df4cb1c069e1874edd31b4311f1884172cec0e10  # v6
      - uses: ./.github/actions/setup-rust
        with:
          shared-key: linux-stable
          with-nextest: true
          with-sccache: true
          with-mold: true
      - name: Credential port tests (tau-ports)
        run: cargo nextest run --profile ci -p tau-ports credential
      - name: Credential provider tests (tau-runtime-tokio)
        run: cargo nextest run --profile ci -p tau-runtime-tokio credentials
```

(If `ci.yml` aggregates required jobs into a `ci-summary`/`needs:` list, add `test-credential-chain` to that `needs:` array so the lane is gating. Search for `ci-summary` and `needs:` near the end of the file; if present, append `test-credential-chain` to the list.)

- [ ] **Step 2: Validate the YAML locally**

Run: `python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/ci.yml')); print('ok')"`
Expected: `ok`.

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "ci: add test-credential-chain / linux lane"
```

---

# PR-3 — Config (`tau-pkg`)

Two Unchecked→validate surfaces: per-agent declaration (project) and chain config (scope/home).

## Task 3.1: Per-agent credential declaration

**Files:**
- Modify: `crates/tau-pkg/src/project/project.rs`

- [ ] **Step 1: Write the failing validation tests**

In `crates/tau-pkg/src/project/project.rs`, in the existing `#[cfg(test)] mod tests` block, add (adapt helper names to the module's existing test helpers for building an `UncheckedProjectConfig`/agent — search the test module for an existing `fn ... -> UncheckedAgent` or a TOML-parse helper and reuse it):

```rust
#[test]
fn agent_credentials_validate_ok() {
    let toml = r#"
[project]
name = "p"
description = "d"

[agents.assistant]
display_name = "A"
package = "anthropic@^1"
llm_backend = "anthropic"

[[agents.assistant.credentials]]
id = "anthropic_api_key"
env = "ANTHROPIC_API_KEY"
"#;
    let cfg: UncheckedProjectConfig = toml::from_str(toml).unwrap();
    let validated = cfg.validate().unwrap();
    let agent = validated.agents.get("assistant").unwrap();
    assert_eq!(agent.credentials.len(), 1);
    assert_eq!(agent.credentials[0].id.as_str(), "anthropic_api_key");
    assert_eq!(agent.credentials[0].env, "ANTHROPIC_API_KEY");
}

#[test]
fn agent_credentials_reject_bad_id() {
    let toml = r#"
[project]
name = "p"
description = "d"
[agents.a]
display_name = "A"
package = "x@^1"
llm_backend = "x"
[[agents.a.credentials]]
id = "Bad Id"
env = "X"
"#;
    let cfg: UncheckedProjectConfig = toml::from_str(toml).unwrap();
    assert!(cfg.validate().is_err());
}

#[test]
fn agent_credentials_reject_bad_env_name() {
    let toml = r#"
[project]
name = "p"
description = "d"
[agents.a]
display_name = "A"
package = "x@^1"
llm_backend = "x"
[[agents.a.credentials]]
id = "ok_id"
env = "lower_case"
"#;
    let cfg: UncheckedProjectConfig = toml::from_str(toml).unwrap();
    assert!(cfg.validate().is_err());
}

#[test]
fn agent_credentials_reject_duplicate_env() {
    let toml = r#"
[project]
name = "p"
description = "d"
[agents.a]
display_name = "A"
package = "x@^1"
llm_backend = "x"
[[agents.a.credentials]]
id = "id_one"
env = "SAME"
[[agents.a.credentials]]
id = "id_two"
env = "SAME"
"#;
    let cfg: UncheckedProjectConfig = toml::from_str(toml).unwrap();
    assert!(cfg.validate().is_err());
}
```

- [ ] **Step 2: Run — expect failure (fields/types don't exist yet)**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg agent_credentials`
Expected: FAIL — `UncheckedAgent` has no `credentials` field; `AgentEntry` has no `credentials`.

- [ ] **Step 3: Add the unchecked + validated types**

In `crates/tau-pkg/src/project/project.rs`, add near `UncheckedAgent` (after the struct):

```rust
/// `[[agents.<id>.credentials]]` entry — unchecked deserialization.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UncheckedAgentCredential {
    /// Logical credential id the chain resolves (e.g. `anthropic_api_key`).
    pub id: String,
    /// Environment-variable name the host injects the resolved secret into.
    pub env: String,
}

/// Validated per-agent credential declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCredential {
    /// Validated logical credential id.
    pub id: tau_ports::CredentialId,
    /// Validated environment-variable name (`[A-Z_][A-Z0-9_]*`).
    pub env: String,
}
```

Add the field to `UncheckedAgent` (after `produces`):

```rust
    /// `[[agents.<id>.credentials]]` declarations; default empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub credentials: Vec<UncheckedAgentCredential>,
```

Add the field to `AgentEntry` (after `context`):

```rust
    /// Validated credential declarations (β.5).
    pub credentials: Vec<AgentCredential>,
```

- [ ] **Step 4: Add the `ProjectConfigError` variant**

In the `ProjectConfigError` enum, add:

```rust
    /// A credential declaration on an agent failed validation.
    #[error("agent {id:?}: credential declaration invalid: {message}")]
    CredentialDeclaration {
        /// Agent id whose credential declaration failed.
        id: String,
        /// Human-readable reason.
        message: String,
    },
```

- [ ] **Step 5: Validate in `validate_agent`**

In `validate_agent`, before constructing the `AgentEntry`, add (uses `tau_ports::CredentialId`; ensure `tau-ports` is a dep of `tau-pkg` — it already is, used elsewhere):

```rust
    // β.5: validate credential declarations.
    let mut credentials = Vec::with_capacity(raw.credentials.len());
    let mut seen_envs = alloc_set();
    for cred in raw.credentials {
        let cid = tau_ports::CredentialId::parse(cred.id.clone()).map_err(|e| {
            ProjectConfigError::CredentialDeclaration {
                id: id.clone(),
                message: alloc::format!("invalid id {:?}: {}", cred.id, e.reason),
            }
        })?;
        if !is_valid_env_name(&cred.env) {
            return Err(ProjectConfigError::CredentialDeclaration {
                id: id.clone(),
                message: alloc::format!("invalid env var name {:?} (must match [A-Z_][A-Z0-9_]*)", cred.env),
            });
        }
        if !seen_envs.insert(cred.env.clone()) {
            return Err(ProjectConfigError::CredentialDeclaration {
                id: id.clone(),
                message: alloc::format!("duplicate env var {:?}", cred.env),
            });
        }
        credentials.push(AgentCredential { id: cid, env: cred.env });
    }
```

And include `credentials` in the `AgentEntry { ... }` construction.

If the module is `std` (not `no_std`), replace `alloc::format!` with `format!`, `alloc_set()` with `std::collections::BTreeSet::new()`, and drop the helper. Add the env-name helper near the other free functions in the file:

```rust
/// Returns true if `name` is a valid POSIX-ish env var name: `[A-Z_][A-Z0-9_]*`.
fn is_valid_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_uppercase() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}
```

(`tau-pkg` is a `std` crate — use `std::collections::BTreeSet` directly and `format!`; remove the `alloc_set()` placeholder.)

- [ ] **Step 6: Run the tests**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg agent_credentials`
Expected: PASS (4 tests).

- [ ] **Step 7: Run the full tau-pkg suite (catch construction-site breaks)**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg`
Expected: PASS. If other `AgentEntry { ... }` literals exist, the compiler flags them — add `credentials: Vec::new()` (or the validated vec) at each. Fix until green.

- [ ] **Step 8: fmt + commit**

```bash
git add crates/tau-pkg/src/project/project.rs
git commit -m "feat(tau-pkg): per-agent [[credentials]] declaration + validation"
```

## Task 3.2: Scope-level chain config

**Files:**
- Create: `crates/tau-pkg/src/scope_credentials.rs`
- Modify: `crates/tau-pkg/src/scope.rs`
- Modify: `crates/tau-pkg/src/lib.rs`

- [ ] **Step 1: Write the validated config module + tests**

Create `crates/tau-pkg/src/scope_credentials.rs`:

```rust
//! `[credentials]` chain configuration (β.5), stored in scope/home
//! `config.toml`. Deployment-specific: the same bundle resolves
//! credentials from env locally, files in k8s, or (later) Vault in prod.
//!
//! Unchecked→validate discipline mirrors `project::project`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Unchecked `[credentials]` block.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct UncheckedCredentialsConfig {
    /// Provider names in precedence order. Empty → implicit `["env"]`.
    #[serde(default)]
    pub chain: Vec<String>,
    /// Provider definitions keyed by name.
    #[serde(default)]
    pub providers: BTreeMap<String, UncheckedProvider>,
}

/// Unchecked provider definition (`[credentials.providers.<name>]`).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UncheckedProvider {
    /// Provider kind: `"env"` or `"file"`.
    #[serde(rename = "type")]
    pub kind: String,
    /// `file`: secrets directory.
    #[serde(default)]
    pub dir: Option<String>,
    /// `file`: credential-id → filename map.
    #[serde(default)]
    pub key_map: BTreeMap<String, String>,
}

/// Validated chain configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialsChainConfig {
    /// Ordered, validated providers.
    pub chain: Vec<ProviderConfig>,
}

/// A validated provider configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderConfig {
    /// Environment-variable provider.
    Env,
    /// File provider with a secrets dir and id→filename map.
    File {
        /// Secrets directory.
        dir: String,
        /// Credential-id → filename.
        key_map: BTreeMap<String, String>,
    },
}

/// Errors validating a `[credentials]` block.
#[non_exhaustive]
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CredentialsConfigError {
    /// A name in `chain` has no matching provider definition.
    #[error("chain references undefined provider {0:?}")]
    UndefinedProvider(String),
    /// A provider has an unknown `type`.
    #[error("provider {name:?}: unknown type {kind:?}")]
    UnknownKind {
        /// Provider name.
        name: String,
        /// The unknown kind string.
        kind: String,
    },
    /// A `file` provider is missing `dir`.
    #[error("file provider {0:?}: missing `dir`")]
    FileMissingDir(String),
}

impl Default for CredentialsChainConfig {
    fn default() -> Self {
        // Zero-config default: env-only.
        Self { chain: vec![ProviderConfig::Env] }
    }
}

impl UncheckedCredentialsConfig {
    /// Validate into a [`CredentialsChainConfig`]. An empty `chain`
    /// defaults to `["env"]`. `"env"` needs no provider definition.
    pub fn validate(self) -> Result<CredentialsChainConfig, CredentialsConfigError> {
        let names = if self.chain.is_empty() {
            vec!["env".to_string()]
        } else {
            self.chain
        };

        let mut chain = Vec::with_capacity(names.len());
        for name in names {
            if name == "env" && !self.providers.contains_key("env") {
                chain.push(ProviderConfig::Env);
                continue;
            }
            let def = self
                .providers
                .get(&name)
                .ok_or_else(|| CredentialsConfigError::UndefinedProvider(name.clone()))?;
            match def.kind.as_str() {
                "env" => chain.push(ProviderConfig::Env),
                "file" => {
                    let dir = def
                        .dir
                        .clone()
                        .ok_or_else(|| CredentialsConfigError::FileMissingDir(name.clone()))?;
                    chain.push(ProviderConfig::File { dir, key_map: def.key_map.clone() });
                }
                other => {
                    return Err(CredentialsConfigError::UnknownKind {
                        name: name.clone(),
                        kind: other.to_string(),
                    });
                }
            }
        }
        Ok(CredentialsChainConfig { chain })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_defaults_to_env_only() {
        let cfg = UncheckedCredentialsConfig::default().validate().unwrap();
        assert_eq!(cfg.chain, vec![ProviderConfig::Env]);
    }

    #[test]
    fn env_then_file_validates() {
        let toml = r#"
chain = ["env", "file"]
[providers.file]
type = "file"
dir = "/var/run/secrets"
key_map = { anthropic_api_key = "anthropic-key" }
"#;
        let unchecked: UncheckedCredentialsConfig = toml::from_str(toml).unwrap();
        let cfg = unchecked.validate().unwrap();
        assert_eq!(cfg.chain.len(), 2);
        assert_eq!(cfg.chain[0], ProviderConfig::Env);
        match &cfg.chain[1] {
            ProviderConfig::File { dir, key_map } => {
                assert_eq!(dir, "/var/run/secrets");
                assert_eq!(key_map.get("anthropic_api_key").unwrap(), "anthropic-key");
            }
            _ => panic!("expected file provider"),
        }
    }

    #[test]
    fn undefined_provider_rejected() {
        let toml = r#"chain = ["vault"]"#;
        let unchecked: UncheckedCredentialsConfig = toml::from_str(toml).unwrap();
        assert_eq!(
            unchecked.validate().unwrap_err(),
            CredentialsConfigError::UndefinedProvider("vault".to_string())
        );
    }

    #[test]
    fn file_without_dir_rejected() {
        let toml = r#"
chain = ["file"]
[providers.file]
type = "file"
"#;
        let unchecked: UncheckedCredentialsConfig = toml::from_str(toml).unwrap();
        assert_eq!(
            unchecked.validate().unwrap_err(),
            CredentialsConfigError::FileMissingDir("file".to_string())
        );
    }

    #[test]
    fn unknown_kind_rejected() {
        let toml = r#"
chain = ["weird"]
[providers.weird]
type = "smoke-signal"
"#;
        let unchecked: UncheckedCredentialsConfig = toml::from_str(toml).unwrap();
        assert!(matches!(
            unchecked.validate().unwrap_err(),
            CredentialsConfigError::UnknownKind { .. }
        ));
    }
}
```

- [ ] **Step 2: Declare the module**

In `crates/tau-pkg/src/lib.rs`, add `pub mod scope_credentials;` alongside the other `pub mod` lines.

- [ ] **Step 3: Add the field to `ScopeConfig`**

In `crates/tau-pkg/src/scope.rs`, add to `ScopeConfig` (additive, `#[serde(default)]` so existing `config.toml` files keep parsing):

```rust
    /// `[credentials]` chain configuration (β.5). Absent → env-only default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials: Option<crate::scope_credentials::UncheckedCredentialsConfig>,
```

- [ ] **Step 4: Run the tests**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg scope_credentials`
Expected: PASS (5 tests).

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg`
Expected: PASS (scope.rs `ScopeConfig` construction sites, if any, get a `credentials: None` — fix until green).

- [ ] **Step 5: fmt + commit**

```bash
git add crates/tau-pkg/src/scope_credentials.rs crates/tau-pkg/src/scope.rs crates/tau-pkg/src/lib.rs
git commit -m "feat(tau-pkg): scope-level [credentials] chain config + validation"
```

---

# PR-4 — Host bridge (`tau-runtime-tokio`)

Wire it end to end: build the chain from config, resolve each declared credential, inject into the child env before spawn.

## Task 4.1: `build_chain`

**Files:**
- Create: `crates/tau-runtime-tokio/src/credentials/build.rs`
- Modify: `crates/tau-runtime-tokio/src/credentials/mod.rs`

- [ ] **Step 1: Write `build_chain` + test**

Create `crates/tau-runtime-tokio/src/credentials/build.rs`:

```rust
//! Build a [`CredentialChain`] from validated scope config.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use tau_pkg::scope_credentials::{CredentialsChainConfig, ProviderConfig};
use tau_ports::credential::CredentialChain;

use super::{EnvProvider, FileProvider};

/// Construct a runnable [`CredentialChain`] from validated config.
/// `env` members read the real process environment.
pub fn build_chain(config: &CredentialsChainConfig) -> CredentialChain {
    let mut chain = CredentialChain::new();
    for provider in &config.chain {
        match provider {
            ProviderConfig::Env => {
                chain.push(Arc::new(EnvProvider::from_process_env()));
            }
            ProviderConfig::File { dir, key_map } => {
                let key_map: BTreeMap<String, String> = key_map.clone();
                chain.push(Arc::new(FileProvider::new(PathBuf::from(dir), key_map)));
            }
        }
    }
    chain
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_builds_single_env_member() {
        let chain = build_chain(&CredentialsChainConfig::default());
        assert_eq!(chain.len(), 1);
    }

    #[test]
    fn env_then_file_builds_two_members() {
        let mut key_map = BTreeMap::new();
        key_map.insert("k".to_string(), "f".to_string());
        let cfg = CredentialsChainConfig {
            chain: vec![
                ProviderConfig::Env,
                ProviderConfig::File { dir: "/tmp".to_string(), key_map },
            ],
        };
        assert_eq!(build_chain(&cfg).len(), 2);
    }
}
```

- [ ] **Step 2: Export it**

In `crates/tau-runtime-tokio/src/credentials/mod.rs`, add:

```rust
mod build;
pub use build::build_chain;
```

- [ ] **Step 3: Run**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-tokio credentials::build`
Expected: PASS (2 tests).

- [ ] **Step 4: Commit**

```bash
git add crates/tau-runtime-tokio/src/credentials
git commit -m "feat(tau-runtime-tokio): build_chain from validated config"
```

## Task 4.2: Resolve-then-inject in `plugin_host`

**Files:**
- Modify: `crates/tau-runtime-tokio/src/plugin_host/process.rs`
- Create: `crates/tau-runtime-tokio/tests/credential_inject.rs`

This task threads a resolved chain + the agent's credential declarations into the spawn path. The exact plumbing depends on how `spawn_plugin` receives per-agent data today; the steps below describe the seam precisely.

- [ ] **Step 1: Write the integration test first (mock plugin echoes its env)**

Create `crates/tau-runtime-tokio/tests/credential_inject.rs`:

```rust
//! End-to-end: a File-mounted secret reaches an unmodified child process
//! under its declared env var. Uses a tiny inline "plugin" that echoes a
//! requested env var to stdout, proving the resolve-then-inject bridge
//! without depending on a real LLM plugin binary.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use tau_ports::credential::{CredentialChain, CredentialId, CredentialProvider, CredentialRequest};
use tau_runtime_tokio::credentials::{build_chain, EnvProvider, FileProvider};

/// Build a chain [env, file] where env misses and file hits, then assert
/// the secret resolves — this is the value the host injects into a child.
#[tokio::test]
async fn file_secret_resolves_for_injection() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("anthropic-key"), b"sk-ant-injected\n").unwrap();

    let mut key_map = BTreeMap::new();
    key_map.insert("anthropic_api_key".to_string(), "anthropic-key".to_string());

    let chain = CredentialChain::new()
        .with(Arc::new(EnvProvider::new(|_| None)))
        .with(Arc::new(FileProvider::new(
            dir.path().to_path_buf(),
            key_map,
        )));

    let req = CredentialRequest::new(CredentialId::parse("anthropic_api_key").unwrap())
        .with_env_name("ANTHROPIC_API_KEY");
    let resolved = chain.resolve(&req).await.unwrap().expect("file should resolve");
    assert_eq!(resolved.secret.expose_str().unwrap(), "sk-ant-injected");
    assert_eq!(resolved.source, "file");

    // build_chain over the same logical config also yields a 2-member chain.
    use tau_pkg::scope_credentials::{CredentialsChainConfig, ProviderConfig};
    let mut km = BTreeMap::new();
    km.insert("anthropic_api_key".to_string(), "anthropic-key".to_string());
    let cfg = CredentialsChainConfig {
        chain: vec![
            ProviderConfig::Env,
            ProviderConfig::File { dir: dir.path().display().to_string(), key_map: km },
        ],
    };
    let _built: PathBuf = dir.path().to_path_buf();
    assert_eq!(build_chain(&cfg).len(), 2);
}
```

(This test exercises the public resolve + `build_chain` surface end-to-end. The actual env-injection into a spawned subprocess is asserted by Step 4's host-level test once the seam is wired; if the spawn API is awkward to drive from an integration test, this resolve-level assertion plus the unit test in Step 3 is the gating coverage. Do not delete it.)

- [ ] **Step 2: Wire the resolve-then-inject seam**

In `crates/tau-runtime-tokio/src/plugin_host/process.rs`, the spawn path calls `configure_plugin_command_env(&mut command, run_id, agent_id, |n| std::env::var(n).ok());` (~line 279), then later `wrap_spawn`/`spawn`.

Add a step **after** `configure_plugin_command_env` and **before** the sandbox `wrap_spawn` block: for each declared credential, resolve via the chain and `command.env(decl.env, secret)`.

Thread two new inputs into `spawn_plugin` (follow the existing parameter-passing style — these likely come from the same struct that already carries `sandbox`, `plugin_name`, etc.):

```rust
// New params (or fields on the existing spawn-context struct):
//   chain: Option<&tau_ports::credential::CredentialChain>
//   credentials: &[tau_pkg::project::project::AgentCredential]   // agent's declarations

if let Some(chain) = chain {
    use tau_ports::credential::{CredentialProvider, CredentialRequest};
    for decl in credentials {
        let req = CredentialRequest::new(decl.id.clone()).with_env_name(decl.env.clone());
        match chain.resolve(&req).await {
            Ok(Some(resolved)) => {
                // expose_str: API keys are UTF-8; a non-UTF-8 secret for an
                // env-injected credential is a misconfiguration.
                match resolved.secret.expose_str() {
                    Ok(s) => {
                        command.env(&decl.env, s);
                    }
                    Err(_) => {
                        return Err(RuntimeError::CredentialResolution {
                            plugin: plugin_name.clone(),
                            id: decl.id.to_string(),
                            reason: "resolved secret is not valid UTF-8".to_string(),
                        });
                    }
                }
            }
            // Not found anywhere: leave whatever configure_plugin_command_env
            // already set (today's env passthrough). Backward-compatible.
            Ok(None) => {}
            Err(e) => {
                return Err(RuntimeError::CredentialResolution {
                    plugin: plugin_name.clone(),
                    id: decl.id.to_string(),
                    reason: e.to_string(),
                });
            }
        }
    }
}
```

Add the error variant to `RuntimeError` (in `crates/tau-runtime-tokio/src/error.rs`, matching the existing `#[non_exhaustive]` thiserror pattern):

```rust
    /// Resolving a declared credential through the chain failed.
    #[error("plugin {plugin}: credential {id} resolution failed: {reason}")]
    CredentialResolution {
        /// Plugin name.
        plugin: String,
        /// Logical credential id.
        id: String,
        /// Human-readable reason.
        reason: String,
    },
```

Plumb `chain` and `credentials` from the caller. Trace upward from `spawn_plugin` to where the `Runtime` builds the spawn context: the chain is built once (via `build_chain` over the scope config, Task 4.1) and stored on the runtime/host; `credentials` comes from the `AgentEntry.credentials` for the agent being spawned. Where the runtime does not yet thread scope config, default `chain` to `None` (→ today's behavior exactly). **Backward-compat invariant:** `chain == None` OR `credentials.is_empty()` ⇒ zero behavior change.

- [ ] **Step 3: Add a focused unit test for the inject decision (in process.rs)**

If `spawn_plugin` is hard to unit-test directly, extract the inject loop into a testable helper and test it:

```rust
/// Resolve declared credentials against the chain and apply them to
/// `command`'s environment. Returns the env names actually injected.
async fn inject_credentials(
    command: &mut tokio::process::Command,
    chain: Option<&tau_ports::credential::CredentialChain>,
    credentials: &[tau_pkg::project::project::AgentCredential],
    plugin_name: &str,
) -> Result<Vec<String>, RuntimeError> {
    // ... body from Step 2 ...
}
```

```rust
#[cfg(test)]
mod credential_inject_tests {
    use super::*;
    use std::sync::Arc;
    use tau_ports::credential::{BakedProvider, CredentialChain, CredentialId};
    use tau_pkg::project::project::AgentCredential;

    #[tokio::test]
    async fn injects_resolved_secret() {
        let chain = CredentialChain::new().with(Arc::new(
            BakedProvider::new().with(CredentialId::parse("k").unwrap(), b"v".to_vec()),
        ));
        let decls = vec![AgentCredential {
            id: CredentialId::parse("k").unwrap(),
            env: "MY_KEY".to_string(),
        }];
        let mut cmd = tokio::process::Command::new("true");
        let injected = inject_credentials(&mut cmd, Some(&chain), &decls, "test")
            .await
            .unwrap();
        assert_eq!(injected, vec!["MY_KEY".to_string()]);
    }

    #[tokio::test]
    async fn no_chain_injects_nothing() {
        let decls = vec![AgentCredential {
            id: CredentialId::parse("k").unwrap(),
            env: "MY_KEY".to_string(),
        }];
        let mut cmd = tokio::process::Command::new("true");
        let injected = inject_credentials(&mut cmd, None, &decls, "test").await.unwrap();
        assert!(injected.is_empty());
    }
}
```

Make `inject_credentials` return the injected env names for assertability; the production call site ignores the returned vec (or uses it for tracing).

- [ ] **Step 4: Run the tests**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-tokio credential`
Expected: PASS — `credential_inject` integration test + `credential_inject_tests` unit tests.

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-tokio`
Expected: PASS — full crate suite, confirming the spawn-path change broke nothing.

- [ ] **Step 5: fmt + clippy + commit**

```bash
git add crates/tau-runtime-tokio/src/plugin_host/process.rs crates/tau-runtime-tokio/src/error.rs crates/tau-runtime-tokio/tests/credential_inject.rs
git commit -m "feat(tau-runtime-tokio): resolve-then-inject credential bridge in plugin_host"
```

---

# PR-5 — Docs

## Task 5.1: How-to + reference pages

**Files:**
- Create: `docs/how-to/use-mounted-secrets.md`
- Create: `docs/reference/credential-providers.md`
- Modify: `docs/SUMMARY.md`

- [ ] **Step 1: Write the how-to**

Create `docs/how-to/use-mounted-secrets.md`:

```markdown
# Use a mounted secret as a credential

This guide shows how to feed a Kubernetes / Docker mounted secret to an
agent's plugin **without changing the plugin** — using the β.5 credential
chain.

## 1. Declare the credential on the agent (`tau.toml`)

```toml
[agents.assistant]
llm_backend = "anthropic"

[[agents.assistant.credentials]]
id  = "anthropic_api_key"
env = "ANTHROPIC_API_KEY"
```

`id` is the logical name the chain resolves; `env` is the variable the
plugin already reads. This declaration travels in the bundle.

## 2. Configure the chain for the deployment (scope/home `config.toml`)

```toml
[credentials]
chain = ["env", "file"]

[credentials.providers.file]
type = "file"
dir  = "/var/run/secrets"
key_map = { anthropic_api_key = "anthropic-key" }
```

The host tries `env` first (today's behavior), then reads
`/var/run/secrets/anthropic-key`.

## 3. Mount the secret

Mount your secret so the file lands at `/var/run/secrets/anthropic-key`.
The host resolves it and injects it as `ANTHROPIC_API_KEY` into the
plugin process. The unmodified plugin reads it exactly as before.

## Zero-config default

With no `[credentials]` block and no `credentials` declaration, the
behavior is identical to earlier tau: each plugin reads its own env var.
```

- [ ] **Step 2: Write the reference page**

Create `docs/reference/credential-providers.md`:

```markdown
# Credential providers

The β.5 credential chain resolves a logical credential id through an
ordered list of providers. First match wins; a provider that does not
hold the credential is skipped; a configured provider that fails aborts
resolution (fail-fast).

## Providers shipped today

| Provider | `type` | Resolves from | Config |
|---|---|---|---|
| Env | `env` | process environment (`env` name from the declaration) | none (default) |
| File | `file` | `<dir>/<key_map[id]>` | `dir`, `key_map` |
| Baked | — | in-memory (tests/embedded) | constructed in code |

## Chain config (`[credentials]`, scope/home `config.toml`)

```toml
[credentials]
chain = ["env", "file"]

[credentials.providers.file]
type = "file"
dir  = "/var/run/secrets"
key_map = { anthropic_api_key = "anthropic-key" }
```

- `chain` is an ordered list of provider names. Empty or absent ⇒
  `["env"]`.
- `env` needs no `[credentials.providers.env]` entry.

## Per-agent declaration (`tau.toml`)

```toml
[[agents.<id>.credentials]]
id  = "anthropic_api_key"   # [a-z0-9_.-]
env = "ANTHROPIC_API_KEY"   # [A-Z_][A-Z0-9_]*
```

Validated at build time: bad `id`, bad `env`, or a duplicate `env`
within one agent is a build error.

## Deferred providers

`SecretManager` (Vault/AWS/GCP/Azure), `WorkloadIdentity` (SPIFFE/IRSA),
`DeviceIdentity` (secure-element), and `TokenBroker` (OIDC/OAuth2) are
reserved. The async port, `Ok(None)`/`Err` walk, byte `Secret`, and
`expires_at` rotation hook make each a non-breaking addition.
```

- [ ] **Step 3: Add both to `SUMMARY.md`**

In `docs/SUMMARY.md`, add the how-to under the How-to section and the
reference under the Reference section. Find the existing `# How-to` /
how-to list and `# Reference` list and add:

```markdown
- [Use a mounted secret as a credential](how-to/use-mounted-secrets.md)
```

```markdown
- [Credential providers](reference/credential-providers.md)
```

- [ ] **Step 4: Build the book**

Run: `cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build`
Expected: only `[INFO]` lines; no broken-link errors. Then `rm -rf docs/book`.

- [ ] **Step 5: Commit**

```bash
git add docs/how-to/use-mounted-secrets.md docs/reference/credential-providers.md docs/SUMMARY.md
git commit -m "docs(β.5): credential chain how-to + reference"
```

## Task 5.2: ROADMAP check-off

**Files:**
- Modify: `ROADMAP.md`

- [ ] **Step 1: Mark β.5 done + migration note**

In `ROADMAP.md` §β.5 (line ~389), append a status line under the DoD bullet:

```markdown
- **Status (2026-06-14):** Shipped. Port + `CredentialChain` in `tau-ports`;
  Env/File/Baked providers; host resolve-then-inject bridge; per-agent
  declaration + scope-level chain config; `test (credential-chain / linux)`
  CI lane green. The five plugins are **unchanged** — the bridge injects
  resolved secrets into their existing env vars; per-plugin migration stays
  coupled to in-tree `LlmBackend` extraction.
```

- [ ] **Step 2: Commit**

```bash
git add ROADMAP.md
git commit -m "docs(roadmap): mark β.5 credential chain shipped"
```

---

## Self-Review Notes (spec coverage)

- Spec §3 port shape → Tasks 1.2–1.5 (Secret, ids, request/resolved, trait, chain).
- Spec §3.1 walk + §3.2 errors → Task 1.5 (`CredentialChain::resolve` fail-fast) + Task 1.4 (`CredentialError`).
- Spec §4 providers → Task 1.5 (Baked), Tasks 2.1/2.2 (Env/File).
- Spec §5 host bridge → Task 4.2.
- Spec §6.1 per-agent declaration → Task 3.1; §6.2 chain config → Task 3.2; `build_chain` → Task 4.1.
- Spec §8 testing + CI lane → tests throughout + Task 2.3.
- Spec §9 PR plan → PR-1…PR-5 structure.
- Spec §7 deferred providers → documented in Task 5.1 reference page; reserved by the async port shape (no code).

**Type consistency check:** `CredentialId` (parse/as_str), `Secret` (from_bytes/expose_bytes/expose_str), `CredentialRequest` (new/with_env_name), `ResolvedCredential` (new/with_expiry, fields secret/expires_at/source), `CredentialProvider::{name,resolve}`, `CredentialChain::{new,with,push,len,is_empty}`, `DynCredentialProvider`, `ProviderConfig::{Env,File}`, `CredentialsChainConfig`, `AgentCredential::{id,env}` — names are identical across all tasks that reference them.
```
