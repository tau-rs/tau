//! Mounted-secret-directory credential provider. Reads
//! `<dir>/<key_map[id]>` and trims a single trailing newline. The DoD
//! CI provider (Kubernetes / Docker secret mounts).

use std::collections::BTreeMap;
use std::path::PathBuf;

use tau_ports::credential::{CredentialProvider, CredentialRequest, ResolvedCredential, Secret};
use tau_ports::CredentialError;

/// Resolves a credential by reading a file from a secrets directory.
/// The `key_map` maps a logical credential id to a filename in `dir`.
///
/// Asymmetry note: unlike the env provider (which treats an empty value as
/// absent → `Ok(None)`), `FileProvider` treats a present-but-empty file as a
/// present (empty) secret → `Ok(Some(..))`.
pub struct FileProvider {
    dir: PathBuf,
    key_map: BTreeMap<String, String>,
}

impl FileProvider {
    /// Provider name — single source of truth for both `name()` and the
    /// `source` field on resolved credentials, so the two can't drift.
    const NAME: &str = "file";

    /// Construct from a directory and an id→filename map.
    pub fn new(dir: PathBuf, key_map: BTreeMap<String, String>) -> Self {
        Self { dir, key_map }
    }
}

/// Trim a single trailing `\n` (and a preceding `\r`) — mounted secrets
/// often carry a trailing newline that is not part of the key.
fn trim_trailing_newline(mut bytes: Vec<u8>) -> Vec<u8> {
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
    }
    bytes
}

impl CredentialProvider for FileProvider {
    fn name(&self) -> &str {
        Self::NAME
    }

    async fn resolve(
        &self,
        req: &CredentialRequest,
    ) -> Result<Option<ResolvedCredential>, CredentialError> {
        let Some(filename) = self.key_map.get(req.id.as_str()) else {
            return Ok(None);
        };
        let path = self.dir.join(filename);
        match tokio::fs::read(&path).await {
            Ok(bytes) => Ok(Some(ResolvedCredential::new(
                Secret::from_bytes(trim_trailing_newline(bytes)),
                Self::NAME,
            ))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(CredentialError::Io {
                reason: format!("{}: {e}", path.display()),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tau_ports::credential::CredentialId;

    fn key_map() -> BTreeMap<String, String> {
        let mut m = BTreeMap::new();
        m.insert("anthropic_api_key".to_string(), "anthropic-key".to_string());
        m
    }

    fn req(id: &str) -> CredentialRequest {
        CredentialRequest::new(CredentialId::parse(id).unwrap())
    }

    #[tokio::test]
    async fn reads_and_trims_newline() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("anthropic-key"), b"sk-ant-file\n").unwrap();
        let p = FileProvider::new(dir.path().to_path_buf(), key_map());
        let got = p.resolve(&req("anthropic_api_key")).await.unwrap().unwrap();
        assert_eq!(got.secret.expose_str().unwrap(), "sk-ant-file");
        assert_eq!(got.source, "file");
    }

    #[tokio::test]
    async fn unmapped_id_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let p = FileProvider::new(dir.path().to_path_buf(), key_map());
        assert!(p.resolve(&req("other_id")).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn missing_file_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let p = FileProvider::new(dir.path().to_path_buf(), key_map());
        assert!(p
            .resolve(&req("anthropic_api_key"))
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn non_notfound_io_error_surfaces_as_io() {
        // Create a regular file, then use it as the "dir" so <file>/<filename>
        // resolves through a non-directory => ENOTDIR (not NotFound).
        let tmp = tempfile::tempdir().unwrap();
        let not_a_dir = tmp.path().join("iam_a_file");
        std::fs::write(&not_a_dir, b"x").unwrap();
        let p = FileProvider::new(not_a_dir, key_map());
        let res = p.resolve(&req("anthropic_api_key")).await;
        assert!(
            matches!(res, Err(tau_ports::CredentialError::Io { .. })),
            "got {:?}",
            res.map(|opt| opt.map(|c| c.source))
        );
    }

    #[tokio::test]
    async fn empty_file_yields_empty_secret_not_none() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("anthropic-key"), b"").unwrap();
        let p = FileProvider::new(dir.path().to_path_buf(), key_map());
        let got = p.resolve(&req("anthropic_api_key")).await.unwrap().unwrap();
        assert!(got.secret.is_empty());
    }

    #[tokio::test]
    async fn newline_only_file_yields_empty_secret() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("anthropic-key"), b"\n").unwrap();
        let p = FileProvider::new(dir.path().to_path_buf(), key_map());
        let got = p.resolve(&req("anthropic_api_key")).await.unwrap().unwrap();
        assert!(got.secret.is_empty());
    }

    #[tokio::test]
    async fn chain_env_then_file() {
        use crate::credentials::EnvProvider;
        use std::sync::Arc;
        use tau_ports::credential::CredentialChain;

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("anthropic-key"), b"from-file").unwrap();

        // env miss -> file hit
        let chain = CredentialChain::new()
            .with(Arc::new(EnvProvider::new(|_| None)))
            .with(Arc::new(FileProvider::new(
                dir.path().to_path_buf(),
                key_map(),
            )));
        let r = CredentialRequest::new(CredentialId::parse("anthropic_api_key").unwrap())
            .with_env_name("ANTHROPIC_API_KEY");
        let got = chain.resolve(&r).await.unwrap().unwrap();
        assert_eq!(got.secret.expose_str().unwrap(), "from-file");
        assert_eq!(got.source, "file");
    }
}
