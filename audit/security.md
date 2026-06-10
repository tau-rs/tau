# Security findings

Scope prioritized: plugin host IPC + spawn, serve-mode stdio transport,
package install (`git clone` + `cargo build` + cross-check), `.tau` bundle
ingestion / IR decode, sandbox proxy egress filter, secrets handling.
Severity scale: Critical / High / Medium / Low.

---

## S1. Default (env-var) API-key provisioning is broken by `env_clear()`, forcing secrets into plaintext `tau.toml`

**Severity:** High
**Locations:**
- `crates/tau-runtime-tokio/src/plugin_host/process.rs:208-222` (spawn with `.env_clear()`, only `TAU_PLUGIN_RUN_ID`/`TAU_PLUGIN_AGENT_ID`/`PATH` re-added)
- `crates/tau-plugins/anthropic/src/plugin.rs:42` (`resolve_api_key(&cfg, |n| std::env::var(n).ok())`)
- `crates/tau-plugins/anthropic/src/config.rs:146-170`
- `crates/tau-cli/src/cmd/plugin_loader.rs:242-245` and `crates/tau-app/src/serve/lifecycle.rs:141` (host forwards only the agent `[config]` table; it never injects the key)

**Description:** Every plugin is spawned with `env_clear()`. The host re-adds
only three variables and never forwards `ANTHROPIC_API_KEY` / `OPENAI_API_KEY`.
Inside the plugin process, `from_config` resolves the key via
`std::env::var(api_key_env)`, which now returns `None`, so the documented and
default auth path (set the env var) always fails with `InvalidEnvVar`. The only
working alternative is `cfg.api_key` set directly in `[agents.<id>.config]` —
the path the code itself flags as test-only and warns against
(`config.rs:151-154`). Users are therefore funnelled into committing live API
keys into `tau.toml` (a tracked project file).

**Impact:** Secrets land in version-controlled project files; high risk of
credential leakage via git history and shared repos. The intended secret-from-
environment design is non-functional through the runtime.

**Recommendation:** Resolve secrets host-side before spawn and pass them through
an explicit, audited channel (e.g. a per-plugin secret env allowlist re-applied
after `env_clear`, or a dedicated secret field in the handshake that is never
Debug-printed). Keep `tau.toml` plaintext keys test-only and reject them outside
a test flag.

---

## S2. `tau install` builds and executes untrusted code from arbitrary git URLs with no sandbox

**Severity:** Medium
**Locations:**
- `crates/tau-pkg/src/install.rs:245` (`Git::clone` of caller-supplied source)
- `crates/tau-pkg/src/install.rs:598-664` (`cargo build --release` in the cloned tree)
- `crates/tau-pkg/src/install.rs:409-427` (cross-check **spawns** the freshly built binary)

**Description:** Installing a package clones an arbitrary URL, then runs
`cargo build` inside the clone (which executes the package's `build.rs` and proc
macros at build time) and afterwards spawns the produced binary for the Layer-2
capability cross-check. None of these steps run under the Layer-4 sandbox — the
sandbox is only applied to plugins spawned by the runtime later, not during
install. So `tau install <url>` is remote-code-execution-by-design the moment a
user installs an untrusted package.

**Impact:** A malicious package compromises the host at install time, before any
capability or sandbox enforcement applies.

**Recommendation:** Document the trust boundary loudly (install == trust the
package author), and consider running the install-time build + cross-check under
the same sandbox tier the plugin will later run under, or at minimum a
network-restricted build. A signed-source / pinned-commit allowlist would also
help (see S3).

---

## S3. `.tau` bundle verification proves self-consistency, not authenticity

**Severity:** Medium
**Locations:**
- `crates/tau-pkg/src/bundle/verify.rs:46-74` (verification pipeline)
- `crates/tau-pkg/src/bundle/verify.rs:250-270` (`verify_self_hash_step` — hash is recomputed over the same manifest)
- `crates/tau-cli/src/cmd/run.rs:79-118` → `crates/tau-cli/src/cmd/ir_dispatcher.rs` (verified IR payload is decoded and executed)

