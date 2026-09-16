//! The persistent, encrypting payload store (ADR-0012 §4).
//!
//! [`Disk`] implements the kernel's [`Blobs`] port over a directory: one
//! sealed copy per `(reference, owner)`, one random key per owner, and a
//! tombstone list of the owners that were shredded. Shredding is dropping a
//! key: every copy sealed for that owner becomes unreadable, in every backup
//! of `objects/` ever taken, because the backup holds locked drawers and the
//! key is gone.
//!
//! ```text
//! <store>/
//!   STORE          {"magic":"TAUB","v":1,"digest":"sha256","aead":"xchacha20poly1305"}
//!   objects/
//!     <ref hex>/   one directory per reference
//!       <owner>    one sealed copy per owner (decimal agent id): ciphertext + tag
//!   keys/
//!     <owner>      32 random bytes, written on the owner's first put
//!   shredded       append-only: one owner per line, in shred order
//! ```
//!
//! Sealing is XChaCha20-Poly1305 under the owner's key, with the nonce
//! being the first 24 bytes of the reference and the additional data the
//! full reference followed by the owner id (8 bytes, big-endian). Content
//! addressing is what makes a deterministic nonce safe: one key never sees
//! the same nonce with two different plaintexts, because the same nonce
//! means the same reference means the same bytes. What it buys is
//! ciphertext that is a pure function of key and plaintext — no randomness
//! on the write path — and additional data that binds a copy to its place,
//! so a file moved to another reference's directory or another owner's name
//! fails to open instead of opening as the wrong content.
//!
//! The security statement is precise: shredding is as good as the deletion
//! of a 32-byte file in `keys/`, and no better. Keys are stored raw in v1;
//! wrapping them under an operator key is the harness's next step, and a
//! backup policy must exclude `keys/`.
//!
//! One writer per store: `put`'s idempotence is per process.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{Key, KeyInit, XChaCha20Poly1305, XNonce};
use serde_json::Value;
use tau_kernel::abi::{AgentId, BlobRef};
use tau_kernel::blob::{digest, Blobs};

/// The header's magic. A file that does not open with it is not a store.
pub const MAGIC: &str = "TAUB";
/// The layout version this crate writes and reads.
pub const VERSION: u64 = 1;
/// The digest behind every reference, by name (ADR-0012 §2).
pub const DIGEST: &str = "sha256";
/// The cipher every copy is sealed with, by name (ADR-0012 §4).
pub const AEAD: &str = "xchacha20poly1305";

const HEADER_FILE: &str = "STORE";
const OBJECTS_DIR: &str = "objects";
const KEYS_DIR: &str = "keys";
const SHREDDED_FILE: &str = "shredded";
const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 24;

/// Why [`Disk::open`] refused.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum OpenError {
    /// The directory could not be read or initialised.
    #[error("{path}: {source}")]
    Io {
        /// What was being touched.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
    /// The directory exists and is not empty, but has no `STORE` header:
    /// refused rather than initialised over whatever is there.
    #[error("{0}: not a store (no STORE header) and not empty")]
    NotAStore(PathBuf),
    /// `STORE` is not a JSON object.
    #[error("STORE header is not JSON: {0}")]
    Malformed(#[source] serde_json::Error),
    /// A header field names something this crate cannot read.
    #[error("STORE header: {field} is {value}, which this store cannot read")]
    Header {
        /// The field.
        field: &'static str,
        /// Its value, as written.
        value: String,
    },
}

/// What a reopened store can say about a reference, for tooling and the
/// harness (ADR-0012 §4). Agents see [`Option`] through `read`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Some copy opens.
    Present,
    /// Copies exist and none opens: every owner's key is gone.
    Shredded,
    /// No copy was ever stored here.
    Absent,
}

type KeyBytes = [u8; KEY_LEN];

/// A store over a directory. See the crate docs for the layout.
#[derive(Debug)]
pub struct Disk {
    root: PathBuf,
    /// Keys read or generated so far. Cache: `keys/` is the truth, and a
    /// shred removes from both.
    keys: BTreeMap<AgentId, KeyBytes>,
    /// The first I/O failure on the write path, if any. The port's `put`
    /// and `shred` are infallible by contract; a failure leaves a
    /// reference with no copy, which reads as `None`, and is recorded here
    /// so the harness can tell a full disk from an erasure.
    fault: Option<String>,
}

