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
- `-rss_limit_mb=N` — abort on memory blowup (default 2 GiB)

## Targets

| Target | Parser | Seed corpus |
|--------|--------|-------------|
| `frame_decode` | `tau_plugin_protocol::Frame::decode` | 5 seeds: empty, nil, empty array, unknown type discriminator, notification skeleton |

## Triage

- **Crash** — libFuzzer writes the input to `artifacts/<target>/crash-<sha>`. Add to seed corpus + open issue.
- **Slow input** — written to `artifacts/<target>/slow-unit-<id>`. Means a parse path is exponential.
- **OOM** — `oom-<sha>`. Usually means unbounded array decoding without a length cap.

## CI

Not in the default CI matrix today (cargo-fuzz needs nightly +
sanitizers). Wired in a follow-up nightly-only CI lane.
