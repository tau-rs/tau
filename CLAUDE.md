# CARGO RULES — read before running any cargo command

This workspace has 8 crates sharing one `target/.cargo-lock`. Concurrent
cargo invocations queue on this lock and waste 2–4 minutes per build.
Every cargo command MUST follow these rules. No exceptions.

## Rule 1: Always set CARGO_TARGET_DIR

NEVER run bare `cargo`. ALWAYS prefix with `CARGO_TARGET_DIR=<path>`.

| Caller | CARGO_TARGET_DIR value |
|---|---|
| Main agent (top-level Bash tool) | `target/main` |
| Any subagent spawned via Agent tool | `target/agent-<role>` where `<role>` is the subagent's purpose (e.g. `spec-review`, `solution-review`, `impl`, `adversary`) |
| One-off diagnostic from main agent (cargo --version, cargo metadata, etc.) | `target/main` |
| `lefthook` pre-commit hooks (host-side) | `target/lefthook/fmt`, `target/lefthook/clippy`, `target/lefthook/test`, `target/lefthook/check-linux` (one per command) |
| `lefthook` deep-gate (opt-in, Podman container) | `target/lefthook-podman` (mounted as a named Podman volume `target-cache` so it persists across runs) |

If you cannot determine your role, use `target/agent-misc`. Never omit the variable.

The `target/lefthook/*` and `target/lefthook-podman` paths are reserved
for the pre-commit hook and the opt-in deep gate defined in
`lefthook.yml`. Contributors install the pre-commit hook with `lefthook
install` after `brew install lefthook podman`. See
`docs/dev-environment.md` for full setup.

## Rule 2: Always scope to a single crate

Use `-p <crate>`. Never invoke cargo from the workspace root without `-p`.

✅ `CARGO_TARGET_DIR=target/main cargo test -p tau-domain`
❌ `cargo test`
❌ `cargo test --workspace`
❌ `CARGO_TARGET_DIR=target/main cargo test`  (no -p)

## Rule 3: Always wrap with timeout

| Command | Timeout |
|---|---|
| `cargo test` | 300s |
| `cargo build` / `cargo check` | 180s |
| `cargo clippy` | 240s |
| `cargo fmt --check` | 30s |

Format: `timeout 300 env CARGO_TARGET_DIR=target/main cargo test -p tau-domain`

## Rule 4: Always set CARGO_INCREMENTAL=0

Cargo's incremental compilation defaults to `1` (on) for the dev
profile. sccache cannot deduplicate incremental-compilation outputs
because they embed compilation-state metadata, so leaving incremental
on means **0% Rust cache hit rate** through sccache (verified —
3,907 hits / 2,854 misses without `CARGO_INCREMENTAL=0`, all 2 of the
hits were Rust). Disabling incremental restores normal sccache
caching.

Per-agent target dirs (Rule 1) plus sccache (with incremental
disabled) gives the best of both worlds: each agent has an isolated
target dir that doesn't collide with the main agent's, but the
underlying rustc cache is shared via sccache.

Combine with Rule 1:

    timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-<role> cargo test -p <crate>

## Rule 5: Before invoking cargo, check for active builds

If another cargo process is running on a shared target dir, your build
will queue on the lock. Quick check:

    pgrep -af cargo | grep -v grep

If you see another cargo invocation using the same CARGO_TARGET_DIR you
were about to use, EITHER wait for it OR pick a different target dir
(e.g. `target/agent-<role>-2`). Do not just launch and hope.

## Rule 6: Prefer `cargo nextest` for tests

CI runs `cargo nextest run` everywhere except doctests. Using nextest
locally matches CI behavior more closely (per-test isolation, parallel
binary execution). Install once: `cargo install cargo-nextest --locked`.

For doctests, still use `cargo test --doc` — nextest doctest support is
incomplete.

`.config/nextest.toml` configures `retries = 2` to handle timing-sensitive
flakes that nextest's parallelism can expose vs cargo test's serial
execution.

