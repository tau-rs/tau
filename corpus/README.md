# The determinism corpus

Logs the reducer folded once, with the state hash it produced. Every night
`tier3.yml` refolds each of them with today's `main` and expects the same
hash. That is ADR-0003's "we do not break userspace", executable: a log
recorded on any past build must fold to the same state on every future one.

The corpus is frozen history. It is consumed by CI, not by any crate, which
is why it lives at the repository root and not under `kernel/` or `sim/`.
The three milestone logs are *copies* of `kernel/tests/fixtures/*.log`: the
kernel's fixtures may be re-recorded when a milestone test changes, but a
corpus entry never changes once it is in.

## Contents

| fixture | source | entries | recorded at | hash pinned at |
|---|---|---|---|---|
| `m0-walking-skeleton` | `kernel/tests/fixtures/m0-walking-skeleton.log` | 10 | ABI 0 | `a62e88c` (#80) |
| `m1a-cancel` | `kernel/tests/fixtures/m1a-cancel.log` | 11 | ABI 0 | `a62e88c` (#80) |
| `m1b-wall` | `kernel/tests/fixtures/m1b-wall.log` | 9 | ABI 0 | `a62e88c` (#80) |
| `tier1-seed-a` | `soak soak --seed 0x7a75000000000001 --events 5000` at `f7bcdc5` | 5032 | ABI 0 | `a62e88c` (#80) |
| `tier1-seed-b` | `soak soak --seed 0x7a75000000000002 --events 5000` at `f7bcdc5` | 5018 | ABI 0 | `a62e88c` (#80) |
| `tier1-seed-c` | `soak soak --seed 0x7a75000000000003 --events 5000` at `f7bcdc5` | 5012 | ABI 0 | `a62e88c` (#80) |
| `m2a-hooks` | `kernel/tests/fixtures/m2a-hooks.log` | 53 | ABI 1 | `a62e88c` (#80) |

The six ABI 0 logs are the ones first pinned at `f7bcdc5` (#62), unchanged
byte for byte; ADR-0008 (#80) made `hooks` a canonical field of `State`, so
every fold's hash moved once and the sidecars were re-pinned with that
reason. They are the corpus's proof that a log from before a bump still
folds after it. `m2a-hooks` is the first log with `Attached`, `Verdicts`,
and `Emitted` entries: its `Attached` entries name native programs no
build has, and the fold does not need them.

The three soak logs were the Tier 1 seeds at the Tier 1 size when they were
recorded. `PINNED` in `sim/tests/determinism.rs` pins what the *current*
generator produces for those seeds; since #80 the generator attaches hooks
at boot, so the two no longer describe the same logs. The sidecars here
pin the fold of a frozen log; `PINNED` pins the generator.

## Adding a fixture

Each fixture is a pair: `NAME.log`, a newline-delimited JSON log as
`Log::write_to` writes it, and `NAME.hash`, one line holding the state hash
the current `main` folds it to. Keep each log under ~1 MB; the job refolds
all of them in one runner-minute and the repository carries them forever.

A sim run, from the repository root:

```sh
cargo build --release -p tau-sim --bin soak
./target/release/soak soak --seed <S> --events <N> \
  --log corpus/<name>.log --hash corpus/<name>.hash
```

`soak` refuses to write a hash its own refold did not reproduce, so a sidecar
it wrote is already a same-build refold. For a log recorded elsewhere (a
release, a harness run, a bug report), write the sidecar from a refold on
`main` and check it the way the job will:

```sh
./target/release/soak refold --log corpus/<name>.log --expect corpus/<name>.hash
```

Add a row to the table above with the `main` SHA the hash was pinned at.
The job fails closed below seven fixtures, so a fixture can be added but the
floor in `tier3.yml` should move up with the count.

## A hash that moved

A sidecar that no longer matches means the reducer's behaviour changed for
an input it had already accepted. That is a reducer-behaviour change, and the
job files it as one (`tier3: determinism drift — <fixture>`). Re-pinning the
sidecar is never the fix on its own: it needs an ADR-level reason, the way
#62 re-pinned the milestone snapshots because the hash's *definition* changed
(canonical state only, ADR-0003's "everything else is cache"), and the way
#80 re-pinned every sidecar because ADR-0008 added a canonical field. A
re-pin without that reason quietly narrows the promise this directory exists
to keep.
