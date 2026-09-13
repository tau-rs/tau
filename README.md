# tau

[![tier1](https://github.com/tau-rs/tau/actions/workflows/tier1.yml/badge.svg)](https://github.com/tau-rs/tau/actions/workflows/tier1.yml)

> An **agent kernel**: the minimal, stable substrate on which agent harnesses
> and pipelines are composed. Explicitly not another agent framework.

**Status:** pre-M0. The frozen ABI and the pipeline exist; the kernel does not
yet. That order is deliberate — the gates land before the code they gate, so the
first line of kernel logic arrives as a pull request that already flows through
them.

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
