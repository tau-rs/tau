# Audit remediation — 2026-08-22 full-workspace bug audit

Remediation of the 2026-08-22 codebase audit (7 parallel audit agents over
main @ `760d5360`, findings independently spot-verified). Nine independent
lanes, one Conductor workspace per lane. All lanes are parallel-safe: crate
footprints are disjoint (H5 was folded into lane A for that reason).

File/line references are as of `760d5360`; main moves fast — if a line has
drifted, re-locate by symbol name before editing.

## Status board

Legend: ⬜ blocked · 🟡 ready · 🔵 in-progress · 🟣 in-review · ✅ done

| Lane | Scope (crates) | Deps | Status | Claim branch |
|---|---|---|---|---|
| A | bundle pipeline (tau-pkg, tau-cli/cmd) | — | 🟡 | `fix/audit-lane-a` |
| B | MCP expansion + lowering (tau-ir-lower, tau-pkg) | — | 🟡 | `fix/audit-lane-b` |
| C | capability soundness (tau-ports, tau-domain, tau-wasm-host, tau-wasm-guest) | — | 🟡 | `fix/audit-lane-c` |
| D | MCP transport (tau-mcp-tokio) | — | 🟡 | `fix/audit-lane-d` |
| E | plugin host runtime (tau-runtime-tokio, tau-runtime-core) | — | 🟡 | `fix/audit-lane-e` |
| F | observability / run log (tau-observe, tau-cli lib+tracing, tau-workflow) | — | 🟡 | `fix/audit-lane-f` |
| G | sandbox adapters (tau-sandbox-darwin/-windows/-native) | — | 🟡 | `fix/audit-lane-g` |
| H | codegen / TS extraction (tau-ts-extract, tau-sdk-codegen) | — | 🟡 | `fix/audit-lane-h` |
| I | shell + LLM plugins (tau-plugins/*) | — | 🟡 | `fix/audit-lane-i` |
| J | file 2 design issues (no code) | — | 🟡 | n/a (issues only) |

Already resolved elsewhere: #620 ([allow.models] run path) fixed by #628.
Still filed, out of scope here: #621 (wasm Branch/Loop feature-fit),
#623 (bundle entry agent alphabetical).

## Handoff protocol (every lane session follows this)

**Claim check (before any work):**
`gh pr list --search "audit-lane-<x>" --state all` AND
`git ls-remote origin 'refs/heads/fix/audit-lane-<x>'`.
If either shows activity → STOP, report "lane already claimed". Branch
existence IS the claim; there is no separate board-flip to claim.

**Working method:** TDD (failing test first — superpowers:test-driven-development).
Follow the repo CLAUDE.md cargo rules exactly (CARGO_TARGET_DIR, -p scoping,
timeouts, CARGO_INCREMENTAL=0, nextest). No drive-by refactors; every change
traceable to a lane node.

**Gate before PR (in this order):**
1. `timeout 30 env CARGO_TARGET_DIR=target/main cargo fmt --check` (separate gate — clippy green ≠ fmt clean)
2. `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo clippy -p <each touched crate> --all-targets`
3. `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo nextest run -p <each touched crate>`

**Merge:** `gh pr create --base main`, conventional title
`fix(<scope>): <summary> (audit lane <X>)`, PR body links this plan.
Enrol `gh pr merge <N> --squash --delete-branch --auto`; run
`gh pr update-branch <N>` whenever the PR is BEHIND. In the SAME PR, flip
this board's row for your lane to ✅ (🟣 while in review is optional).

**Session-end report (in chat, always):** merged PR number, nodes completed,
deviations from spec, anything discovered-but-not-fixed (file an issue),
then list lanes still unclaimed (loop the claim check over a–i) so the
human knows what to paste next. All lanes were handed out on day 0, so no
new handoff blocks need to be emitted — just report what remains.

## Lane task specs

### Lane A — bundle pipeline (tau-pkg, tau-cli/cmd)
- **A1 [S]** `crates/tau-pkg/src/bundle/verify.rs:32` — bump
  `MAX_SUPPORTED_SCHEMA_VERSION` 4→5. Asset-bearing bundles (file prompts)
  are written as v5 by `bundle/build.rs:370` and currently fail every
  `tau run --bundle` with `UnsupportedSchemaVersion`. Add an e2e test:
  project with `[agents.x.prompt] system_file` → build → run --bundle OK.
- **A2 [M, after A1]** `crates/tau-pkg/src/bundle/build.rs:91` reads
  `<root>/tau.lock` but `tau install` writes `tau-lock.toml`
  (`scope.rs:542`) — nothing ever creates `tau.lock`. DECIDED: canonical =
  `tau-lock.toml`; read it in build/verify/reproduce; accept `tau.lock` as
  fallback with a deprecation warning for one release; migrate fixtures
  (they hand-write `tau.lock` today).
- **A3 [S]** `crates/tau-pkg/src/bundle/reproduce.rs:222` —
  `diff_manifests` also compares `triggers`, `assets`, `ir_format`,
  `selected_agents` so "NOT reproducible" never renders an empty
  divergences list.
- **A4 [M]** `crates/tau-cli/src/cmd/run.rs:131` — governance gate must
  re-derive the verdict from the byte-verified cwd source instead of
  trusting the bundle's stamped `[governance] verdict` (stamp is editable +
  self-hash unsigned). DECIDED: re-derive (source already proven identical).
- **A5 [S]** `crates/tau-cli/src/cmd/run.rs:307` — `--resume` off the
  bundle path is silently ignored; make it a hard error
  ("--resume requires --bundle"). Wiring cwd-path resume is a feature, not
  this fix.
- **A6 [S]** (= audit H5) `crates/tau-cli/src/cmd/build.rs:721` —
  `sanitize_crate_name` accepts a leading digit → uncompilable generated
  crate. Prefix `t` (or error). Tests: `2048`, `3d-viewer`.

### Lane B — MCP expansion + lowering (tau-ir-lower, tau-pkg)
- **B1 [M]** `crates/tau-ir-lower/src/lower/resolve.rs:110` —
  `expand_mcp_entry` clones the author `ToolSpec` (incl. `name`) into every
  expanded server tool and ignores `ResolvedServerTool.input_schema`.
  Multi-tool servers → duplicate `spec.name` → `NameCollision` at run
  (agent_loop.rs:477). Fix: expanded name = `<entry>.<server_tool>`, pass
  server `input_schema` through (author schema only as fallback). Test:
  2-tool MCP bundle builds AND runs.
- **B2 [S, after B1]** `resolve.rs:127` — rebuild
  `workflow.capability_table` after expansion (remove author ToolId, insert
  per-expanded-tool intersected caps). Assert `build_wasm`'s
  `world_from_module` (tau-cli build_wasm.rs:104) sees the new entries, not
  the stale author envelope.
- **B3 [S]** `crates/tau-ir-lower/src/lower/parse.rs:361` +
  `crates/tau-pkg/src/project/project.rs:1495` — a deliverable id equal to
  a goal id silently overwrites the goal check (single BTreeMap). Add
  cross-table id-uniqueness validation → hard build error.
- **B4 [S]** `crates/tau-ir-lower/src/lower/typecheck.rs:241` — pass
  `visible_from_prior` into the Check `Locus::Output` ordering check and
  `Branch.on`/`Loop.until` condition scopes so nested-step outputs that are
  legal in `${steps.x.output}` templates are equally legal in conditions
  (EPIC 4.2 Decision 1 consistency).

### Lane C — capability soundness (tau-ports, tau-domain, tau-wasm-host, tau-wasm-guest)
- **C1 [L]** `crates/tau-ports/src/target/wasi_map.rs:336` +
  `crates/tau-wasm-host/src/wasi.rs:33` — `resolve_wasi_config` folds
  hosts×methods into two independent unions; grants `GET a.com` +
  `POST b.com` ⇒ guest may `POST a.com` (bounding-box widening the D3
  lattice bans). Fix: keep rectangles —
  `net: Vec<EgressRule { hosts, methods }>`; `EgressPolicy::permits` =
  ∃ rule with host∈rule ∧ method∈rule. ⚠ pub-field change in tau-ports =
  BREAKING → semver bump. Regression test mirrors
  `subset_and_meet_sound_for_multientry_http`.
- **C2 [S]** `crates/tau-domain/src/package/host.rs:76` — port must be
  digits-only (reject `+80`, `0080` at parse; they can never match at run).
- **C3 [S]** `crates/tau-domain/src/package/capability/lattice/mod.rs:388`
  — add `Forward ∧ Forward` meet arm (equal → Some(that), else None);
  law test: `meet([F],[F])` non-empty iff `subset` holds (idempotence).
- **C4 [S]** `crates/tau-domain/src/package/capability.rs:879` —
  `Custom` constructor rejects names that match fixed kinds or lack the
  `custom.` prefix (today a Custom named `fs.read` round-trips through
  canonical bytes into a typed wide grant).
- **C5 [S]** `crates/tau-wasm-guest/src/dispatcher.rs:284` —
  `fs_read_via_wasi` treats `LastOperationFailed` as EOF, returning
  truncated content as success. Distinguish `Closed` vs
  `LastOperationFailed` exactly like `fetch_via_wasi` (lines 217–226).

### Lane D — MCP transport (tau-mcp-tokio)
- **D1 [S]** `src/transport_http/sse.rs:30` — per-chunk `from_utf8` kills
  the stream when a multi-byte char splits across network chunks. Buffer
  raw bytes; decode up to `error.valid_up_to()`, carry the tail.
- **D2 [M]** `src/host_lifecycle/inbound_dispatch.rs:79` — the pump and
  `call_tool` race one framer mutex and the pump DISCARDS Response
  messages it wins (call_tool then hangs to 60s timeout). Fix: single
  demux owner — one read loop routes Response → pending-call map,
  Request → handler; `call_tool` never reads the transport directly.
  Test: concurrent server-initiated request + `tools/call` on stdio.
- **D3 [S]** `src/transport_stdio/server.rs:35` — child stderr is piped
  (spawn.rs:53) but never drained → deadlock at ~64 KiB. Spawn a drain
  task (debug-log or ring-buffer last N KiB).
- **D4 [S]** `src/transport_stdio/framer.rs:42` — `read_line` with no size
  cap + boxed self-recursion on blank lines. Add a max_message_size bound
  (mirror `FramerOptions`) and loop instead of recursing.

### Lane E — plugin host runtime (tau-runtime-tokio, tau-runtime-core)
- **E1 [M]** `src/plugin_host/ipc_llm.rs:147` + `process.rs:731-815` —
  `in_flight_streams` entries are inserted per `llm.stream` call and only
  cleared at plugin EOF → unbounded growth in long-lived hosts. Remove the
  entry on stream-end; RAII guard cleans up on caller error/cancel between
  insert and response.
- **E2 [S, after E1]** `process.rs:788` — read loop holds the streams
  mutex across `tx.send(chunk).await` on a bounded(64) channel: one
  stalled consumer blocks ALL RPCs on the plugin. Clone the sender, drop
  the guard, then await (or per-stream forwarding task).
- **E3 [S]** `crates/tau-runtime-core/src/stream.rs:687,1855` — durable
  turn checkpoints are persisted AFTER `yield TurnCompleted`; a consumer
  cancelling at that boundary re-executes the whole turn on resume.
  Persist before yielding (both sites). Test: drop stream at
  TurnCompleted, resume does not re-run the turn's tools.

### Lane F — observability / run log (tau-observe, tau-cli, tau-workflow)
- **F1 [S]** `crates/tau-cli/src/lib.rs:50` — `WorkflowRunLogLayer` has no
  per-layer filter, so `--quiet`/restrictive `RUST_LOG` empties
  `.tau/workflow-runs/<id>.jsonl`. Wrap in the same `filter_fn` bypass
  `PluginRecordingLayer` has (lib.rs:73-82).
- **F2 [M, after F1]** `crates/tau-observe/src/layers/workflow_run_log.rs:98`
  — per-event fire-and-forget `tokio::spawn` loses the tail at process
  exit (breaks `tau workflow run --resume` by dropping the final Ok
  record) and can reorder lines. Add pending-counter + `flush()`
  (mirror plugin_recording.rs:120); call flush at workflow-run end;
  serialize writes per file.
- **F3 [S]** `crates/tau-observe/src/install.rs:413` —
  `install_non_blocking_inner` never composes `extra_layers`/`otlp`, so
  `--log-non-blocking` silently drops the run log, protocol recording,
  and OTLP. Compose them; test a layer receives events on this path.
- **F4 [S]** `crates/tau-workflow/src/persistence.rs:262` — producer
  always emits `error:"", detail:""` (schema drift vs legacy
  skip-if-None). Emit only when Some; fix
  `tau-observe/tests/layer_format_compat.rs:126` to drive the REAL
  producer path instead of hand-editing the emission.
- **F5 [S]** both layers (`workflow_run_log.rs:98`,
  `plugin_recording.rs:182`) — bare `tokio::spawn` in `on_event` panics
  off-runtime. Use `Handle::try_current()` with a sync-write or
  counted-drop fallback.

### Lane G — sandbox adapters (tau-sandbox-darwin, -windows, -native)
- **G1 [M]** `crates/tau-sandbox-darwin/src/lib.rs:198` —
  `wrap_spawn_macos` rebuilds the Command for sandbox-exec and never
  re-pipes stdio (0 stdin/stdout/stderr refs in the file) — the #617
  landmine, unfixed on darwin. Copy the strict.rs:456-473 restore pattern
  + comment; add a RebuildingGate-style test that takes `child.stdin`
  (the existing tests mask the bug via `Command::output()`).
- **G2 [S]** `crates/tau-sandbox-windows/src/lib.rs:238` — same
  pattern, same fix, same test shape.
- **G3 [S]** `crates/tau-sandbox-darwin/src/lib.rs:256` — SBPL profile is
  written to predictable `/tmp/tau-darwin-<pid>-<n>.sb` world-readable,
  chmod 0600 after. Use a 0o700 per-run dir (reuse the proxy
  `make_run_dir` pattern) + create with mode 0600 via
  `OpenOptions::mode`.
- **G4 [S]** `crates/tau-sandbox-darwin/src/profile.rs:105` —
  `quote_sbpl` guards `"` only with `debug_assert`; release builds emit
  an injectable profile. Hard error on `"` or `\n` in paths.
- **G5 [S]** `crates/tau-sandbox-native/src/light.rs:294` — DECIDED:
  docs-fix only. `exec.rs` module doc must state Execute rides all read
  paths + baseline dirs; per-command exec gating is a feature → file an
  issue, don't change runtime behavior in this lane.

### Lane H — codegen / TS extraction (tau-ts-extract, tau-sdk-codegen)
- **H1 [M]** `crates/tau-ts-extract/src/lower.rs:455,581` — `toml_str`
  doesn't escape newlines/control chars (multi-line system prompt →
  unparseable TOML, position 0:0 error) and all numbers go through
  `Number::from_f64` (`maxLength: 10` → `10.0`, breaking TOML↔TS
  byte-equal IR). Route through a real TOML encoder (`toml::Value`);
  preserve i64 before f64 fallback. Add conformance fixtures WITH a
  multi-line prompt and an integer schema field.
- **H2 [S, after H1]** `lower.rs:798,913` — non-string-literal elements
  in capability `paths` / agent `produces` are silently dropped → hard
  error (match the pipeline/goals strictness in the same file).
- **H3 [S]** `lower.rs:192` — duplicate singleton factory calls
  (`models()`, `pipeline()`, `goals()`, `deliverables()`) silently
  last-write-win in const-name-alphabetical order → hard error naming
  both consts.
- **H4 [M]** `crates/tau-sdk-codegen/src/emit_python.rs:91` — generated
  Python SDK's TOML renderer: no string escaping (multi-line prompt →
  invalid TOML), unquoted/unvalidated table keys (`[models.<alias>]`
  splice + `_toml_inline` key injection). Port the Rust `toml_key()` +
  escaping helpers into the generated code; roundtrip test with hostile
  names.

### Lane I — shell + LLM plugins (tau-plugins)
- **I1 [S]** `crates/tau-plugins/shell/src/runner.rs:92` — after timeout
  kill, `stdout_task.await` blocks forever if a grandchild holds the pipe
  fd. Wrap the joins in a timeout; on expiry return partial output with
  `timed_out=true`.
- **I2 [S]** `anthropic/src/client.rs:136`, `openai/src/client.rs:144`,
  `ollama/src/client.rs:135` — `Retry-After` parsed unclamped;
  `secs * 1000` can overflow and valid large values sleep for a day.
  `checked_mul` + clamp `.min(60_000)` (same cap as `backoff_only`);
  one test per client.

### Lane J — issues only (no code)
File two design issues on tau-rs/tau:
1. **Governance gate scope**: `tau serve`/`chat`/`dev`/`workflow run`
   execute agents with no `[allow]` gate; ADR-0057 names only
   build/run/check. Needs a ruling (extend gate vs document scope).
2. **Egress host:port matching UX**: `EgressPolicy::permits` requires
   exact authority equality, so a cap for `a.com` never matches
   `a.com:8443` (fail-closed surprise). Decide whether portless hosts
   should match any port, and align `HostName` canon.
Reference this plan + the audit chat of 2026-08-22 in both.
