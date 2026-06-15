# β.5 — Credential provider chain (design spec)

**Status:** approved (brainstorm 2026-06-14)
**ADR:** [0047-credential-provider-chain](../../decisions/0047-credential-provider-chain.md)
**ROADMAP:** §β.5; per-plugin migration triggers (anthropic/ollama/openai row); CI gate `test (credential-chain / linux)`.

## 1. Problem

The five in-tree plugins are separate subprocesses, and each rolls its own
credential loader *inside its own process*:

| plugin | env var | held as | missing-cred behavior |
|---|---|---|---|
| `anthropic` | `ANTHROPIC_API_KEY` (`api_key_env` configurable) | `secrecy::SecretString` | `ConfigError::InvalidEnvVar` |
| `openai` | `OPENAI_API_KEY` (`api_key_env` configurable) | `secrecy::SecretString` | `ConfigError::InvalidEnvVar` |
| `ollama` | `OLLAMA_BEARER_TOKEN` (`bearer_token_env` configurable) | `Option<SecretString>` | `Ok(None)` — optional |
| `fs-read`, `shell` | none | — | — |

There is no way to source a credential from anywhere other than the plugin's
own process environment: a mounted Kubernetes secret, a Vault lease, or an
IRSA workload identity cannot reach a plugin without bespoke per-plugin code.

β.5 introduces a **Strategy + Chain** credential port so deployment — not the
plugin author — decides where a credential comes from, while **every existing
env-var path keeps working byte-for-byte**.

## 2. Non-goals

- **NG-1.** β.5 does **not** migrate the five plugins off their loaders.
  The migration-trigger table couples plugin migration to *in-tree
  `LlmBackend` extraction*, a separate later event. β.5 makes unmodified
  plugins benefit from the chain via a host-side bridge (§5).
- **NG-2.** No cloud SecretManager / WorkloadIdentity / DeviceIdentity /
  TokenBroker adapter ships in v1. Their shape is reserved (§7) so they
  slot into the unchanged port later.
- **NG-3.** Tau still does not *manage* identity or mint credentials
  (ROADMAP NG9). The chain *resolves* a credential the deployment already
  provisioned; it never creates or stores one.

## 3. Port shape (`tau-ports`)

`tau-ports` is `#![no_std]` + `alloc`, uses native `async fn in trait` (no
`async-trait`), per-port `#[non_exhaustive]` `thiserror` enums, `Send + Sync`.
The new port follows those idioms exactly. New dependency: `zeroize`
(`no_std`-capable).

```rust
// tau-ports/src/credential/secret.rs
/// A resolved secret value. Redacts on Debug; zeroized on drop.
/// Holds bytes, not String — device / secure-element keys are binary.
pub struct Secret(zeroize::Zeroizing<alloc::vec::Vec<u8>>);

impl Secret {
    pub fn from_bytes(b: alloc::vec::Vec<u8>) -> Self;
    pub fn expose_bytes(&self) -> &[u8];
    pub fn expose_str(&self) -> Result<&str, core::str::Utf8Error>;
}
impl core::fmt::Debug for Secret { /* "Secret(<redacted>)" */ }
```

```rust
// tau-ports/src/credential/id.rs
/// A logical credential identifier, e.g. "anthropic_api_key".
/// Validated: non-empty, [a-z0-9_.-], snake_case-friendly.
pub struct CredentialId(alloc::string::String);
```

```rust
// tau-ports/src/credential/mod.rs
#[non_exhaustive]
pub struct CredentialRequest {
    pub id: CredentialId,
    /// Provider hints carried from the declaration. Env reads `env_name`;
    /// File maps `id` via its own key_map. Future providers read more.
    pub env_name: Option<alloc::string::String>,
}

#[non_exhaustive]
pub struct ResolvedCredential {
    pub secret: Secret,
    /// Unix-millis expiry, for rotating providers (TokenBroker, Vault lease).
    /// `None` = no known expiry. Consumer re-resolves past expiry.
    pub expires_at: Option<i64>,
    /// Which provider satisfied the request, for tracing/audit.
    pub source: &'static str,
}

#[allow(async_fn_in_trait)]
pub trait CredentialProvider: Send + Sync {
    fn name(&self) -> &str;

    /// `Ok(Some)` = resolved. `Ok(None)` = not here, try the next provider.
    /// `Err` = this provider owns the request but failed (e.g. Vault down).
    async fn resolve(
        &self,
        req: &CredentialRequest,
    ) -> Result<Option<ResolvedCredential>, CredentialError>;
}
```

```rust
// tau-ports/src/credential/chain.rs
/// A composite provider that walks members in order. Itself a CredentialProvider.
pub struct CredentialChain {
    members: alloc::vec::Vec<alloc::sync::Arc<dyn CredentialProvider>>,
}
```

