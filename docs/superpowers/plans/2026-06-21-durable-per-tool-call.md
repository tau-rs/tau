# Durable Execution — `per_tool_call` Granularity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `CheckpointGranularity::PerToolCall` — checkpoint after each tool call within a turn and resume mid-turn, re-dispatching only the tools that hadn't completed before a crash.

**Architecture:** Extends ADR-0053 A-minimal additively. The not-yet-run tools at a mid-turn checkpoint are **not derivable from tau's message history** (tool-call messages are materialized one-at-a-time during dispatch and the LLM `tool_use.id` is not preserved — see "Design note" below), so the remaining `ToolUse`s are carried explicitly on `TurnCheckpoint.pending_tool_uses` (serde-skipped when empty → `PerTurn` stays byte-identical). On resume the agent loop seeds `total_turns = ckpt.turn - 1`, re-emits `ToolCallStarted` for the carried tools, and re-enters the **existing** dispatch loop with `pending_tool_uses` pre-seeded — skipping only the LLM-drain prologue. No dispatch logic is duplicated.

**Tech Stack:** Rust (no_std core kernel + tokio host), `async-stream`, serde, `cargo nextest`.

## Global Constraints

- **CARGO RULES (CLAUDE.md):** every cargo command is `timeout <N> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/<dir> cargo <cmd> -p <crate>`. Main agent → `target/main`; subagents → `target/agent-<role>`. Tests via `cargo nextest run`; doctests via `cargo test --doc`. Timeouts: test 300, build/check 180, clippy 240, fmt 30.
- **Branch protection:** work on a `feat/*` branch, PR to `main`, auto-merge via merge queue (`gh pr merge <#> --squash --auto`; **no** `--delete-branch` with the queue).
- **Commit identity:** `git -c user.name="LEBOCQ Titouan" -c user.email="75916953+LEBOCQTitouan@users.noreply.github.com" commit --no-verify -m "..."` (pre-commit runs the flaky suite + corrupts git identity).
- **IR byte-stability:** every change must keep a durable-absent and a `PerTurn` module byte-identical to today. The new enum variant is only emitted when used; the new checkpoint field is `#[serde(skip_serializing_if = "Vec::is_empty")]`.
- **Conventional commits**, imperative, scoped.

### Design note (why explicit state, not reconstruct-from-history)

In `crates/tau-runtime-core/src/stream.rs` the per-tool dispatch loop (`for tool_use in &pending_tool_uses`, ~line 621) pushes a `ToolCall` message **inside** the loop (line ~1408), interleaved with each `ToolResult` (line ~1590). The full set of `pending_tool_uses` is an **ephemeral in-memory vec**. Therefore at a mid-turn checkpoint:

1. Tools that have not started yet are **absent from `messages`** (their `ToolCall` message is only pushed when their iteration begins) — every `ToolCall` in history already has a matching `ToolResult`, indistinguishable from a completed turn.
2. The `ToolCall` message stores only `args`; the LLM's original `tool_use.id` is **not preserved** (provider conversion synthesizes `toolu_{m.id}` in `run.rs`).

So "re-dispatch only the remaining tools" cannot be recovered from history. The remaining `ToolUse`s are carried explicitly on the checkpoint. This is the handoff's sanctioned fallback ("explicit state if the dispatch loop can't be cleanly re-entered").

---

## Task 1: IR enum variant `PerToolCall`

**Files:**
- Modify: `crates/tau-ir/src/durable.rs:52-58` (enum) + doc comments at `:48-51` and module header `:12-16`
- Test: `crates/tau-ir/src/durable.rs` (existing `#[cfg(test)] mod tests`)

**Interfaces:**
- Produces: `tau_ir::durable::CheckpointGranularity::PerToolCall` (serde `"per_tool_call"`), consumed by Tasks 4, 5, 6, 7.

- [ ] **Step 1: Write the failing test** — add to `mod tests` in `durable.rs`:

```rust
    #[test]
    fn per_tool_call_round_trips_snake_case() {
        let d = Durability::new(
            CheckpointGranularity::PerToolCall,
            DurableStore::File,
        );
        let json = serde_json::to_string(&d).expect("serialize");
        assert!(json.contains("per_tool_call"), "got: {json}");
        let back: Durability = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(d, back);
    }
```

- [ ] **Step 2: Run it, verify it fails to compile**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-ir`
Expected: FAIL — `no variant named PerToolCall`.

- [ ] **Step 3: Add the variant** — in `crates/tau-ir/src/durable.rs`, extend the enum:

```rust
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
pub enum CheckpointGranularity {
    /// Commit a checkpoint after each completed turn.
    #[serde(rename = "per_turn")]
    PerTurn,
    /// Commit a checkpoint after each completed tool call within a turn.
    /// Narrows (does not close) the at-least-once window — exactly-once
    /// stays A-full's job. Resume re-dispatches only the tools that had
    /// not completed before the crash (ADR-0053 follow-up).
    #[serde(rename = "per_tool_call")]
    PerToolCall,
}
```

Update the module-header doc (`:12-16`) and the enum doc (`:48-51`) to note `PerToolCall` now ships (drop it from the "additive for later" list).

- [ ] **Step 4: Run the test, verify it passes**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir`
Expected: PASS (all `durable` tests, including the new one).

