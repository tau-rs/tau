# The known-answer fixture — a store sealed once, opened forever

This directory is an artefact, not an output. It holds one store directory
exactly as a `tau-store` build wrote it, and `store/tests/known_answer.rs`
asks today's build to open it (#167). Everything else in `store/tests/`
seals and opens inside a single build: that proves the cipher round-trips,
and nothing at all about a store already on someone's disk.

```text
known-answer-v1/
  README.md      this file
  plaintext.bin  the bytes that were sealed: a sentence, then every byte value once
  store/         a store directory, as written
    STORE        {"aead":"xchacha20poly1305","digest":"sha256","magic":"TAUB","v":1}
    keys/1234567890          the owner's key: 32 ascending bytes, 0x00..0x1f
    objects/<ref>/1234567890 the sealed copy: ciphertext and tag, no nonce
```

The key is committed on purpose and is safe to commit: it is a test key for
a test payload, and the whole point is that it never changes. It is
ascending bytes precisely so it can never be mistaken for a key a real
store generated.

**No credential is committed here and none is ever needed.** `keys/…` is a
32-byte symmetric key for this fixture's payload and nothing else — not a
provider API key, not a Keychain item, not anything an API console issued.
The tests are pure file and cipher work: no network, no provider call, no
token spent, in CI or anywhere else. Provider credentials live only in the
macOS Keychain and are read by `just live record`, which is never part of
a test run.

## What a failing test here means

Not "the fixture is stale". It means **a store written by an older build no
longer reads**, which is the one thing this crate must never do quietly:

| Test that fails | What moved |
|---|---|
| `a_blob_sealed_by_an_older_build_still_opens` | the header, the cipher, the nonce rule, the additional data, or the file layout |
| `the_committed_plaintext_still_digests_to_the_committed_reference` | the digest behind every `BlobRef` on disk (`sha2`) |
| `sealing_the_committed_plaintext_reproduces_the_committed_bytes` | the sealed byte layout, in a way that may still open — a new build would file *different* bytes for the same payload |

The usual cause is a dependency bump: `sha2`, `chacha20poly1305`. A green
Tier 1 with these three red is the signal that the bump is not a bump but a
format change, and that every store in the field needs a `v: 2` and a
migration path (ADR-0012 §4), not a re-recorded fixture.

## Re-recording

Deliberate, never automatic. This fixture is **outside** the
`TAU_UPDATE_FIXTURES` flow, which re-records every fixture in a run: an
on-disk compatibility oracle that rewrites itself when it disagrees with
the code is worth nothing. Re-recording takes three steps and a human in
the middle of them:

1. Decide, in an ADR, that the format moves and `v` bumps. Until that
   exists there is nothing to re-record.
2. `cargo test -p tau-store --test known_answer -- --ignored --nocapture \
   record_the_known_answer_fixture`. It rebuilds `store/` from scratch and
   prints the reference it filed the payload under.
3. If the reference moved, the recorder fails and says so: set `REF_HEX` in
   `store/tests/known_answer.rs` to the printed value by hand, then read
   `git diff -- store/tests/fixtures` and say in the commit message what
   moved and why. Same discipline as the frozen ABI snapshots: a diff here
   is read, never accepted reflexively.

Nothing in the normal test run writes to this directory.

## Provenance

These bytes are not merely "recorded today". The same payload, sealed under
the same key by the build at `c808ae4` — the commit *before* `sha2` 0.11
(#161) and `chacha20poly1305` 0.11 (#163) landed — is byte-for-byte the
file committed here, header and reference included. That is the by-hand
argument made in those two pull requests, turned into something a test run
re-checks on every commit from now on.
