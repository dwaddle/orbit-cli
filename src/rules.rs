use std::fs;
use std::path::{Path, PathBuf};

/// Candidate filenames for project rules, in order of priority.
const RULE_CANDIDATES: &[&str] = &[
    ".orbitrules",
    "AGENTS.md",
    ".agents/AGENTS.md",
    ".orbit/rules.md",
];

/// Result of searching for project-level rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRules {
    pub path: PathBuf,
    pub content: String,
}

impl ProjectRules {
    /// Search for project rules starting from `start_dir` and traversing upward to the root or git boundary.
    pub fn discover(start_dir: &Path) -> Option<Self> {
        let mut current = start_dir.to_path_buf();
        if !current.is_dir() {
            if let Some(parent) = current.parent() {
                current = parent.to_path_buf();
            }
        }

        loop {
            for candidate in RULE_CANDIDATES {
                let candidate_path = current.join(candidate);
                if candidate_path.is_file() {
                    if let Ok(content) = fs::read_to_string(&candidate_path) {
                        let trimmed = content.trim();
                        if !trimmed.is_empty() {
                            return Some(Self {
                                path: candidate_path,
                                content: trimmed.to_string(),
                            });
                        }
                    }
                }
            }

            // Stop if at root or parent does not exist
            match current.parent() {
                Some(parent) if parent != current => {
                    // If current directory contains a .git folder and no rules found here, stop searching upwards
                    if current.join(".git").exists() {
                        break;
                    }
                    current = parent.to_path_buf();
                }
                _ => break,
            }
        }

        None
    }

    /// Formats the loaded rules into a prompt section for system prompt injection.
    pub fn format_for_prompt(&self) -> String {
        format!(
            "\n## PROJECT-SPECIFIC RULES & CONSTRAINTS (Loaded from `{}`):\n{}\n",
            self.path.display(),
            self.content
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;

    #[test]
    fn test_discover_orbitrules_in_current_dir() {
        let temp_dir = std::env::temp_dir().join("orbit_test_rules_1");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let rules_file = temp_dir.join(".orbitrules");
        let mut f = File::create(&rules_file).unwrap();
        writeln!(f, "# Test Project Rules\n- Enforce strict typing.").unwrap();

        let discovered = ProjectRules::discover(&temp_dir);
        assert!(discovered.is_some());
        let rules = discovered.unwrap();
        assert_eq!(rules.path, rules_file);
        assert!(rules.content.contains("Enforce strict typing"));
        assert!(rules.format_for_prompt().contains("PROJECT-SPECIFIC RULES"));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_discover_agents_md_fallback() {
        let temp_dir = std::env::temp_dir().join("orbit_test_rules_2");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let agents_file = temp_dir.join("AGENTS.md");
        let mut f = File::create(&agents_file).unwrap();
        writeln!(f, "# Agent instructions for repo").unwrap();

        let discovered = ProjectRules::discover(&temp_dir);
        assert!(discovered.is_some());
        assert_eq!(discovered.unwrap().path, agents_file);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_discover_parent_dir_lookup() {
        let parent = std::env::temp_dir().join("orbit_test_parent_rules");
        let child = parent.join("sub_dir").join("deep");
        let _ = fs::remove_dir_all(&parent);
        fs::create_dir_all(&child).unwrap();

        let rules_file = parent.join(".orbitrules");
        fs::write(&rules_file, "Root rules").unwrap();

        let discovered = ProjectRules::discover(&child);
        assert!(discovered.is_some());
        assert_eq!(discovered.unwrap().path, rules_file);

        let _ = fs::remove_dir_all(&parent);
    }

    #[test]
    fn test_discover_none_when_empty() {
        let temp_dir = std::env::temp_dir().join("orbit_test_rules_empty");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let discovered = ProjectRules::discover(&temp_dir);
        assert!(discovered.is_none());

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