- [ ] **Step 5: Commit**

```bash
git -c user.name="LEBOCQ Titouan" -c user.email="75916953+LEBOCQTitouan@users.noreply.github.com" \
  commit --no-verify -am "feat(durable): add CheckpointGranularity::PerToolCall variant"
```

---

## Task 2: Bump IR format version `v2.1.0` → `v2.2.0`

**Files:**
- Modify: `crates/tau-ir/src/module.rs:32-34` (CURRENT const + comment), `:99-100` (assertions)
- Modify: `crates/tau-ir/src/canonical.rs:66` (assertion)
- Modify: `crates/tau-ir-conformance/tests/conformance.rs:839` (doc comment mentioning `v2.0.0→v2.1.0`)

**Interfaces:**
- Produces: `IrFormatVersion::CURRENT == "v2.2.0"`.

- [ ] **Step 1: Update the constant** — `module.rs:34`:

```rust
    // MINOR v2.1.0: Agent.durable additive optional field (ADR-0053).
    // MINOR v2.2.0: CheckpointGranularity::PerToolCall variant +
    // TurnCheckpoint.pending_tool_uses additive optional field (ADR-0053
    // follow-up). Byte-stable when durable absent / PerTurn.
    pub const CURRENT: &'static str = "v2.2.0";
```

- [ ] **Step 2: Update the three assertions**

`module.rs:99-100`:
```rust
        assert_eq!(IrFormatVersion::CURRENT, "v2.2.0");
        assert_eq!(IrFormatVersion::current().0, "v2.2.0");
```
Rename the test fn `ir_format_version_is_v2_1_0` → `ir_format_version_is_v2_2_0`.

`canonical.rs:66`:
```rust
        assert_eq!(m.ir_format.0, "v2.2.0");
```

- [ ] **Step 3: Update the conformance doc comment** — `conformance.rs:839`: change `v2.0.0→v2.1.0` to `v2.1.0→v2.2.0`.

- [ ] **Step 4: Run the affected tests**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git -c user.name="LEBOCQ Titouan" -c user.email="75916953+LEBOCQTitouan@users.noreply.github.com" \
  commit --no-verify -am "feat(durable): bump ir_format v2.1.0 -> v2.2.0 (PerToolCall)"
```

---

## Task 3: Accept `"per_tool_call"` in project validation

**Files:**
- Modify: `crates/tau-pkg/src/project/project.rs:1470-1492` (the `durable.checkpoint` validation block), doc at `:215-216`
- Test: `crates/tau-pkg/src/project/project.rs` (existing tests module — find the durable validation tests near the other `durable` test fns)

**Interfaces:**
- Consumes: nothing new.
- Produces: a validated `DurableEntry { checkpoint: "per_tool_call", store: "file" }`.

- [ ] **Step 1: Write the failing test** — add beside the existing durable-validation tests:

```rust
    #[test]
    fn durable_accepts_per_tool_call() {
        let toml = r#"
            packages = []
            [project]
            name = "p"
            [models.m]
            backend = "mock"
            model = "m"
            [agents.a]
            package = "x@^0.1"
            model = "m"
            [agents.a.durable]
            checkpoint = "per_tool_call"
            store = "file"
        "#;
        let cfg = load_project_from_str(toml).expect("valid per_tool_call durable");
        let agent = cfg.agents.get("a").unwrap();
        let durable = agent.durable.as_ref().expect("durable present");
        assert_eq!(durable.checkpoint, "per_tool_call");
    }
```

> **Note for implementer:** match the exact test-helper used by neighboring tests (e.g. `load_project_from_str` / `parse` / `ProjectConfig::from_toml_str`). Read the closest existing `durable` test in this file and copy its harness verbatim.

- [ ] **Step 2: Run it, verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg durable_accepts_per_tool_call`
Expected: FAIL — validation rejects `"per_tool_call"` as unsupported.

- [ ] **Step 3: Widen the validation** — `project.rs:1473`:

```rust
            if d.checkpoint != "per_turn" && d.checkpoint != "per_tool_call" {
                return Err(/* keep the existing error type/shape */ ...
                    format!(
                        "durable.checkpoint {:?} unsupported (accepts \"per_turn\" or \"per_tool_call\")",
                        d.checkpoint
                    ));
            }
```

Update the doc comment at `:215-216` and `:752` to read `("per_turn" or "per_tool_call")`.

- [ ] **Step 4: Run the test, verify it passes**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg`
Expected: PASS (new test + all existing durable tests).

- [ ] **Step 5: Commit**

```bash
git -c user.name="LEBOCQ Titouan" -c user.email="75916953+LEBOCQTitouan@users.noreply.github.com" \
  commit --no-verify -am "feat(durable): accept per_tool_call in project validation"
