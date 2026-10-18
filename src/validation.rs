use std::path::{Path, PathBuf};
use crate::error::{OrbitError, Result};

/// Maximum file size to read (10MB)
const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024;

/// Maximum command length
const MAX_COMMAND_LENGTH: usize = 10_000;

/// Blocked dangerous commands
const BLOCKED_COMMANDS: &[&str] = &[
    "rm -rf /",
    "dd if=",
    "mkfs",
    "format",
    ":(){ :|:& };:",
    "chmod -R 777 /",
    "chown -R",
    "> /dev/sda",
];

pub fn validate_file_path(path: &str) -> Result<PathBuf> {
    let path = Path::new(path);
    
    // Prevent directory traversal
    if path.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return Err(OrbitError::InvalidInput(
            "Path traversal detected (..)".to_string()
        ));
    }
    
    // Prevent absolute paths to system directories
    if let Ok(canonical) = path.canonicalize() {
        let path_str = canonical.to_string_lossy();
        if path_str.starts_with("/etc") 
            || path_str.starts_with("/sys") 
            || path_str.starts_with("/proc")
            || path_str.starts_with("/dev") {
            return Err(OrbitError::InvalidInput(
                "Access to system directories denied".to_string()
            ));
        }
    }
    
    Ok(path.to_path_buf())
}

pub fn validate_file_size(path: &Path) -> Result<()> {
    let metadata = std::fs::metadata(path)?;
    
    if metadata.len() > MAX_FILE_SIZE {
        return Err(OrbitError::InvalidInput(format!(
            "File too large: {} bytes (max: {} bytes)",
            metadata.len(),
            MAX_FILE_SIZE
        )));
    }
    
    Ok(())
}

pub fn validate_command(cmd: &str) -> Result<String> {
    let trimmed = cmd.trim();
    
    if trimmed.is_empty() {
        return Err(OrbitError::InvalidInput("Empty command".to_string()));
    }
    
    if trimmed.len() > MAX_COMMAND_LENGTH {
        return Err(OrbitError::InvalidInput(format!(
            "Command too long: {} chars (max: {})",
            trimmed.len(),
            MAX_COMMAND_LENGTH
        )));
    }
    
    // Check for blocked patterns
    let lower = trimmed.to_lowercase();
    for blocked in BLOCKED_COMMANDS {
        if lower.contains(blocked) {
            return Err(OrbitError::InvalidInput(format!(
                "Blocked command pattern: {}",
                blocked
            )));
        }
    }
    
    Ok(trimmed.to_string())
}

pub fn validate_model_name(model: &str) -> Result<String> {
    let trimmed = model.trim();
    
    if trimmed.is_empty() {
        return Err(OrbitError::InvalidInput("Empty model name".to_string()));
    }
    
    if trimmed.len() > 200 {
        return Err(OrbitError::InvalidInput("Model name too long".to_string()));
    }
    
    // Basic format validation (provider/model-name)
    if !trimmed.contains('/') {
        return Err(OrbitError::InvalidInput(
            "Invalid model format (expected: provider/model)".to_string()
        ));
    }
    
    Ok(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_file_path_rejects_traversal() {
        assert!(validate_file_path("../../../etc/passwd").is_err());
        assert!(validate_file_path("./valid/path.rs").is_ok());
    }

    #[test]
    fn test_validate_command_blocks_dangerous() {
        assert!(validate_command("rm -rf /").is_err());
        assert!(validate_command("ls -la").is_ok());
    }

    #[test]
    fn test_validate_model_name() {
        assert!(validate_model_name("anthropic/claude-3").is_ok());
        assert!(validate_model_name("invalid").is_err());
        assert!(validate_model_name("").is_err());
    }
}
