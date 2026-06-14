# ADR-0046: Credential provider chain (β.5)

**Status:** Accepted
**Date:** 2026-06-14
**Supersedes:** none

## Context

The five in-tree plugins are separate subprocesses, and each rolls its own
credential loader *inside its own process*: `anthropic`/`openai` read an API
key from `ANTHROPIC_API_KEY`/`OPENAI_API_KEY` (configurable var name) and hold
it as `secrecy::SecretString`; `ollama` reads an optional `OLLAMA_BEARER_TOKEN`;
`fs-read`/`shell` take none. There is no way to source a credential from
anywhere other than the plugin's own process environment — a mounted Kubernetes
secret, a Vault lease, or an IRSA workload identity cannot reach a plugin
without bespoke per-plugin code.

ROADMAP §β.5 calls for a Strategy + Chain credential port so that **deployment**,
not the plugin author, decides where a credential comes from, while **every
existing env-var path keeps working byte-for-byte**. The per-plugin migration
table couples actual plugin migration to a *separate* later event (in-tree
`LlmBackend` extraction); β.5 lands the port + chain and makes unmodified
plugins benefit via a host-side bridge.

tau's standing principle — *any check that could run at build time must run at
build time* — applies: an agent's declared credential `id`/`env` and a
deployment's chain config are validated at `tau build` / config load, not
discovered at spawn time.

Full design detail is in
`docs/superpowers/specs/2026-06-14-beta-5-credential-provider-chain-design.md`.

## Decisions

### Decision 1 — Async Strategy + Chain port in `tau-ports`

`CredentialProvider` is a public port in `tau-ports`, matching that crate's
idioms (`#![no_std]` + `alloc`, native `async fn in trait`, `Send + Sync`,
`#[non_exhaustive]` `thiserror`):

```rust
#[allow(async_fn_in_trait)]
pub trait CredentialProvider: Send + Sync {
    fn name(&self) -> &str;
    async fn resolve(&self, req: &CredentialRequest)
        -> Result<Option<ResolvedCredential>, CredentialError>;
}
```

`resolve` is **async** even though v1's three providers are effectively sync,
because the deferred providers (SecretManager, WorkloadIdentity, TokenBroker)
are network calls. Async now means they slot in with zero port churn.
`CredentialChain` is a composite that walks members in order and is *itself* a
`CredentialProvider` (Strategy + Chain). The Chain is pure logic and lives in
`tau-ports`; concrete I/O adapters live in adapter crates.

### Decision 2 — `Ok(None)` walk semantics with fail-fast on `Err`

The chain distinguishes three member outcomes:

| Outcome | Meaning | Chain action |
|---|---|---|
| `Ok(Some(_))` | provider resolved the credential | return it (first match wins) |
| `Ok(None)` | provider does not hold this credential | continue to next member |
| `Err(e)` | provider owns it but failed (e.g. Vault down) | **fail-fast** — return `Err(e)` |

Fail-fast is security-motivated: a *configured* secret manager that is
unreachable must surface as an error, not silently fall through to a weaker
provider. A whole-chain `Ok(None)` becomes "credential not found" at the same
point today's `InvalidEnvVar` surfaces, so when `env` is the only member the
error UX is unchanged.

### Decision 3 — tau-native `Secret` over re-exporting `secrecy`

The port boundary speaks a new `tau-ports` type rather than re-exporting
`secrecy::SecretString`:

```rust
pub struct Secret(zeroize::Zeroizing<Vec<u8>>); // redacting Debug, zeroize-on-drop
```

Rationale: (a) keeps `tau-ports` `no_std`-clean — `zeroize` is `no_std`-capable,
`secrecy` is heavier and std-leaning; (b) holds **bytes**, not `String` — device
/ secure-element keys are binary, so a `String`-typed secret would be a design
hole the moment `DeviceIdentity` lands. Plugin crates keep `secrecy` internally;
only the port boundary uses `Secret`.

### Decision 4 — Host resolve-then-inject bridge; plugins unmodified

Before spawning a plugin, the tokio host runs the chain for each credential the
agent declares and injects the resolved `Secret` into the child's environment
under the declared env-var name. The plugin then reads that env var exactly as
it does today. This is the integration that makes a File/Vault secret reach an
**unmodified** subprocess plugin.

Two backward-compat invariants, both byte-identical to today:

- An agent that declares **no** credentials → host injects nothing; child
  inherits the parent environment as today.
- No `[credentials]` block in scope config → chain defaults to `["env"]`;
  `EnvProvider` reads the same var the plugin would have read itself.

Opting into the chain requires *declaring a credential*; changing where it
resolves requires *configuring a non-env member*. When a plugin later migrates
to an in-tree `LlmBackend`, it calls the chain directly and the thin
env-injection shim retires; the port, providers, config, and declarations stay.

### Decision 5 — Two-layer config: declaration in the bundle, chain in the deployment

Credentials have two concerns with different homes:

