# EPIC 3.1 — Capability → WASI/WIT mapping table

**Status:** approved (design)
**Date:** 2026-07-23
**Roadmap:** `docs/superpowers/plans/vision-roadmap.md` EPIC 3, story 3.1
**Crate surface:** `tau-ports::target` (new `wasi_map` module). Read-only
consumption of `tau-domain` capability types. No `tau-pkg` touch.

## Goal

Provide the foundational **data + mapping** that lowers each tau
[`Capability`] to its WASI/WIT realization on the wasm target. This is the
table EPIC 3's downstream stories consume:

- **3.2** generates the WIT world from the `imports` this table yields.
- **3.3** configures the host `WasiCtx` (allowed-hosts, preopens) from the
  `config` fragments this table yields.
- **3.4** drops the in-guest gate on wasm, keyed off `disposition`.
- **3.5** proves the generated WIT is reproducible from declared caps.

3.1 is **the table only**. It does not generate a WIT world, build a
`WasiCtx`, drop any gate, or touch reproducibility. Those are separate
downstream stories (slicing-policy.md: keep the slice end-to-end but
minimal).

## Non-goals

- Generating `.wit` text (3.2).
- Applying config to a real `WasiCtx` / resolving globs to preopen dirs (3.3).
- Removing the in-guest capability gate (3.4).
- Transitive-closure resolution of WASI interfaces. This table lists the
  **direct** interface a capability binds to; 3.2 pulls in the transitive
  dependencies (`wasi:io`, `wasi:clocks`, `wasi:sockets` under `wasi:http`,
  …).
- Raw-socket networking. tau's only network capability is HTTP; a future
  `net.tcp`/`net.udp` capability would map to `wasi:sockets` — recorded here
  as a reserved future path, not implemented.

## Foundations consumed (all merged, read-only)

- `tau_domain::Capability` (`#[non_exhaustive]`): `Filesystem(FsCapability)`,
  `Network(NetCapability)`, `Process(ProcessCapability)`,
  `Agent(AgentCapability)`, `Skill(SkillCapability)`, `TaskList`, `Plan`,
  `Custom`.