```

---

## Task 4: Lower `"per_tool_call"` → `CheckpointGranularity::PerToolCall`

**Files:**
- Modify: `crates/tau-ir-lower/src/lower/parse.rs:436-446` (`lower_durable`)
- Test: `crates/tau-ir-lower/src/lower/parse.rs` (tests module) **or** the lower integration test that already covers `per_turn` durable — find it first.

**Interfaces:**
- Consumes: `CheckpointGranularity::PerToolCall` (Task 1), validated `"per_tool_call"` string (Task 3).
- Produces: an IR `Agent.durable = Some(Durability { checkpoint: PerToolCall, store: File })`.

- [ ] **Step 1: Write the failing test** — mirror the existing `per_turn` lowering test (locate it via `git grep -n "lower_durable\|PerTurn" crates/tau-ir-lower`):

```rust
    #[test]
    fn lower_durable_maps_per_tool_call() {
        let entry = agent_entry_with_durable("per_tool_call", "file"); // reuse the existing helper
        let d = lower_durable(&entry).expect("durable present");
        assert_eq!(d.checkpoint, tau_ir::durable::CheckpointGranularity::PerToolCall);
        assert_eq!(d.store, tau_ir::durable::DurableStore::File);
    }
```

- [ ] **Step 2: Run it, verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir-lower lower_durable_maps_per_tool_call`
Expected: FAIL — currently the wildcard collapses `"per_tool_call"` to `PerTurn`.

- [ ] **Step 3: Add the arm** — `parse.rs:440`:

```rust
    let checkpoint = match entry.durable.as_ref()?.checkpoint.as_str() {
        "per_turn" => CheckpointGranularity::PerTurn,
        "per_tool_call" => CheckpointGranularity::PerToolCall,
        _ => CheckpointGranularity::PerTurn,
    };
```

> Match the actual surrounding shape — the snippet above shows the intended arm; preserve the existing `?`/field access exactly.

- [ ] **Step 4: Run the test, verify it passes**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir-lower`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git -c user.name="LEBOCQ Titouan" -c user.email="75916953+LEBOCQTitouan@users.noreply.github.com" \
  commit --no-verify -am "feat(durable): lower per_tool_call to PerToolCall granularity"
```

---

## Task 5: Carry pending tools on the checkpoint + plumb granularity into `RunOptions`

**Files:**
- Modify: `crates/tau-ports/src/orchestration.rs:266-280` (`TurnCheckpoint` field)
- Modify: `crates/tau-runtime-core/src/options.rs:164` area (add `durable_granularity`), `:208` (Debug), `:230` (Default)
- Modify: `crates/tau-runtime-core/src/interpreter/agent_loop.rs:545-551` (set the field)
- Fix construction sites: `crates/tau-runtime-core/src/stream.rs:268`, `:3186`; `crates/tau-runtime-tokio/src/checkpoint.rs:98` (the compiler will flag any others)
- Test: `crates/tau-ports/src/orchestration.rs` (serde-skip test)

**Interfaces:**
- Produces:
  - `TurnCheckpoint.pending_tool_uses: Vec<tau_ports::llm::ToolUse>` (default empty, serde-skipped when empty).
  - `RunOptions.durable_granularity: Option<tau_ir::durable::CheckpointGranularity>` (set by `prepare_agent_run`, read by Task 6/7).

- [ ] **Step 1: Write the failing byte-stability test** — in `orchestration.rs` (gated `#[cfg(all(test, feature = "serde"))]`, matching the crate's existing test gating):

```rust
    #[test]
    fn per_turn_checkpoint_omits_pending_field() {
        let c = TurnCheckpoint {
            run_id: "r".into(),
            turn: 1,
            history: alloc::vec![],
            input_tokens: 0,
            output_tokens: 0,
            pending_tool_uses: alloc::vec![],
        };
        let json = serde_json::to_string(&c).unwrap();
        assert!(
            !json.contains("pending_tool_uses"),
            "empty pending must be skipped for byte-stability; got {json}"
        );
    }
```

- [ ] **Step 2: Run it, verify it fails to compile**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-ports --features serde`
Expected: FAIL — `TurnCheckpoint` has no field `pending_tool_uses`.

- [ ] **Step 3a: Add the field** — `orchestration.rs`, after `output_tokens`:

```rust
    /// Cumulative output (completion) tokens through `turn`.
    pub output_tokens: u64,
    /// Tools the model requested in `turn` that had **not** completed when
    /// this snapshot was taken (`PerToolCall` mid-turn checkpoints only).
    /// Empty for a `PerTurn` / turn-boundary checkpoint — serde-skipped so
    /// those snapshots stay byte-identical to the A-minimal wire form.
    /// On resume the runtime re-dispatches exactly these before the next
    /// LLM call; carried explicitly because they are not derivable from
    /// `history` (see ADR-0053 follow-up).
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Vec::is_empty")
    )]
    pub pending_tool_uses: Vec<crate::llm::ToolUse>,
```

- [ ] **Step 3b: Add `RunOptions.durable_granularity`** — `options.rs`, beside `resume_from`:

```rust
    /// The declaring agent's checkpoint granularity, when durable. Gates the
    /// mid-turn (per-tool) checkpoint + resume path (ADR-0053 follow-up).
    pub durable_granularity: Option<tau_ir::durable::CheckpointGranularity>,
```
Add `durable_granularity: None` to the `Default` impl (`:230` area) and, for completeness, `.field("durable_granularity", &self.durable_granularity)` to the manual `Debug` impl (`:208` area).

- [ ] **Step 3c: Set it in `prepare_agent_run`** — `agent_loop.rs:545-551`:

```rust
    if agent.durable.is_some() {
        if let Some(handles) = dispatcher.checkpointing() {
            run_options.checkpoint_store = Some(handles.store);
            run_options.run_id = Some(handles.run_id);
            run_options.resume_from = handles.resume;
            run_options.durable_granularity =
                agent.durable.as_ref().map(|d| d.checkpoint);
        }
    }
