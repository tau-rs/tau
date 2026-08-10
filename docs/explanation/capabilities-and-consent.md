# Capabilities and consent

Capabilities are tau's typed answer to "what is a plugin allowed to
do?". Consent is tau's answer to "who decides that's OK?". Together
they form the model that every other system in tau depends on: the
sandbox enforces capabilities, the lockfile records consent, the
runtime gates tool calls against the granted set.

This page explains the model. Pages elsewhere — [Packages](packages.md),
[Sandboxing](sandboxing.md), the [package manifest
schema](../reference/package-manifest-schema.md) — assume you've read
this one.

## What a capability is

A capability is a *typed, narrow grant of a specific kind of access*.
Concrete examples from `crates/tau-domain/src/package/capability.rs`:

- `fs.read` with `paths = ["${PROJECT}/**"]` — read files matching this
  glob, nothing else.
- `net.http` with `hosts = ["api.anthropic.com"]`, `methods = ["POST"]`
  — POST HTTPS requests to that host, nothing else.
- `process.spawn` with `commands = ["git"]` — spawn `git`, nothing
  else.
- `skill.spawn` with `allowed_skills = ["critic"]` — invoke the
  `critic` skill as a sub-agent, nothing else.

The shape is consistent: a verb (`read`, `http`, `spawn`) and a
narrowing parameter (paths, hosts, methods, command names). The
verb tells the runtime which mechanism to apply; the parameter
tells the mechanism what to allow.

Three properties hold across every variant:

1. **Capabilities are positive, not negative.** A plugin without
   `net.http` doesn't get filtered network access — it gets no
   network access. There is no "block-list" mode; every grant is
   explicit.
2. **Capabilities are typed.** Adding a new shape (e.g. introducing
   `process.spawn.timeout`) is a schema change with serde-level
   evolution rules; it cannot be conjured by a misformatted manifest
   (ADR-0002 §3).
3. **Capabilities are inert until granted.** A capability in
   `tau.toml` is a *declaration of need*, not a grant. The grant
   happens when a user (or scope policy) accepts it at install time.

## Declared vs granted

This is the most important distinction in the model. The package
manifest *declares* capabilities the plugin *needs*. The lockfile,
after user consent, records what was *granted*. These can differ.