- `NetCapability::Http { hosts: HostSet, methods: Option<BTreeSet<HttpMethod>> }`
  — D4-B one-host semantics (PR #487, ADR-0064).
- `FsCapability::{ Read { paths }, Write { paths, max_bytes }, Exec { paths } }`.
- `HostSet::{ Any, Exact(BTreeSet<HostName>) }` — exact hostnames or typed
  `Any`; no globs (ADR-0064).
- Frozen WIT world `tau:host@0.1.0` (`wit/tau-host.wit`): only `complete`,
  `now-millis`, `next-u64` cross the host boundary. Native tools, MCP,
  skills, taskllist, plan, and the context pipeline are all in-guest — no
  host import.

## Network mapping decision: `wasi:http` outgoing, not raw `wasi:sockets`

The roadmap wording is `network → wasi:sockets + allowed-hosts`. tau's only
network capability is **HTTP**, and the egress allow-list is enforced at the
`wasi:http` outgoing-handler host hook (`WasiHttpCtx`), which filters by
**hostname** — exactly what [`HostSet`] carries. Mapping to raw
`wasi:sockets` would:

1. Filter at the socket layer, which only sees **IP:port** after DNS — a
   lossy, TOCTOU-prone reconstruction of a hostname allow-list.
2. Grant the guest a raw TCP/UDP stack it never asked for — strictly wider
   than `net.http`, contradicting the Epic DoD ("wasm caps ==
   `[allow]`-bounded set").

Therefore `net.http` binds to `wasi:http/{types,outgoing-handler}`. Raw
`wasi:sockets` is present only as the transitive transport 3.2 resolves
under `wasi:http`, never as a direct table entry. The roadmap's
"wasi:sockets" is read as *"the networking family"* shorthand.

## API

New module `crates/tau-ports/src/target/wasi_map.rs`, `no_std` + `alloc`,
`#![forbid(unsafe_code)]` (workspace lint). Re-exported from
`target/mod.rs`.

`map_capability` is **total** — every capability yields a `WasiMapping`,
including the `Unsupported`/`HostMediated` dispositions — so there is no
fallible boundary and no `thiserror` error type is introduced.

```rust
/// Lower one tau capability to its WASI/WIT realization on the wasm target.
pub fn map_capability(cap: &Capability) -> WasiMapping;

/// The WASI/WIT realization of a single capability on the wasm target.
pub struct WasiMapping {
    /// WIT interface imports the generated world must declare (3.2).
    /// Empty unless `disposition == Wasi`.
    pub imports: Vec<WitInterface>,
    /// Runtime config fragment this capability contributes to `WasiCtx` (3.3).
    pub config: WasiConfig,
    /// How this capability is satisfied on the wasm target.
    pub disposition: Disposition,
}

/// How a capability is satisfied on the wasm target.
#[non_exhaustive]
pub enum Disposition {
    /// Bounded by a WASI import + config: network, fs.read, fs.write.
    Wasi,
    /// Enforced in-guest by the tau runtime; no WASI surface:
    /// taskllist, plan, agent.spawn, skill.spawn.
    InGuest,
    /// Requires host mediation outside the WASI ABI; out of scope for wasm
    /// capability gating: hardware / generic `Custom`.
    HostMediated,
    /// Cannot be expressed on the wasm target: fs.exec, process.spawn.
    Unsupported {
        /// Human-readable reason (surfaced by 3.2/3.4 diagnostics).
        reason: &'static str,
    },
}

/// Runtime configuration a capability contributes to the host `WasiCtx`.
#[non_exhaustive]
pub enum WasiConfig {
    /// No runtime config (non-Wasi dispositions).
    None,
    /// Network egress filter. `hosts` reuses D4-B `HostSet` semantics
    /// (exact | typed `Any`); `methods == None` means all methods.
    AllowedHosts {
        /// Allowed hostnames.
        hosts: HostSet,
        /// Allowed HTTP methods; `None` = all.
        methods: Option<BTreeSet<HttpMethod>>,
    },
    /// Filesystem preopens derived from the capability's glob paths.
    /// Glob → directory resolution is deferred to 3.3.
    Preopens(Vec<Preopen>),
}

/// A single preopen derived from an fs capability.
pub struct Preopen {
    /// Glob patterns from the fs capability (dir resolution deferred to 3.3).
    pub paths: Vec<String>,
    /// Read-only (fs.read) or read-write (fs.write).
    pub access: PreopenAccess,
}

/// Preopen access mode.
pub enum PreopenAccess {
    /// fs.read → read-only preopen.
    ReadOnly,
    /// fs.write → read-write preopen.
    ReadWrite,
}

/// The WASI interfaces this table references. `package_id()` returns the
/// fully-qualified WIT package id, e.g. `"wasi:http/outgoing-handler@0.2.3"`.
#[non_exhaustive]
pub enum WitInterface {
    /// `wasi:http/types`.
    WasiHttpTypes,
    /// `wasi:http/outgoing-handler`.
    WasiHttpOutgoingHandler,
    /// `wasi:filesystem/types`.
    WasiFilesystemTypes,
    /// `wasi:filesystem/preopens`.
    WasiFilesystemPreopens,
}

impl WitInterface {
    /// Fully-qualified WIT package id (interface path + `@` + version).
    pub fn package_id(&self) -> &'static str;
}

/// WASI preview-2 version this table pins (wasip2, wasmtime-45, β.7.5).
pub const WASI_VERSION: &str = "0.2.3";
```

## The table (`map_capability`)

| Capability | disposition | imports | config |
|---|---|---|---|
| `Network(Http { hosts, methods })` | `Wasi` | `WasiHttpTypes`, `WasiHttpOutgoingHandler` | `AllowedHosts { hosts, methods }` |
| `Filesystem(Read { paths })` | `Wasi` | `WasiFilesystemTypes`, `WasiFilesystemPreopens` | `Preopens([{ paths, ReadOnly }])` |
| `Filesystem(Write { paths, .. })` | `Wasi` | `WasiFilesystemTypes`, `WasiFilesystemPreopens` | `Preopens([{ paths, ReadWrite }])` |
| `Filesystem(Exec { .. })` | `Unsupported { reason }` | — | `None` |
| `Process(Spawn { .. })` | `Unsupported { reason }` | — | `None` |
| `Agent(Spawn { .. })` | `InGuest` | — | `None` |
| `Skill(Spawn { .. })` | `InGuest` | — | `None` |
| `TaskList { .. }` | `InGuest` | — | `None` |
| `Plan { .. }` | `InGuest` | — | `None` |
| `Custom { .. }` | `HostMediated` | — | `None` |

`Unsupported` reasons: fs.exec → `"wasm target has no exec surface"`;
process.spawn → `"wasm target cannot spawn OS processes"`.

`hosts` and `methods` flow through **unchanged** from the capability into
`AllowedHosts` — the table does not narrow or widen the D4-B set;
`HostSet::Any` and `Exact` both pass verbatim.

## Data flow

```
tau_domain::Capability
        │
        ▼
  map_capability(&cap)  ── pure, total, no_std ──►  WasiMapping
                                                     ├── imports:  Vec<WitInterface>  ──► 3.2 (WIT world gen)
                                                     ├── config:   WasiConfig         ──► 3.3 (WasiCtx build)
                                                     └── disposition: Disposition     ──► 3.4 (gate-drop decision)
```

## Testing (part of done, TDD)

One unit test asserting the mapping for each table row, plus:

- `HostSet::Any` and `HostSet::Exact({..})` both flow into `AllowedHosts`
  verbatim (no narrowing).
- `methods: None` and `methods: Some({POST})` both preserved verbatim.
- fs.read → `PreopenAccess::ReadOnly`; fs.write → `PreopenAccess::ReadWrite`;
  the capability's `paths` appear verbatim in the `Preopen`.
- Every `WitInterface::package_id()` starts with `"wasi:"` and ends with
  `"@{WASI_VERSION}"` (guards version drift across interfaces).
- `Unsupported` variants carry a non-empty `reason`.
- `map_capability` is total: it returns for every constructible capability
  variant (the `#[non_exhaustive]` catch-all arm, if reachable, maps to a
  conservative `HostMediated`/`Unsupported` — see Open questions).

## Open questions / decisions

- **`#[non_exhaustive]` catch-all.** `Capability`, `FsCapability`, etc. are
  `#[non_exhaustive]`, so `map_capability` needs catch-all arms. A future,
  as-yet-unknown capability variant is conservatively **not** granted a WASI
  import: it maps to `HostMediated` (out of scope for wasm gating) rather
  than silently `Wasi`. This keeps the table fail-closed. Decided:
  fail-closed to `HostMediated`.

## Downstream contract (informative)

3.2 will call `map_capability` over the used-and-`[allow]`-bounded capability
set, union the `imports`, and emit a `world` importing exactly those
interfaces (plus transitive closure). 3.3 will fold the `config` fragments
into a single `WasiCtx` (union of `AllowedHosts` via the existing
`host_union` lattice op; concatenation of `Preopen`s). This table is the
single source of truth both consume.