**Description:** `verify_bundle` checks that the bundle's recorded self-hash
matches its own canonical content, that the schema version/target match, and
that the local `tau.toml` + installed package trees hash to the recorded values.
The self-hash is *computed by whoever built the bundle* — it is an integrity
checksum, not a signature. A bundle is fully attacker-controllable; the only
thing preventing a foreign bundle from running is that the local `tau.toml` and
installed packages must already hash-match the bundle's recorded values. The IR
payload (`ir_payload.canonical_ir_bytes_hex`) is only checked against its own
stored hash (`verify.rs:222-245`), not cross-validated against the local
`tau.toml`, so the executed IR can diverge in capabilities from the project the
user inspected.

**Impact:** "Verified" overstates the guarantee. If a future flow ever runs a
bundle against state it controls (or relaxes the cwd-match requirement), the
self-hash provides no authenticity. The IR-vs-tau.toml capability divergence is
a latent capability-escalation surface.

**Recommendation:** Rename/clarify the model (integrity vs authenticity); if
bundles are meant to be distributed, add detached signatures with a trust
anchor. Cross-check the decoded IR's declared capabilities against the verified
`tau.toml` before `run_via_ir`.

---

## S4. Serve mode runs all plugins fully unsandboxed

**Severity:** Medium
**Locations:**
- `crates/tau-app/src/serve/lifecycle.rs:94-100, 142-184` (`sandbox_plan = None`, `PluginHostOptions::default()`, `sandbox_adapter` is `None`)

**Description:** `build_runtime` for serve mode spawns every LLM-backend and tool
plugin with no sandbox adapter. This is documented as a "v1 simplification," but
serve mode is a *public, embeddable surface* (an IDE/SDK spawns `tau serve` and
streams runtime.run calls). The capability check still happens in-process, but
the Layer-4 backstop that "catches a plugin trying to bypass the wire contract"
(per architecture-overview.md) is absent.

**Impact:** A compromised or buggy plugin in an embedded serve deployment has the
full ambient authority of the serve process (filesystem, network) with no OS-
level containment.

**Recommendation:** Apply the same sandbox resolution the CLI `run`/`chat` path
uses (`plugin_loader::build_host_options` + adapter resolve) in serve mode, or
gate serve startup behind an explicit `--no-sandbox` acknowledgement.

---

## S5. Sandbox proxy HTTP path enforces neither destination port nor TLS (asymmetric with CONNECT)

**Severity:** Medium
**Locations:**
- `crates/tau-sandbox-proxy/src/lib.rs:205-251` (`handle_http`)
- compare `crates/tau-sandbox-proxy/src/lib.rs:150-203` (`handle_connect` restricts port to 443 and verifies the TLS SNI matches the CONNECT host)

**Description:** The CONNECT (HTTPS) path restricts the port to 443 and verifies
the ClientHello SNI equals the requested host. The plaintext HTTP path checks
only the host allowlist (`allowed_hosts.iter().any(|h| h == &req.host)`) — it
accepts any port and performs no TLS verification, then splices bytes through.
A sandboxed plugin can therefore open a plaintext channel to an allowlisted host
on any port.

**Impact:** Weaker egress containment than the HTTPS path implies; plaintext
exfiltration / arbitrary-port reach to allowlisted hosts. Host matching is also
exact-string with no port component, so the allowlist cannot express
host:port granularity.

**Recommendation:** Mirror the CONNECT restrictions on the HTTP path (or drop
plaintext egress entirely), and include port in the allowlist semantics.

---

## S6. Proxy control socket is world-writable (0o666)

**Severity:** Low
**Locations:**
- `crates/tau-sandbox-proxy/src/lib.rs:95-99` (`set_permissions(... from_mode(0o666))`)
- `crates/tau-sandbox-proxy/src/lib.rs:103-114` (`make_temp_sock_path` in shared `/tmp`)

