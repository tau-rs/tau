# β.7 — `tau dev` one-engine REPL — design

**Status:** Approved 2026-06-10. Implements ROADMAP Phase β sub-project β.7.

**Date:** 2026-06-10.

**Scope amendment to ROADMAP:** this spec splits β.7 as originally written into two sub-projects: β.7 (this spec — REPL only) and **β.7.5** (the IR-to-wasm AOT compiler). See *§9 ROADMAP edit*.

**Builds on:** β.1 (`tau-runtime-core`), β.2 (workflow IR + `run_via_ir`), β.3 (MCP facilitator + `McpBridge`).

**Preserves:** every existing CLI verb (`tau run`, `tau chat`, `tau serve`, `tau build`, `tau workflow`, `tau mcp`, `tau check`, `tau skill`) continues to do what it does today. `tau dev` is purely additive.

**Adds:** `tau dev <project>` — a hot-reload REPL that drives the existing β.3 runtime path (`tau-runtime-tokio` + `McpBridge` + `run_via_ir`) with a stdin loop and a `notify`-driven file watcher.

**Out of scope (deferred to β.7.5):** ahead-of-time lowering of the workflow IR to a runnable wasm component. ROADMAP §β.2's footnote ("AOT lands in β.7") is amended below — see §9.

---

## 1. Goals & non-goals

### Goals (load-bearing)