```

- [ ] **Step 3d: Fix the three construction sites** — add `pending_tool_uses: alloc::vec![]` (or `vec![]` in std test code) to each `TurnCheckpoint { .. }` literal: `stream.rs:268` (the `persist_checkpoint_if_durable` builder — Task 6 will pass a real value here, leave `vec![]` for now), `stream.rs:3186` (test), `checkpoint.rs:98` (test helper). Build to let the compiler enumerate any site this list missed.

- [ ] **Step 4: Run tests, verify pass**

Run:
```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ports --features serde,test-fixtures
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-tokio
```
Expected: PASS (new serde-skip test + existing checkpoint round-trip tests; the file-store keep-all/isolation tests still pass because the field is absent in their JSON).

- [ ] **Step 5: Commit**

```bash
git -c user.name="LEBOCQ Titouan" -c user.email="75916953+LEBOCQTitouan@users.noreply.github.com" \
  commit --no-verify -am "feat(durable): carry pending_tool_uses on checkpoint + plumb granularity"
```

---

## Task 6: Write a mid-turn checkpoint after each tool (PerToolCall)

**Files:**
- Modify: `crates/tau-runtime-core/src/stream.rs:259-279` (`persist_checkpoint_if_durable` signature), `:621` (loop index), `:597` + `:1609` (turn-boundary call sites pass empty), new per-tool site after `:1596`
- Test: `crates/tau-runtime-core/src/stream.rs` (new in-core test mirroring `durable_agent_persists_a_checkpoint_per_turn`)

**Interfaces:**
- Consumes: `RunOptions.durable_granularity` (Task 5), `TurnCheckpoint.pending_tool_uses` (Task 5).
- Produces: a `turn-<n>.json` overwritten after each tool with `pending_tool_uses = tools not yet run`; finalized empty by the existing turn-boundary write.

- [ ] **Step 1: Change `persist_checkpoint_if_durable` to take pending** — `stream.rs:259`:

```rust
fn persist_checkpoint_if_durable(
    options: &RunOptions,
    turn: u32,
    messages: &[Message],
    tokens: &crate::options::TokenUsage,
    pending_tool_uses: Vec<tau_ports::llm::ToolUse>,
) {
    if let (Some(store), Some(run_id)) =
        (options.checkpoint_store.as_ref(), options.run_id.as_ref())
    {
        let ckpt = tau_ports::orchestration::TurnCheckpoint {
            run_id: run_id.clone(),
            turn,
            history: messages.to_vec(),
            input_tokens: tokens.input_tokens,
            output_tokens: tokens.output_tokens,
            pending_tool_uses,
        };
        if let Err(e) = store.persist(&ckpt) {
            warn!(name = "runtime.checkpoint_failed", turn, error = %e);
        }
    }
}
```

Update the two existing turn-boundary callers (`:597`, `:1609`) to pass `Vec::new()` (turn fully done → no pending).

- [ ] **Step 2: Index the dispatch loop + add the per-tool site** — `stream.rs:621`, change the loop header to enumerate:

```rust
            for (tool_idx, tool_use) in pending_tool_uses.iter().enumerate() {
```

Then immediately after the `yield RunEvent::ToolCallCompleted { .. }` at `:1592-1596` (steady-state tool result), add:

```rust
                // ADR-0053 follow-up: PerToolCall commits a mid-turn
                // checkpoint after each tool. The remaining (not-yet-run)
                // tools are carried explicitly — they are NOT in `messages`.
                // Overwrites turn-<n>.json (atomic-rename safe); the
                // turn-boundary write later finalizes it with empty pending.
                if options.durable_granularity
                    == Some(tau_ir::durable::CheckpointGranularity::PerToolCall)
                {
                    let remaining: Vec<tau_ports::llm::ToolUse> =
                        pending_tool_uses[tool_idx + 1..].to_vec();
                    persist_checkpoint_if_durable(
                        &options, total_turns, &messages, &aggregated_tokens, remaining,
                    );
                }
```

> If the inner `for` body borrows `pending_tool_uses` immutably elsewhere, the `[tool_idx + 1..].to_vec()` slice is a fresh owned `Vec` and does not conflict. Confirm the loop variable rename didn't break the agent-spawn / skill-spawn sub-branches that also reference `tool_use` (they use the binding name, unaffected).

- [ ] **Step 3: Write the test** — mirror `durable_agent_persists_a_checkpoint_per_turn`, but one turn with **two** tool uses and `durable_granularity` set:

```rust
    #[tokio::test]
    async fn per_tool_call_checkpoints_carry_remaining_tools() {
        use tau_ports::fixtures::{make_tool_spec, make_tool_use, MockCheckpointStore, MockTool};
        use tau_ports::CheckpointStore as _;

        // One turn: two tool uses (a, b), then an end-turn turn.
        let spec_a = make_tool_spec("tool-a".into(), "a".into(), Value::Null);
        let spec_b = make_tool_spec("tool-b".into(), "b".into(), Value::Null);
        let a: Arc<dyn DynTool> = Arc::new(MockTool::new("tool-a", spec_a));
        let b: Arc<dyn DynTool> = Arc::new(MockTool::new("tool-b", spec_b));
        let (mut tools, mut validators, mut specs) = make_tool_entry("tool-a", a);
        let (tb, vb, sb) = make_tool_entry("tool-b", b);
        tools.extend(tb); validators.extend(vb); specs.extend(sb);

        let llm: Arc<dyn DynLlmBackend> = Arc::new(ScriptedLlm::multi_turn(vec![
            vec![
                Ok(CompletionChunk::ToolUse(make_tool_use("a".into(), "tool-a".into(), Value::Null))),
                Ok(CompletionChunk::ToolUse(make_tool_use("b".into(), "tool-b".into(), Value::Null))),
                Ok(CompletionChunk::Finish { stop_reason: PortsStopReason::ToolUse, usage: Some(PortsTokenUsage::new(10, 5)) }),
            ],
            vec![
                Ok(CompletionChunk::Text { delta: "done".into() }),
                Ok(CompletionChunk::Finish { stop_reason: PortsStopReason::EndTurn, usage: Some(PortsTokenUsage::new(3, 2)) }),
            ],
        ]));

        let store = Arc::new(MockCheckpointStore::new());
        let opts = RunOptions {
            checkpoint_store: Some(store.clone()),
            run_id: Some("run-ptc".into()),
            durable_granularity: Some(tau_ir::durable::CheckpointGranularity::PerToolCall),
            ..test_run_options()
        };

        let stream = run_streaming_inner(
            llm, agent_def(), manifest_with_no_capabilities(),
            vec![], user_msg("hi"), opts, tools, validators, vec![], specs, vec![], vec![],
        );
        let events = strip_gate_events(collect_events(Box::pin(stream)).await);
        assert!(matches!(events.last(), Some(RunEvent::RunCompleted { .. })), "got {events:#?}");

        // After tool-a's mid-turn checkpoint, turn-1 carried [tool-b].
        // The turn-1 boundary write then finalized it with empty pending,
        // so load_latest (turn 2) carries no pending.
        let latest = store.load_latest(&String::from("run-ptc")).unwrap().unwrap();
        assert!(latest.pending_tool_uses.is_empty(), "final checkpoint must be clean");
    }
```

> **If `MockCheckpointStore` only keeps the latest-per-turn**, also assert the intermediate via a store that records every `persist` — check `tau_ports::fixtures::MockCheckpointStore`'s API (`persisted_turns`) and, if it exposes the full history, assert one persisted snapshot for turn 1 had `pending_tool_uses == [tool-b]`. If it does not, the resume test in Task 7 is the authoritative DoD for the carried-pending behavior; keep this test focused on "no double-count + clean finalize".

- [ ] **Step 4: Run tests, verify pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core`
Expected: PASS — new test + both existing durable tests (`durable_agent_persists_a_checkpoint_per_turn`, `durable_resume_re_enters_at_next_turn_without_rebilling`) unchanged.

- [ ] **Step 5: Commit**

```bash
git -c user.name="LEBOCQ Titouan" -c user.email="75916953+LEBOCQTitouan@users.noreply.github.com" \
  commit --no-verify -am "feat(durable): write per-tool-call mid-turn checkpoints"
```

---

## Task 7: Resume mid-turn — re-dispatch only the pending tools (the DoD)

**Files:**
- Modify: `crates/tau-runtime-core/src/stream.rs:301-334` (resume seeding: carry pending + adjust `total_turns`), `:445-579` (wrap the LLM-drain prologue in an `else` of a `mid_turn_resume` guard; hoist the four accumulators above it)
- Test: `crates/tau-runtime-core/src/stream.rs` (new DoD test mirroring `durable_resume_re_enters_at_next_turn_without_rebilling`)

**Interfaces:**
- Consumes: `TurnCheckpoint.pending_tool_uses`, `RunOptions.resume_from`.
- Produces: a resumed run that re-dispatches exactly the carried tools (no LLM call, no turn double-count) then continues normally.

- [ ] **Step 1: Write the failing DoD test**:

```rust
    /// DoD: a turn issued two tools; the process crashed after tool-a's
    /// mid-turn checkpoint. Resume re-dispatches ONLY tool-b (tool-a is not
    /// re-invoked) and completes — with a single subsequent LLM call.
    #[tokio::test]
    async fn per_tool_call_resume_redispatches_only_pending_tool() {
        use tau_ports::fixtures::{make_tool_spec, make_tool_use, MockCheckpointStore, MockTool};
        use tau_ports::CheckpointStore as _;

        // tool-a already completed (must NOT run again); tool-b is pending.
        let spec_a = make_tool_spec("tool-a".into(), "a".into(), Value::Null);
        let spec_b = make_tool_spec("tool-b".into(), "b".into(), Value::Null);
        let a = Arc::new(MockTool::new("tool-a", spec_a));
        let b = Arc::new(MockTool::new("tool-b", spec_b));
        let a_dyn: Arc<dyn DynTool> = a.clone();
        let b_dyn: Arc<dyn DynTool> = b.clone();
        let (mut tools, mut validators, mut specs) = make_tool_entry("tool-a", a_dyn);
        let (tb, vb, sb) = make_tool_entry("tool-b", b_dyn);
        tools.extend(tb); validators.extend(vb); specs.extend(sb);

        // Mid-turn checkpoint: turn 1 in progress, tool-a done, tool-b pending.
        let checkpoint = tau_ports::TurnCheckpoint {
            run_id: "run-mid".into(),
            turn: 1,
            history: alloc::vec![
                user_msg("do both"),
                // tool-a's ToolCall + ToolResult already in history:
                agent_tool_call_msg("tool-a", Value::Null),
                tool_result_msg("tool-a", Value::Null),
            ],
            input_tokens: 10,
            output_tokens: 5,
            pending_tool_uses: alloc::vec![
                make_tool_use("b".into(), "tool-b".into(), Value::Null),
            ],
        };

        // EXACTLY ONE turn available — the post-completion turn 2.
        let llm: Arc<dyn DynLlmBackend> = Arc::new(ScriptedLlm::new(vec![
            Ok(CompletionChunk::Text { delta: "final".into() }),
            Ok(CompletionChunk::Finish { stop_reason: PortsStopReason::EndTurn, usage: Some(PortsTokenUsage::new(4, 2)) }),
        ]));

        let store = Arc::new(MockCheckpointStore::new());
        let opts = RunOptions {
            checkpoint_store: Some(store.clone()),
            run_id: Some("run-mid".into()),
            resume_from: Some(checkpoint),
            durable_granularity: Some(tau_ir::durable::CheckpointGranularity::PerToolCall),
            ..test_run_options()
        };

        let stream = run_streaming_inner(
            llm, agent_def(), manifest_with_no_capabilities(),
            vec![], user_msg("ignored-on-resume"), opts, tools, validators, vec![], specs, vec![], vec![],
        );
        let events = strip_gate_events(collect_events(Box::pin(stream)).await);

        // tool-b dispatched exactly once; tool-a never re-invoked.
        assert_eq!(b.invocation_count(), 1, "tool-b must run once on resume");
        assert_eq!(a.invocation_count(), 0, "tool-a must NOT be re-invoked");

        // ToolCallStarted(tool-b) precedes ToolCallCompleted(tool-b) (invariant).
        let started = events.iter().position(|e| matches!(e, RunEvent::ToolCallStarted { name, .. } if name == "tool-b"));
        let completed = events.iter().position(|e| matches!(e, RunEvent::ToolCallCompleted { name, .. } if name == "tool-b"));
        assert!(started.is_some() && completed.is_some() && started < completed);

        // The partial turn finished as turn 1 (no double-count), then turn 2 ran.
        let turns: Vec<u32> = events.iter().filter_map(|e| match e {
            RunEvent::TurnCompleted { turn, .. } => Some(*turn), _ => None,
        }).collect();
        assert_eq!(turns, vec![1, 2], "finish turn 1 mid-turn, then turn 2; got {turns:?}");
        assert!(matches!(events.last(), Some(RunEvent::RunCompleted { .. })));
    }
```

> **Helpers:** `make_tool_use` exists in `tau_ports::fixtures`. `MockTool` must expose an invocation counter — check its API; if it lacks one, use a counting wrapper already present in the test module, or assert "tool-a not re-invoked" via the absence of a second `ToolCallCompleted{name:"tool-a"}` in `events` instead. `agent_tool_call_msg` / `tool_result_msg` are small local builders — add them next to `user_msg` in the test module (sender `Address::Agent(..)`→`Address::Tool("tool-a")` with `MessagePayload::ToolCall`, and `Address::Tool("tool-a")`→agent with `MessagePayload::ToolResult`).

- [ ] **Step 2: Run it, verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core per_tool_call_resume`
Expected: FAIL — today resume seeds `total_turns = ckpt.turn` and makes a fresh LLM call, so tool-b is never dispatched (and the single-turn script may be exhausted / wrong turn numbers).

- [ ] **Step 3a: Adjust resume seeding** — `stream.rs:301-334`. Capture pending and seed `total_turns = ckpt.turn - 1` when mid-turn:

```rust
        // Carry any mid-turn pending tools (PerToolCall resume).
        let mut resume_pending: Vec<tau_ports::llm::ToolUse> = options
            .resume_from
            .as_ref()
            .map(|c| c.pending_tool_uses.clone())
            .unwrap_or_default();

        let (mut messages, mut total_turns, mut aggregated_tokens) =
            if let Some(ckpt) = options.resume_from.clone() {
                let tokens = crate::options::TokenUsage {
                    input_tokens: ckpt.input_tokens,
                    output_tokens: ckpt.output_tokens,
                    total_tokens: None,
                };
                // Mid-turn resume re-runs the SAME turn (finishing its pending
                // tools) before the loop advances, so seed one below `turn`;
                // the first `total_turns += 1` brings it back to `turn`. A
                // turn-boundary resume (empty pending) seeds `turn` as before.
                let seed_turn = if ckpt.pending_tool_uses.is_empty() {
                    ckpt.turn
                } else {
                    ckpt.turn.saturating_sub(1)
                };
                (ckpt.history, seed_turn, tokens)
            } else {
                /* ... existing fresh-run arm unchanged ... */
            };
```

- [ ] **Step 3b: Hoist accumulators + guard the LLM-drain prologue** — at `stream.rs:445`, move the four `let mut` accumulators to just **above** the request-building (before `:373`), then wrap the prologue (`CompletionRequest` build through token accumulation, ~`:373-579`) in `else`:

```rust
            let mut accumulated_text = String::new();
            let mut turn_stop_reason: Option<StopReason> = None;
            let mut turn_usage: Option<TokenUsage> = None;
            let mut pending_tool_uses: Vec<tau_ports::ToolUse> = Vec::new();

            // PerToolCall mid-turn resume: the first turn after a mid-turn
            // checkpoint finishes the carried tools WITHOUT an LLM call. Only
            // the first iteration sees a non-empty `resume_pending`; we take
            // it so subsequent turns fall through to the normal drain.
            let mid_turn_resume = !resume_pending.is_empty();
            if mid_turn_resume {
                pending_tool_uses = core::mem::take(&mut resume_pending);
                // Re-emit ToolCallStarted so the "Started precedes Completed"
                // invariant holds on the resumed stream (the assistant
                // tool-call message is already in `messages` from the
                // pre-crash run, so it is NOT re-pushed here).
                for tu in &pending_tool_uses {
                    yield RunEvent::ToolCallStarted {
                        id: tu.id.clone(),
                        name: tu.name.clone(),
                        args: tu.input.clone(),
                    };
                }
            } else {
                // ... existing prologue: build request, context pipeline,
                // InferenceCallStarted, drain LLM stream (populating the four
                // accumulators), InferenceCallCompleted, token-usage events,
                // push assistant Text msg, accumulate tokens ...
            }
```

The empty-check at `:582`, the dispatch loop (`:621`, now with Task 6's per-tool checkpoint), `TurnCompleted`, and the turn-boundary checkpoint all run **unchanged** for both branches — that is the whole point: zero dispatch-logic duplication.

> **Borrow/lifetime watch:** `core::mem::take(&mut resume_pending)` requires `resume_pending` declared `mut` (Step 3a) and not borrowed across the loop. The drain `else`-branch still declares no shadowing `pending_tool_uses` — delete the old inner `let mut pending_tool_uses` at `:448` (now hoisted). Same for the other three accumulators at `:445-447`.

- [ ] **Step 4: Run tests, verify pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core`
Expected: PASS — the new DoD test, Task 6's test, and BOTH original durable tests (turn-boundary resume still takes the `else` branch because its `pending_tool_uses` is empty).

- [ ] **Step 5: Commit**

```bash
git -c user.name="LEBOCQ Titouan" -c user.email="75916953+LEBOCQTitouan@users.noreply.github.com" \
  commit --no-verify -am "feat(durable): resume mid-turn, re-dispatch only pending tools"
```

---

## Task 8: Conformance fixture `17_durable_per_tool_call` + checkpoint.rs doc note

**Files:**
- Create: `crates/tau-ir-conformance/fixtures/17_durable_per_tool_call/{workflow.toml,mock_llm.jsonl,expected_report.json}`
- Modify: `crates/tau-ir-conformance/tests/conformance.rs` (two test fns mirroring fixture 16's pair)
- Modify: `crates/tau-runtime-tokio/src/checkpoint.rs:1-8` (module doc: note PerToolCall overwrites `turn-<n>.json` within a turn, last-write-wins)

**Interfaces:**
- Consumes: everything above. A conformance run drives the interpreter with **no** `CheckpointStore`, so durable is observationally inert — the fixture must cross-mode-conform identically to its durable-less twin.

- [ ] **Step 1: Create `workflow.toml`** — a single turn issuing **two** native tool calls, with a `per_tool_call` durable block:

```toml
packages = ["mock-llm"]

[project]
name = "fixture-17"

[models.mock-1]
backend = "mock-llm"
model = "mock-1"

[agents.fan]
display_name = "Fan Controller"
package      = "fan-ctrl@^0.1"
model        = "mock-1"
tool_refs    = ["read_temp", "read_humidity"]
max_turns    = 2

# ADR-0053 follow-up: per-tool-call durable. Additive IR metadata; with no
# CheckpointStore in the conformance harness it does not change the observable
# side-effect multiset, so this fixture cross-mode-conforms like fixture 16.
[agents.fan.durable]
checkpoint = "per_tool_call"
store = "file"

[tools.read_temp]
native      = "ReadTemp"
description = "Read the current temperature."
capabilities = []

[tools.read_humidity]
native      = "ReadHumidity"
description = "Read the current humidity."
capabilities = []
```

> Confirm `ReadHumidity` (or whatever second native exists) is a registered native tool in the conformance harness's native registry. If only one native is available, reuse `read_temp` twice with distinct ids in `mock_llm.jsonl` and a single `[tools.read_temp]` entry (the args/multiset still differ per call only if inputs differ — prefer a genuinely second native; check `git grep -n "ReadTemp" crates/tau-ir-conformance`).

- [ ] **Step 2: Create `mock_llm.jsonl`** — turn 0 issues both tools, turn 1 ends:

```jsonl
{"turn": 0, "response": {"tool_uses": [{"id": "1", "name": "read_temp", "input": {}}, {"id": "2", "name": "read_humidity", "input": {}}], "stop_reason": "tool_use"}}
{"turn": 1, "response": {"text": "ok", "stop_reason": "end_turn"}}
```

- [ ] **Step 3: Create `expected_report.json`** — two tool calls, message count = user + 2×(toolcall+toolresult) + final assistant text = 6:

```json
{
  "run_outcome_kind": "Completed",
  "tool_calls": { "read_temp:{}": 1, "read_humidity:{}": 1 },
  "message_added_count": 6
}
```

> Verify `message_added_count` against fixture 02 / the actual harness counting (run the dev-mode test once and read the produced count if it mismatches; adjust to the observed value — the exact number is harness-defined, not guessed).

- [ ] **Step 4: Add the two conformance tests** — in `conformance.rs`, after the fixture-16 block:

```rust
#[tokio::test(flavor = "current_thread")]
async fn fixture_17_dev_mode_completed_with_per_tool_call() {
    let dir = fixture_dir("17_durable_per_tool_call");
    let report = DevMode.run(&dir).await;
    assert!(report.build_refused.is_none(), "got build_refused: {:?}", report.build_refused);
    assert!(matches!(report.run_outcome, Some(RunOutcome::Completed { .. })), "got: {:?}", report.run_outcome);
    assert_eq!(count_tool_calls(&report, "read_temp"), 1);
    assert_eq!(count_tool_calls(&report, "read_humidity"), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn fixture_17_cross_mode_conformance() {
    let dir = fixture_dir("17_durable_per_tool_call");
    let dev = DevMode.run(&dir).await;
    let bundle = BundleMode.run(&dir).await;
    assert_conform(&dev, &bundle);
}
```

- [ ] **Step 5: Document the file-store semantics** — `checkpoint.rs:1-8`, append a sentence:

```
//! Under `PerToolCall`, multiple checkpoints are written within one turn —
//! they share `turn-<n>.json` (last-write-wins; the atomic `.tmp`-rename
//! makes overwrite safe), and the turn-boundary write finalizes it with an
//! empty `pending_tool_uses`. `load_latest` still returns the highest-`n`
//! file, whose `pending_tool_uses` directs mid-turn resume.
```

- [ ] **Step 6: Run the conformance suite + doctests**

Run:
```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir-conformance
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test --doc -p tau-ir -p tau-ports
```
Expected: PASS (fixtures 16 and 17, cross-mode conformance, all prior fixtures).

- [ ] **Step 7: Commit**

```bash
git -c user.name="LEBOCQ Titouan" -c user.email="75916953+LEBOCQTitouan@users.noreply.github.com" \
  commit --no-verify -am "test(durable): conformance fixture 17 per_tool_call + file-store doc"
```

---

## Final verification (before PR)

- [ ] Workspace-wide affected crates green:
```
for c in tau-ir tau-ir-lower tau-pkg tau-ports tau-runtime-core tau-runtime-tokio tau-ir-conformance; do
  timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p $c || break
done
```
- [ ] Clippy clean on the two heaviest crates:
```
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-runtime-core -p tau-ir -- -D warnings
```
- [ ] `cargo fmt --check` (30s) on each touched crate.
- [ ] Confirm a durable-absent module and a `PerTurn` checkpoint serialize byte-identically to pre-change (the Task 5 serde-skip test + fixture 16 staying green cover this).
- [ ] Open PR: `gh pr create --base main`, then `gh pr merge <#> --squash --auto`.

## DoD checklist (from the handoff)

- [x] PerToolCall fixture/test: turn issues ≥2 tools; crash after tool 1; resume re-dispatches ONLY tool 2 → **Task 7 DoD test**.
- [x] Existing PerTurn behavior unchanged → both original durable tests stay green; fixture 16 unchanged.
- [x] New conformance fixture `17_durable_per_tool_call` cross-mode conforms, inert without a store → **Task 8**.
- [x] IR round-trip byte-stable when durable absent; `v2.2.0` asserted → **Tasks 1, 2, 5**.
- [x] FileCheckpointStore: `turn-<n>.json` last-write-wins within a turn, documented → **Task 8 Step 5**.

## Self-review notes

- **Spec coverage:** handoff's 5 extension points → Tasks 1 (durable.rs), 2 (module/canonical), 3 (project.rs), 4 (parse.rs), 5–7 (stream.rs + plumbing). Hard part split into checkpoint (6) and resume (7). DoD fixture → 8. All covered.
- **Type consistency:** `pending_tool_uses: Vec<tau_ports::llm::ToolUse>` used identically in `TurnCheckpoint` (Task 5), the persist builder (Task 6), and resume seeding (Task 7). `durable_granularity: Option<CheckpointGranularity>` set in Task 5, read in Tasks 6 & 7. `seed_turn`/`mid_turn_resume` local to Task 7.
- **Open risk resolved:** the "re-enter the 976-line dispatch block" concern is handled by seed-and-skip (Task 7 Step 3b) — the dispatch loop is shared, only the LLM-drain prologue is guarded. No duplication; if the prologue-wrap proves to entangle `turn_span`/borrow lifetimes, fall back to a pre-loop block, but the hoist-and-guard approach is expected to be clean.
