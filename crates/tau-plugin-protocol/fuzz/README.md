# tau-plugin-protocol fuzz harnesses

cargo-fuzz targets that feed arbitrary bytes into the MessagePack-RPC
frame decoder and assert it returns a typed `ProtocolError` instead of
panicking, crashing, or running unbounded.

This is the primary boundary where untrusted bytes from a plugin
subprocess enter the runtime, so robustness here directly improves
plugin-isolation guarantees.

## One-time setup

```bash
rustup toolchain install nightly
cargo install cargo-fuzz
```

## Run a target

```bash
cd crates/tau-plugin-protocol/fuzz
cargo +nightly fuzz run frame_decode -- -max_total_time=60
```

Useful libFuzzer flags (after `--`):
- `-max_total_time=N` — wall-clock budget in seconds
- `-runs=N` — total iteration cap
- `-jobs=N` — parallel workers
- `-rss_limit_mb=N` — abort when total process RSS exceeds N (default 2 GiB)
- `-malloc_limit_mb=N` — abort when a *single* allocation exceeds N.
  Defaults to `-rss_limit_mb`, i.e. effectively off; set it explicitly.

## Targets

| Target | Parser | Seed corpus |
|--------|--------|-------------|
| `frame_decode` | `tau_plugin_protocol::Frame::decode` | 5 shape seeds (empty, nil, empty array, unknown type discriminator, notification skeleton), 4 huge-declared-length headers, a depth-limit nesting bomb, and the #676 CI artifact |

## Triage

- **Crash** — libFuzzer writes the input to `artifacts/<target>/crash-<sha>`. Add to seed corpus + open issue.
- **Slow input** — written to `artifacts/<target>/slow-unit-<id>`. Means a parse path is exponential.
- **Single-allocation blowup** — `-malloc_limit_mb` exceeded. This is the
  real unbounded-allocation signal.
- **RSS limit exceeded, no single large allocation** — probably *not* a
  target bug. Under AddressSanitizer, per-allocation bookkeeping
  (redzones, quarantine, the append-only stack depot) accumulates over
  the session, so a fast target grows RSS on session length alone. The
  saved `oom-<sha>` artifact is just whatever was executing when the
  limit tripped, not the cause — replaying it usually passes.

  Check before concluding "unbounded allocation":

  ```bash
  # Does the artifact blow a single allocation on its own?
  cargo +nightly fuzz run frame_decode -- -malloc_limit_mb=64 <artifact>
  ```

  Issue #676 was misdiagnosed twice this way — first as single-input
  amplification, then as a leak on the decode path. It was neither;
  `Frame::decode` retains zero bytes across millions of calls
  (`tests/decode_allocation_bound.rs` in the parent crate pins that).

## CI

Run nightly by `.github/workflows/fuzz-nightly.yml`, and as a release
gate by `.github/workflows/release.yml`. Both pin `-malloc_limit_mb`,
`-rss_limit_mb` and `ASAN_OPTIONS`; see the comments there for why the
defaults are the wrong detector.
