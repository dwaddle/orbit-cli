use regex::Regex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolPolicy {
    Auto,
    Ask,
    Deny,
}

impl std::str::FromStr for ToolPolicy {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().trim() {
            "auto" | "allow" => Ok(Self::Auto),
            "ask" | "prompt" => Ok(Self::Ask),
            "deny" | "block" => Ok(Self::Deny),
            _ => Err(()),
        }
    }
}

impl ToolPolicy {
    pub fn parse_policy(s: &str) -> Option<Self> {
        s.parse().ok()
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Ask => "ask",
            Self::Deny => "deny",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ToolPermissions {
    policies: HashMap<String, ToolPolicy>,
    default_policy: ToolPolicy,
}

impl Default for ToolPermissions {
    fn default() -> Self {
        let mut policies = HashMap::new();
        policies.insert("view_file".to_string(), ToolPolicy::Auto);
        policies.insert("write_file".to_string(), ToolPolicy::Ask);
        policies.insert("run_command".to_string(), ToolPolicy::Ask);
        policies.insert("invoke_subagent".to_string(), ToolPolicy::Auto);
        Self {
            policies,
            default_policy: ToolPolicy::Ask,
        }
    }
}

impl ToolPermissions {
    pub fn new(default_policy: ToolPolicy) -> Self {
        Self {
            policies: HashMap::new(),
            default_policy,
        }
    }

    pub fn set_policy(&mut self, tool: &str, policy: ToolPolicy) {
        self.policies.insert(tool.to_string(), policy);
    }

    pub fn check_permission(&self, tool: &str) -> ToolPolicy {
        self.policies
            .get(tool)
            .copied()
            .unwrap_or(self.default_policy)
    }

    pub fn list_policies(&self) -> Vec<(&str, ToolPolicy)> {
        let mut list: Vec<(&str, ToolPolicy)> = self
            .policies
            .iter()
            .map(|(k, v)| (k.as_str(), *v))
            .collect();
        list.sort_by_key(|(k, _)| *k);
        list
    }
}

pub struct WorkspaceSandbox;

impl WorkspaceSandbox {
    pub fn sanitize_path(target: &Path, workspace_root: &Path) -> Result<PathBuf, String> {
        let root = workspace_root
            .canonicalize()
            .unwrap_or_else(|_| workspace_root.to_path_buf());

        let full_path = if target.is_absolute() {
            target.to_path_buf()
        } else {
            root.join(target)
        };

        let canonical = if full_path.exists() {
            full_path.canonicalize().map_err(|e| e.to_string())?
        } else {
            // For not-yet-existing files, canonicalize the parent directory
            let parent = full_path.parent().unwrap_or(&root);
            let canonical_parent = parent.canonicalize().unwrap_or_else(|_| root.clone());
            if let Some(file_name) = full_path.file_name() {
                canonical_parent.join(file_name)
            } else {
                canonical_parent
            }
        };

        if canonical.starts_with(&root) {
            Ok(canonical)
        } else {
            Err(format!(
                "Access denied: path '{}' escapes workspace boundary '{}'",
                target.display(),
                workspace_root.display()
            ))
        }
    }
}

pub struct SecretRedactor {
    patterns: Vec<Regex>,
}

impl Default for SecretRedactor {
    fn default() -> Self {
        let patterns = vec![
            Regex::new(r"(?i)(Bearer\s+)[a-zA-Z0-9_\-\.]{20,}").unwrap(),
            Regex::new(r"sk-[a-zA-Z0-9]{20,}").unwrap(),
            Regex::new(r"ghp_[a-zA-Z0-9]{20,}").unwrap(),
            Regex::new(r"gho_[a-zA-Z0-9]{20,}").unwrap(),
            Regex::new(r"github_pat_[a-zA-Z0-9_]{22,}").unwrap(),
            Regex::new(r"AKIA[0-9A-Z]{16}").unwrap(),
            Regex::new(r#"(?i)(api[_-]?key\s*[:=]\s*['"]?)[a-zA-Z0-9_-]{20,}['"]?"#).unwrap(),
            Regex::new(r#"(?i)(token\s*[:=]\s*['"]?)[a-zA-Z0-9_-]{20,}['"]?"#).unwrap(),
        ];
        Self { patterns }
    }
}

impl SecretRedactor {
    pub fn redact(&self, input: &str) -> String {
        // Fast-path heuristic: avoid regex evaluations if no token indicators exist
        let lower = input.to_ascii_lowercase();
        let might_contain_secret = lower.contains("bearer")
            || lower.contains("sk-")
            || lower.contains("ghp_")
            || lower.contains("gho_")
            || lower.contains("github_pat_")
            || input.contains("AKIA")
            || lower.contains("key")
            || lower.contains("token");

        if !might_contain_secret {
            return input.to_string();
        }

        let mut result = input.to_string();
        for pattern in &self.patterns {
            if pattern.is_match(&result) {
                result = pattern
                    .replace_all(&result, |caps: &regex::Captures| {
                        if let Some(prefix) = caps.get(1) {
                            format!("{}[REDACTED_SECRET]", prefix.as_str())
                        } else {
                            "[REDACTED_SECRET]".to_string()
                        }
                    })
                    .to_string();
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_tool_permissions_default_and_custom() {
        let mut perms = ToolPermissions::default();
        assert_eq!(perms.check_permission("view_file"), ToolPolicy::Auto);
        assert_eq!(perms.check_permission("write_file"), ToolPolicy::Ask);
        assert_eq!(perms.check_permission("unknown_tool"), ToolPolicy::Ask);

        perms.set_policy("write_file", ToolPolicy::Auto);
        perms.set_policy("run_command", ToolPolicy::Deny);
        assert_eq!(perms.check_permission("write_file"), ToolPolicy::Auto);
        assert_eq!(perms.check_permission("run_command"), ToolPolicy::Deny);
    }

    #[test]
    fn test_workspace_sandbox_boundary() {
        let temp = tempdir().unwrap();
        let ws = temp.path();

        let allowed = ws.join("src/main.rs");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::write(&allowed, "fn main() {}").unwrap();

        let res = WorkspaceSandbox::sanitize_path(Path::new("src/main.rs"), ws);
        assert!(res.is_ok());

        let escaping = Path::new("../../etc/passwd");
        let err = WorkspaceSandbox::sanitize_path(escaping, ws);
        assert!(err.is_err());
        assert!(err.unwrap_err().contains("escapes workspace boundary"));
    }

    #[test]
    fn test_secret_redactor() {
        let redactor = SecretRedactor::default();

        let text = "My key is sk-1234567890abcdef1234567890 and token: 'ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ123456'";
        let redacted = redactor.redact(text);
        assert!(!redacted.contains("sk-1234567890"));
        assert!(!redacted.contains("ghp_ABCDEF"));
        assert!(redacted.contains("[REDACTED_SECRET]"));

        let auth_header = "Authorization: Bearer secret_access_token_value_long_enough_12345";
        let redacted_auth = redactor.redact(auth_header);
        assert!(redacted_auth.contains("Bearer [REDACTED_SECRET]"));
    }
}