### 3.1 Walk semantics & error policy

The chain walks members in declared order:

- member returns `Ok(Some)` → **return it** (first match wins).
- member returns `Ok(None)` → **continue** to the next member.
- member returns `Err(e)` → **fail-fast**: the chain returns `Err(e)`.

Fail-fast is deliberate and security-motivated: a *configured* Vault that is
unreachable must surface as an error, not silently fall through to a weaker
provider. A provider that simply does not hold the key returns `Ok(None)`,
which is not an error and does continue. After the whole chain returns
`Ok(None)`, the host treats it as "credential not found" and surfaces it at
the same point today's `InvalidEnvVar` surfaces — so when `env` is the only
member the error UX is unchanged.

### 3.2 Errors

```rust
// tau-ports/src/error.rs (new enum, alongside LlmError/ToolError/…)
#[non_exhaustive]
pub enum CredentialError {
    NotFound { id: String },                          // whole chain exhausted
    ProviderUnavailable { provider: String, reason: String }, // Vault down, etc.
    Malformed { id: String, reason: String },         // bad file content / shape
    Io { reason: String },
    Internal { reason: String },
}
```

## 4. Providers shipped in v1

| Provider | Crate | `no_std` | Behavior |
|---|---|---|---|
| `BakedProvider` | `tau-ports` (fixtures) | yes | In-memory `BTreeMap<CredentialId, Vec<u8>>`. Deterministic test provider; seeds embedded/wasm story. |
| `EnvProvider` | `tau-runtime-tokio` | no | Reads `CredentialRequest.env_name` from process env. **This is today's behavior** — the zero-config default. |
| `FileProvider` | `tau-runtime-tokio` | no | Reads `<dir>/<key_map[id]>` from a mounted-secret dir. The DoD CI provider. Trailing newline trimmed; missing file → `Ok(None)`; unreadable file → `Err(Io)`. |

The Chain combinator is pure logic and lives in `tau-ports`. Env/File need
`std` and live with the other tokio adapters (`TokioClock`, `OsRandom`) in
`tau-runtime-tokio`.

## 5. Host bridge — resolve-then-inject (the integration)

The bridge lets a chain-sourced secret reach an **unmodified** subprocess
plugin. Before spawning a plugin, the tokio host:

```
host spawns plugin for agent "assistant" (llm_backend = "anthropic")
   declaration: credentials = [{ id = "anthropic_api_key", env = "ANTHROPIC_API_KEY" }]
   │
   ├─ build_chain(scope_cfg)  →  [EnvProvider, FileProvider]
   │
   ├─ for each declared credential:
   │     chain.resolve(CredentialRequest { id, env_name: Some(env) })
   │       EnvProvider  : read $ANTHROPIC_API_KEY → Some? return : None
   │       FileProvider : read <dir>/anthropic-key → Some? return : None
   │     → Ok(Some(ResolvedCredential { secret, source = "file" }))
   │
   ├─ inject into child env:  ANTHROPIC_API_KEY = <secret>
   │
   ▼
spawn → anthropic plugin reads ANTHROPIC_API_KEY exactly as it always has ✓
```

Backward-compat invariants (both byte-identical to today):

- Agent declares **no** `credentials` → host injects nothing; child inherits
  the parent environment as it does today.
- No `[credentials]` block in scope config → chain defaults to `["env"]`;
  `EnvProvider` reads the same var the plugin would have read itself.

Only by *declaring a credential* does an agent opt into the chain; only by
*configuring a non-env member* does a deployment change where it resolves from.

When a plugin later migrates to an in-tree `LlmBackend`, it calls the chain
directly and the thin env-injection shim retires; the port, providers, config,
and declarations all stay.

## 6. Config surface

### 6.1 Per-agent declaration — intrinsic, in the bundle (`tau.toml` → `tau-pkg::project`)

```toml
[agents.assistant]
llm_backend = "anthropic"

[[agents.assistant.credentials]]
id  = "anthropic_api_key"     # logical id the chain resolves
env = "ANTHROPIC_API_KEY"     # host injects the resolved Secret under this env name
```

Repeatable: an agent may declare several credentials. Added as
`UncheckedAgentCredential` on `UncheckedAgent`, validated in
`UncheckedProjectConfig::validate`:

- `id` parses as a `CredentialId` (non-empty, `[a-z0-9_.-]`).
- `env` is a valid env-var name (`[A-Z_][A-Z0-9_]*`).
- no duplicate `env` within one agent (would shadow).

This declaration travels in the bundle and is reproducible — it is the same in
every deployment.

### 6.2 Chain config — deployment-specific, in scope/home (`TAU_HOME` → `tau-pkg::scope`)

```toml
[credentials]
chain = ["env", "file"]            # order = precedence

[credentials.providers.file]
type = "file"
dir  = "/var/run/secrets"
key_map = { anthropic_api_key = "anthropic-key" }   # id → filename
```

