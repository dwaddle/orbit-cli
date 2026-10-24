use anyhow::{anyhow, Result};
use std::env;
use std::path::Path;
use std::process::Command;

pub struct ExternalEditor;

impl ExternalEditor {
    pub fn get_editor_command() -> String {
        env::var("EDITOR")
            .or_else(|_| env::var("VISUAL"))
            .unwrap_or_else(|_| {
                if Command::new("nano").arg("--version").output().is_ok() {
                    "nano".to_string()
                } else if Command::new("vim").arg("--version").output().is_ok() {
                    "vim".to_string()
                } else {
                    "vi".to_string()
                }
            })
    }

    pub fn edit_file(path: &Path, workspace: &Path) -> Result<()> {
        let full_path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            workspace.join(path)
        };

        if !full_path.exists() {
            if let Some(parent) = full_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&full_path, "")?;
        }

        let editor = Self::get_editor_command();
        let status = Command::new(&editor)
            .arg(&full_path)
            .status()
            .map_err(|e| anyhow!("Failed to spawn editor '{}': {}", editor, e))?;

        if status.success() {
            Ok(())
        } else {
            Err(anyhow!("Editor '{}' exited with error status", editor))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_editor_fallback() {
        let editor = ExternalEditor::get_editor_command();
        assert!(!editor.is_empty());
    }
}
