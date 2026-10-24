use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub id: String,
    pub created_at: String,
    pub model: String,
    pub workspace: String,
    pub messages: Vec<SavedMessage>,
}

pub struct SessionManager;

impl SessionManager {
    pub fn sessions_dir() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".orbit")
            .join("sessions")
    }

    pub fn save_session(
        id: &str,
        model: &str,
        workspace: &Path,
        messages: Vec<SavedMessage>,
    ) -> Result<PathBuf> {
        let dir = Self::sessions_dir();
        fs::create_dir_all(&dir)?;

        let created_at = chrono_timestamp();
        let snapshot = SessionSnapshot {
            id: id.to_string(),
            created_at,
            model: model.to_string(),
            workspace: workspace.display().to_string(),
            messages,
        };

        let file_path = dir.join(format!("{}.json", id));
        let json = serde_json::to_string_pretty(&snapshot)?;
        fs::write(&file_path, json)?;
        Ok(file_path)
    }

    pub fn load_session(id: &str) -> Result<SessionSnapshot> {
        let file_path = Self::sessions_dir().join(format!("{}.json", id));
        let json = fs::read_to_string(&file_path)?;
        let snapshot: SessionSnapshot = serde_json::from_str(&json)?;
        Ok(snapshot)
    }

    pub fn list_sessions() -> Result<Vec<(String, String, String, usize)>> {
        let dir = Self::sessions_dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }

        let mut out = Vec::new();
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                if let Ok(content) = fs::read_to_string(&path) {
                    if let Ok(snap) = serde_json::from_str::<SessionSnapshot>(&content) {
                        out.push((snap.id, snap.created_at, snap.model, snap.messages.len()));
                    }
                }
            }
        }
        out.sort_by(|a, b| b.1.cmp(&a.1)); // most recent first
        Ok(out)
    }
}

fn chrono_timestamp() -> String {
    let out = std::process::Command::new("date")
        .arg("+%Y-%m-%dT%H:%M:%S")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "2026-10-24T08:00:00".to_string());
    if out.is_empty() {
        "2026-10-24T08:00:00".to_string()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_session_serialization_and_deserialization() {
        let temp = tempdir().unwrap();
        let file_path = temp.path().join("test_session.json");

        let messages = vec![
            SavedMessage {
                role: "user".to_string(),
                content: "Implement feature X".to_string(),
            },
            SavedMessage {
                role: "assistant".to_string(),
                content: "Feature X implemented successfully.".to_string(),
            },
        ];

        let snapshot = SessionSnapshot {
            id: "sess_123".to_string(),
            created_at: "2026-10-24T08:00:00".to_string(),
            model: "anthropic/claude-3.5-sonnet".to_string(),
            workspace: "/home/user/project".to_string(),
            messages,
        };

        let json = serde_json::to_string(&snapshot).unwrap();
        fs::write(&file_path, &json).unwrap();

        let loaded: SessionSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.id, "sess_123");
        assert_eq!(loaded.messages.len(), 2);
        assert_eq!(loaded.messages[0].role, "user");
    }
}
