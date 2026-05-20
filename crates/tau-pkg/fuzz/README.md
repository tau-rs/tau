# tau-pkg fuzz harnesses

cargo-fuzz targets that feed arbitrary bytes into tau-pkg parsers and
assert the parser returns a typed error instead of panicking, crashing,
or running unbounded.

## One-time setup

```bash
rustup toolchain install nightly
cargo install cargo-fuzz
```

Nightly is required because cargo-fuzz uses libFuzzer + sanitizers (not
available on the stable toolchain `rust-toolchain.toml` pins).

## Run a target

From the **repo root** (the `cargo +nightly fuzz` subcommand cd's into
the `fuzz/` directory under the hood when given a `--fuzz-dir`):

```bash
cd crates/tau-pkg/fuzz
cargo +nightly fuzz run lockfile_from_toml_str -- -max_total_time=60
```

Or run indefinitely until you Ctrl-C (typical local exploration):

```bash
cargo +nightly fuzz run lockfile_from_toml_str
```

libFuzzer flags pass through after `--`. Useful ones:
- `-max_total_time=N` — wall-clock budget in seconds
- `-runs=N` — total iteration cap (alternative to time-based)
- `-jobs=N` — parallel workers
- `-rss_limit_mb=N` — abort on memory blowup (default 2 GiB)

## Targets

| Target | Parser | Seed corpus | Notes |
|--------|--------|-------------|-------|
| `lockfile_from_toml_str` | `LockFile::from_toml_str` | `corpus/lockfile_from_toml_str/` | 4 seeds: empty, v4 legacy, v6 minimal, schema_version=999 |

## Triage

- **Crash** — libFuzzer writes the input to `artifacts/<target>/crash-<sha>`. Add to corpus + open issue.
- **Slow input** — libFuzzer writes to `artifacts/<target>/slow-unit-<id>`. Means a parse path is exponential; investigate.
- **Memory** — `oom-<sha>`. Usually means recursive descent without depth limit.

## CI

Not run in the default CI matrix today (cargo-fuzz needs nightly +
sanitizers, materially different toolchain). The standalone Cargo.toml
in this directory is intentionally NOT a workspace member — adding it
would force every regular `cargo` invocation to drag in `libfuzzer-sys`
+ nightly-only features.

Follow-up to wire as a nightly-only CI job is tracked separately.

## Adding a new target

```bash
cd crates/tau-pkg/fuzz
cargo +nightly fuzz add my_new_target
# Edit fuzz_targets/my_new_target.rs
# Add seed corpus under corpus/my_new_target/
# Add a row to the Targets table above
```
