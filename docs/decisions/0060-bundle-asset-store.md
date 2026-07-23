# ADR-0060: Bundles carry a content-addressed asset store; prompts are assets or inline

**Status:** Accepted
**Date:** 2026-07-19
**Deciders:** tau maintainers

## Context

Before `ir_format v2.5.0`, an `Agent.prompt` was a bare `String`. Lowering
stored a `system_file` agent's prompt as the **path string** and the
interpreter sent it verbatim as the system prompt. Two defects followed:

1. **Non-hermetic.** The bundle's IR pinned the *path*, not the *content*. The
   same bundle run on a different machine (or after the file changed) produced
   different behavior while hashing identically.
2. **Wrong at run time.** A `system_file` agent ran with a filesystem path as
   its system prompt instead of the file's contents.

The bundle already recorded a per-agent `system_prompt_sha256` for
reproducibility, but the prompt *bytes* never reached the IR or the
interpreter. This violates tau's build-time-enforcement stance (G: "any check
that can run at build time must") and its hermeticity guarantee.

`PromptSource` (untagged `Inline | Asset`, ADR predecessor `ir_format v2.5.0`)
made the fix *expressible*; this ADR records how the bytes are carried and
resolved.

## Decision

Bundles gain a **content-addressed asset store**: file-like resources keyed by
`"sha256:" + 64 lowercase hex`, each with a `kind` tag (`prompt` today;
`#[non_exhaustive]` for skill bodies / templates / schemas later).

- **Lowering** reads a `system_file` prompt's bytes at build time via an
  injected `prompt_file` closure (I/O stays out of the pure lowering pass),
  hashes them, emits `PromptSource::Asset("sha256:…")` into the IR, and
  collects the blob into `LowerOutput.assets` (deduped by hash). A missing or
  unreadable file is a **hard build error** (`LowerError::PromptFileUnreadable`),
  moving prompt-file existence from run time to build time. Paths never enter
  the IR.
- **The bundle** carries the blobs in a new `[[assets]]` section (bytes
  hex-encoded, mirroring `[ir_payload]`), sorted by hash and hashed into the
  bundle self-hash via the canonical TOML. A bundle with assets declares
  `schema_version = 5`; older tau rejects it rather than dropping the bytes.
  Old bundles (no section) parse unchanged (absent ⇒ empty).
- **The runtime** resolves `Asset` references at agent-run assembly by looking
  the hash up in a map the host supplies through a new
  `ToolDispatcher::assets()` accessor (mirroring `clock()`/`random()`/
  `artifact_reader()`). Native `tau run --bundle` and `tau dev` both wire it;
  the default is `None` (inline-only runs behave exactly as before).
- **Closed-world checks** at load time: every `Asset` reference resolves to a
  bundle asset (no dangling refs) and every asset is referenced (no orphans);
  each asset's bytes are re-hashed against its key (tamper-evidence the bundle
  self-hash alone does not provide).

The blob type (`AssetBlob`/`AssetKind`) lives in the no_std `tau-ir` crate so
the interpreter can consume the asset map without pulling the std lowering
stack into a wasm guest. The bundle keeps its own opaque `BundleAsset`
(`kind`/`bytes_hex`); the CLI bridges the two.

## Consequences

- `system_file` agents now run hermetically with the file **content** as their
  prompt; the bundle hash pins the content. The original bug is fixed for the
  native path end-to-end (`tau build`, `tau run --bundle`, `tau dev`).
- `ir_format v2.4.0 → v2.5.0` (the `PromptSource` type) and bundle
  `schema_version 4 → 5` (the asset store) are both additive.
- New obligations: the wasm/WIT lane (embedding the asset store into the guest
  so `PromptSource::Asset` resolves under `any-wasi-strict`) is deferred to a
  follow-up PR; until then `tau build --wasm` warns rather than silently
  shipping an unresolvable module.
- The existing per-agent `system_prompt_sha256` is retained (a bundle
  reproducibility record); it is consistent-by-construction with the asset
  hash because both read the file through the same `read_prompt_file` helper.

## Alternatives considered

- **Inline the bytes into the IR `Agent.prompt` directly.** Rejected: it bloats
  every module with duplicated prompt text, defeats cross-agent dedup, and
  gives file-like resources (skill bodies, schemas) no general home. The asset
  section is that home.
- **Pre-resolve `Asset → Inline` host-side before handing the module to the
  interpreter.** Rejected: it rewrites the module, breaking the symmetry
  between the shipped IR bytes and their hash (and the wasm guest would receive
  a different module than the one that was built and verified).
- **Put `AssetBlob` in `tau-ir-lower`.** Rejected: the no_std interpreter must
  consume the asset map, and `tau-ir-lower` depends on the std `tau-pkg` — that
  would drag std into the wasm guest. `tau-ir` is the shared no_std home.
- **A hash-keyed TOML map (`[assets."sha256:…"]`).** Rejected in favor of a
  sorted `[[assets]]` array-of-tables: it matches the existing
  `[[agents]]`/`[[packages]]` discipline and avoids TOML quoted-key handling in
  the hand-rolled canonical emitter.
