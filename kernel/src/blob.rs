//! The content-addressed payload store: the port, and its in-memory form.
//!
//! Payloads are not in the log (ADR-0004): an envelope carries a [`BlobRef`]
//! and the bytes live behind a [`Blobs`] store the harness supplies at boot.
//! ADR-0012 names the contract. Three verbs: `put` seals bytes for an
//! *owner* — the agent whose log entry carries the reference — `get` reads
//! them back by reference, and `shred` drops an owner's key so every copy
//! sealed for it is unreadable, everywhere, from then on.
//!
//! The kernel is the only caller of `put` and `shred`; `get` is
//! [`Kernel::read`](crate::kernel::Kernel::read). The reducer never sees the
//! store: entries carry references, and a reference is a value the fold
//! compares and copies but never dereferences (ADR-0012 §5).
//!
//! [`Memory`] is the store the kernel boots with when the harness supplies
//! none; the persistent, encrypting store is the `tau-store` crate, which
//! implements the same port and passes the same tests.

use std::collections::{BTreeMap, BTreeSet};

use sha2::{Digest, Sha256};

use crate::abi::{AgentId, BlobRef};

/// SHA-256 of `bytes`.
pub(crate) fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// The reference `bytes` would be stored under, without storing them.
///
/// SHA-256 of the plaintext (ADR-0012 §2): a *store* fact, named here and
/// nowhere in the ABI. The empty payload maps to [`BlobRef::EMPTY`] rather
/// than to the digest of zero bytes, because the ABI reserves that value for
/// exactly this. Computable by anyone holding the bytes and no key, which is
/// what lets the sim build every entry with no store at all.
#[must_use]
pub fn digest(bytes: &[u8]) -> BlobRef {
    if bytes.is_empty() {
        BlobRef::EMPTY
    } else {
        BlobRef::from_bytes(sha256(bytes))
    }
}

/// The store port (ADR-0012 §1). The kernel defines it; the harness supplies
/// an implementation at [`Kernel::boot_with`](crate::kernel::Kernel::boot_with).
///
/// Every implementation keeps the two rules the empty reference needs:
/// `put` of zero bytes stores nothing and returns [`BlobRef::EMPTY`], and
/// `get` of [`BlobRef::EMPTY`] is `Some` of zero bytes before and after any
/// shred. The empty payload is a value of the ABI, not content.
pub trait Blobs: Send {
    /// Stores `bytes` for `owner`; returns their reference. Idempotent per
    /// owner: the same bytes from the same owner are one stored copy.
    fn put(&mut self, owner: AgentId, bytes: &[u8]) -> BlobRef;

    /// The bytes behind `blob`, if any copy this store holds is still
    /// readable.
    fn get(&self, blob: &BlobRef) -> Option<Vec<u8>>;

    /// Drops `owner`'s key. Every copy sealed for `owner` becomes
    /// unreadable; a copy of the same bytes another owner put stays.
    /// Shredding an owner that put nothing, or one already shredded, is a
    /// no-op.
    fn shred(&mut self, owner: AgentId);

    /// The first write-path failure this store hit, if any, latched: once
    /// `Some`, it stays `Some`, and it names the first failure and not the
    /// latest.
    ///
    /// `put` and `shred` are infallible by contract — the kernel calls them
    /// while it is writing an entry, and a full disk is not an answer it can
    /// give an agent — so a store that could not write records it and the
    /// kernel asks here after every write (ADR-0012 §1). The kernel
    /// classifies by what came back and never by what the store says while
    /// it is being called, the same shape as driver supervision (ADR-0014
    /// §1). The first `Some` faults the run: a payload the log names and the
    /// store never held is a run whose truth is incomplete (ADR-0003), and
    /// it would otherwise read as `None` — indistinguishable from an
    /// erasure.
    ///
    /// The default is `None`, which is the honest answer for a store that
    /// cannot fail: [`Memory`] never writes anything.
    fn fault(&self) -> Option<String> {
        None
    }
}

/// One stored payload: its bytes, and the owners holding a copy.
#[derive(Debug)]
struct Object {
    bytes: Vec<u8>,
    owners: BTreeSet<AgentId>,
}

/// The in-memory store: no persistence, no cipher, the same observable
/// semantics as the disk store. A payload is kept once and the owners that
/// put it are counted; `shred` removes the owner from every count and drops
/// a payload nobody owns any more.
#[derive(Debug, Default)]
pub struct Memory {
    objects: BTreeMap<BlobRef, Object>,
}

impl Memory {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// How many distinct payloads are stored.
    #[must_use]
    pub fn len(&self) -> usize {
        self.objects.len()
    }

    /// Whether the store holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }
}

impl Blobs for Memory {
    fn put(&mut self, owner: AgentId, bytes: &[u8]) -> BlobRef {
        let blob = digest(bytes);
        if blob == BlobRef::EMPTY {
            return blob;
        }
        self.objects
            .entry(blob)
            .or_insert_with(|| Object {
                bytes: bytes.to_vec(),
                owners: BTreeSet::new(),
            })
            .owners
            .insert(owner);
        blob
    }

    fn get(&self, blob: &BlobRef) -> Option<Vec<u8>> {
        if *blob == BlobRef::EMPTY {
            return Some(Vec::new());
        }
        self.objects.get(blob).map(|o| o.bytes.clone())
    }

    fn shred(&mut self, owner: AgentId) {
        self.objects.retain(|_, object| {
            object.owners.remove(&owner);
            !object.owners.is_empty()
        });
    }
}