Added as `UncheckedCredentialsConfig` parsed from the scope/home config,
validated via a new `validate` pass mirroring the project pattern:

- every name in `chain` has a matching `[credentials.providers.<name>]` (or is
  a built-in default like `env`).
- `file` provider requires `dir`; `key_map` values are non-empty filenames.

This lives in `TAU_HOME`, **not** the bundle: the same bundle runs env-only
locally, file-backed in k8s, Vault in prod, without a rebuild. Reproducibility
of the bundle is preserved because deployment wiring is not baked into it.

**Zero-config default:** absent `[credentials]`, the chain is implicitly
`["env"]`.

## 7. Deferred providers (reserved, non-breaking)

The async port + `Ok(None)`/`Err` walk + `expires_at` rotation hook + binary
`Secret` make every deferred provider a *non-breaking addition*: a new struct
that implements `CredentialProvider`, registered in `build_chain`, plus a
`[credentials.providers.<name>]` config arm.

| Provider | Future home | Reserved by |
|---|---|---|
| `SecretManager` (Vault / AWS / GCP / Azure) | own adapter crate(s) per SDK | async resolve; `ProviderUnavailable`; `expires_at` (lease TTL) |
| `WorkloadIdentity` (SPIFFE / IRSA) | own adapter crate | async resolve; `source` provenance |
| `DeviceIdentity` (secure-element) | platform adapter crate | binary `Secret` (non-UTF-8 keys) |
| `TokenBroker` (OIDC / OAuth2 BFF) | tied to γ.2 browser BFF (ROADMAP §γ.2) | `expires_at` rotation; async resolve |

## 8. Testing & CI

- **Unit (`tau-ports`):** `Secret` redacts in `Debug` and zeroizes; `expose_str`
  rejects non-UTF-8; `CredentialId`/`env` validation; chain walk
  (`None`→next, first `Some` wins, `Err`→fail-fast); `BakedProvider`.
- **Unit (`tau-runtime-tokio`):** `EnvProvider` hit/miss; `FileProvider`
  hit/miss/unreadable/newline-trim; `build_chain` from config.
- **Integration:** `FileProvider` against a temp mounted-secret dir; **host
  resolve-then-inject** with a mock plugin that echoes its environment —
  proves a File-sourced secret reaches an unmodified child under the declared
  env name.
- **Config:** `UncheckedProjectConfig::validate` accepts a well-formed
  `[[agents.<id>.credentials]]` and rejects bad `id`/`env`/duplicates;
  scope `validate` accepts/rejects chain configs.
- **New CI lane `test (credential-chain / linux)`** runs the chain +
  File-provider integration test. Satisfies DoD: ≥1 non-`Env` provider
  exercised by CI.

## 9. Multi-PR plan

Spec → plan → subagent-execute, matching the β.7 / β.8 flow.

1. **PR-1 — port (`tau-ports`).** `Secret`, `CredentialId`,
   `CredentialRequest`, `ResolvedCredential`, `CredentialProvider`,
   `CredentialChain`, `CredentialError`, `BakedProvider` fixture, `zeroize`
   dep. Pure; no wiring. **+ ADR-0047.**
2. **PR-2 — adapters (`tau-runtime-tokio`).** `EnvProvider`, `FileProvider`,
   `build_chain`. Unit + File integration tests. **Adds the CI lane.**
3. **PR-3 — config (`tau-pkg`).** Per-agent `[[…credentials]]` (project,
   Unchecked→validate) + `[credentials]` chain (scope/home, Unchecked→validate).
   Validation tests.
4. **PR-4 — host bridge (`tau-runtime-tokio` `plugin_host`).**
   resolve-then-inject + mock-plugin integration test. Wires PR-1/2/3 end to
   end.
5. **PR-5 — docs.** mdBook how-to (mounted secret in k8s) + reference page;
   ROADMAP β.5 check-off; migration-trigger note that plugins remain on their
   loaders, served by the bridge.

No plugin crate (`anthropic`/`openai`/`ollama`/`fs-read`/`shell`) is modified
in any PR.

## 10. Definition of done

- `CredentialProvider` port + `CredentialChain` in `tau-ports`.
- Env + File + Baked providers ship; File exercised by the new
  `test (credential-chain / linux)` CI lane.
- Per-agent declaration + scope-level chain config, both Unchecked→validate.
- Host resolve-then-inject bridge: a File-mounted secret reaches an unmodified
  plugin under its declared env var.
- Existing `ANTHROPIC_API_KEY` / `OPENAI_API_KEY` / `OLLAMA_BEARER_TOKEN` paths
  unchanged — verified by the no-declaration / no-chain byte-identical fallback
  tests.
- ADR-0047 records the durable decisions; docs published.
