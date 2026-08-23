//! Filesystem [`CheckpointStore`] for durable execution (ADR-0053).
//!
//! Per-turn snapshot files under `<scope>/.tau/runs/<run_id>/turn-<n>.json`,
//! one file per completed turn, keep-all. Writes go to `turn-<n>.json.tmp`
//! then atomic-rename to `turn-<n>.json`, so a crash mid-write leaves the
//! previous turn's file intact — the resume point, which is exactly the
//! at-least-once boundary A-minimal promises. `load_latest` returns the
//! highest-`n` file.
//!
//! Under `PerToolCall`, multiple checkpoints are written within one turn —
//! they share `turn-<n>.json` (last-write-wins; the atomic `.tmp`-rename
//! makes overwrite safe), and the turn-boundary write finalizes it with an
//! empty `pending_tool_uses`. `load_latest` still returns the highest-`n`
//! file, whose `pending_tool_uses` directs mid-turn resume.

use std::path::PathBuf;

use tau_ports::{CheckpointError, CheckpointStore, RunId, TurnCheckpoint};

/// A [`CheckpointStore`] backed by per-turn JSON snapshot files.
pub struct FileCheckpointStore {
    /// Project scope root; the runs directory is `<root>/.tau/runs`.
    scope_root: PathBuf,
}

impl FileCheckpointStore {
    /// Construct a store rooted at `scope_root` (the project scope dir).
    pub fn new(scope_root: impl Into<PathBuf>) -> Self {
        Self {
            scope_root: scope_root.into(),
        }
    }

    /// `<scope>/.tau/runs/<run_id>` — the directory holding one run's
    /// per-turn snapshot files.
    fn run_dir(&self, run_id: &str) -> PathBuf {
        self.scope_root.join(".tau").join("runs").join(run_id)
    }
}

fn io_err(e: std::io::Error) -> CheckpointError {
    CheckpointError::Io(e.to_string())
}

impl CheckpointStore for FileCheckpointStore {
    fn persist(&self, ckpt: &TurnCheckpoint) -> Result<(), CheckpointError> {
        let dir = self.run_dir(&ckpt.run_id);
        std::fs::create_dir_all(&dir).map_err(io_err)?;

        let json = serde_json::to_vec_pretty(ckpt)
            .map_err(|e| CheckpointError::Serialization(e.to_string()))?;

        // Atomic write: stage to `.tmp`, then rename onto the final name.
        let final_path = dir.join(format!("turn-{}.json", ckpt.turn));
        let tmp_path = dir.join(format!("turn-{}.json.tmp", ckpt.turn));
        std::fs::write(&tmp_path, &json).map_err(io_err)?;
        std::fs::rename(&tmp_path, &final_path).map_err(io_err)?;
        Ok(())
    }

    fn load_latest(&self, run_id: &RunId) -> Result<Option<TurnCheckpoint>, CheckpointError> {
        let dir = self.run_dir(run_id);
        if !dir.exists() {
            return Ok(None);
        }

        // Find the highest committed `turn-<n>.json`. A half-written
        // `turn-<n>.json.tmp` ends in `.tmp`, not `.json`, so it is ignored.
        let mut best: Option<(u32, PathBuf)> = None;
        for entry in std::fs::read_dir(&dir).map_err(io_err)? {
            let entry = entry.map_err(io_err)?;
            let file_name = entry.file_name();
            let name = file_name.to_string_lossy();
            let Some(stem) = name
                .strip_prefix("turn-")
                .and_then(|s| s.strip_suffix(".json"))
            else {
                continue;
            };
            let Ok(turn) = stem.parse::<u32>() else {
                continue;
            };
            if best.as_ref().is_none_or(|(b, _)| turn > *b) {
                best = Some((turn, entry.path()));
            }
        }

        let Some((_, path)) = best else {
            return Ok(None);
        };
        let bytes = std::fs::read(&path).map_err(io_err)?;
        let ckpt: TurnCheckpoint = serde_json::from_slice(&bytes)
            .map_err(|e| CheckpointError::Serialization(e.to_string()))?;
        Ok(Some(ckpt))
    }
}

impl tau_ports::orchestration::SuspensionStore for FileCheckpointStore {
    fn persist_suspension(
        &self,
        s: &tau_ports::orchestration::PipelineSuspension,
    ) -> Result<(), CheckpointError> {
        let dir = self.run_dir(&s.run_id);
        std::fs::create_dir_all(&dir).map_err(io_err)?;
        let json = serde_json::to_vec_pretty(s)
            .map_err(|e| CheckpointError::Serialization(e.to_string()))?;
        let final_path = dir.join("suspend.json");
        let tmp_path = dir.join("suspend.json.tmp");
        std::fs::write(&tmp_path, &json).map_err(io_err)?;
        std::fs::rename(&tmp_path, &final_path).map_err(io_err)?;
        Ok(())
    }

