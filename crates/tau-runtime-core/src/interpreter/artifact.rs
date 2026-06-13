//! Reading produced artifacts (files / named outputs) for check evaluation.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::error::RuntimeError;

/// Reads filesystem artifacts so checks can inspect produced content.
/// Host-implemented (`std::fs`); `no_std` core stays I/O-free.
pub trait ArtifactReader: Send + Sync {
    /// Read a path's bytes. `Ok(None)` means the path does not exist.
    fn read_path(&self, path: &str) -> Result<Option<Vec<u8>>, RuntimeError>;
}

/// In-memory reader for tests.
#[derive(Debug, Default, Clone)]
pub struct InMemoryArtifactReader {
    files: BTreeMap<String, Vec<u8>>,
}

impl InMemoryArtifactReader {
    /// Empty reader.
    pub fn new() -> Self {
        Self {
            files: BTreeMap::new(),
        }
    }

    /// Seed a path with bytes (builder).
    pub fn with_file(mut self, path: &str, bytes: &[u8]) -> Self {
        self.files.insert(String::from(path), bytes.to_vec());
        self
    }
}

impl ArtifactReader for InMemoryArtifactReader {
    fn read_path(&self, path: &str) -> Result<Option<Vec<u8>>, RuntimeError> {
        Ok(self.files.get(path).cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_memory_reader_returns_seeded_bytes_and_none_for_missing() {
        let reader = InMemoryArtifactReader::new().with_file("/x", b"hi");
        assert_eq!(reader.read_path("/x").unwrap(), Some(b"hi".to_vec()));
        assert_eq!(reader.read_path("/y").unwrap(), None);
    }
}
