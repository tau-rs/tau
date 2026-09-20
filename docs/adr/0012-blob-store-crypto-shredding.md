# ADR-0012: Blob store and crypto-shredding — content by hash, erasure by key drop

**Status:** Accepted
**Date:** 2026-09-16
**Deciders:** tau core
**Amends:** [ADR-0004](0004-abi-freeze.md) (the digest behind `BlobRef` is
named, as a store fact and not a wire fact; no bump), [ADR-0003](0003-substrate.md)
(invariant 3 gains its one exception: payload bytes are not cache, they are
the part of the truth that is designed to be deletable)

## Context

The log is the kernel and everything else is cache (ADR-0003). Every log
entry that carries content — a request an agent sent, a driver's reply, an
exit result, a hook's reason or note, a cancel reason — carries it as a
[`BlobRef`]: 32 bytes, a content hash, and nothing else (ADR-0004). The
bytes themselves live in a *blob store*, a content-addressed table the
kernel fills as it commits entries and that agents read from by reference.
HANDOFF §4 item 5 decided why on day one: payloads out of the log buy
deduplication for free and, decisively, make erasure possible — "per-subtree
encryption, delete key = payload gone, structural log survives." This ADR is
that design. The kernel and store work is a follow-on
([#136](https://github.com/tau-rs/tau/issues/136)).

The picture to hold: the log is a ledger that records *that* a letter was
sent, by whom, to whom, at what cost, and the fingerprint of the letter. The
letters themselves sit in a separate cabinet, each drawer locked with a key
that belongs to one agent. Shredding an agent's drawer means melting the
key; the ledger still balances, every row still reads, and the fingerprint
column still says which letter was in which envelope. Only the letters are
unreadable — everywhere, including in every backup ever taken of the
cabinet, because the backup holds locked drawers and the key is gone.

What is already true, and shapes the decision:

- **The store exists, in memory, and its contract is small.**
  `kernel/src/blob.rs` (M0) is a `BTreeMap<BlobRef, Vec<u8>>` with `put`,
  `get`, and a `digest` function: SHA-256 of the bytes, except that the
  empty payload maps to `BlobRef::EMPTY` because the ABI reserves that
  value. The kernel calls `put` at exactly the sites where an entry is
  committed with a payload — `send`, `reply`, `exit`, `cancel`, and the hook
  roll (`Deny` reasons, `Emit` notes, failure messages). Nothing else
  writes.
- **Reading is not a syscall and already returns an option.**
  `Kernel::read(BlobRef) -> Option<Vec<u8>>`, surfaced as `Handle::read`.
  The doc comment says why it is not the eighth syscall: a blob is immutable
  content-addressed data, and reading it has no effect to log. `libtau`
  already handles `None`: `infer()` returns `InferError::MissingPayload`,
  and the tool loop renders a missing reply as a tool result that says so.
  Erasure lands on a path userspace was already written to walk.
- **The fold has never seen the store.** `reducer.rs` contains no reference
  to it; `State` holds `BlobRef`s (mailbox envelopes, `Outcome::Exited`,
  cancel reasons) and never bytes; `State::hash()` is over the canonical
  state, so it is over references. The corpus (`corpus/`) and the kernel
  fixtures are logs with no store beside them, and
  `every_corpus_log_folds_to_its_sidecar` folds them to their pinned hashes
  today. "A fold with every payload missing equals the fold with them
  present" is not a test this ADR must add; it is the test the repository
  has been running since #91.
- **Snapshots carry references, never bytes.** ADR-0011 §1: a snapshot is
  the canonical state, and `tau replay --from` needs the log and the
  snapshot and nothing else. Neither verb has ever opened a store, because
  none exists on disk.
- **Drivers get bytes inline.** A `Delivery` to a driver carries
  `payload: Vec<u8>`; drivers hold no references and never call `read`. A
  driver's copy of a prompt — or a model provider's — is outside what tau
  can erase, and this ADR says so rather than implying otherwise.
- **The runtime log is in memory.** `Log::write_to` and `read_from` are the
  file form the CLI and the tests use; a running kernel's `Log` is a `Vec`.
  So "persistence" for the store means a file format and a handle the
  harness passes in, exactly as it does for the log, not a change to how
  the kernel runs.
- **The lineage is kernel state.** Every agent record has a `parent`, the
  harness's root has `None`, finished agents keep their records
  (`Status::Exited` / `Aborted`, "persists for accounting"), and the
  reducer maintains a `children` index. The kernel can name a subtree,
  finished or live, without help.

Two failures this ADR exists to prevent, one on each side. The first is an
erasure that is not one: the bytes are unlinked from the live store but
survive in a backup, a snapshot, or a second copy somebody else made, and
the operator who was told "gone" was told wrong. The second is an erasure
that breaks the run: a replay, a snapshot join, or a hash comparison that
needed the content and now fails, so the price of forgetting one subtree is
the integrity of the whole log. Every decision below follows from asking
**what must remain true after a key is gone** — and the answer is: every
structural fact, every hash, and nothing about the content.

## Decision

### 1. The store's contract, and who may call it

The store is a **port the kernel defines and an adapter the harness
supplies**. The port lives in `tau_kernel::blob`, next to today's in-memory
store, which becomes one implementation of it:

```rust
pub trait Blobs: Send {
    /// Stores `bytes` for `owner`; returns their reference. Idempotent per owner.
    fn put(&mut self, owner: AgentId, bytes: &[u8]) -> BlobRef;
    /// The bytes behind `blob`, if any copy this store holds is still readable.
    fn get(&self, blob: &BlobRef) -> Option<Vec<u8>>;
    /// Drops `owner`'s key. Every copy sealed for `owner` becomes unreadable.
    fn shred(&mut self, owner: AgentId);
    /// The first write-path failure, latched. `None` by default: a store that
    /// cannot fail says nothing (amendment 2026-09-20).
    fn fault(&self) -> Option<String> { None }
}
```

Three verbs, and the one new word is `owner`. A payload has exactly one
owner: **the agent whose log entry carries the reference**. The kernel
knows it at every `put` site, because it is writing that entry:

| Entry | Reference | Owner |
|---|---|---|
| `Sent { msg, via }` | `msg.payload` | the agent in `msg.from` — the sender |
| `Replied { msg, to }` | `msg.payload` | `to` — the agent that owns the corr |
| `Emitted { hook, to, msg }` | `msg.payload` | `to` — the agent the note is delivered to |
| `Exited { agent, result }` | `result` | `agent` — the one that exited |
| `Cancelled { agent, reason, .. }` | `reason` | `agent` — the root of the frozen subtree |
| `Verdicts { subject, roll }` | `Ruling::Deny(reason)`, `Ruling::Failed { error }` | `subject` — the agent whose action was judged |

The rule is derived from who *reads*: the reference is handed to exactly the
agents that `recv`, `wait` on, or are refused by that entry, and every one
of them is the owner or inside the owner's subtree. (A cancel reason reaches
every frozen agent, all of which are descendants of `agent`; a hook's
`Deny` reason is returned to `subject` as the error.) One consequence is
named here so it is not discovered later: a parent that `wait()`s for a
child owns nothing of the child's `result`. If the child's subtree is
shredded, the parent's later `read` of that result returns `None`. That is
erasure working — content produced inside a subtree is gone everywhere,
including from the summary it handed up — and not a fault. §Alternatives
keeps the two-owner rule and says why it lost.

**Who calls the port.** The kernel, at its commit sites, and no one else:
agents cannot `put` (there is no syscall for it; content enters the store
only as the payload of an effect the log records), drivers cannot (they
`reply` and the kernel stores), and the reducer cannot and does not (ADR-0003
invariant 2, the kernel never parses payloads; the fold does not even hold a
handle to the store). `get` is `Kernel::read`, unchanged. `shred` is the
harness's, through the kernel (§3), the way `attach` is (ADR-0008): a
privilege of the code that booted the run, not of any agent in it.

**Where it lives.** The port and the in-memory `Memory` store stay in
`tau_kernel::blob`, because the kernel must be bootable with no store
supplied and the sim and every kernel test run on `Memory`. The persistent,
encrypting store is a new workspace crate, **`store/` (`tau-store`)**, which
depends on `tau-kernel` for `BlobRef`, `AgentId` and the port, and on the
crypto crates the kernel should not carry. The layout rule in `CLAUDE.md`
— the kernel never touches the outside world — is why it is not a kernel
module; the fact that it implements no `Driver` and answers no `send` is
why it is not in `drivers/`. Dependencies point inward: `tau-store` knows
the kernel's types, the kernel knows only the trait.

**Three verbs, and one question** (amendment 2026-09-20,
[#144](https://github.com/tau-rs/tau/issues/144)). `put` and `shred` return
nothing a caller can refuse, because the kernel calls them while it is
writing an entry and a full disk is not an answer it can give an agent. A
store that could not write therefore *records* it, and the kernel *asks*
after every write — the same shape as driver supervision, where the kernel
classifies by what came back and never by what the callee says while it is
being called (ADR-0014 §1). The first `Some` faults the run
(`KernelError::Faulted`), and the entry that would have named the
reference is not written. The reason: a payload the log names and the
store never held is a run whose truth is incomplete (ADR-0003), and the
missing payload is otherwise indistinguishable from an erasure —
`read` returns `None` either way. `Kernel::fault()` is how the harness
asks, because `boot_with` took its store by value. One honest consequence,
accepted: a transient write failure stops the run instead of degrading it.
`Kernel::shred` reports the same latch as `ShredError::Faulted` rather
than as an erasure it cannot promise.

The kernel gains one boot form beside `Kernel::boot(log, spawner)`:
`Kernel::boot_with(log, spawner, Box<dyn Blobs>)`. `boot` is `boot_with`
over `Memory::new()`. Nothing in the seven syscalls, the reducer, or the
`Entry` kinds changes.

### 2. The digest: SHA-256 of the plaintext, named here and nowhere frozen

`BlobRef` is **SHA-256 of the payload bytes**, with the empty payload
mapping to `BlobRef::EMPTY` (32 zero bytes) instead of to
`sha256("")`. This is what `blob.rs` has computed since M0; the ADR's job
is to name it as a *store* fact, per ADR-0004's promise that "the ABI names
no hash function." The digest is computed over the plaintext, before
sealing, so:

- a reference is computable by anyone holding the bytes and no key — the
  sim builds every entry with `blob::digest` and has no store at all, and
  that stays true;
- identical payloads from one owner are one stored copy (`put` is
  idempotent per owner);
- changing the digest is a change to the store's header (§4), not to any
  wire type: a log written under SHA-256 keeps its references, a store that
  adopts a new digest for new content simply files new objects under new
  references, and a reader looks up whatever reference the entry carries.
  The three fixture snapshots, `PINNED` and the corpus sidecars do not move
  because the function is *named*; they would move if it *changed*, which
  is the point of naming it.

**One honest consequence, accepted.** A `BlobRef` in the log is a
fingerprint of the plaintext, and the log is not erased. After a shred,
someone holding the log and a *candidate* payload can hash the candidate and
confirm whether it was the content. For a model reply this is noise — the
space of candidates is the space of paragraphs — but for a low-entropy
payload (`{"ok":true}`, a yes/no tool result, a short reason string) it is
a real confirmation channel. This ADR accepts it, for two reasons. A keyed
or salted digest would close the channel but would make the reference
uncomputable without the secret: the sim, every fixture-recording test, and
`tau replay`'s in-process comparison would have to carry a key, and every
pinned hash would move once. And the channel is *confirmation*, not
*recovery*: nothing about the content that was not already guessed is
learned. The rule for anyone who needs confirmation resistance is the
ordinary one: content that must not be guessable is content that carries
entropy of its own. §Alternatives keeps the salted digest for a future ADR.

### 3. Keys: one per agent, sealed per owner, shredding by subtree

**Every agent has one data key**, 32 random bytes, created by the store on
the agent's first `put` and never derived from anything. A payload is
*sealed* (encrypted and authenticated, §4) under its owner's key and stored
as one object per `(reference, owner)`. Two owners that put identical bytes
each get their own sealed copy; `get` returns the plaintext of the first
copy whose key is still held. So deduplication is **within an owner**, and
a payload two subtrees both produced disappears only when both are
shredded — never earlier, which would over-erase, and never later, which
would under-erase.

**Shredding is by subtree, and the kernel names the subtree.**

```text
Kernel::shred(root: AgentId) -> Result<(), ShredError>
```

The kernel collects `root` and every descendant from its state, refuses
with `ShredError::Live(id)` if any of them is `Live` or `Cancelling`, and
otherwise calls the store's `shred(owner)` for each. Two things follow
from doing the walk in the kernel and not in the store:

- the store holds no lineage — a flat keyring, one key per owner, and
  `shred` is "forget this one key" — so the store's correctness does not
  depend on knowing the tree, and a key never has to be derived or
  re-wrapped when the tree changes;
- the precondition is a kernel fact. A live agent inside a shredded
  subtree would `put` new payloads under a key that no longer exists, or
  worse under a fresh one; refusing until the subtree is finished keeps
  "shredded" a past-tense word. The harness that wants a live subtree gone
  cancels it first (ADR-0002), waits for the abort, then shreds.

The root's own key is dropped too: `shred(root)` erases the root agent's
payloads, not only its descendants'. Shredding an agent that never `put`
anything is a no-op, not an error. Shredding twice is a no-op.

**What `read` returns afterwards: `None`.** The same `None` as for a
reference the store never held. This is deliberate. `Handle::read` is
agent-facing, an agent has no business knowing whether content is absent
because it was never stored or because an operator erased it, and `libtau`
already treats `None` as "the reply's payload is not in the blob store".
The *store* knows the difference — an object with no readable copy is
`Shredded`, a reference with no object is `Absent` — and exposes it as a
second method on the persistent store for tooling (§4), outside the port.

**Erasure is not a log entry.** The log records what happened in the run;
shredding is an operation on the store, after the fact, and it must leave
every entry byte-for-byte as written — that is the property (§5). A
thirteenth `Entry` kind would be an ABI change and a reducer change
(ADR-0010), for a fact the fold would have to ignore. Instead the persistent
store keeps a **tombstone list**: one line per shredded owner, appended when
the key is dropped, so `Shredded` can be answered after the object files
themselves are eventually compacted away and so an audit can list what was
erased. ADR-0003 invariant 3 — "any behavior that works without a log entry
is a bug" — is about *effects on the run*; a shred has none, by
construction, and the test in §6 is that a fold cannot tell it happened.

### 4. The persistent store: layout, sealing, and the header that names both

A `tau-store` directory, one per run, beside the log file the harness
writes (when it writes one; the store does not need the log to exist):

```text
<store>/
  STORE                        {"magic":"TAUB","v":1,"digest":"sha256","aead":"xchacha20poly1305"}
  objects/
    <ref hex>/                 one directory per reference
      <owner>                  one sealed copy per owner: nonce-free ciphertext + tag
  keys/
    <owner>                    32 random bytes, written on the owner's first put
  shredded                     append-only: one owner id per line, in shred order
```

**`STORE` is the store's header**, the log header's cousin: a reader parses
it before it touches an object, refuses a `magic` that is not `TAUB` or a
`v` it does not know, and learns the digest and the AEAD by name. Both are
*here* and nowhere in `kernel/src/abi/`: changing either bumps `v`, and a
store at `v: 2` may file new objects under a new digest while still reading
`v: 1` objects by whatever reference the entry carries. The ABI freeze's
promise about `BlobRef` is kept by putting the algorithm names in a file the
ABI does not own.

**Sealing** is XChaCha20-Poly1305 (an authenticated cipher with a 24-byte
nonce; RustCrypto's implementation, MIT/Apache, admissible under
`deny.toml`). Key: the owner's. **Nonce: the first 24 bytes of the
reference.** AAD: the full reference and the owner id. A deterministic nonce
is safe exactly when one key never sees the same nonce with two different
plaintexts, and content addressing guarantees that: the same nonce under the
same key means the same reference, which means the same bytes. What this
buys is a store whose ciphertext is a pure function of `(key, plaintext)` —
no RNG on the write path, reproducible bytes in tests, and an idempotent
`put` that can compare rather than trust. The AAD binds the ciphertext to
its place: a copy moved to another reference's directory or another owner's
name fails to open, instead of opening as the wrong content.

**Keys live apart from objects on purpose.** The whole promise of
crypto-shredding is that a backup of `objects/` is worthless without
`keys/`, so the operator's backup policy must be able to say "objects: keep
forever; keys: never leave this volume, or leave only wrapped." A layout
that put the key next to the object would make every backup a copy of the
key. In v1 the key files hold the raw key; wrapping them under an operator
master key (a KEK, a Keychain item, a KMS) is a decision this ADR leaves to
the harness and names as the next step, because it changes where a secret
is held and not what the store does. Until then, the security statement is
precise: **shredding is as good as the deletion of a 32-byte file in
`keys/`**, and no better.

**Open, reopen, and what a reopened store can answer.** `Disk::open(path)`
reads `STORE`, refuses or proceeds, and loads nothing else eagerly: keys are
read on first use, objects on `get`. A `put` writes the key file if absent
(fsync), then the object (write to a temp name, fsync, rename), so a crash
leaves either a complete object or none. A `shred` appends the tombstone,
then unlinks the key, then fsyncs `keys/`, in that order, so a crash between
the two can leave a tombstone without a deleted key — which the next
`shred` of the same owner finishes — and never a deleted key without a
tombstone. Beside the port, `Disk` exposes:

```text
Disk::status(&BlobRef) -> Status   // Present | Shredded | Absent
```

`Present`: some copy's key is held. `Shredded`: objects exist, or the owner
is in `shredded`, and no copy opens. `Absent`: no object and no tombstone
names an owner of it. This is for `tau` tooling and for the harness; agents
see `Option`.

**One writer.** A store directory belongs to one running kernel. A `LOCK`
file with the writer's pid is the follow-on's; the ADR requires only that
two kernels never share a store, because `put`'s idempotence is per process.

### 5. What replay and snapshots need from the store

**Nothing**, and the reason fits in one sentence: **the fold's only inputs
are entries, entries carry references and never bytes, and a reference is
a value the fold compares and copies but never dereferences.** `tau replay`
folds a log with no store beside it today and prints a hash the corpus
pins; `tau snapshot` writes a state that is references all the way down;
`tau replay --from` checks a prefix digest over log lines and a state hash
over canonical fields, neither of which is a payload. After every key in
the world is dropped, all three produce the bytes they produce today. This
is the property HANDOFF §4 item 5 was bought for, and it is not a design
goal of this ADR so much as a fact of the ones before it, which this ADR
must merely not break: no decision above touches `reducer.rs`, `Entry`,
`State`, or `FOLD`.

A restored snapshot (ADR-0011) rejoins a store the same way a fresh kernel
does: the harness opens the `Disk` beside the log and passes it to
`boot_with`. Mailboxes in the restored state hold references; the first
`recv` that reads one either finds its copy or gets `None`, exactly as a
never-snapshotted run would.

### 6. Tests: what already holds, and what the follow-on must add

Already pinned on `main`, and this ADR relies on them as they are:

| Test | What it already proves for the store |
|---|---|
| `every_corpus_log_folds_to_its_sidecar` (`kernel/tests/replay_cli.rs`) | a fold with *every* payload missing equals the pinned hash: the corpus has no store and never did |
| `every_fixture_folds_to_what_the_reducer_folds_in_process` | the in-process fold and the file fold agree, both over references only |
| `libtau/tests/tool_loop.rs` (the missing-reply case, line ~318) | userspace renders a `None` from `read` as a tool result that says the content is unavailable, and continues |
| `InferError::MissingPayload` (`libtau/src/infer.rs`) | `infer()` names the reference it could not read, instead of failing opaquely |
| `every_frozen_type_round_trips` (`kernel/tests/abi_snapshot.rs`) | `BlobRef`'s wire form is pinned: 64 lowercase hex characters, and this ADR adds no byte to it |

What the follow-on adds, each named so it can be checked off. All run on
`Memory` and `Disk` alike unless the row says otherwise; the port is one
contract with two implementations, and the same suite proves both:

- **`a_shred_is_invisible_to_the_fold`**: one scenario run twice, on
  `Memory` and on `Disk`; shred a finished subtree on the `Disk` run; the
  two logs are byte-for-byte equal via `Log::write_to`, `State::hash()`
  matches, and `tau replay` over the written log matches too. §5 as a test.
- **`a_shred_is_observable_only_through_read`**: after the shred, `read`
  returns `None` for every reference the subtree's entries carry and `Some`
  for every reference outside it, and nothing else on `Kernel` changed its
  answer.
- **`a_shred_of_a_live_subtree_is_refused`**: one live descendant is enough
  for `ShredError::Live`, naming it; cancel, wait for the abort, shred
  succeeds.
- **`the_root_shreds_its_own_payloads_too`**, and
  **`a_shred_of_an_agent_that_put_nothing_is_a_no_op`**, and
  **`a_second_shred_is_a_no_op`**.
- **`identical_bytes_from_two_owners_survive_the_shred_of_one`**: `read`
  still returns the bytes; shred the other; `None`. §3's copy rule.
- **`the_empty_reference_is_never_stored_and_never_shredded`**:
  `read(EMPTY)` is `Some(&[])` before and after any shred, and `objects/`
  never gains a directory for it.
- **`a_reopened_store_reads_what_it_wrote_and_not_what_it_shredded`**
  (`Disk`): put, shred one owner, drop the handle, `open` again; `status`
  answers `Present` / `Shredded` / `Absent` correctly for a reference of
  each kind, and `get` agrees.
- **`deleting_the_keys_directory_shreds_everything`** (`Disk`): remove
  `keys/` behind the store's back, reopen; every `get` is `None`, every
  object file is untouched. The backup argument of §4, executable.
- **`sealed_bytes_are_a_function_of_key_and_plaintext`** and
  **`a_copy_does_not_open_under_another_key_reference_or_owner`** (`Disk`):
  determinism of the nonce rule, and the AAD binding, each as a test.
- **`a_store_header_it_cannot_read_is_refused`** (`Disk`): wrong `magic`,
  unknown `v`, unknown `digest` or `aead` name, each refused naming the
  field and the value.
- **`infer_reports_a_shredded_reply_as_missing_payload`** (`libtau`): the
  existing variant, reached through an actual shred rather than a stubbed
  store.
- **`a_parent_reading_a_shredded_childs_result_gets_none`**: §1's named
  consequence, so it is a pinned behaviour and not a surprise.
- **`a_put_the_store_cannot_write_faults_the_kernel`**,
  **`a_shred_that_cannot_drop_a_key_is_not_reported_as_erasure`** and
  **`a_store_that_writes_cleanly_never_faults`** (`Disk`, amendment
  2026-09-20): a read-only `keys/` is what an unwritable volume looks like
  from the kernel's side. `drained` and `Kernel::fault()` name the write
  that failed, no `Exited` entry is logged for the payload the store did
  not take, and a shred that could not unlink a key answers
  `ShredError::Faulted` with the content still readable — the failure
  reported as a failure, not as erasure. Skipped, not faked, where the
  permission bits do not bite (root, or a filesystem that ignores them);
  the tests probe rather than guess.
- **Nothing re-pins.** No fixture, no sidecar, no `PINNED`, no `FOLD`
  moves; `just abi` shows no diff. If any does, the lane has touched
  something this ADR said it would not.

## Consequences

- **One kernel-and-store lane, filed:**
  [#136](https://github.com/tau-rs/tau/issues/136), blocked by this ADR's
  issue (#121) as a native dependency. It lands the `Blobs` port and
  `Memory` in `tau_kernel::blob`, `owner` at the six `put` sites,
  `Kernel::boot_with` and `Kernel::shred`, the `tau-store` crate with
  `Disk`, `status`, the tombstone list and the `STORE` header, and the
  tests in §6. No ABI bump, no `abi-change` label: nothing enters
  `kernel/src/abi/`. No reducer change: `FOLD` stays where ADR-0011 left
  it.
- **Erasure has a precise meaning now.** "Shred subtree *S*" means: every
  payload whose entry names an agent in *S* is unreadable through tau,
  forever, in every copy of `objects/` anywhere, from the moment the key
  files are gone. It does not mean: the log forgets *S* existed, a driver
  or a model provider forgets what it was sent, a native hook program
  forgets what it saw, or a `BlobRef` stops being a fingerprint. Each of
  those is stated above so that nobody is told "gone" and told wrong.
- **The fold is unchanged, provably.** §5 is a claim about code that
  already exists, and §6's first test pins it against the code that will.
  Snapshots, `tau replay`, the corpus and the sentinel are exactly as
  ADR-0011 left them.
- **Deduplication is per owner.** Two agents that produce the same bytes
  cost two sealed copies. Payloads are overwhelmingly unique (a model
  request, a model reply, a tool's output); the copies this costs are the
  small structural strings — cancel reasons, hook messages — and the cost
  is measured in kilobytes per run.
- **A parent can lose a child's result.** Named in §1, tested in §6. A
  harness that must keep a summary after erasing what it summarised copies
  the summary into an entry of its own — a `send`, or the parent's own
  `exit` — which seals it under the parent's key. The store does not do
  this for it, because the store cannot know which readings were meant to
  outlive the content.
- **The key material is the security boundary, and v1 stores it raw.** A
  raw key file in `keys/` is the whole secret. Backups must exclude it; a
  second volume or a wrapped keyring is the next step, and it changes the
  harness and not the store. ADR-0004's amendment row and this ADR both say
  so, so the gap is a known one and not a discovered one.
- **`ADR-0003` invariant 3 has its one exception, named.** "The log is the
  kernel; everything else is cache" holds for every derived thing — state,
  indexes, snapshots. Payload bytes are not derivable from the log; they
  are the other half of the truth, kept outside the log *so that* they can
  be deleted while the log cannot. The amendment row on ADR-0003 records
  this so the invariant is not read as "the store may be dropped and
  rebuilt," which it may not.

## Alternatives considered

**One key per run (the root's), everything sealed under it.** Rejected.
The erasure unit that matters is not the process but the request: a
long-lived root agent serving many users spawns a subtree per request, and
"forget this user's request" is a subtree, not a run. Per-run keys make
that impossible without restarting the root; per-agent keys make it a walk
the kernel already knows how to do.

**A key hierarchy: each agent's key wrapped under its parent's, so a
subtree shreds in O(1).** Rejected as complexity in the wrong place. It
puts lineage inside the store, so every spawn becomes a store operation
(derive, wrap, write) and every shred a chain of unwraps to verify; and it
buys a single unlink where the flat design does one unlink per agent in a
subtree that is, in every run so far, a few dozen agents. The kernel holds
the tree; the store holds keys.

**A keyed or salted digest, so a `BlobRef` is not a fingerprint.**
Rejected for now, kept for a future ADR (§2). It closes a confirmation
channel that this ADR accepts, at the cost of making the reference
uncomputable without a secret — by the sim, by fixture recording, by
`tau replay`'s in-process comparison — and of moving every pinned hash
once. The channel confirms, it does not recover, and the fix for a payload
that must not be confirmable is entropy in the payload.

**Sealing a child's `exit` result under the parent's key as well.**
Rejected (§1). It would make the result survive the child's shred, which
is the intuitive reading of "the parent was handed it" and the wrong
reading of "erase what this subtree produced." A summary derived from
erased content is erased content. The parent that needs it re-emits it in
an entry of its own, where the rule already seals it correctly.

**One sealed copy per reference, shared across owners.** Rejected. The
first owner's shred would erase the second owner's payload, or the second
owner's copy would keep the first owner's alive; either is a lie to one of
them. A copy per owner is exact, and it costs nothing on the payloads that
are actually large.

**A `Shredded` log entry.** Rejected (§3). It is a thirteenth `Entry`
kind — an ABI change under ADR-0010 and a reducer change to accept and
ignore it — recording an event that, by construction, has no effect on the
fold. The log records the run; the store records what has been done to the
store. A tombstone list beside the objects gives the audit the same fact
without teaching the fold to skip a line.

**A three-way answer from `Handle::read`: present, shredded, absent.**
Rejected for agents, kept for tooling (§4). An agent that behaves
differently on "shredded" than on "absent" is an agent whose behaviour
depends on an operator action the log does not record, which is ADR-0003
invariant 4 by another name. The harness and the CLI, which act outside the
run, get `Disk::status`.

**AES-256-GCM instead of XChaCha20-Poly1305.** Not chosen, not
foreclosed. GCM's 12-byte nonce would take the reference's first 12 bytes,
which is still collision-free under one key for the same reason; the
extended nonce simply leaves more room, and the cipher has no
hardware-dependence in its constant-time story. Both are names in `STORE`,
and a store at `v: 2` may prefer the other.

**The persistent store as a kernel module.** Rejected. It would put a
cipher, a filesystem, and fsync semantics inside the crate whose one rule
is that it does not touch the outside world, and it would make the fuzz and
sim builds carry them. The port in the kernel and the adapter in
`tau-store` is the ordinary hexagonal split, and it is what the log already
does in miniature (`Log` in the kernel, files in the CLI).

**The persistent store in `drivers/`.** Rejected. It satisfies the letter
of "the only code that touches the outside world" and muddles the meaning
of the crate: a driver answers `send`; a store answers `read`. A separate
crate says what it is.

## Amendments

- **2026-09-20** — A write the store could not do faults the run
  ([#144](https://github.com/tau-rs/tau/issues/144)). §1 left the port's
  infallibility with a hole this ADR itself named: `Disk` recorded its
  first I/O failure and nobody read it, because `boot_with` takes the store
  by value. The port gains a defaulted `fault() -> Option<String>`
  (additive; `Memory` takes the default and every implementation keeps
  compiling), the kernel asks after every write, and the first `Some`
  latches `KernelError::Faulted` — with the entry that would have named the
  reference left unwritten, and `Kernel::fault()` as the harness's
  accessor. `ShredError` gains `Faulted`. Nothing enters `kernel/src/abi/`:
  no `ABI` bump, no `abi-change` label, no snapshot, sidecar, `PINNED` or
  `FOLD` moves. The three tests are listed in §6. The honest consequence,
  stated in §1: a transient write failure now stops the run instead of
  degrading it. The alternative — the port's accessor alone, with the
  harness free to ask — was rejected because a harness that merely *could*
  ask will not, and the failure it would miss is silent data loss wearing
  the costume of a legitimate erasure.

[`BlobRef`]: ../../kernel/src/abi/msg.rs