| Concern | Home | Why |
|---|---|---|
| Per-agent declaration `[[agents.<id>.credentials]]` (`id` + `env`) | `tau.toml` → `tau-pkg::project`, Unchecked→validate | intrinsic to the agent; travels in the bundle; reproducible |
| Chain config `[credentials]` (providers + order) | `TAU_HOME` scope config → `tau-pkg::scope`, Unchecked→validate | deployment-specific; same bundle runs env-locally / file-in-k8s / Vault-in-prod without rebuild |

Baking Vault endpoints into a reproducible bundle would be a design hole; the
split keeps the bundle portable. Both halves are validated at build/load time
(unknown provider names, bad `id`/`env`, duplicate `env`, missing `file.dir`).

### Decision 6 — Ship Env + File + Baked; reserve the four heavy providers

v1 ships three providers:

| Provider | Crate | `no_std` | Role |
|---|---|---|---|
| `BakedProvider` | `tau-ports` (fixtures) | yes | deterministic test provider; seeds embedded/wasm |
| `EnvProvider` | `tau-runtime-tokio` | no | today's behavior; zero-config default |
| `FileProvider` | `tau-runtime-tokio` | no | mounted-secret dir + `key_map`; the DoD CI provider |

SecretManager (Vault/AWS/GCP/Azure), WorkloadIdentity (SPIFFE/IRSA),
DeviceIdentity (secure-element), and TokenBroker (OIDC/OAuth2 BFF) are deferred.
The async port + `Ok(None)`/`Err` walk + `expires_at` rotation hook + binary
`Secret` make each a non-breaking addition (a new struct + a config arm).

### Decision 7 — `ResolvedCredential.expires_at` reserves rotation without a handle

`resolve` returns a *value* (`ResolvedCredential { secret, expires_at, source }`),
not a refreshable handle. Rotating providers (TokenBroker, Vault leases) carry
`expires_at`; the consumer re-resolves past expiry. This keeps the port a pure
resolver (like `CapabilityResolver`) and avoids a handle-lifecycle contract in
v1 while reserving the rotation story.

## Consequences

**Positive:**

- A mounted Kubernetes secret or Vault lease can reach an **unmodified** plugin
  today, with no plugin-code change.
- Backward compatibility is absolute: no declaration / no chain block →
  byte-identical to today's env-var path.
- The async port, byte `Secret`, fail-fast walk, and `expires_at` hook make all
  four deferred providers non-breaking additions.
- Bundle reproducibility is preserved: deployment wiring lives in scope config,
  not the bundle.
- The chain is exercised end-to-end by a new CI lane
  (`test (credential-chain / linux)`) via the File provider.

**Negative / obligations:**

- The host gains a resolve-then-inject step in the spawn path (one injection
  point); it is a no-op when nothing is declared.
- Two config-validation surfaces must stay in sync with the provider set
  (project declaration + scope chain).
- `Secret`-as-bytes means UTF-8 string consumers call `expose_str()` and handle
  the (practically impossible for API keys) non-UTF-8 case.

## Alternatives considered

**Standalone port, plugins fully untouched (synthetic CI consumer only).**
Rejected: lands an unused port that reads as dead code and rots; the
resolve-then-inject bridge is a genuinely useful capability (mounted secret →
live plugin) and is not throwaway — only the thin shim retires at the later
LlmBackend extraction.

**Re-export `secrecy::SecretString` as the port type.** Rejected: pulls a
heavier, std-leaning crate into `no_std` `tau-ports`, and `String`-typed
secrets exclude binary device/secure-element keys — a future breaking change.

**Chain config in `tau.toml` (single validate path).** Rejected: bakes
deployment wiring (Vault endpoints) into the reproducible bundle — the wrong
layer; every redeploy would edit the bundle. The ROADMAP explicitly says
deployment configures the chain order.

**Sync `resolve`.** Rejected: SecretManager/WorkloadIdentity/TokenBroker are
network calls; a sync port would force a breaking signature change when the
first network provider lands.

**Migrate the five plugins onto the port now.** Rejected: the migration-trigger
table couples plugin migration to in-tree `LlmBackend` extraction, a separate
event; doing it now means solving (and then discarding) the subprocess
credential-injection boundary the bridge already covers.

**Flat single-credential-per-agent declaration.** Rejected: caps an agent at
one credential — a design hole the moment any agent needs two (e.g. an API key
plus an org token). The per-agent list costs one table entry for the common
case.

## References

- Design spec:
  `docs/superpowers/specs/2026-06-14-beta-5-credential-provider-chain-design.md`
- Related ADRs: [ADR-0006](0006-tau-runtime.md) (error/failure dichotomy),
  [ADR-0014](0014-sandboxing.md) (capability model),
  [ADR-0044](0044-deliverables-and-goals.md) (build-time checks pattern)
- ROADMAP: §β.5; per-plugin migration triggers; CI gate
  `test (credential-chain / linux)`
- Philosophy: [`docs/explanation/tau-philosophy.md`](../explanation/tau-philosophy.md)
