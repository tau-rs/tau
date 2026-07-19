# Escape-hatch registry

Each entry below names a place where tau core uses a structural escape
hatch (`Custom`, `InternalError`) instead of typed variants. Per
ADR-0002, every escape hatch must be documented here with rationale
and promotion trigger.

**PR rule:** any PR that introduces, promotes, or removes an escape
hatch updates this file in the same commit. The CI test
`crates/tau-domain/tests/escape_hatch_registry.rs` enforces this
mechanically.

## Active escape hatches

| Anchor | Location | Reason | Promotion trigger | Sub-project |
|---|---|---|---|---|
| <a id="capability-custom"></a>`capability-custom` | `Capability::Custom { name, params }` | Plugin-defined capability outside tau's typed vocabulary. Since D7-B, requires an explicit `custom.` kind prefix (deliberate escape-hatch intent) — an unprefixed unknown kind is a hard parse error with a did-you-mean. | When tau-runtime ships namespace enforcement for a new namespace (sub-project 4+), promote the namespace's verbs to typed variants. | 1 |
| <a id="capability-forward"></a>`capability-forward` | `Capability::Forward { kind, params }` | A capability kind unknown to *this* tau, accepted only because a package manifest declared a `vocab_version` newer than this build's `KNOWN_VOCAB` (D7-B forward-compat). Fail-closed in the capability lattice and surfaced by `tau check` as an info finding. | When `KNOWN_VOCAB` is bumped so the kind becomes typed, `Forward` for that kind disappears; if a kind proves permanent, promote it to a typed variant. | 1 |
| <a id="messagepayload-custom"></a>`messagepayload-custom` | `MessagePayload::Custom { kind, body }` | Plugin-specific message kinds (MCP resources, skill-specific shapes) not yet enumerated. | When MCP plugin trait stabilizes (sub-project 2+), promote `mcp.*` shapes; same for skill-specific message kinds. | 1 |
| <a id="packagekind-custom"></a>`packagekind-custom` | `PackageKind::Custom { kind }` | All package kinds go through `Custom` at v0.1; no typed variants exist. | When tau-ports lands plugin traits for LLM/Tool/Storage/Sandbox (sub-project 2), consider promoting matching `PackageKind` variants. | 1 |
| <a id="failurekind-internalerror"></a>`failurekind-internalerror` | `FailureKind::InternalError` | Catch-all for failures not matching the v0.1 typed kinds (Crashed, BackendError, PolicyDenied, OutOfResources). tau-runtime hasn't yet emitted enough variety to identify recurring shapes. | When tau-runtime construction sites for `InternalError` exceed 3 distinct contexts, file an ADR proposing typed variants for the recurring shapes. | 1 |
| <a id="llmerror-internal"></a>`llmerror-internal` | `LlmError::Internal { message }` | catch-all for plugin failures not matching named LLM-error variants | promote when 2+ distinct contexts surface | 2 |
| <a id="toolerror-internal"></a>`toolerror-internal` | `ToolError::Internal { message }` | catch-all for plugin failures not matching named tool-error variants | promote when 2+ distinct contexts surface | 2 |
| <a id="storageerror-internal"></a>`storageerror-internal` | `StorageError::Internal { message }` | catch-all for storage-plugin failures not matching named variants | promote when 2+ distinct contexts surface | 2 |
| <a id="sandboxerror-internal"></a>`sandboxerror-internal` | `SandboxError::Internal { message }` | catch-all (provisional — sandbox trait itself is provisional at v0.1) | promote alongside Phase-1 sandbox impl | 2 |
| <a id="completionrequest-provider-specific"></a>`completionrequest-provider-specific` | `CompletionRequest.provider_specific: BTreeMap<String, Value>` | provider-specific LLM call params (top_k, presence_penalty, response_format, etc.) not yet typed in core | promote a key when it appears in 2+ plugins | 2 |
| <a id="scopeerror-internal"></a>`scopeerror-internal` | `ScopeError::Internal { message }` | catch-all for scope-resolution failures not yet covered by typed variants (e.g., XDG resolution edge cases, future env-var handling) | promote when 2+ distinct contexts surface | 3 |
| <a id="registryerror-internal"></a>`registryerror-internal` | `RegistryError::Internal { message }` | catch-all for lockfile / registry-read failures not yet covered by typed variants | promote when 2+ distinct contexts surface | 3 |
| <a id="installerror-internal"></a>`installerror-internal` | `InstallError::Internal { message }` | catch-all for install lifecycle failures not reportable as `Git`, `Manifest`, `Registry`, `Scope`, `SourceManifestMismatch`, or `Locked` | promote when 2+ distinct contexts surface | 3 |
| <a id="uninstallerror-internal"></a>`uninstallerror-internal` | `UninstallError::Internal { message }` | catch-all for uninstall failures not yet covered by typed variants | promote when 2+ distinct contexts surface | 3 |
| <a id="builderror-internal"></a>`builderror-internal` | `BuildError::Internal { message }` | catch-all for invariant violations during `RuntimeBuilder::build()` not yet covered by typed variants | promote when 2+ distinct contexts surface | 4 |
| <a id="runtimeerror-internal"></a>`runtimeerror-internal` | `RuntimeError::Internal { message }` | catch-all for kernel-level invariant violations during `Runtime::run` not yet covered by typed variants | promote when 2+ distinct contexts surface | 4 |
| <a id="contextnodekind-custom"></a>`contextnodekind-custom` | `ContextNodeKind::Custom { source, package }` | β.4 v1 ships only builtin transformers; `Custom` is reserved for future native/wasm/mcp extension points that are not yet typed. | When tau-context ships its first non-builtin delivery lane (native plugin or wasm), promote to a typed `Native`/`Wasm`/`Mcp` variant family. | β.4 |
| <a id="credentialerror-internal"></a>`credentialerror-internal` | `CredentialError::Internal { reason }` | catch-all for credential-resolution failures not matching `NotFound`, `ProviderUnavailable`, `Malformed`, or `Io` | promote when 2+ distinct contexts surface | β.5 |

## Promoted escape hatches

(none yet)

## Removed escape hatches

(none yet)
