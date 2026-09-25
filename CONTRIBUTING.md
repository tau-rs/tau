# Contributing

## The short version

```sh
just check      # before every commit: fmt + clippy + unit tests, seconds
just test       # before every pull request: everything Tier 1 runs
```

Branch from `main`, keep the branch short-lived, use
[conventional commits](https://www.conventionalcommits.org/), open a pull
request. `main` is protected: the merge queue is the only way in, and it re-runs
Tier 1 on the *merged* state, which is what kills "green on the branch, red on
main".

## Picking up work

Open issues are the to-do list; nothing else is. An issue is startable when it
has no open blocker, no assignee, and no open pull request. Claim it by
assigning yourself, and put `Closes #N` in the pull request body so the merge
closes it.

The one exception is Dependabot (`.github/dependabot.yml`): its weekly grouped
pull requests have no issue to close, because the bump *is* the whole unit of
work. They take Tier 1 and the merge queue like any other change.

Blockers are native issue dependencies (the issue's *Relationships → Blocked
by*, or `POST /repos/{owner}/{repo}/issues/{n}/dependencies/blocked_by`), never
prose. A blocker that is a *condition* with no issue yet — a crate that does not
exist, a second maintainer — keeps the `status:gated` label with the reason in
the body until someone files the issue and adds the edge. `status:ready` means
no open blocker. Anything that lists readiness reads only the dependency graph,
so a "Gated on: #N" sentence is invisible to it.

## The one rule that is different here

`kernel/src/abi/` is frozen. It evolves additively or not at all, and a pull
request that touches it must either bump `pub const ABI: u16` or carry the
`abi-change` label with a linked ADR. Three gates enforce this — `CODEOWNERS`,
the CI diff gate, and `insta` snapshots of the wire format — and they are
redundant on purpose, because each of them fails differently. See
[ADR-0004](docs/adr/0004-abi-freeze.md).

If a snapshot test fails, the ABI is telling you that you changed it. Read the
diff before running `cargo insta accept`.

Any new type in `kernel/src/abi/` arrives with its snapshot in the same pull
request. A frozen type with no pinned wire form is frozen in name only.

## Proposing an eighth syscall

Show that it is *inexpressible* as a program over the existing seven. If it is
expressible, it belongs in `libtau`, which has no special powers and needs no
ADR. A proposal that fails this test still gets written down — an unrecorded
rejection gets re-proposed in six months. See
[ADR-0002](docs/adr/0002-seven-syscalls.md).

## Where a change belongs

The sorting rule, applied to every feature:

| Scope | Home |
|---|---|
| Per-agent behavior | a program (agent code, or `libtau`) |
| Per-endpoint behavior | a driver |
| Cross-cutting policy | a hook |

Hooks never do I/O and never run inference. ML-based screening is a driver or an
agent, not a hook.

## Determinism is not optional

The reducer is a pure fold: no clock, no RNG, no iteration-order leaks. `HashMap`
and `HashSet` are denied workspace-wide, as are `SystemTime::now` and
`Instant::now` — time enters the system as log entries from the clock driver.
These are lints rather than review comments because the failure they prevent
shows up as a state-hash mismatch on someone else's machine, weeks later.

Agent futures hold only plain memory and the kernel handle. No guards across an
`await`: a drop-guard that runs during a hard abort is state the log never saw.

## Tiers

| Tier | When | What |
|---|---|---|
| 0 | locally, on save | `just check` — seconds |
| 1 | every PR + merge queue | fmt, clippy, tests, ABI guard, cargo-deny, macOS smoke — under 8 minutes |
| 2 | `deep-ci` label, or automatically on `kernel/**` | property tests, simulation soak, fuzzing, sanitizers, full macOS matrix |
| 3 | scheduled | determinism drift sentinel, long fuzz, dependency drift, benchmarks, flake hunter |

Tier 3 opens issues rather than failing builds. Drift is not the committer's
fault, and blaming the wrong person trains everyone to ignore red.

### Provider drift is caught by hand

No tier talks to a paid model provider. The cassettes under
`drivers/tests/cassettes/` are a photograph of each provider taken the day
they were recorded, and the replay tests compare the driver against that
photograph. So when Anthropic or OpenAI changes the shape of a tool-call id,
a usage field or a thinking signature, replay stays green: it is checking
the driver against the past, not the present. Nothing scheduled notices.

The only detector is a person running the live modes from Keychain keys:

- `just live record` re-records the cassettes; a diff in the committed
  files is the provider having moved.
- `just live e2e` runs the tool-loop programs against the real APIs and
  asserts the shape of what happened, not the bytes.

Who and when: the maintainer, before any PR that touches
`drivers/src/model/**` lands, and after any provider changelog entry that
names the Messages or chat-completions wire. A re-record that changes a
cassette goes in its own PR that says which provider moved and how.

The honest consequence: between those runs, provider drift is undetected by
design. That is a decision (#139), not a gap waiting for a workflow:
provider keys do not live in repository secrets and CI never spends on a
provider API. A key in a public repository's secrets is reachable by anyone
with write access, and a nightly that spends is a bill that grows with every
row somebody adds. The keyless half of the same idea, `just live e2e ollama`
against a local model, is what a nightly can run (#168), but it guards our
driver and tool loop against a real server; a local model's API does not
move, so it says nothing about the providers.