```mermaid
flowchart LR
    M["tau.toml<br/><i>declared</i><br/>fs.read, net.http, …"]
    P{{"<code>tau install</code><br/>user consents?"}}
    L["lockfile<br/><i>granted</i><br/>(possibly narrowed)"]
    O["project override<br/><code>[agents.&lt;id&gt;]</code>"]
    R["runtime<br/>+ kernel"]
    M -->|"need"| P
    P -->|"yes"| L
    P -.->|"no — abort"| X[(install fails)]
    O -.->|"narrows"| L
    L -->|"grant"| R

A plugin author publishes `tau.toml` saying "I need `fs.read` on
`${PROJECT}/**` and `net.http` to `api.example.com`". `tau install`
prints those needs verbatim, the user accepts or aborts, and the
lockfile records the accepted set.

Two paths can narrow the declared set into a smaller granted set:

- **Project override** (`crates/tau-cli/src/config/project.rs`).
  An `[agents.<id>]` block can override a capability with a narrower
  shape: a plugin that declares `fs.read paths = ["**"]` can be
  granted only `paths = ["${PROJECT}/src/**"]` by the project
  configuration. The override must be a *subset* of the manifest's
  grant; an override that expands the package's grant fails
  validation (`CapabilityOverrideExpands` error).
- **Scope policy** (`<scope>/config.toml`). Future hardening will
  allow scope-level deny lists; today the scope's contribution is
  the `[sandbox] required_tier` which sets the minimum enforcement
  floor (see [Sandboxing](sandboxing.md)).

The intersection is what the runtime and the kernel see. The agent
asking a tool to do something the *granted* set doesn't permit gets
an error before the tool plugin is even called; the *declared* set
does not back-door past the override.

## Consent: where the human stands

G14 makes the constitutional commitment: **packages declare their
capabilities at install time; users see what a package claims to do
before installing**. The mechanism:

1. `tau install <source>` clones, parses the manifest, and prints
   the `[[capabilities]]` block to the user (the human-readable
   `tau install --dry-run` shows exactly what `tau install` would
   prompt for).
2. The user accepts (continues) or aborts (Ctrl-C). There is no
   "remember my choice" — `tau install` is interactive by default;
   non-interactive environments must pass `--yes`.
3. The accepted set is hashed into the lockfile alongside the
   content hash. `tau verify` later detects drift (a third party
   editing `~/.tau/packages/<name>/tau.toml`).
4. Escalation requires re-install. `tau install --force <source>`
   re-prompts for the new capability set. There is no in-place
   "expand my grants" verb.

The asymmetry is intentional. *Restricting* a granted capability
(via project override) is cheap and silent — you're handing the
plugin less than it asked for, which is always safe.
*Expanding* a granted capability requires going back through the
prompt — you're handing the plugin more than the lockfile knew
about, which the user must have a chance to refuse.

## Governed by default

The `[allow]` section — the project's *constitution* (ADR-0057) — is
the third quantity in the model. Where a package manifest *declares*
what a plugin needs and the lockfile records what a user *granted*,
`[allow]` is what the project as a whole *permits*: a ceiling over the
capabilities, model aliases, MCP servers and tools any agent in the
project may use. Every capability an agent or tool resolves to must
fall inside that ceiling, or the build is refused.

As of ADR-0057 (Decision D2), `tau build` and the dev-path `tau run`
are **governed by default**. A project `tau.toml` with no `[allow]`
section is a hard error:

```text
error[GOV000]: no [allow] section declared
  = tau builds are governed by default; declare a ceiling or opt out explicitly
  = help: run `tau init --allow` to scaffold a constitution from installed
          packages' declared capabilities, or pass --allow-ungoverned
```

The command exits `2`. `tau init --allow` scaffolds the section for
you, seeded with the commented least-privilege union of your installed
packages' declared capabilities — uncomment and narrow the ones you
actually need. The root ceiling uses kind-as-key entries; model
aliases, MCP servers and tools each get a sub-table:

```toml
[allow]
"fs.write" = { paths = ["${PROJECT}/**"] }
"net.http" = { hosts = ["api.anthropic.com"] }

[allow.models.default]
backend = "anthropic"
model   = "claude-haiku-4-5"

[allow.mcp.weather]
url = "https://api.weather.com/mcp"

[allow.tools.write_file]
native = "WriteFile"
```

> **Model aliases move under `[allow]`.** In a governed project, model
> aliases live in `[allow.models.<alias>]`, not the top-level
> `[models]` — an existing EPIC 1.2 rule. A governed project uses
> `[allow.models.<alias>]`; a bare project uses `[models]`.

### The two escape hatches

Governance can be waived two different ways, and the difference is
recorded, not silent:

| Flag | Meaning | Recorded verdict |
|---|---|---|
| `--allow-ungoverned` | Build/run with **no** `[allow]` ceiling at all. | `ungoverned` |
| `--no-governance` | The project **has** an `[allow]` ceiling; skip checking it this once. | `skipped` |
| *(neither, with a valid `[allow]`)* | Ceiling present and enforced. | `governed` |

The two flags are mutually exclusive on the CLI. The distinction is
deliberate: `--allow-ungoverned` says "I have declared no ceiling on
purpose"; `--no-governance` says "I have a ceiling but am deferring the
check" — different security postures, so they leave different traces.
`--no-governance` with no ceiling to skip still fails `GOV000`.

### The bundle carries its verdict

A built bundle records the outcome in a `[governance]` record whose
`verdict` field is one of `governed` / `ungoverned` / `skipped`. The
record is hashed into the bundle self-hash, so a verdict cannot be
rewritten after the fact. Bundles that carry a `[governance]` record
use bundle `schema_version = 4`; an older tau refuses such a bundle
rather than silently ignoring the verdict.

### Governed on both ends

Governance is checked at *run* time too, not only at build. Running a
bundle whose verdict is `ungoverned` requires `--allow-ungoverned`
again — `tau run --bundle <ungoverned-bundle>` otherwise fails with
`GOV000` (exit 2):

```text
error[GOV000]: bundle was built without a governance ceiling (verdict: ungoverned)
  = running an ungoverned bundle is governed by default
  = help: pass --allow-ungoverned to run it anyway
```

A `governed` or `skipped` bundle runs without the flag; only the
"no ceiling ever" case (`ungoverned`) is gated on both ends.

## Enforcement: where the kernel stands

Once granted, three places re-check the same set:

| Layer | What it checks | What it returns on mismatch |
|---|---|---|
| **Wire** (in `tau-runtime::kernel`) | The agent's tool call is for a tool method whose required capability is in the granted set. | A typed `CapabilityDenied` error returned as the tool result; the agent can react. |
| **Process** (in the sandbox adapter) | The plugin's actual syscall / network attempt is permitted by the kernel-level rules derived from the same granted set. | EACCES / SIGSYS / proxy 403 — observable to the plugin, surfaced to the agent as a tool failure. |
| **Lockfile** (`tau verify`) | The set on disk hasn't been tampered with between install and run. | A `SkillContentDrift` / equivalent error; the plugin refuses to load. |

All three derive from the same `Capability` set by construction. A
divergence between wire-layer gating and kernel-layer gating would
be a bug — but importantly, both layers are necessary: an
in-process gate can be evaded by a misbehaving plugin binary; the
kernel layer can be evaded by a kernel bug. Defence in depth (G12).

### wasm profile: WASI-dispositioned capabilities are enforced at the host

On the wasm target the dispatch-site wire check does **not** re-gate
capabilities whose `Disposition` is `Wasi` (EPIC 3.1) — that is,
`net.http` and `fs.read`/`fs.write`. On wasm these are enforced by the
generated WIT world (EPIC 3.2) plus the embedder's host `WasiCtx` /
`EgressPolicy` (EPIC 3.3), built from the same allow-bounded capability
set: an unauthorized host, method, or path is rejected at the `wasi:http`
/ `wasi:filesystem` boundary before any syscall. The in-guest dispatch
gate skips exactly those caps at compile time (`cfg!(target_arch =
"wasm32")`, so native builds keep the in-guest gate as a provable no-op
off-wasm) — the host is the sole gate for WASI-dispositioned effects.
Every other capability (agent spawn, task-list, plan, skill, and any
`Custom` shape) is still checked in-guest on every target. (EPIC 3.4)

## CapabilityShape: the resolver's vocabulary

When the resolver picks a sandbox adapter (ADR-0015 Decision 4), it
doesn't reason about individual paths or hostnames — it reasons
about *shapes*. The `CapabilityShape` enum erases payload details:

```text
fs.read paths=[…]        ──► FilesystemRead
fs.write paths=[…]       ──► FilesystemWrite
fs.exec paths=[…]        ──► ProcessExec
net.http hosts=[…]       ──► NetworkHttp
agent.spawn …            ──► AgentSpawn
skill.spawn …            ──► SkillSpawn
Custom { name=… }        ──► Custom { name }
```

Adapters advertise the shapes they can satisfy. The resolver checks
"can this adapter cover every shape in the plan?" before picking it.
Why this indirection: a Linux adapter that can do `FilesystemRead`
covers both `fs.read paths=["**"]` and `fs.read paths=["/tmp/*"]`
with the same primitive (landlock). The shape is the right level of
abstraction for the registry to advertise; the payload is the right
level for the kernel rule.

## Custom capabilities — the escape hatch

`Capability::Custom { name, params }` exists for the case "this
plugin needs a capability tau-domain doesn't have a typed variant
for yet" (ADR-0002 §2). The cost the model pays for the escape
hatch:

- **No OS-level enforcement.** No adapter advertises a kernel rule
  for an unknown shape. The runtime can refuse to grant it, log it,
  or pass it through, but the kernel layer has nothing to enforce.
- **Registered in the escape-hatch registry.** Every `Custom`
  capability appears in [escape-hatches.md](escape-hatches.md) with
  rationale, location, and a promotion trigger.
- **A promotion path.** When enough plugins need a particular
  custom shape, the answer is to add a typed variant (a non-breaking
  schema addition) — and ADR-0002's canonicalisation-at-deserialize
  rule means existing manifests upgrade automatically.

Custom is for *prototyping*. Long-lived custom capabilities are a
documentation gap; treat them as TODO items, not as the steady state.

## Where the model meets the wire

Tool plugins go further than other kinds. Install-time Layer 2
cross-check (ADR-0016) spawns the plugin binary and asks every
declared tool method for its actually-required capabilities. The
aggregated set is compared against the manifest *bidirectionally*:

- **Binary claims more than manifest declared** — install fails.
  The plugin author shipped a binary that needs `net.http` but
  forgot to add it to `tau.toml`.
- **Manifest declares more than binary needs** — install fails.
  The plugin author over-claimed in the manifest; the user would
  have consented to capabilities the binary doesn't actually use.

Both directions matter. The first prevents privilege escalation
through manifest forgery; the second prevents stale grants from
accumulating.

LLM-backend, storage, and other ports today fall through to
manifest-only Layer 2 (ADR-0016 Decision 1) because their wire
protocol doesn't yet have a per-method capability query. Phase 2
hardening extends this.

## What the model does not cover

Three classes of access that look like capabilities but aren't:

- **Environment variables**, **stdin** — agents see whatever the
  parent process sees, gated by `tau run`'s own env handling, not
  by the sandbox. Credential abstraction (G13) handles the only
  sensitive case.
- **Compute / time / memory** — the current model has no
  capability for "this plugin may use at most N seconds of CPU".
  See [Sandboxing](sandboxing.md) §"What sandboxing does not do".
- **Side-channel observation** — a plugin with `net.http` to
  `host-a` can observe latency variations on the host; that is
  not a separate capability. Capabilities are a coarse access model,
  not a covert-channel containment model.

The boundary is "things a hostile plugin binary should not be able
to do that we can express as a typed kernel-enforceable rule". Side
channels, resource limits, and credential containment live at
adjacent layers.

## See also

- [The three-gate guarantee](three-gate-guarantee.md) — where the declared /
  granted / allowed model sits in tau's three enforcement gates.
- [Packages](packages.md) — the unit of extension; capabilities are
  declared inside the package manifest.
- [Sandboxing](sandboxing.md) — how the kernel enforces the granted
  set.
- [Escape hatches](escape-hatches.md) — the registry of `Custom`
  capabilities currently in tau core.
- [Package manifest schema](../reference/package-manifest-schema.md)
  — every capability variant and its payload.
- [`CONSTITUTION.md`](../../CONSTITUTION.md) G12, G13, G14 — the
  guidelines this model fulfils.
- [ADR-0002](../decisions/0002-manifest-format.md) — capability
  shape, canonicalisation, escape-hatch policy.
- [ADR-0015](../decisions/0015-sandbox-activation.md) — declarative
  requirements + resolver.
- [ADR-0016](../decisions/0016-plugin-compat-verification.md) —
  install-time Layer 2 bidirectional cross-check.