**Description:** The egress-proxy Unix socket is created world-writable in the
shared temp dir so the container bridge (uid 1000) can dial it. Any local user
on the host can connect to the proxy and use it as an egress relay to the
allowlisted hosts for the lifetime of the run.

**Impact:** Local-user egress relay / minor confused-deputy on multi-tenant
hosts. The comment argues connections are validated against the allowlist, which
bounds but does not eliminate the exposure.

**Recommendation:** Place the socket in a per-run directory with `0o700` and
chown to the container UID, or use an abstract/peer-cred-checked socket instead
of world-writable mode.

---

## S7. `git clone` invoked without `--` argument terminator

**Severity:** Low
**Locations:**
- `crates/tau-pkg/src/git.rs:96-100` (`cmd.arg("clone"); ... cmd.arg(&url_string).arg(dest)` — no `--` before the URL)
- `crates/tau-pkg/src/git.rs:97-98` (`--branch <rev>` where `rev` comes from the URL fragment)

**Description:** The URL and destination are passed positionally with no `--`
separator. If a `PackageSource` whose string begins with `-` ever reaches this
function, git would interpret it as an option (e.g. `--upload-pack=...`). This is
mitigated today by `PackageSource` URL-scheme validation, but the wrapper itself
is not defensive.

**Impact:** Latent argument-injection if source parsing is ever loosened.

**Recommendation:** Insert `--` before the URL/dest: `git clone [opts] -- <url> <dest>`.

---

## S8. Plugin config holding the API key derives `Debug` with the key as a plain `String`

**Severity:** Low
**Locations:**
- `crates/tau-plugins/anthropic/src/config.rs:31-43` (`#[derive(Debug)] ... pub api_key: Option<String>`)
- equivalent in `crates/tau-plugins/openai/src/config.rs:41-48`

**Description:** `api_key` is a plain `Option<String>` inside a `Debug`-deriving
config struct (unlike the downstream client which correctly uses
`secrecy::SecretString`). Any `{:?}` of the config — a stray trace, a panic
message, an error wrapper — would print the key verbatim.

**Impact:** Accidental secret disclosure in logs/diagnostics.

**Recommendation:** Type the field as `secrecy::SecretString` (or a custom
redacting `Debug`) end-to-end, not just at the HTTP client boundary.

---

## S9. Workflow run-log path interpolates names without sanitization

**Severity:** Low
**Locations:**
- `crates/tau-workflow/src/persistence.rs:148-153` (`run_log_path` → `format!("{workflow_name}-{run_id}.jsonl")` joined under `.tau/workflow-runs`)

**Description:** `workflow_name` and `run_id` are interpolated directly into the
log filename. If either can contain `/` or `..` (workflow names originate from
user config), the write escapes the intended directory.

**Impact:** Path traversal on log write for crafted workflow names.

**Recommendation:** Validate/percent-encode the components, or assert they match
a `[A-Za-z0-9._-]+` charset before building the path.

---

## Notes / non-findings (checked, acceptable)

- **Frame reader DoS:** `FramedReader` caps frames at 64 MiB and pre-sizes the
  buffer to the prefix length (`crates/tau-plugin-protocol/src/framer.rs:99-115`).
  Bounded; acceptable, though the buffer is allocated to the full claimed length
  before the body arrives (a peer can force a 64 MiB allocation per connection).
- **IR decode:** `from_canonical_bytes` uses `serde_json::from_slice` returning
  `Result` (`crates/tau-ir/src/canonical.rs:31-33`) — no panic; serde_json's
  default recursion limit bounds nesting DoS.
- **Serve stdio has no auth** — by design (parent process is trusted), and
  tracing is correctly routed to stderr so it cannot corrupt the NDJSON stream
  (`crates/tau-app/src/serve/tracing_init.rs`). The net bridge binds
  `127.0.0.1` only (`crates/tau-sandbox-native/src/bin/tau-net-bridge.rs:26-27`),
  not `0.0.0.0`.
