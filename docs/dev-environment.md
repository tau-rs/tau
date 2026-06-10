# tau dev environment — Linux pre-commit hook + opt-in deep gate

Goal: catch cross-platform build/runtime issues locally before pushing, so CI is confirmation rather than discovery. Also unblocks local debugging of F task 6.5 follow-ups (strict_net_filter integration test hang, Container-adapter network filtering).

This iteration covers **Linux only** (running on Apple Silicon Mac). Windows + macOS legs are deferred to follow-up PRs.

## TL;DR — one-time setup

```bash
# 1. Use rustup, not Homebrew rust (see "Toolchain setup" below if you have Homebrew rust installed).
#    rustup honors rust-toolchain.toml and auto-provisions cross-targets.

# 2. Install tools (FOSS only)
brew install lefthook podman
brew install filosottile/musl-cross/musl-cross   # for cross-compile-check (x86_64 included by default)

# 3. Initialize Podman's hidden Linux VM (--rootful required for caps)
podman machine init --cpus 4 --memory 8192 --rootful
podman machine start

# 4. Wire the pre-commit git hook
lefthook install

# 5. Verify
lefthook run pre-commit --all-files     # ~30-60s, must exit 0
lefthook run deep-gate   --all-files     # ~3-5min cold, opt-in pre-flight
```

After this, every `git commit` runs the fast checks. The deep Linux gate is **opt-in** — it is not a push hook; run `lefthook run deep-gate` on demand (e.g. before a release tag) when you want a full local pre-flight. CI runs the same Linux jobs on every PR, so CI is the gate.

## Toolchain setup — rustup, not Homebrew rust

This project requires the rustup-managed Rust toolchain. `rust-toolchain.toml` declares the channel, components, and cross-compile targets that rustup auto-provisions.

**If you have Homebrew rust installed, remove it before installing rustup:**

```bash
which cargo                              # should be ~/.cargo/bin/cargo
                                         # if it shows /opt/homebrew/bin/cargo, fix below
brew list rust 2>/dev/null && brew uninstall rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
# Restart your shell, or `source ~/.cargo/env`.
which cargo                              # now ~/.cargo/bin/cargo
cargo --version                          # should NOT say "(Homebrew)"
```

If you can't or won't uninstall Homebrew rust, ensure `~/.cargo/bin` precedes `/opt/homebrew/bin` in `$PATH`:

```bash
echo 'export PATH="$HOME/.cargo/bin:$PATH"' >> ~/.zshrc
source ~/.zshrc
which cargo                              # confirm rustup's cargo wins
```

The `check-linux-x86` pre-commit step requires the musl stdlib for the `x86_64-unknown-linux-musl` target. Homebrew rust doesn't ship this stdlib; rustup auto-installs it from `rust-toolchain.toml`'s `targets` list. Skipping this prerequisite means `check-linux-x86` will fail with "can't find crate for `core`".

## Architecture

```
┌──────────────────────────────────────────────────────────────────┐
│ Apple Silicon Mac (host)                                         │
│                                                                  │
│  git commit ──── lefthook pre-commit ─── ~30-60s (host, no VM)   │
│                  • cargo fmt --all -- --check                    │
│                  • cargo clippy --workspace --all-targets        │
│                  • cargo nextest run --workspace --all-targets   │
│                  • cargo check --target x86_64-unknown-linux-musl│
│                                                                  │
│  lefthook run deep-gate (opt-in) ──────  ~3-5min (Linux VM)     │
│                                                                  │
│  ┌────────────────────────────────────────────────────────────┐  │
│  │ Podman machine (Linux VM, persistent)                      │  │
│  │                                                            │  │
│  │  ephemeral container per deep-gate run:                    │  │
│  │  podman run --rm                                           │  │
│  │    --cap-add SYS_ADMIN --cap-add NET_ADMIN                 │  │
│  │    --security-opt seccomp=unconfined                       │  │
│  │    --security-opt apparmor=unconfined                      │  │
│  │    -v $WORKSPACE:/workspace                                │  │
│  │    -v cargo-cache:/usr/local/cargo/registry                │  │
│  │    -v target-cache:/workspace/target/lefthook-podman       │  │
│  │    rust:1.82-bookworm                                      │  │
│  │    bash -c 'cargo nextest run --workspace --all-targets'   │  │
│  └────────────────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────────────────┘
```

## Interactive Linux debugging

When you need to investigate a test failure or reproduce a bug interactively (e.g., the strict_net_filter integration test hang from F task 6.5 follow-ups), use the same image and caps as the gate, just with `bash` instead of `cargo nextest`:

```bash
podman run -it --rm \
  --cap-add SYS_ADMIN --cap-add NET_ADMIN \
  --security-opt seccomp=unconfined \
  --security-opt apparmor=unconfined \
  -v "$PWD":/workspace:Z \
  -v cargo-cache:/usr/local/cargo/registry \
  -v target-cache:/workspace/target/lefthook-podman \
  -w /workspace \
  docker.io/library/rust:1.82-bookworm \
  bash
```

Inside the container, `apt-get install -y iproute2 nftables` then run cargo commands directly. Same kernel, same caps, same crate cache as the automated gate — what fails interactively will fail in the gate, and vice versa.

