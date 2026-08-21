# EPIC 3.6-b — Guest fs effects → wasi:filesystem (binary-observable, host-enforced)

**Issue:** tau-rs/tau#596
**Date:** 2026-08-21
**Status:** Design approved (brainstorm), pending spec review
**Predecessor:** EPIC 3.6 net-only (#585 / `3e1c7540`),
`docs/superpowers/specs/2026-08-10-epic-3-6-guest-effect-abi-design.md`

## Context

EPIC 3.6 made a granted `net.http` capability **binary-observable** and
**host-enforced**: the production guest routes a `native = "Fetch"` tool through
`wasi:http` via its own `generate_all` wit-bindgen bindings, gated by a
`tau_cap_net_http` cfg that `build.rs` emits iff the cap-derived world imports
`wasi:http`. The compiled component's *actual* imports then include `wasi:http`
(wasm-ld no longer DCE-strips it, because the effect arm is statically reachable
from `run`), and enforcement is the host `EgressPolicy`/`WasiCtx` (3.3/3.4) with
**no in-guest capability gate**.

3.6 shipped net only; `fs.read`/`fs.write` were deferred to this story (3.6-b).
The host-side plumbing already exists:

- `resolve_wasi_config` folds a cap set into a `WasiConfiguration` whose
  `preopens: Vec<ResolvedPreopen>` carry `{host_dir, access, granularity, from}`
  (glob→directory resolved, dedup'd) —
  `crates/tau-ports/src/target/wasi_map.rs`.
- The host embedder preopens each `host_dir` under `sandbox_root` at the
  **guest-visible name `host_dir`** with `DirPerms/FilePerms` from
  `PreopenAccess` (ReadOnly vs ReadWrite) —
  `crates/tau-wasm-host/src/lib.rs` (`wasi_ctx_from_config`,
  `preopened_dir(&host_path, &p.host_dir, dir_perms, file_perms)`).
- `map_capability` already maps `fs.read`/`fs.write` to
  `Disposition::Wasi` importing `wasi:filesystem/types` + `wasi:filesystem/preopens`,
  and the vendored WIT for both is present under
  `crates/tau-wasm-guest/wit/deps/wasi-filesystem/`.

What is **missing** is the guest side: the production guest never calls
`wasi:filesystem`, so the compiled fs component DCE-strips the import (the
`build_wasm_world_dod.rs` `wasi:filesystem` regression guard is vacuous), and no
live round-trip proves an ungranted path is host-denied *through the real guest*.

## What net did not have: the descriptor/stream ceremony

Net was a single host call (`outgoing-handler::handle(request, None)`). Fs is a
capability-security descriptor dance: the guest holds **no ambient root** on
`wasm32-wasip2` — it can only reach files reachable from a preopened directory
descriptor the host handed it. The guest must therefore resolve a requested path
*relative to* a preopen before it can open anything.

```
tool arg {path:"/data/ok.txt"}
        │
        ▼
wasi:filesystem/preopens.get-directories()  ──► [(desc_A, "/data"), …]   (HOST WasiCtx state)
        │
        │  find the entry whose guest-path is a PREFIX of the requested path
        ▼
  match "/data"  →  relative = "ok.txt"
        │
        ▼
desc_A.open-at(path-flags::symlink-follow, "ok.txt", <open-flags>, <descriptor-flags>)
        │                                            ──► result<descriptor, error-code>   (HOST enforces)
        ▼
   read:  read-via-stream(0)  → input-stream  → blocking_read loop → utf8-lossy
   write: write-via-stream(0) → output-stream → blocking-write-and-flush(content)
```

## Decision

### D1 — Reuse the 3.6 cfg-gate + generated-bindings mechanism

All new guest code lives in `crates/tau-wasm-guest` and uses the guest's OWN
`generate_all` bindings (no external `wasi` crate — the guest already exports
`cabi_realloc`, so a second one is a dup-symbol link error; identical rationale
to 3.6).

- `build.rs`: after computing the cap-derived world text, emit
  `cargo:rustc-check-cfg=cfg(tau_cap_fs_read)` and
  `cargo:rustc-check-cfg=cfg(tau_cap_fs_write)` **unconditionally** (workspace
  lints = `-D warnings`), and emit `cargo:rustc-cfg=tau_cap_fs_read` **and**
  `cargo:rustc-cfg=tau_cap_fs_write` iff the world text contains
  `wasi:filesystem`. Mirrors the existing `tau_cap_net_http` block.
- `guest.rs`/`lib.rs`: widen the `wit_wasi` re-export cfg from
  `tau_cap_net_http` to `any(tau_cap_net_http, tau_cap_fs_read, tau_cap_fs_write)`
  so `crate::wit_wasi::filesystem::*` is reachable whenever any WASI effect is
  granted.
- `dispatcher.rs`: `GuestDispatcher::invoke` gains a `#[cfg(tau_cap_fs_read)]`
  arm matching `native == "Read"` and a `#[cfg(tau_cap_fs_write)]` arm matching
  `native == "Write"`, each keyed on the **declared native fn name** (the stable
  contract via `native_fn_name`), calling the generated `wasi::filesystem`
  bindings. Ungranted / non-fs tools fall through to
  `tau_native_tools::invoke` unchanged, exactly as the `Fetch` arm does.

Tool contracts (mirror `Fetch`'s `{url,method} → {status,body}`):

| native | args | ok result | err |
|---|---|---|---|
| `Read`  | `{path: string}`                 | `{content: string, bytes: number}` | `Err(String)` |
| `Write` | `{path: string, content: string}`| `{bytes: number}`                  | `Err(String)` |

A host denial surfaces as `ToolInvocationResult.error = Some(msg)` (never a
panic / trap), identical to the `Fetch` arm.

### D2 — Both fs cfgs gate on the single `wasi:filesystem` world import

`fs.read` and `fs.write` map to the **same** two interfaces
(`wasi:filesystem/types` + `/preopens`); the world *text* cannot distinguish
read from write. So `build.rs` sets both `tau_cap_fs_read` and
`tau_cap_fs_write` together whenever `wasi:filesystem` is present, and both the
Read and Write arms compile whenever fs is granted at all.

**Read-vs-write is enforced at runtime by the host**, not the cfg: the host
preopens ReadOnly (`DirPerms::READ`) vs ReadWrite (`DirPerms::all()`) from
`PreopenAccess`. A `Write` against a read-only preopen fails at the host
`open-at`/`write-via-stream` with an `error-code`. The two-cfg naming is kept
(not collapsed to one `tau_cap_fs`) for symmetry with the per-effect gate model
and to leave room for the interfaces to diverge later; today they move in
lockstep.

### D3 — Preopen-relative resolution is descriptor plumbing, not a cap gate

The guest computes reachability **solely** from the host-provided
`get-directories()` list — it never consults a capability set, an allow-list, or
any policy. Two outcomes:

- **A matching preopen exists** → the guest strips the preopen's guest-path
  prefix and passes the remainder straight to the host `open-at`. It does **not**
  itself reject `..` components or normalize the path — the **host** `open-at`
  rejects escapes at the descriptor boundary (`cap-std` sandbox). Passing the
  raw remainder through keeps 3.4's "no in-guest gate" invariant intact.
- **No matching preopen** → the guest holds no descriptor for the path and
  cannot fabricate one (there is no ambient root on wasip2). It returns a denial.

This is the WASI capability-security model: *absence of a preopen ⇒ absence of
capability*. The enforcement point is the host's `get-directories` set (its
`WasiCtx`), which the host populates solely from granted caps.

### D4 — Denial-marker asymmetry vs net

Net's denial is a host hook (`WasiHttpHooks::send_request` → `EgressPolicy`)
returning a `wasi:http` `error-code` (`HttpRequestDenied`); the round-trip
asserts that *exact* code (#546 lesson).

Fs's ungranted-path denial (D3, no-preopen branch) is **guest-observed
absence**: the guest never calls the host for that path, so **there is no host
`error-code` by construction**. The round-trip therefore asserts an exact,
stable **guest-authored** marker — `FsAccessDenied` — not a bare
`contains("denied")`. The *enforcement* remains 100% host (what the host placed
in `get-directories`); only the *marker string* is guest-emitted. This
asymmetry is intrinsic to the descriptor model and is pinned in ADR-0066.

### D5 — ADR

A short ADR-0066 (`docs/decisions/0066-...`) pins D2, D3, and D4 — the subtle
soundness points a future reader of the fs arm will need. Added to `SUMMARY.md`.

## Definition of Done

1. **`build_wasm_world_dod.rs`** — add an `fs-read` fixture
   (`crates/tau-cli/tests/fixtures/wasm-build/fs-read/tau.toml`:
   `[tools.read_file] native = "Read"`, cap `{kind="fs.read", paths=["/data/**"]}`);
   assert its compiled component's *actual* imports contain a
   `wasi:filesystem/` interface (positive binary-observable assertion mirroring
   the existing `wasi:http/` one). The `net-http` fixture's existing
   `!wasi:filesystem` negative assertion continues to hold.
2. **`crates/tau-cli/tests/wasi_fs_roundtrip.rs`** (new, `#[ignore]`, wasm lane)
   — build the real fs guest via `tau build wasm`, grant `fs.read` on
   `/data/**` (host preopens `<sandbox>/data`), drive a cassette that `Read`s an
   **ungranted** `/etc/secret`; assert the emitted events contain
   `FsAccessDenied`. Denial-only, offline — mirrors `wasi_http_roundtrip.rs`.
3. **#597 blind spot** — no clippy job for the cfg-ON fs arm has landed. Run a
   manual `-D warnings` clippy of `tau-wasm-guest` for `wasm32-wasip2 --release`
   with `TAU_WORLD_WIT` pointed at an fs-granting world before PR (as #585 did),
   and note it in the PR body for #597 to fold in.
4. Roadmap tick (the EPIC-3 line that reads `net shipped; fs → 3.6-b`).

## Non-goals / YAGNI

- The positive/connected read+write path stays **offline-untested** by design,
  same as net (a granted preopen would need real files but the round-trip's job
  is the *denial* invariant; the positive read is covered by the pre-existing
  host-side `wasi_fs_enforcement.rs` fs-probe test).
- No `..`-traversal escape assertion — that is defense-in-depth the host
  `open-at` already covers and the fs-probe test already exercises. (Considered
  and dropped during brainstorm to keep the round-trip lean.)
- Both `Read` and `Write` arms are implemented (both cfgs), but only the `Read`
  denial is round-tripped; `Write` is covered structurally + by the
  world-import DoD.

## Soundness invariants (must hold, mirror 3.6)

1. **Fail-closed via cfg**: ungranted fs ⇒ world lacks `wasi:filesystem` ⇒ cfgs
   off ⇒ arms not compiled ⇒ no import.
2. **No in-guest cap gate**: the Read/Write arms perform zero capability checks;
   reachability derives only from host `get-directories`, and access from host
   `open-at`/stream `error-code`s.
3. **No panic path**: every WASI `result` → `Err(String)` tool error.
4. **DCE-survival**: the arms are statically reachable from `run`, so the
   `wasi:filesystem` import survives wasm-ld — the binary-observable DoD.
5. **Guest owns `cabi_realloc`**: no external `wasi` crate.