impl Disk {
    /// Opens the store at `path`, or initialises one there.
    ///
    /// An existing store is validated by its `STORE` header: a `magic`
    /// that is not [`MAGIC`], or a `v`, `digest` or `aead` this crate does
    /// not know, is refused naming the field and the value. A path that
    /// does not exist, or an empty directory, becomes a new store. A
    /// non-empty directory with no header is refused.
    ///
    /// Nothing else is read eagerly: keys are read on first use, objects
    /// on `get`.
    ///
    /// # Errors
    ///
    /// [`OpenError`], as above.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, OpenError> {
        let root = path.as_ref().to_path_buf();
        let header = root.join(HEADER_FILE);
        match fs::read(&header) {
            Ok(bytes) => check_header(&bytes)?,
            Err(err) if err.kind() == io::ErrorKind::NotFound => initialise(&root)?,
            Err(source) => {
                return Err(OpenError::Io {
                    path: header,
                    source,
                })
            }
        }
        Ok(Self {
            root,
            keys: BTreeMap::new(),
            fault: None,
        })
    }

    /// The directory this store lives in.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.root
    }

    /// Whether `blob` is readable, erased, or was never here.
    #[must_use]
    pub fn status(&self, blob: &BlobRef) -> Status {
        if *blob == BlobRef::EMPTY {
            return Status::Present;
        }
        let owners = self.owners_of(blob);
        if owners.is_empty() {
            return Status::Absent;
        }
        if owners
            .iter()
            .any(|owner| self.open_copy(blob, *owner).is_some())
        {
            Status::Present
        } else {
            Status::Shredded
        }
    }

    /// The owners shredded so far, in shred order: the tombstone list.
    ///
    /// # Errors
    ///
    /// The list could not be read.
    pub fn shredded(&self) -> io::Result<Vec<AgentId>> {
        let text = match fs::read_to_string(self.root.join(SHREDDED_FILE)) {
            Ok(text) => text,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(err),
        };
        Ok(text
            .lines()
            .filter_map(|line| line.trim().parse().ok().map(AgentId::new))
            .collect())
    }

    /// The first write-path failure, if one happened. A `put` that failed
    /// stored no copy; a `shred` that failed may have left the key.
    #[must_use]
    pub fn fault(&self) -> Option<&str> {
        self.fault.as_deref()
    }

    fn objects_dir(&self, blob: &BlobRef) -> PathBuf {
        self.root.join(OBJECTS_DIR).join(blob.to_hex())
    }

    fn object_path(&self, blob: &BlobRef, owner: AgentId) -> PathBuf {
        self.objects_dir(blob).join(owner_name(owner))
    }

    fn key_path(&self, owner: AgentId) -> PathBuf {
        self.root.join(KEYS_DIR).join(owner_name(owner))
    }

    /// Every owner with a copy of `blob`, in id order.
    fn owners_of(&self, blob: &BlobRef) -> BTreeSet<AgentId> {
        let Ok(entries) = fs::read_dir(self.objects_dir(blob)) else {
            return BTreeSet::new();
        };
        entries
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().to_str()?.parse::<u64>().ok())
            .map(AgentId::new)
            .collect()
    }

    /// `owner`'s key if it is held: cached, or still on disk.
    fn held_key(&self, owner: AgentId) -> Option<KeyBytes> {
        if let Some(key) = self.keys.get(&owner) {
            return Some(*key);
        }
        read_key(&self.key_path(owner)).ok().flatten()
    }

    /// `owner`'s key, generating and writing it on the first put.
    fn key_for(&mut self, owner: AgentId) -> io::Result<KeyBytes> {
        if let Some(key) = self.keys.get(&owner) {
            return Ok(*key);
        }
        let path = self.key_path(owner);
        let key = match read_key(&path)? {
            Some(key) => key,
            None => {
                let mut key = [0u8; KEY_LEN];
                getrandom::getrandom(&mut key).map_err(|err| io::Error::other(err.to_string()))?;
                write_durably(&path, &key)?;
                key
            }
        };
        self.keys.insert(owner, key);
        Ok(key)
    }

    /// The plaintext of `owner`'s copy of `blob`, if the key is held and
    /// the copy opens under it.
    fn open_copy(&self, blob: &BlobRef, owner: AgentId) -> Option<Vec<u8>> {
        let key = self.held_key(owner)?;
        let sealed = fs::read(self.object_path(blob, owner)).ok()?;
        unseal(&key, blob, owner, &sealed)
    }

    fn try_put(&mut self, owner: AgentId, blob: BlobRef, bytes: &[u8]) -> io::Result<()> {
        let key = self.key_for(owner)?;
        let path = self.object_path(&blob, owner);
        if path.exists() {
            return Ok(());
        }
        let sealed =
            seal(&key, &blob, owner, bytes).map_err(|_| io::Error::other("sealing failed"))?;
        write_durably(&path, &sealed)
    }

    fn try_shred(&mut self, owner: AgentId) -> io::Result<()> {
        self.keys.remove(&owner);
        let key = self.key_path(owner);
        let tombstoned = self.shredded()?.contains(&owner);
        if tombstoned && !key.exists() {
            return Ok(());
        }
        // Tombstone first, key second: a crash between the two leaves a
        // tombstone the next shred finishes, never a dropped key with no
        // record of the drop.
        if !tombstoned {
            let mut list = OpenOptions::new()
                .append(true)
                .create(true)
                .open(self.root.join(SHREDDED_FILE))?;
            writeln!(list, "{}", owner_name(owner))?;
            list.sync_all()?;
        }
        match fs::remove_file(&key) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
        sync_dir(&self.root.join(KEYS_DIR))
    }
}