1. **`tau dev <project>` boots in under a second** on a project with one agent + one native tool + one MCP server.
2. **Manifest edits hot-reload via explicit `:reload`** — conversation history is preserved across reload.
3. **No new runtime path** — `tau dev` calls into the same `run_via_ir` that β.3 PR-5 wired; the agent loop, IR interpreter, MCP facilitator are all unmodified.
4. **Vercel-DX feel without watch-server chaos** — REPL with explicit reload is the default; `--watch` opts into auto-reload for users who want it.
5. **Cassette-replay compatible** — projects using `cassette:` MCP URLs (PR #300) work in `tau dev` without modification.

### Non-goals (β.7 v1)

- **AOT compilation of IR to wasm.** Split out to β.7.5.
- **Persistent session save/resume.** History is in-memory only; `tau dev --resume <id>` deferred to β.4 (ContextManager).
- **Tool code hot-reload.** Editing a native Rust tool still requires `cargo install tau --force` to rebuild. Tool code hot-reload is β.8's territory (TS surface via esbuild).
- **TUI / fancy ANSI cursor management.** `rustyline` for line editing only; no curses, no panels.
- **Watch-on-MCP-contract-file.** `.tau/mcp/*.contract.json` is derived state; not in the watch set.
- **Multi-project / monorepo support.** One project per invocation.
- **`tau run` migration to the same path.** `tau run` continues to use the legacy plugin host. Convergence is a γ-or-later concern.
- **Multi-prompt batch mode.** `-p "single prompt"` covers one-shot; `-p1 -p2 -p3` is YAGNI.

---

## 2. User-facing surface

### 2.1 CLI

```
tau dev <PROJECT> [OPTIONS]

OPTIONS:
  -p, --prompt <STR>    Run one turn with this prompt and exit (single-shot).
      --agent <NAME>    Pick a non-default agent. Default = first declared.
      --watch           Auto-reload on file change (Mastra-style). No manual :reload.
      --no-color        Disable ANSI coloring of output.
  -h, --help            Print help.

EXAMPLES:
  tau dev myproject/                    # REPL mode
  tau dev myproject/ -p "hello"         # one-shot
  tau dev myproject/ --watch            # auto-reload on save
  tau dev myproject/ --agent reviewer   # pick a non-default agent
```

### 2.2 REPL command surface

| Command | Behavior |
|---|---|
| `> <text>` | Run as a turn (prompt sent to the current agent) |
| `:reload` | Apply pending manifest changes; keep conversation history. No-op if no change pending. |
| `:state` | Print: history length, token estimate, current agent name, list of active MCP clients |
| `:history` | Print last 20 messages (with `--all` for full) |
| `:agents` | List agents in the project; mark the current one |
| `:agent <name>` | Switch the current agent for subsequent turns |
| `:clear` | Reset conversation history. Manifest stays. |
| `:help` | List commands |
| `:quit`, Ctrl-D | Exit (drops MCP clients cleanly, exit 0) |

Ctrl-C during a turn cancels the turn and returns to the prompt. Ctrl-C at the prompt is a no-op (printing a hint, like Python's REPL).

### 2.3 Boot sequence (the <1s promise)

```
tau dev myproject/
│
├─ T+0ms     parse CLI args (clap)
├─ T+5ms     read myproject/tau.toml
├─ T+15ms    validate via ProjectConfig::parse_str
├─ T+20ms    resolve IR (β.2 lower_to_ir)
├─ T+30ms    initialize Tokio runtime (current-thread flavor)
├─ T+40ms    register native tools into NativeRegistry
├─ T+50ms    spawn `notify` file watcher
├─ T+60ms    print banner + prompt
└─ ready
```

MCP servers are NOT spawned at boot — they're lazy. First tool call to an MCP server spawns its transport (stdio, http, or cassette per β.3 PR-6). Subsequent calls reuse the open client. Server processes are killed on `:reload` or `:quit`.

Boot-time perf target: **< 500ms median**, **< 1s p99**, on a project with 1 agent + 2 native tools + 1 MCP server declared (none yet contacted).

---

## 3. Architecture

### 3.1 Components

```
                 tau dev <project>
                        │
                        ▼
  ┌──────────────────────────────────────────────────────────────┐
  │  tau-cli::cmd::dev          (new module)                      │
  │                                                                │
  │   ┌─────────────────┐    ┌──────────────────┐                  │
  │   │ repl::run_loop  │    │ watcher::spawn   │                  │
  │   │ (stdin parser)  │    │ (notify crate)   │                  │
  │   └────────┬────────┘    └────────┬─────────┘                  │
  │            │                       │                            │
  │            └─────┬────────────────┘                             │
  │                  ▼                                              │
  │            session::DevSession                                  │
  │              • project_root: PathBuf                            │
  │              • project: ProjectConfig                           │
  │              • ir: IrModule                                     │
  │              • history: Vec<Message>                            │
  │              • current_agent: AgentId                           │
  │              • pending_reload: AtomicBool                       │
  │              • mcp_clients: HashMap<String, McpClient>          │
  │              • notify_handle: notify::RecommendedWatcher        │
  │                                                                │
  └──────────────────────────────────────────────────────────────┘
                        │
                        ▼
  ┌──────────────────────────────────────────────────────────────┐
  │  EXISTING (β.3-shipped) — driven, not modified                │
  │                                                                │
  │   tau-runtime-tokio::run_via_ir(ir, dispatcher, plan, ...)     │
  │     │                                                          │
  │     ├─ tau-runtime-core agent loop                             │
  │     ├─ McpBridge (β.3 PR-5) routes MCP tool calls              │
  │     ├─ NativeRegistry for in-tree compiled tools               │
  │     └─ host_lifecycle::open() for MCP transport dial           │
  └──────────────────────────────────────────────────────────────┘
```

All new code lives in `crates/tau-cli/src/cmd/dev/`. No new workspace crate.

### 3.2 Module layout

```
crates/tau-cli/src/cmd/dev/
├── mod.rs        # pub async fn run(args: DevArgs, output) — dispatcher
├── session.rs    # DevSession struct, load / run_turn / reload / drop
├── repl.rs       # REPL loop, command parser, rustyline integration
├── watcher.rs    # notify-based file watcher; emits ReloadHint events
├── output.rs     # turn-output formatter (turn header, tool call lines, response)
└── commands.rs   # REPL command implementations (:reload, :state, ...)
```

### 3.3 Lifecycle (the load-bearing flow)

```
tau dev myproject/                  DevSession::load(root) -> Self
                                    ├─ read tau.toml
                                    ├─ ProjectConfig::parse_str
                                    ├─ pick default agent (first in [agents.*])
                                    ├─ lower to IR (β.2 path)
                                    ├─ build NativeRegistry
                                    ├─ spawn watcher (watch tau.toml + workflows/*.toml + prompt files)
                                    └─ ready

> "what's the weather?"             session.run_turn(prompt) -> Result
                                    ├─ history.push(user_msg)
                                    ├─ build CapabilityPlan from project
                                    ├─ build dispatcher (NativeRegistry + McpBridge)
                                    ├─ call run_via_ir(ir, dispatcher, plan, history, ...)
                                    ├─ for each event:
                                    │    output::render(event)  → stdout
                                    ├─ history.push(assistant_msg + tool calls)
                                    └─ return Ok

(edit tau.toml externally)          watcher fires → session.flag_pending_reload()
                                    ├─ set pending_reload = true
                                    └─ at next prompt, print "manifest changed; type :reload"

> :reload                           session.reload() -> Result
                                    ├─ if !pending_reload: print "nothing to reload"
                                    ├─ re-read tau.toml, re-parse, re-lower IR
                                    ├─ if parse error: print error, KEEP OLD config + history, return
                                    ├─ drop all McpClient values (transport dies → server dies)
                                    ├─ rebuild NativeRegistry (in case [tools.*] changed)
                                    ├─ KEEP history
                                    ├─ pending_reload = false
                                    └─ print "reloaded; <N> messages preserved"

> "and Berlin?"                     run_turn() with NEW IR + KEPT history
                                    └─ MCP clients respawn lazily as needed

> :quit (or Ctrl-D)                 DevSession::drop()
                                    ├─ drop notify_handle (stops watcher)
                                    ├─ drop mcp_clients (kills server processes)
                                    └─ exit 0
```

### 3.4 `--watch` mode

`--watch` flips one bit: when the watcher detects a change, instead of setting `pending_reload`, the watcher directly calls `session.reload()` and re-prints the prompt. Active turns are NOT interrupted (the reload waits until the current turn completes). Mid-typing input is preserved (rustyline handles that).

If `--watch` is set AND `-p "<prompt>"` is also set: `--watch` is ignored (one-shot doesn't loop, so watching is meaningless). Print a warning at boot.

### 3.5 `-p` one-shot mode

```
tau dev myproject/ -p "hello"
│
├─ DevSession::load(root)
├─ session.run_turn("hello")
├─ on RunCompleted: exit 0
└─ on RunFailed: exit 1
```

No REPL, no watcher, no history persistence. Exits as soon as the turn completes. Used for scripted invocations + CI smoke tests.

---

## 4. File watch scope

`notify::RecommendedWatcher` registered for the following paths under `<project_root>`:

| Path pattern | Watched | Trigger reload? |
|---|---|---|
| `tau.toml` | yes | yes |
| `workflows/*.toml` | yes (glob) | yes |
| Files referenced by `[agents.<id>.prompt] system_file = "..."` | yes (resolved at boot) | yes |
| `Tau.lock` | no (derived from `tau build` / `tau resolve`) | — |
| `.tau/mcp/*.contract.json` | no (derived from `tau mcp pin`) | — |
| `.tau/sessions/*` | no (derived state) | — |
| `~/.tau/global.toml` | no (out of scope v1) | — |

If a watched file is deleted, `pending_reload` is set; the next `:reload` re-parses from disk and errors honestly (which is what the user wants — they deleted it on purpose).

If a watched path's existence changes between boots (e.g. user added a `[agents.X.prompt] system_file = "..."` mid-session), the watcher set is re-registered as part of `:reload`.

**Cross-platform:** `notify` handles inotify (Linux), FSEvents (macOS), ReadDirectoryChangesW (Windows). Polling fallback at 1s interval if native APIs fail (rare; logged as a warning).

---

## 5. Error handling

| Condition | Behavior |
|---|---|
| `tau.toml` malformed at boot | Print error with file path + line number, exit 64 (usage) |
| `tau.toml` malformed after `:reload` | Print error, KEEP previous valid config + history. Hint: "fix and retry `:reload`" |
| `tau.toml` references a `[tools.X]` that doesn't exist | Validator catches it; same as malformed |
| MCP server crashes mid-turn | Surface error to user via output, drop that McpClient (next call re-dials), continue REPL |
| `cassette:` URL points to missing file | Tool call fails with `LifecycleError::Io`; print error; REPL continues |
| `notify` fails to register watcher | Warn at boot, fall back to manual-`:reload`-only mode |
| Ctrl-C during a turn | Cancel the run via existing cancellation token (β.3 PR-5.1 deferral makes this best-effort for now); return to prompt |
| Ctrl-C at the prompt | Print hint "use :quit or Ctrl-D to exit", stay at prompt |
| Ctrl-D | `DevSession::drop()`, exit 0 |
| Stdin closed (piped input ended) | Same as Ctrl-D |

**Cancellation caveat (β.3 PR-5.1 leftover):** real cancellation propagation through `run_via_ir` is deferred. v1 Ctrl-C behavior: stop reading input + stop printing events; the underlying turn completes in background (its MCP calls finish naturally). Document this in `:help` output.

---

## 6. Testing

### 6.1 Unit tests

`crates/tau-cli/src/cmd/dev/repl.rs`:
- `parse_command("hello")` → `Command::Prompt("hello")`
- `parse_command(":reload")` → `Command::Reload`
- `parse_command(":agent fan-monitor")` → `Command::SwitchAgent("fan-monitor")`
- `parse_command(":quit")` → `Command::Quit`
- `parse_command(":not-a-command")` → `Command::UnknownColon(":not-a-command")`
- (empty line) → `Command::Empty`

### 6.2 Integration tests (under `crates/tau-cli/tests/`)

| Test | What it verifies |
|---|---|
| `cmd_dev_one_shot.rs` | `tau dev <fixture> -p "hi"` returns exit 0 + prints expected response (cassette-replay-driven) |
| `cmd_dev_watcher.rs` | After REPL boot, editing tau.toml causes `pending_reload` to flip within 500ms |
| `cmd_dev_reload.rs` | After `:reload`, the new tau.toml's changes (e.g. prompt edit) take effect on next turn; history is preserved |
| `cmd_dev_reload_keeps_history.rs` | After 2 turns, edit tau.toml, `:reload`, then run a 3rd turn that references prior context — assert history was passed in |
| `cmd_dev_malformed_reload.rs` | After 1 turn with valid tau.toml, write malformed tau.toml, `:reload` → error printed, OLD config still in effect for next turn |
| `cmd_dev_boot_time.rs` | Boot time < 1500ms for a minimal project (lenient bound in CI; tighter on local) |
| `cmd_dev_mcp_cassette.rs` | Project with `[tools.weather] mcp = "cassette:./weather.jsonl"` boots + first turn round-trips through cassette |
| `cmd_dev_quit.rs` | `:quit` exits 0; Ctrl-D exits 0 |
| `cmd_dev_switch_agent.rs` | `:agent reviewer` then next turn uses the `reviewer` agent's IR (verified via different output) |
| `cmd_dev_watch_flag.rs` | `tau dev --watch` auto-reloads without `:reload` (verified by 2 turns separated by file edit) |
| `cmd_dev_help.rs` | `:help` prints all 9 commands; `tau dev --help` prints CLI flags |

### 6.3 Manual smoke (CI-skippable)

The "simplified fan-monitor" — a stripped-down version of the β.6 canonical scenario without ContextManager (since β.4 isn't shipped). Lives at `examples/dev-smoke-fan-monitor/`:

```toml
[project]
name = "dev-smoke-fan-monitor"
version = "0.0.1"

[agents.fan-monitor]
prompt.system = "Watch the temperature; turn on the fan if above 30°C."
tool_refs = ["read_temp", "set_fan"]

[tools.read_temp]
native = "ReadTemp"

[tools.set_fan]
native = "SetFan"
```

Smoke: `tau dev examples/dev-smoke-fan-monitor/ -p "what's the temperature?"` round-trips. (Native tools `ReadTemp` + `SetFan` exist in tree from β.1 fixtures.)

---

## 7. Dependencies (new)

| Crate | Version | Why |
|---|---|---|
| `notify` | `^6` | Cross-platform file watcher |
| `rustyline` | `^14` | Stdin REPL with line editing + history |

Both are mature, widely-used (`notify` is the de facto Rust file-watch lib; `rustyline` is the standard REPL crate). Both work on Linux, macOS, Windows. License-compatible (MIT/Apache).

These add to the workspace dep tree; size impact verified at < 200KB compiled. No `tokio` extra features needed.

---

## 8. Sub-project sizing

| Phase | Tasks | Tests | Effort |
|---|---|---|---|
| 1 — `tau-cli::cmd::dev` scaffold + `DevArgs` + dispatch + smoke help test | 1 | 1 | ~2h |
| 2 — `DevSession::load` + project loader + history container | 2 | 3 | ~4h |
| 3 — REPL loop + command parser + `rustyline` integration | 2 | 5 unit | ~6h |
| 4 — File watcher (`notify`) + `pending_reload` mechanics | 1 | 2 | ~3h |
| 5 — `:reload` impl + MCP client lifecycle on reload | 2 | 4 | ~6h |
| 6 — `-p` one-shot mode + `--watch` flag | 1 | 2 | ~3h |
| 7 — Simplified-fan-monitor example + boot-time test + manual smoke | 1 | 2 | ~3h |
| 8 — ROADMAP edit (β.7 / β.7.5 split) + ADR-0040 + push + PR + auto-merge | 1 | — | ~2h |

**Total: ~11 tasks, ~20 tests, ~2 weeks.**

---

## 9. ROADMAP edit (shipped as part of this PR)

### Current ROADMAP §β.7 (lines 465–476)

```
### β.7 — Dev / release one-engine discipline

- Builds on: existing tau run / tau chat / tau serve (dev-side surface) and tau
  build / tau run --bundle (release-side surface).
- Preserves: every existing CLI verb continues to do what it does today. tau dev
  is new; nothing existing is renamed or removed.
- Adds: tau dev — a hot-reload host shell driving tau-runtime-core directly,
  with user tools as callbacks. The new zero-toolchain on-ramp.
- Supersedes: nothing.
- DoD: tau dev <project> boots in under a second; editing a tool hot-reloads;
  the same project lowers cleanly via tau build wasm.
```

### Amended

```
### β.7 — `tau dev` one-engine REPL

- Builds on: β.1 (tau-runtime-core), β.2 (workflow IR + run_via_ir), β.3 (MCP
  facilitator + McpBridge).
- Preserves: every existing CLI verb continues to do what it does today. tau dev
  is new; nothing existing is renamed or removed.
- Adds: tau dev — a hot-reload REPL driving the existing β.3 runtime path
  (tau-runtime-tokio + McpBridge + run_via_ir) with a stdin loop and a notify-
  driven file watcher. REPL semantics: explicit :reload by default, --watch
  opts into Mastra-style auto-reload, -p "<prompt>" for one-shot.
- Supersedes: nothing.
- DoD: tau dev <project> boots in under 1s; editing the manifest hot-reloads
  via :reload while preserving conversation history; the simplified-fan-monitor
  smoke runs end-to-end.

Design: docs/superpowers/specs/2026-06-10-beta-7-tau-dev-design.md
ADR: 0040 — to be authored during β.7 implementation (shipped in the same PR; records the REPL-explicit-reload over Mastra-style-auto-reload decision + the β.7/β.7.5 split rationale)

### β.7.5 — IR-to-wasm AOT compiler (split out from β.7, 2026-06-10)

- Builds on: β.2 (workflow IR), β.7 (REPL gives us a working dev/host path to
  test against).
- Preserves: tau dev unchanged. tau build wasm is the new artifact path.
- Adds: ahead-of-time lowering of the workflow IR + tau-runtime-core + linked
  native tools to a runnable wasm component (WASI 0.2). The artifact runs in
  wasmtime; γ.1 extends to Spin + browser hosts.
- DoD: tau build wasm <project> produces a wasm component that executes the
  simplified-fan-monitor scenario in wasmtime, with the same observable
  RunEvent stream as tau dev produced.
- Sized: ~4–8 weeks. Wasm component model integration is the hard part.

(This sub-project was originally folded into β.7 via the β.2 footnote 'AOT
lands in β.7'; split out 2026-06-10 because wasm AOT complexity ballooned
after β.3 PR-6 expanded the MCP surface — the in-wasm MCP-facilitator path
deserves its own ADR and conformance scope.)
```

### β.2's "AOT lands in β.7" footnote (line 76)

Amended in same commit:

```
> Implementation status (2026-06-10): The workflow IR shipped in β.2 (PRs
> #263–#271). See ADR-0037 and the design spec. v0 uses partial-interpret
> lowering; AOT (wasm component artifact) lands in β.7.5. Conformance suite
> + tau run --bundle interpreter dispatch shipped in β.2.6.1/β.2.6.2.
```

### β.6's dependency on AOT

β.6's spec language says "exercises both the interpreted dev profile and the compiled wasm artifact." With β.7.5 split out, β.6's wasm-side dependency moves from β.7 to β.7.5. β.6 can start design work in parallel with β.7.5 implementation (the conformance gate is largely test infrastructure).

### γ.1's dependency

γ.1 row ("Builds on: β.6/β.7 baseline") becomes ("Builds on: β.6/β.7/β.7.5 baseline"). γ.1's "existing tau build wasm target slot" becomes a real artifact only after β.7.5 ships.

---

## 10. Risks

| Risk | Mitigation |
|---|---|
| `notify` flakes on macOS (FSEvents has known quirks with editor atomic-save patterns) | Fall back to polling at 1s interval if a watched file disappears between events; document in `:help` |
| `rustyline` history file location (~/.tau/dev_history) collides with concurrent `tau dev` invocations | Per-session history disabled in v1; only in-process line history |
| Boot time > 1s on slow filesystems (NFS, encrypted home dirs) | Document the perf target as "typical local filesystem"; CI assertion is lenient (1500ms) |
| MCP client respawn-on-reload makes the first turn after reload slow | Acceptable; document; can optimize later with smart-diff (kill only changed/removed clients) |
| Mid-turn Ctrl-C doesn't actually cancel the underlying `run_via_ir` (β.3 PR-5.1 deferral) | Document in `:help`. β.3.1 will fix this; β.7 doesn't block on it |
| Tau-runtime-core's `RefCell<RunState>` makes `run_via_ir` non-`Send` (per memory: tau-runtime-core β.1.3.5) | Use `current_thread` Tokio flavor (matches conformance harness pattern); document |

---

## 11. Open questions (for the implementation plan, not blocking spec approval)

- Should `:state` show the IR's tool list with their current capability shapes? (Probably yes for transparency, but adds rendering code. Defer to v1.1 if it bloats the plan.)
- Should `--no-color` honor `NO_COLOR` env var per spec.no-color.org? (Yes — table stakes; mention in plan.)
- Should `tau dev` warn at boot if the project's tools include stdio-MCP (which won't survive a future `tau build wasm` once β.7.5 ships)? **Deferred to β.7.5** — β.7 doesn't ship wasm at all, so warning is premature.

---

## 12. Lineage

This spec descends from:
- **2026-05-29 philosophy pivot** — established "one engine, two modes" + "Vercel-DX feel" framing
- **ROADMAP.md §β.7** (now superseded by §9 above)
- **β.2's footnote on AOT** (now superseded by §9 above)
- **β.3 PR-5 / PR-6** — provides the `run_via_ir` + McpBridge + cassette URL path that this REPL drives