## Why these rules exist

Past sessions accumulated 24 lock-contended builds totaling ~36 minutes
of pure waiting. `sccache` (`RUSTC_WRAPPER=sccache`, set in user env)
ensures distinct target dirs share the rustc compile cache, so the disk
and CPU cost of multiple target dirs is negligible. The rule eliminates
contention without sacrificing speed.

## Reference command shape

Copy-paste template, fill in `<role>`, `<crate>`, and the actual cargo args:

    timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-<role> cargo test -p <crate>

# AGENT PUSH RULES — read before running `git push`

The lefthook deep gate is **opt-in**, not an automatic pre-push hook
(changed 2026-06-10). There is no pre-push git hook anymore, so a plain
`git push` runs no hook and completes normally — including from an agent
runtime. The old silent-kill failure mode (an agent's `git push` dying
mid-hook while the deep-gate Podman container ran ~3-4 min warm / ~15-20
min cold, diagnosed 2026-05-09) no longer applies, because nothing runs
on push. CI runs every Linux job on the PR, so CI is the gate.

## Rule: plain `git push` is fine; run the gate explicitly when you want it

- **Ordinary pushes:** just `git push`. No hook, no special handling.

- **Local pre-flight before a release tag or a large Rust change** you
  want validated before CI — run the deep gate first, then push:

      scripts/agent-push.sh            # runs `lefthook run deep-gate`, then git push

  or inline:

      lefthook run deep-gate && git push

  The gate's Podman container outlives the invoking shell, so if the
  runtime kills the command mid-gate the container keeps running and you
  see its result on the next `lefthook run deep-gate`. If you spot an
  orphaned gate container:

      podman ps                     # find the gate container
      podman rm -f <container-id>   # clean it up

## Keeping PRs up-to-date with main

Branch protection on `main` is `strict: true` — PRs must be up-to-date
with `main` to merge. When other sessions land commits while your PR
is open:

    gh pr update-branch <PR#>

adds a merge-commit from main into the PR branch via GitHub's "Update
branch" button. No local rebase, no force-push, triggers one fresh CI
run. Squash merge collapses the merge commit at merge time so history
stays clean.

### `gh pr merge --auto` is the right tool when main is busy

Auto-merge IS enabled at the repo level (`allow_auto_merge: true` per
`gh api repos/LEBOCQTitouan/tau`). Enrolling a PR via
`gh pr merge <PR#> --squash --delete-branch --auto` puts GitHub
in charge of the final mergeability gate — when all required checks
pass AND the branch is up-to-date, GitHub merges atomically. This
sidesteps the "checks-green-then-main-moved-then-merge-rejected"
race that pure `update-branch` loops hit during multi-PR days.

