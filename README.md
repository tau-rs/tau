# tau

[![tier1](https://github.com/tau-rs/tau/actions/workflows/tier1.yml/badge.svg)](https://github.com/tau-rs/tau/actions/workflows/tier1.yml)

> An **agent kernel**: the minimal, stable substrate on which agent harnesses
> and pipelines are composed. Explicitly not another agent framework.

**Status:** M1, complete. Six of the seven syscalls run (`spawn`, `exit`,
`wait`, `cancel`, `send`, `recv`); every budget dimension is enforced and
reserved before a call; time arrives as `Tick` entries from a virtual or wall
clock, never from a read inside the reducer; `tau-drivers` speaks to Anthropic
and to OpenAI-compatible endpoints behind a per-driver ceiling, and runs code a
model wrote behind a resource fence (the sandbox driver of
[ADR-0009](docs/adr/0009-sandbox-driver.md), through the `tau-sandbox-shim`
binary a harness ships beside itself); and `libtau` supplies `infer()` and the
tool loop with no powers beyond the syscalls. Every
log still refolds to the same state hash. Next is M2: hooks, which bring
`attach`, the seventh syscall; a sandbox driver v0; and `tau replay <log>`.

## The idea

Everything an agent can do is one of seven syscalls, and nothing else is
expressible:

| | | |
|---|---|---|
| `spawn` `exit` `wait` `cancel` | the tree | authority and resources only narrow downward |
| `send` `recv` | the flow | a capability is an address *and* a permission |
| `attach` | the meta | observation as installed programs, every verdict logged |

Three tags for an architect in a hurry: **hexagonal architecture with a single
message-shaped port, an event-sourced core, capability security — enforced at
runtime, not by convention.**

Four invariants carry the design:

1. **No arrow skips the kernel.** Agents never touch drivers; parents never
   touch children directly.
2. **The kernel never parses payloads.** It routes by capability and accounts by
   driver-reported consumption. Model-agnosticism is structural, not a goal.
3. **The log is the kernel; everything else is cache.**
4. **Anything invisible to the log is a replay bug.**

## Reading order

| Document | What it settles |
|---|---|
| [`docs/HANDOFF.md`](docs/HANDOFF.md) | The whole framing — mission, syscalls, layers, pipeline. Start here. |
| [ADR-0001](docs/adr/0001-tau-rebooted.md) | Why this repository starts from scratch |
| [ADR-0002](docs/adr/0002-seven-syscalls.md) | The seven, and the irreducibility test |
| [ADR-0003](docs/adr/0003-substrate.md) | In-process body, event-sourced constitution |
| [ADR-0004](docs/adr/0004-abi-freeze.md) | What is frozen, and the three gates that hold it |
| [ADR-0005](docs/adr/0005-kernel-allocated-ids.md) | Where ids come from, and how replay confirms them |
| [ADR-0006](docs/adr/0006-model-bridge-contract.md) | The bytes a model driver and the tool loop agree on, carried by the kernel but versioned apart from the ABI |
| [ADR-0007](docs/adr/0007-thinking-blocks.md) | Provider reasoning as an opaque block the loop carries unread and only its own driver replays; bridge v2 |
| [ADR-0008](docs/adr/0008-hooks-and-attach.md) | Hooks and the `attach` syscall: five points, three verdicts, two program tiers; ABI 0 → 1 |
| [ADR-0009](docs/adr/0009-sandbox-driver.md) | The sandbox driver v0: a resource fence behind `send`, `compute_ms` accounting, and the isolation ladder above it |
| [ADR-0010](docs/adr/0010-entry-freeze.md) | The `Entry` freeze: the twelve log kinds join the frozen directory, and `ABI` 2 is the first number that names the line format |
| [ADR-0011](docs/adr/0011-snapshots.md) | Snapshots: the canonical state plus a header that binds it to one log and one fold; `State` stays out of the frozen directory, `FOLD` versions the reducer, and `tau replay --from` refuses loudly |

## Layers

```text
Agents (async fns; sealed world; only the 7 syscalls)
  └ libtau (userspace convenience: infer(), tool_loop(), retries, fan-out — no special powers)
──── syscall boundary (frozen) ────
Kernel = Router/Reducer + Log + State(tree, budgets) + Hooks
──── endpoint queues ────
Drivers (border guards: envelope ↔ world protocol; report consumption)
  model | classifier/ML | tool | sandbox | CI | store | clock | human
World (APIs, processes, people, time)
```

## Working on it

```sh
just check     # Tier 0: fmt + clippy + unit tests, seconds
just test      # everything Tier 1 runs, locally
just abi       # the frozen wire format and its invariants
just hooks     # optional pre-commit hook
```

Trunk-based: short-lived branches, conventional commits, merge queue, no direct
pushes to `main`. Changes under `kernel/src/abi/` need an ABI bump or the
`abi-change` label with a linked ADR — see [ADR-0004](docs/adr/0004-abi-freeze.md).

## Milestones

| | |
|---|---|
| **M0** | log + reducer + `spawn`/`exit`/`send`/`recv` + echo driver + one end-to-end test |
| **M1** | `wait`/`cancel`, full budgets, clock driver, real model driver, tool loop in `libtau` |
| **M2** | hooks (native + Rule DSL), sandbox driver v0, `tau replay <log>` |
| **M3** | snapshots, blob store + crypto-shredding, driver supervision, CI driver |
| **M4** | crash-resume, branching with pinned sampling |

`1.0` means the ABI freeze. Nothing else.

## Lineage

tau v1 — the prototype — is archived read-only at
[`tau-rs/tau-legacy`](https://github.com/tau-rs/tau-legacy). *v1 explored the
territory; this tau is the kernel.* The diagnosis is
[ADR-0001](docs/adr/0001-tau-rebooted.md).

## License

MIT OR Apache-2.0.
