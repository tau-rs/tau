# ADR-0001: tau, rebooted — an agent kernel, not an agent framework

**Status:** Accepted
**Date:** 2026-09-13
**Deciders:** tau core

## Context

This is the first commit of the second tau. The first one — 652 commits,
~207k lines of Rust across 36 crates, 70 ADRs, 47 lettered guidelines, five
months — is archived read-only at
[`tau-rs/tau-legacy`](https://github.com/tau-rs/tau-legacy) and is referred to
throughout this repository as **tau v1**.

v1 was not a code-quality failure. Its subsystems were good, its tests were
real, its CI was better than most. It failed at one thing, upstream of all of
that: **it never had a minimal substrate that everything else was expressible in
terms of.** Every subsystem — the workflow IR, the bundle format, the wasm guest
ABI, the MCP family, the sandbox adapters, the SDK codegen — grew its own
interface at its own pace. Nothing was a program *over* anything else; each was
a peer that had to be kept consistent with every other peer by hand.

The 47 guidelines were the cost of that shape, paid monthly. They did not fix
it. Rules are what a project writes when its structure cannot carry the
invariant itself, and the tell is that the rules grew faster than the core: a
constitution, a cheat sheet, an escape-hatch registry, and a build-time
governance gate, all in place before a single interface had stopped moving.

The consequence that hurt most was not slow builds. It was that **features could
be present, reviewed, merged, and inert at the same time**, with nothing in the
shape of the system objecting. A wasm/native parity gate that never once
executed because the profile it compared against was a stub. Sandbox and
container CI lanes dark for months. A fuzz nightly that had been dead since
spring. Every scheduled gate in the repository silently throttled to fire twice
a day regardless of its cron expression. Each was found by accident, one at a
time.

The mission was never the problem. Install packages, run agents, pass messages,
observe what happens — that is still exactly right.

## Decision

**tau keeps its name and abandons its implementation.** This repository is
built from scratch as an **agent kernel**: the minimal, stable substrate on
which agent harnesses and pipelines are composed. Explicitly not another agent
framework.

The design method is borrowed from Linux, point for point:

| Linux | tau |
|---|---|
| A tiny set of syscalls that never break | [Seven syscalls](0002-seven-syscalls.md), frozen |
| Everything is a file | Everything is a message — one envelope, all traffic |
| namespaces + cgroups | Capability namespaces + budgets, as kernel primitives |
| eBPF: observation as installed programs | Hooks: `attach`, three verdicts, every one logged |
| "We do not break userspace" | [The ABI freeze](0004-abi-freeze.md), gated three ways |

Three tags, for an architect reading this in a hurry: **hexagonal architecture
with a single message-shaped port, an event-sourced core, capability security —
enforced at runtime, not by convention.**

The full framing is [`docs/HANDOFF.md`](../HANDOFF.md), which is the seed
document of this repository and is reproduced unchanged in the archive as tau
v1's final session entry.

Two consequences of the decision are load-bearing enough to state as rules:

1. **Ship a working core first; grow by accretion around frozen interfaces.**
   v1's failure mode — a constitution and eight crates before a stable core — is
   the specific anti-pattern this repository exists to avoid.
2. **`1.0` means the ABI freeze**, and nothing else. Version numbers are not a
   marketing surface here.

## Consequences

- **Governance shrinks to one directory and a number.** `kernel/src/abi/` is the
  constitution; `ABI: u16` is its version; `CODEOWNERS`, the CI diff gate, and
  `insta` snapshots are its three gates. There is no `CONSTITUTION.md` in this
  repository and there will not be one.
- **Scope creep has a test instead of a policy.** Any proposed eighth syscall
  must be shown *inexpressible* as a program over the seven. If it is
  expressible, it is `libtau`. See [ADR-0002](0002-seven-syscalls.md).
- **Inert features become structurally harder.** Every effect is a log entry, so
  a code path that never executes produces no entries, and the determinism and
  replay jobs work on entries. This does not make dark code impossible; it makes
  it *visible in the artifact CI already reads*.
- **Nothing is carried over.** No v1 code, no v1 history, no v1 governance text.
  What survives is knowledge: the eleven resolved design decisions in
  `HANDOFF.md` §4 and the tiered pipeline in §7–8 are distilled from what v1
  learned expensively, and this repository starts with those answers.
- **Old links move.** GitHub's rename redirect from `tau-rs/tau` to
  `tau-rs/tau-legacy` died the moment this repository claimed the name. That is
  intended: traffic should find the successor. The archive carries its own
  explanation on its own front page, committed before the rename.
- **crates.io stays split for now.** Bare `tau` is squatted by an abandoned 2015
  crate with no reclamation path, so the kernel publishes as `tau-kernel` and
  ships `[[bin]] name = "tau"` — install once, type `tau` forever, the
  `ripgrep`/`rg` pattern. If the name is ever transferred, `tau` becomes a
  facade re-exporting `tau-kernel`, which is additive.

## Alternatives considered

**Incremental refactor of v1 toward a kernel.** Rejected on cost. The frozen
boundary has to be load-bearing from the first commit for its guarantee to mean
anything; retrofitting it into v1 makes all 36 crates migration targets while
they continue to ship, and during the migration the project has *two*
interfaces — the old peers and the new kernel. That is the present failure mode
with an extra layer, and strictly larger than the rewrite.

**Keep the code, delete the governance.** Rejected: the guidelines were the
symptom. Removing them without introducing a substrate takes away the only thing
holding the peers consistent and leaves the same shape with less scaffolding.

**Rename the project too.** Rejected: the name is the promise, not the code.
Linux survived rewrites of its scheduler and whole subsystems without becoming a
different project. A new name would discard five months of accumulated meaning
to signal a change that the ABI freeze will signal far more credibly.

**Fork instead of archiving v1.** Rejected: a live fork invites partial
migration and implies both trees are maintained. Read-only archival makes this
repository the only place work can happen, which is the point.