## Coverage

```bash
cargo install cargo-llvm-cov --locked
rustup component add llvm-tools-preview
cargo llvm-cov nextest --workspace --no-fail-fast --html
```

Then open `target/llvm-cov/html/index.html` (or your `CARGO_TARGET_DIR`-relative equivalent).
CI runs the same invocation on every PR and posts the percent + the `lcov.info`
artifact to the workflow summary.

Coverage is a signal, not a gate — do not write tests to hit a number.

## Bypassing the pre-commit hook (emergencies only)

```bash
git commit --no-verify        # skip pre-commit
```

`git push` runs no hook (the deep gate is opt-in), so nothing to bypass there.
Don't skip pre-commit routinely — it exists so CI doesn't have to find the bugs.

## Architecture mismatch (known gaps)

Apple Silicon is arm64; CI Linux runners are x86_64. The local gate will catch:

- ✅ All cfg-gating bugs (`cfg(unix)`, `cfg(target_os)`)
- ✅ All build-time API mismatches (the `std::os::fd` class)
- ✅ Most Linux runtime regressions (sandbox tests, IPC, syscall behavior on arm64 Linux)
- ✅ Privilege drift (selective-cap regressions)

The local gate will NOT catch:

- ❌ Arch-specific runtime bugs that only manifest on x86_64 (struct alignment, atomic ordering, syscall numbers in raw asm — landlock syscall numbers differ between x86_64 and arm64 Linux)
- ❌ glibc-version differences (Podman ships current Debian; CI ships Ubuntu)
- ❌ Windows-specific issues (deferred to follow-up PR)
- ❌ macOS-specific issues that require a fresh macOS VM (deferred indefinitely; Mac coverage stays via the host)

These remaining gaps are caught by CI. The deep gate covers ~95% of the cross-platform pain points; CI handles the rest.

## Troubleshooting

**`lefthook: command not found`**: open a new shell after `brew install lefthook` to pick up `$PATH` updates.

**`check-linux-x86` fails with "can't find crate for `core`"**: Homebrew's `cargo` is shadowing rustup's shim on `$PATH`, and Homebrew rust doesn't ship the musl stdlib. Fix: see "Toolchain setup" above. Verify with `which cargo` — it should show `~/.cargo/bin/cargo`, not `/opt/homebrew/bin/cargo`.

**`podman machine` is non-rootful**: privileged-style containers won't work. Recreate:
```bash
podman machine stop
podman machine rm
podman machine init --cpus 4 --memory 8192 --rootful
podman machine start
```

**Deep gate hangs at apt-get**: the container is a fresh Debian — `apt-get update` reaches Debian mirrors. If you're behind a corporate proxy, configure Podman to use it (`podman machine ssh` then add proxy env vars to `/etc/profile.d/`).

**Deep gate fails with permission denied on a syscall**: a test is using a cap outside the documented list. Either expand the cap list in `lefthook.yml` (with a justifying comment) or fix the test to not need that cap. Never silently widen to `--privileged` — that defeats the gate.

**Pre-commit hook didn't run**: check `.git/hooks/pre-commit` exists. If not, re-run `lefthook install`.

**Cargo target dir collision**: lefthook uses `target/lefthook/{fmt,clippy,test,check-linux}` and `target/lefthook-podman`. These don't collide with `target/main` (main agent) or `target/agent-*` (sub-agents) per [CLAUDE.md Rule 1](../CLAUDE.md). If you see lock contention, ensure your bare cargo invocations are using the right `CARGO_TARGET_DIR`.

**Container-VM disk space fills up**: `podman machine` defaults are conservative. If you hit "no space left on device", increase the machine disk size:
```bash
podman machine stop
podman machine set --disk-size 100
podman machine start
```

**QEMU `rustc` SIGSEGV during compilation**: the Podman container is running aarch64 Linux natively (not x86_64 emulation), so this shouldn't happen. If you've added `--platform linux/amd64` somewhere and hit rustc crashes, remove it — Apple Silicon QEMU x86_64 emulation is unstable for rustc workloads.

## What this enables

After this PR ships:
1. F task 6.5 follow-up #2 (strict_net_filter integration test hang) becomes debuggable locally — `podman run -it ...` reproduces the privileged-Linux environment for interactive `gdb`/`strace` work.
2. F task 6.5 follow-up #1 (Container-adapter network filtering) becomes debuggable locally via the same mechanism.
3. The cfg(unix) / cfg(target_os) class of bug is caught at commit time, not CI time.
4. Privilege drift is caught at commit/push time, not silently masked by CI's previous `--privileged`.

## Out of scope (follow-up PRs)

- **Windows VM (UTM + Windows 11 ARM)** — largest setup; tracked as PR2 candidate.
- **macOS VM (Tart)** — Tart is Fair Source, not OSI FOSS; not pursued.
- **x86_64 Linux runtime via QEMU** — too slow; CI provides x86_64 ground truth.
- **Pre-commit fix mode** — the gate is verify-only (`cargo fmt -- --check`), not auto-rewrite (`cargo fmt`).
