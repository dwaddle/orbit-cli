use anyhow::{anyhow, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub id: usize,
    pub path: PathBuf,
    pub previous_bytes: Option<Vec<u8>>,
    pub existed_before: bool,
    pub action: String,
    pub timestamp: SystemTime,
}

#[derive(Debug, Clone)]
pub struct RollbackResult {
    pub id: usize,
    pub path: PathBuf,
    pub restored_bytes: usize,
    pub was_deleted: bool,
    pub action: String,
}

#[derive(Clone, Default)]
pub struct CheckpointManager {
    history: Arc<Mutex<Vec<Snapshot>>>,
}

impl CheckpointManager {
    pub fn new() -> Self {
        Self {
            history: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Records a snapshot of `path` before modification.
    pub fn record_before_modify(&self, path: &Path, action: &str) -> usize {
        let mut guard = self.history.lock().unwrap();
        let id = guard.len() + 1;
        let existed_before = path.exists();
        let previous_bytes = if existed_before {
            fs::read(path).ok()
        } else {
            None
        };

        guard.push(Snapshot {
            id,
            path: path.to_path_buf(),
            previous_bytes,
            existed_before,
            action: action.to_string(),
            timestamp: SystemTime::now(),
        });

        id
    }

    /// Rolls back the most recent snapshot.
    pub fn rollback_last(&self) -> Result<RollbackResult> {
        let mut guard = self.history.lock().unwrap();
        let snapshot = guard
            .pop()
            .ok_or_else(|| anyhow!("No checkpoints available to undo"))?;

        if snapshot.existed_before {
            if let Some(bytes) = snapshot.previous_bytes {
                if let Some(parent) = snapshot.path.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                fs::write(&snapshot.path, &bytes)?;
                Ok(RollbackResult {
                    id: snapshot.id,
                    path: snapshot.path,
                    restored_bytes: bytes.len(),
                    was_deleted: false,
                    action: snapshot.action,
                })
            } else {
                Err(anyhow!(
                    "Cannot restore checkpoint #{}: original file existed but could not be captured",
                    snapshot.id
                ))
            }
        } else {
            // File did not exist before this snapshot; delete newly created file
            let was_deleted = if snapshot.path.exists() {
                fs::remove_file(&snapshot.path).is_ok()
            } else {
                false
            };
            Ok(RollbackResult {
                id: snapshot.id,
                path: snapshot.path,
                restored_bytes: 0,
                was_deleted,
                action: snapshot.action,
            })
        }
    }

    /// Lists recent checkpoints.
    pub fn list_checkpoints(&self) -> Vec<Snapshot> {
        let guard = self.history.lock().unwrap();
        guard.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_checkpoint_and_rollback_existing_file() {
        let tmp = std::env::temp_dir().join(format!("orbit_chk_test_{}", std::process::id()));
        fs::write(&tmp, "initial content").unwrap();

        let mgr = CheckpointManager::new();
        let id = mgr.record_before_modify(&tmp, "write_file");
        assert_eq!(id, 1);

        // Modify file
        fs::write(&tmp, "modified content").unwrap();
        assert_eq!(fs::read_to_string(&tmp).unwrap(), "modified content");

        // Rollback
        let res = mgr.rollback_last().unwrap();
        assert_eq!(res.id, 1);
        assert!(!res.was_deleted);
        assert_eq!(fs::read_to_string(&tmp).unwrap(), "initial content");

        let _ = fs::remove_file(&tmp);
    }

    #[test]
    fn test_checkpoint_and_rollback_new_file() {
        let tmp = std::env::temp_dir().join(format!("orbit_new_file_{}", std::process::id()));
        if tmp.exists() {
            let _ = fs::remove_file(&tmp);
        }

        let mgr = CheckpointManager::new();
        // Record before creating file
        mgr.record_before_modify(&tmp, "write_file");

        // Create file
        fs::write(&tmp, "brand new file").unwrap();
        assert!(tmp.exists());

        // Rollback should delete it
        let res = mgr.rollback_last().unwrap();
        assert!(res.was_deleted);
        assert!(!tmp.exists());
    }

    #[test]
    fn test_empty_rollback_errors() {
        let mgr = CheckpointManager::new();
        assert!(mgr.rollback_last().is_err());
    }

    #[test]
    fn test_checkpoint_and_rollback_binary_file() {
        let tmp = std::env::temp_dir().join(format!("orbit_bin_chk_{}", std::process::id()));
        let binary_payload: Vec<u8> = vec![0x00, 0xFF, 0xFE, 0xAA, 0xBB, 0xCC, 0x80, 0x90];
        fs::write(&tmp, &binary_payload).unwrap();

        let mgr = CheckpointManager::new();
        mgr.record_before_modify(&tmp, "write_file");

        // Overwrite binary file
        fs::write(&tmp, b"corrupted bytes").unwrap();
        assert_eq!(fs::read(&tmp).unwrap(), b"corrupted bytes");

        // Rollback restores original non-UTF8 binary bytes without deleting
        let res = mgr.rollback_last().unwrap();
        assert_eq!(res.restored_bytes, binary_payload.len());
        assert!(!res.was_deleted);
        assert_eq!(fs::read(&tmp).unwrap(), binary_payload);

        let _ = fs::remove_file(&tmp);
    }
}