`gh pr merge --admin` works in some cases despite `enforce_admins:
true` (verified for self-referential `review PR` failures on PRs
that modify `.github/workflows/claude-review.yml` — the workflow
can't validate against itself, so admin bypass is the only path).
Do NOT use it casually; it skips required checks. Document the
reason in the PR body if you do.

The mergeability dance most agent sessions need:

1. `gh pr merge <PR#> --squash --delete-branch --auto`  (one-time enrol)
2. `gh pr update-branch <PR#>` whenever the PR is `BEHIND`
3. GitHub auto-merges when CI is green AND the branch is up-to-date

`--auto` does NOT itself update-branch — keep poking step 2 if main
keeps moving.

## Lefthook tests can corrupt git identity

The lefthook integration test suite writes `Test User
<test@example.com>` to the worktree-local `[user]` config and does
not always restore it. A subsequent commit then picks up that
identity. Safe pattern for every agent-driven commit:

    git -c user.name="<real>" -c user.email="<real>" \
      commit --no-verify -m "..."

`-c` overrides at the command level without persisting. Combined
with `--no-verify` (acceptable for docs-only changes per the rules
above), this also avoids re-triggering the corrupting test run.

# DOCS RULES — read before editing anything under `docs/`

The published book is `mdbook build` + `mdbook-linkcheck` over the
`docs/` tree, deployed to GitHub Pages by `.github/workflows/docs-deploy.yml`.
`book.toml` sets `warning-policy = "error"` for linkcheck, so a single
broken link fails the deploy job.

## Rule: build the book locally before opening a docs PR

Both binaries live at `~/.cargo/bin/{mdbook,mdbook-linkcheck}` but
that directory is not on the agent runtime's PATH. Build with PATH
prepended for the duration of the call, from the `docs/` directory:

    cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build

A clean build produces only `[INFO]` lines and leaves a `docs/book/`
tree (`book/html/` for the site, `book/linkcheck/` for the link
report). Remove `docs/book/` before committing — it is gitignored, but
worth `rm -rf docs/book` after verifying.

If either binary is missing, install once (the user must invoke this,
not the agent — `cargo install` of agent-chosen packages is denied):

    cargo install mdbook --locked --version ^0.4
    cargo install mdbook-linkcheck --locked --version ^0.7

## Rule: every doc page must be in `SUMMARY.md`

mdBook silently skips pages not listed in `docs/SUMMARY.md`. New
ADRs, tutorials, how-tos, reference pages, and explanation pages all
need a corresponding line. Linkcheck only verifies links between
pages that *are* in SUMMARY, so a forgotten entry hides both the page
and any broken outbound links it contains.

## Rule: docs-only PRs don't need the deep gate

The lefthook deep gate is Rust-CI mirroring and is opt-in (not a push
hook), so a docs-only change needs nothing special — just `git push`.
CI's `docs-deploy` job is the real gate. Run `lefthook run deep-gate`
(or `scripts/agent-push.sh`) only if a PR also touches Rust and you
want a local pre-flight before CI.

## Rule: the live URL is `lebocqtitouan.github.io/tau/`

The repository is `LEBOCQTitouan/tau` (capitalized). GitHub Pages
lowercases the owner, so the deployed site is at
`https://lebocqtitouan.github.io/tau/latest/`. `titouanlebocq.github.io`
returns 404 — do not confuse the two when smoke-testing a deploy.

## Rule: Mermaid diagrams via `mdbook-mermaid` (pinned `^0.14`)

`book.toml` declares the `mermaid` preprocessor; both docs CI
workflows (`docs-check.yml` and `docs-deploy.yml`) install
`mdbook-mermaid ^0.14`. The 0.14 series is the last that supports
mdBook 0.4 — newer mdbook-mermaid (0.15+) requires mdBook 0.5+ and
fails at the preprocessor wire protocol with "Unable to parse the
input". Do not bump mdbook-mermaid past 0.14 without also bumping
mdBook + mdbook-linkcheck.

Local install (one-time):

    cargo install mdbook-mermaid --locked --version "^0.14"

Authoring a diagram:

    ```mermaid
    flowchart LR
        A[Node] --> B[Other]
    ```

Two assets are vendored in `docs/` and committed:
`mermaid.min.js` (~2.6 MB) and `mermaid-init.js`. They are referenced
from `book.toml`'s `additional-js`. Do not delete them — the browser
needs them to render the diagrams.

linkcheck gotcha: pulldown-cmark sees mdbook-mermaid's HTML output
as a transparent block, but its state machine occasionally
mis-parses untyped fenced code blocks that *follow* a mermaid block.
Use `[…]` (Unicode ellipsis) instead of `[...]` inside such blocks,
or tag the fence with a language (` ```text `).

Mermaid-label gotcha: Mermaid 11 (what `mdbook-mermaid install`
bundles) interprets `1.` / `2.` prefixes in node and subgraph labels
as ordered-list markdown syntax, which renders as a literal
`Unsupported markdown: list` string in the page. Use `(1)`, `(2)`
or `Step 1`, `Step 2` instead. Same applies to `*` bullets at the
start of labels.
