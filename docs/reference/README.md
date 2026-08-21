# Reference

Information-oriented documentation: precise, factual descriptions of
tau's interfaces, formats, and protocols.

Reference pages are *complete* and *neutral*. They don't teach and they
don't argue — they tell you exactly what a field, a flag, or a protocol
message contains. Reach for them when you already know what you're
looking for and want to confirm a fact without scrolling through prose.

Reference content split:

- **Authored reference** (this section) — schemas, protocols, and
  policies that need narrative framing and stable URLs.
- **Generated reference** — CLI usage (from `clap`) and config/schema
  JSON (from `schemars`) is produced by CI per QG8 and published
  alongside each release. The authored tree does not duplicate it.

## Pages

- [Glossary](glossary.md) — single-page normative definitions for
  the vocabulary used across the book (`capability`, `grant`,
  `port`, `kind`, `adapter`, `tier`, `shape`, …). Each entry
  links to where the concept is treated in detail.
- [Package manifest schema](package-manifest-schema.md) — full
  schema for the *package-side* `tau.toml`: top-level fields,
  `[plugin]`, `[sandbox]`, every `[[capabilities]]` variant and its
  payload, validation rules, and reserved param names.
- [Project manifest schema](project-manifest-schema.md) — the
  *project-side* `tau.toml` that `tau init` scaffolds: `[project]`,
  `[agents.<id>]`, prompts, capability overrides, `requires.tools`.
- [Skill manifest schema](skill-manifest-schema.md) — the
  `kind = "skill"` specifics that layer on top of the package
  manifest: the `[skill]` block, `SKILL.md` frontmatter,
  `${SKILL_DIR}` substitution, and lockfile entries.
- [Sandbox platform support](sandbox-platform-support.md) — the kernel
  features required by tau's native sandbox adapter, the distros
  tested in CI, and the known limitations of the current v0.1
  enforcement.
- [Serve mode protocol](serve-mode-protocol.md) — the JSON-RPC 2.0 +
  NDJSON wire format for tau running as a long-lived subprocess.
  Every method, parameter, error code, and stability rule. One of
  tau's two public surfaces (G6).
- [tau trace](tau-trace.md) — the execution-trace waterfall TUI:
  `<RUN_ID>` / `--last` resolution, live-attach to an in-progress run,
  and keybindings.

## Generated rustdoc

`cargo doc` output for every workspace crate is published under
`/latest/rustdoc/`. Use it for type signatures, trait method bounds,
and per-item examples; this section of the book covers schemas +
protocols that rustdoc can't encode naturally.

The rustdoc landing page redirects to `tau-runtime`'s index — the
kernel crate is the most useful entry point. Navigate to other
crates from there.

## See also

- [Architecture decisions](../decisions/README.md) — ADRs that pin
  protocol and schema decisions.
- [`CONSTITUTION.md`](../../CONSTITUTION.md) — guidelines (G*, NG*,
  QG*, PG*) referenced from many ADRs.
- [`ROADMAP.md`](../../ROADMAP.md) — what is shipping next and what is
  explicitly out of scope.
