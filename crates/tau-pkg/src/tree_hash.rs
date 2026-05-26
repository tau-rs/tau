//! Source-agnostic whole-tree SHA-256 hashing for `tau verify`
//! (ADR-0012 / Tier 2 priority 7).
//!
//! Walks the install dir, sorts files by relative path, and feeds
//! `path\0content\0` for each into a single SHA-256 stream. Excludes
//! `.git/`, `target/`, and any `*.tau-tmp/` directories.
//!
//! Symlinks are NOT followed (`walkdir::WalkDir::follow_links(false)`).
//! A symlink contributes its target path bytes to the hash, not the
//! resolved file's content. This prevents symlink-loop pitfalls.
//!
//! Used at install time (populate `LockedVersion.sha256`) and at
//! verify time (recompute + compare).

use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Lowercase-hex encode a byte slice. digest 0.11's `finalize()` returns
/// a `hybrid_array::Array<u8, ..>` which no longer implements `LowerHex`
/// directly (it did in 0.10 via `GenericArray`), so we format manually.
pub(crate) fn to_hex_lower(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Error from `tree_hash`.
///
/// # Example
///
/// ```
/// use tau_pkg::tree_hash::TreeHashError;
///
/// let err = TreeHashError::Io {
///     path: std::path::PathBuf::from("/no/such/file"),
///     message: "reading file: No such file or directory".to_string(),
/// };
/// let display = format!("{err}");
/// assert!(display.contains("io error at"));
/// assert!(display.contains("/no/such/file"));
/// ```
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum TreeHashError {
    /// I/O error reading a file or walking the tree.
    #[error("io error at {path}: {message}")]
    Io {
        /// The path that errored.
        path: PathBuf,
        /// The error message.
        message: String,
    },
}

/// SHA-256 of a single file. Used for plugin binary hashing
/// (`LockedPlugin.binary_sha256`).
///
/// Returns 64-char lowercase hex.
///
/// # Example
///
/// ```
/// use tau_pkg::tree_hash::sha256_of_file;
///
/// # let tmp = tempfile::tempdir().expect("tempdir");
/// # let path = tmp.path().join("file.bin");
/// # std::fs::write(&path, b"hello world").expect("write");
/// let hex = sha256_of_file(&path).expect("sha256_of_file");
/// assert_eq!(hex.len(), 64);
/// assert!(hex.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
/// ```
pub fn sha256_of_file(path: &Path) -> Result<String, TreeHashError> {
    let bytes = fs::read(path).map_err(|e| TreeHashError::Io {
        path: path.to_path_buf(),
        message: format!("reading file: {e}"),
    })?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Ok(to_hex_lower(&hasher.finalize()))
}

/// Hash entry for a single file in the tree.
///
/// # Example
///
/// ```
/// use tau_pkg::tree_hash::sha256_of_file;
///
/// // `FileHash` is produced internally by `tree_hash`; obtain a SHA-256
/// // via `sha256_of_file` to verify the same hex encoding is used.
/// # let tmp = tempfile::tempdir().expect("tempdir");
/// # let path = tmp.path().join("a.txt");
/// # std::fs::write(&path, b"content").expect("write");
/// let hex = sha256_of_file(&path).expect("sha256");
/// assert_eq!(hex.len(), 64, "SHA-256 is always 64 hex chars");
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileHash {
    /// Relative path from the tree root.
    pub rel_path: String,
    /// SHA-256 hex of the file contents (or symlink target bytes).
    pub sha256: String,
}

/// Compute a whole-tree SHA-256 of `root`.
///
/// Excludes `.git/`, `target/`, and `*.tau-tmp/` directories. Files
/// are sorted lexicographically by relative path. For each file:
///
/// ```text
/// hasher.update(rel_path.as_bytes());
/// hasher.update(&[0]);
/// hasher.update(file_contents);
/// hasher.update(&[0]);
/// ```
///
/// Returns 64-char lowercase hex.
///
/// # Example
///
/// ```
/// use tau_pkg::tree_hash;
///
/// # let tmp = tempfile::tempdir().unwrap();
/// # std::fs::write(tmp.path().join("hello.txt"), b"hello world").unwrap();
/// let hash = tree_hash(tmp.path()).unwrap();
/// assert_eq!(hash.len(), 64);
/// ```
pub fn tree_hash(root: &Path) -> Result<String, TreeHashError> {
    let mut entries: Vec<(String, PathBuf)> = Vec::new();

    for entry in WalkDir::new(root).follow_links(false) {
        let entry = entry.map_err(|e| TreeHashError::Io {
            path: e.path().map(Path::to_path_buf).unwrap_or_default(),
            message: format!("walking tree: {e}"),
        })?;

        if entry.file_type().is_dir() {
            continue;
        }

        // Skip files inside excluded directories.
        let path = entry.path();
        if path.components().any(|c| {
            let s = c.as_os_str().to_string_lossy();
            s == ".git" || s == "target" || s.ends_with(".tau-tmp")
        }) {
            continue;
        }

        let rel = path.strip_prefix(root).map_err(|e| TreeHashError::Io {
            path: path.to_path_buf(),
            message: format!("computing relative path: {e}"),
        })?;
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        entries.push((rel_str, path.to_path_buf()));
    }

    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let mut hasher = Sha256::new();
    for (rel_path, abs_path) in entries {
        let bytes = if abs_path.is_symlink() {
            let target = fs::read_link(&abs_path).map_err(|e| TreeHashError::Io {
                path: abs_path.clone(),
                message: format!("reading symlink: {e}"),
            })?;
            target.to_string_lossy().into_owned().into_bytes()
        } else {
            fs::read(&abs_path).map_err(|e| TreeHashError::Io {
                path: abs_path.clone(),
                message: format!("reading file: {e}"),
            })?
        };

        hasher.update(rel_path.as_bytes());
        hasher.update([0u8]);
        hasher.update(&bytes);
        hasher.update([0u8]);
    }

    Ok(to_hex_lower(&hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn empty_dir_hashes_to_empty_sha256() {
        let dir = TempDir::new().unwrap();
        let h = tree_hash(dir.path()).unwrap();
        // SHA-256 of empty input is a known value:
        assert_eq!(
            h,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn single_file_is_deterministic() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("a.txt"), b"hello").unwrap();
        let h1 = tree_hash(dir.path()).unwrap();
        let h2 = tree_hash(dir.path()).unwrap();
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
    }

    #[test]
    fn nested_dirs_hash_includes_all_files() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("a.txt"), b"hello").unwrap();
        fs::write(dir.path().join("src/b.rs"), b"fn main() {}").unwrap();
        let h_two_files = tree_hash(dir.path()).unwrap();
        // Drop one file → different hash.
        fs::remove_file(dir.path().join("src/b.rs")).unwrap();
        let h_one_file = tree_hash(dir.path()).unwrap();
        assert_ne!(h_two_files, h_one_file);
    }

    #[test]
    fn dot_git_dir_is_excluded() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("a.txt"), b"hello").unwrap();
        let h_no_git = tree_hash(dir.path()).unwrap();

        fs::create_dir_all(dir.path().join(".git/objects")).unwrap();
        fs::write(dir.path().join(".git/HEAD"), b"ref: refs/heads/main").unwrap();
        fs::write(dir.path().join(".git/objects/abc"), b"blob").unwrap();
        let h_with_git = tree_hash(dir.path()).unwrap();
        assert_eq!(h_no_git, h_with_git, ".git/ should be excluded");
    }

    #[test]
    fn target_dir_is_excluded() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("Cargo.toml"), b"[package]").unwrap();
        let h_no_target = tree_hash(dir.path()).unwrap();

        fs::create_dir_all(dir.path().join("target/release")).unwrap();
        fs::write(dir.path().join("target/release/foo"), b"binary").unwrap();
        let h_with_target = tree_hash(dir.path()).unwrap();
        assert_eq!(h_no_target, h_with_target, "target/ should be excluded");
    }
}