    fn load_suspension(
        &self,
        run_id: &RunId,
    ) -> Result<Option<tau_ports::orchestration::PipelineSuspension>, CheckpointError> {
        let path = self.run_dir(run_id).join("suspend.json");
        if !path.exists() {
            return Ok(None);
        }
        let bytes = std::fs::read(&path).map_err(io_err)?;
        let s = serde_json::from_slice(&bytes)
            .map_err(|e| CheckpointError::Serialization(e.to_string()))?;
        Ok(Some(s))
    }

    fn delete_suspension(&self, run_id: &RunId) -> Result<(), CheckpointError> {
        let path = self.run_dir(run_id).join("suspend.json");
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()), // idempotent
            Err(e) => Err(io_err(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tau_domain::{Address, Message, MessagePayload};

    fn ckpt(run_id: &str, turn: u32) -> TurnCheckpoint {
        TurnCheckpoint {
            run_id: run_id.to_string(),
            turn,
            history: vec![Message::new(
                Address::User,
                Address::System,
                MessagePayload::Text {
                    content: format!("turn {turn}"),
                },
            )],
            input_tokens: turn as u64 * 10,
            output_tokens: turn as u64 * 5,
            pending_tool_uses: vec![],
        }
    }

    #[test]
    fn persist_then_load_latest_round_trips_highest_turn() {
        let tmp = tempfile::tempdir().unwrap();
        let store = FileCheckpointStore::new(tmp.path());

        // No checkpoints yet.
        assert!(store.load_latest(&"run-1".to_string()).unwrap().is_none());

        store.persist(&ckpt("run-1", 1)).unwrap();
        store.persist(&ckpt("run-1", 2)).unwrap();
        store.persist(&ckpt("run-1", 3)).unwrap();

        let latest = store.load_latest(&"run-1".to_string()).unwrap().unwrap();
        assert_eq!(latest.turn, 3);
        assert_eq!(latest.input_tokens, 30);
        assert_eq!(latest.history.len(), 1);

        // All three turn files were kept (keep-all).
        let dir = tmp.path().join(".tau/runs/run-1");
        let files: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(files.iter().filter(|f| f.ends_with(".json")).count(), 3);
        assert!(
            files.iter().all(|f| !f.ends_with(".tmp")),
            "no temp files should remain: {files:?}"
        );
    }

    #[test]
    fn load_latest_isolates_by_run_id() {
        let tmp = tempfile::tempdir().unwrap();
        let store = FileCheckpointStore::new(tmp.path());
        store.persist(&ckpt("run-a", 5)).unwrap();
        store.persist(&ckpt("run-b", 1)).unwrap();
        assert_eq!(
            store
                .load_latest(&"run-a".to_string())
                .unwrap()
                .unwrap()
                .turn,
            5
        );
        assert_eq!(
            store
                .load_latest(&"run-b".to_string())
                .unwrap()
                .unwrap()
                .turn,
            1
        );
    }

    #[test]
    fn suspension_round_trips_on_disk() {
        use std::collections::BTreeMap;
        use tau_ports::orchestration::{PipelineSuspension, SuspensionStore};
        let tmp = tempfile::tempdir().unwrap();
        let store = FileCheckpointStore::new(tmp.path());
        assert!(store
            .load_suspension(&"run-1".to_string())
            .unwrap()
            .is_none());

        let mut outputs = BTreeMap::new();
        outputs.insert("seed".to_string(), serde_json::json!("GO"));
        let s = PipelineSuspension {
            run_id: "run-1".into(),
            resume_signal: "approved".into(),
            step_cursor: 1,
            step_id: "pause".into(),
            ir_digest: "sha256:abc".into(),
            outputs,
            attempts: BTreeMap::new(),
        };
        store.persist_suspension(&s).unwrap();

        let got = store
            .load_suspension(&"run-1".to_string())
            .unwrap()
            .unwrap();
        assert_eq!(got, s);
        assert!(tmp.path().join(".tau/runs/run-1/suspend.json").exists());
    }

    #[test]
    fn delete_suspension_removes_file_and_is_idempotent() {
        use std::collections::BTreeMap;
        use tau_ports::orchestration::{PipelineSuspension, SuspensionStore};
        let tmp = tempfile::tempdir().unwrap();
        let store = FileCheckpointStore::new(tmp.path());

        // Deleting a suspension that was never persisted is Ok (idempotent).
        store.delete_suspension(&"run-1".to_string()).unwrap();

        let s = PipelineSuspension {
            run_id: "run-1".into(),
            resume_signal: "approved".into(),
            step_cursor: 1,
            step_id: "pause".into(),
            ir_digest: "sha256:abc".into(),
            outputs: BTreeMap::new(),
            attempts: BTreeMap::new(),
        };
        store.persist_suspension(&s).unwrap();
        let path = tmp.path().join(".tau/runs/run-1/suspend.json");
        assert!(path.exists());

        store.delete_suspension(&"run-1".to_string()).unwrap();
        assert!(!path.exists());
        assert!(store
            .load_suspension(&"run-1".to_string())
            .unwrap()
            .is_none());

        // Deleting again (already gone) is still Ok.
        store.delete_suspension(&"run-1".to_string()).unwrap();
    }
}
