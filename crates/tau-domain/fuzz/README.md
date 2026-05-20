# tau-domain fuzz harnesses

cargo-fuzz targets that feed arbitrary bytes into tau-domain parsers
and assert they return a typed error instead of panicking, crashing,
or running unbounded.

## One-time setup

```bash
rustup toolchain install nightly
cargo install cargo-fuzz
```

## Run a target

```bash
cd crates/tau-domain/fuzz
cargo +nightly fuzz run parse_skill_md -- -max_total_time=60
```

## Targets

| Target | Parser | Seed corpus |
|--------|--------|-------------|
| `parse_skill_md` | `tau_domain::package::skill::parse_skill_md` | 4 seeds: valid, empty, missing closer, no frontmatter |

## Why fuzz `parse_skill_md`

It's user-input-facing — anyone authoring a skill writes a SKILL.md
by hand. The parser handles a YAML frontmatter splitter + serde-yaml
decode + body extraction. A panic here crashes whatever tool is
loading the skill (CLI, runtime, tau-pkg install path).

Same triage signals + CI rationale as the other fuzz directories in
this repo. See `crates/tau-pkg/fuzz/README.md` for the full setup
narrative.