impl Blobs for Disk {
    fn put(&mut self, owner: AgentId, bytes: &[u8]) -> BlobRef {
        let blob = digest(bytes);
        if blob == BlobRef::EMPTY {
            return blob;
        }
        if let Err(err) = self.try_put(owner, blob, bytes) {
            self.fault
                .get_or_insert_with(|| format!("put of {blob} for {owner}: {err}"));
        }
        blob
    }

    fn get(&self, blob: &BlobRef) -> Option<Vec<u8>> {
        if *blob == BlobRef::EMPTY {
            return Some(Vec::new());
        }
        self.owners_of(blob)
            .into_iter()
            .find_map(|owner| self.open_copy(blob, owner))
    }

    fn shred(&mut self, owner: AgentId) {
        if let Err(err) = self.try_shred(owner) {
            self.fault
                .get_or_insert_with(|| format!("shred of {owner}: {err}"));
        }
    }
}

// --- the header -----------------------------------------------------------

fn header_json() -> String {
    serde_json::json!({
        "magic": MAGIC,
        "v": VERSION,
        "digest": DIGEST,
        "aead": AEAD,
    })
    .to_string()
}

fn check_header(bytes: &[u8]) -> Result<(), OpenError> {
    let value: Value = serde_json::from_slice(bytes).map_err(OpenError::Malformed)?;
    let field = |name: &'static str| value.get(name).cloned().unwrap_or(Value::Null);
    let check = |name: &'static str, ok: bool| {
        if ok {
            Ok(())
        } else {
            Err(OpenError::Header {
                field: name,
                value: field(name).to_string(),
            })
        }
    };
    check("magic", field("magic").as_str() == Some(MAGIC))?;
    check("v", field("v").as_u64() == Some(VERSION))?;
    check("digest", field("digest").as_str() == Some(DIGEST))?;
    check("aead", field("aead").as_str() == Some(AEAD))
}

fn initialise(root: &Path) -> Result<(), OpenError> {
    let io = |path: &Path| {
        let path = path.to_path_buf();
        move |source| OpenError::Io { path, source }
    };
    match fs::read_dir(root) {
        Ok(mut entries) => {
            if entries.next().is_some() {
                return Err(OpenError::NotAStore(root.to_path_buf()));
            }
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(io(root)(err)),
    }
    for dir in [OBJECTS_DIR, KEYS_DIR] {
        let path = root.join(dir);
        fs::create_dir_all(&path).map_err(io(&path))?;
    }
    let header = root.join(HEADER_FILE);
    write_durably(&header, header_json().as_bytes()).map_err(io(&header))
}

// --- sealing --------------------------------------------------------------

fn owner_name(owner: AgentId) -> String {
    owner.get().to_string()
}

fn nonce_of(blob: &BlobRef) -> XNonce {
    let mut nonce = [0u8; NONCE_LEN];
    for (dst, src) in nonce.iter_mut().zip(blob.as_bytes()) {
        *dst = *src;
    }
    XNonce::from(nonce)
}

fn aad_of(blob: &BlobRef, owner: AgentId) -> Vec<u8> {
    let mut aad = Vec::with_capacity(KEY_LEN + 8);
    aad.extend_from_slice(blob.as_bytes());
    aad.extend_from_slice(&owner.get().to_be_bytes());
    aad
}

fn seal(
    key: &KeyBytes,
    blob: &BlobRef,
    owner: AgentId,
    bytes: &[u8],
) -> Result<Vec<u8>, chacha20poly1305::Error> {
    let aad = aad_of(blob, owner);
    XChaCha20Poly1305::new(&Key::from(*key)).encrypt(
        &nonce_of(blob),
        Payload {
            msg: bytes,
            aad: &aad,
        },
    )
}

fn unseal(key: &KeyBytes, blob: &BlobRef, owner: AgentId, sealed: &[u8]) -> Option<Vec<u8>> {
    let aad = aad_of(blob, owner);
    XChaCha20Poly1305::new(&Key::from(*key))
        .decrypt(
            &nonce_of(blob),
            Payload {
                msg: sealed,
                aad: &aad,
            },
        )
        .ok()
}

// --- files ----------------------------------------------------------------

/// `Some(key)` if the file holds one, `None` if there is no file.
fn read_key(path: &Path) -> io::Result<Option<KeyBytes>> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    let mut key = [0u8; KEY_LEN];
    file.read_exact(&mut key)?;
    if file.read(&mut [0u8; 1])? != 0 {
        return Err(io::Error::other(format!(
            "{}: a key is {KEY_LEN} bytes, this file is longer",
            path.display()
        )));
    }
    Ok(Some(key))
}

/// Writes `bytes` to `path` so that a crash leaves either the whole file
/// or none: a temporary name in the same directory, fsync, rename, fsync
/// the directory.
fn write_durably(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other(format!("{}: no parent directory", path.display())))?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| io::Error::other(format!("{}: no file name", path.display())))?;
    fs::create_dir_all(dir)?;
    let temp = dir.join(format!(".{name}.tmp"));
    let mut file = File::create(&temp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temp, path)?;
    sync_dir(dir)
}

fn sync_dir(dir: &Path) -> io::Result<()> {
    File::open(dir)?.sync_all()
}
