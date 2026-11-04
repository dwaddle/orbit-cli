use anyhow::{anyhow, Result};
use colored::Colorize;
use std::fs;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemverBump {
    None,
    Patch,
    Minor,
    Major,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitEntry {
    pub hash: String,
    pub commit_type: String,
    pub scope: Option<String>,
    pub description: String,
    pub is_breaking: bool,
}

#[derive(Debug, Clone)]
pub struct ReleasePlan {
    pub current_version: String,
    pub next_version: String,
    pub bump: SemverBump,
    pub commits: Vec<CommitEntry>,
    pub changelog_section: String,
}

pub struct ReleaseOrchestrator;

impl ReleaseOrchestrator {
    pub fn get_latest_git_tag(workspace: &Path) -> Option<String> {
        let output = Command::new("git")
            .args(["describe", "--tags", "--abbrev=0"])
            .current_dir(workspace)
            .output()
            .ok()?;

        if output.status.success() {
            let tag = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !tag.is_empty() {
                Some(tag)
            } else {
                None
            }
        } else {
            None
        }
    }

    pub fn get_commits_since_tag(workspace: &Path, tag: Option<&str>) -> Result<Vec<CommitEntry>> {
        let mut cmd = Command::new("git");
        cmd.current_dir(workspace);

        if let Some(t) = tag {
            cmd.args(["log", &format!("{}..HEAD", t), "--pretty=format:%h|%s"]);
        } else {
            cmd.args(["log", "-n", "50", "--pretty=format:%h|%s"]);
        }

        let output = cmd.output()?;
        if !output.status.success() {
            return Err(anyhow!(
                "Failed to read git commits: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut entries = Vec::new();

        for line in stdout.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some((hash, subject)) = line.split_once('|') {
                entries.push(Self::parse_conventional_commit(hash.trim(), subject.trim()));
            }
        }

        Ok(entries)
    }

    pub fn parse_conventional_commit(hash: &str, subject: &str) -> CommitEntry {
        let is_breaking = subject.contains("BREAKING CHANGE") || subject.contains("!:");

        // e.g. feat(tui): add split view
        let (raw_type, desc) = match subject.split_once(':') {
            Some((t, d)) => (t.trim(), d.trim()),
            None => ("other", subject),
        };

        let (commit_type, scope) = if let Some(open) = raw_type.find('(') {
            if let Some(close) = raw_type.find(')') {
                let ct = &raw_type[..open];
                let sc = &raw_type[open + 1..close];
                (ct.trim_end_matches('!'), Some(sc.to_string()))
            } else {
                (raw_type.trim_end_matches('!'), None)
            }
        } else {
            (raw_type.trim_end_matches('!'), None)
        };

        CommitEntry {
            hash: hash.to_string(),
            commit_type: commit_type.to_lowercase(),
            scope,
            description: desc.to_string(),
            is_breaking,
        }
    }

    pub fn compute_bump(commits: &[CommitEntry]) -> SemverBump {
        if commits.is_empty() {
            return SemverBump::None;
        }

        let mut bump = SemverBump::Patch;

        for c in commits {
            if c.is_breaking {
                return SemverBump::Major;
            }
            if c.commit_type == "feat" {
                bump = SemverBump::Minor;
            }
        }

        bump
    }

    pub fn bump_semver(current: &str, bump: SemverBump) -> Result<String> {
        let clean = current.trim_start_matches('v');
        let parts: Vec<&str> = clean.split('.').collect();
        if parts.len() != 3 {
            return Err(anyhow!("Invalid semver format: {}", current));
        }

        let major: u64 = parts[0].parse()?;
        let minor: u64 = parts[1].parse()?;
        let patch: u64 = parts[2].parse()?;

        let next = match bump {
            SemverBump::None => format!("{}.{}.{}", major, minor, patch),
            SemverBump::Patch => format!("{}.{}.{}", major, minor, patch + 1),
            SemverBump::Minor => format!("{}.{}.0", major, minor + 1),
            SemverBump::Major => format!("{}.0.0", major + 1),
        };

        Ok(next)
    }

    pub fn read_cargo_version(workspace: &Path) -> Result<String> {
        let cargo_toml = workspace.join("Cargo.toml");
        if !cargo_toml.exists() {
            return Err(anyhow!("Cargo.toml not found in {}", workspace.display()));
        }
        let content = fs::read_to_string(&cargo_toml)?;
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("version =") {
                if let Some(val) = trimmed.split('=').nth(1) {
                    let v = val.trim().trim_matches('"').trim_matches('\'');
                    return Ok(v.to_string());
                }
            }
        }
        Err(anyhow!("Version key not found in Cargo.toml"))
    }

    pub fn generate_changelog_section(
        version: &str,
        date_str: &str,
        commits: &[CommitEntry],
    ) -> String {
        let mut section = format!("## [{}] - {}\n\n", version, date_str);

        let mut added = Vec::new();
        let mut fixed = Vec::new();
        let mut changed = Vec::new();
        let mut other = Vec::new();

        for c in commits {
            let item = match &c.scope {
                Some(s) => format!("- **{}**: {} (`{}`)", s, c.description, c.hash),
                None => format!("- {} (`{}`)", c.description, c.hash),
            };

            match c.commit_type.as_str() {
                "feat" => added.push(item),
                "fix" => fixed.push(item),
                "refactor" | "perf" | "chore" => changed.push(item),
                _ => other.push(item),
            }
        }

        if !added.is_empty() {
            section.push_str("### Added\n");
            for a in added {
                section.push_str(&format!("{}\n", a));
            }
            section.push('\n');
        }

        if !fixed.is_empty() {
            section.push_str("### Fixed\n");
            for f in fixed {
                section.push_str(&format!("{}\n", f));
            }
            section.push('\n');
        }

        if !changed.is_empty() {
            section.push_str("### Changed\n");
            for ch in changed {
                section.push_str(&format!("{}\n", ch));
            }
            section.push('\n');
        }

        if !other.is_empty() {
            section.push_str("### Maintenance\n");
            for o in other {
                section.push_str(&format!("{}\n", o));
            }
            section.push('\n');
        }

        section
    }

    pub fn plan_release(workspace: &Path) -> Result<ReleasePlan> {
        let current_version = Self::read_cargo_version(workspace)?;
        let latest_tag = Self::get_latest_git_tag(workspace);
        let commits = Self::get_commits_since_tag(workspace, latest_tag.as_deref())?;

        let bump = Self::compute_bump(&commits);
        let next_version = Self::bump_semver(&current_version, bump)?;
        let today = chrono_lite_today();
        let changelog_section = Self::generate_changelog_section(&next_version, &today, &commits);

        Ok(ReleasePlan {
            current_version,
            next_version,
            bump,
            commits,
            changelog_section,
        })
    }

    pub fn apply_release(workspace: &Path, plan: &ReleasePlan) -> Result<()> {
        if plan.bump == SemverBump::None {
            return Err(anyhow!("No version bump required (no new commits found)"));
        }

        // 1. Update Cargo.toml
        let cargo_toml = workspace.join("Cargo.toml");
        if cargo_toml.exists() {
            let content = fs::read_to_string(&cargo_toml)?;
            let old_ver_line = format!("version = \"{}\"", plan.current_version);
            let new_ver_line = format!("version = \"{}\"", plan.next_version);
            if content.contains(&old_ver_line) {
                let updated = content.replacen(&old_ver_line, &new_ver_line, 1);
                fs::write(&cargo_toml, updated)?;
            }
        }

        // 2. Prepend to CHANGELOG.md
        let changelog_path = workspace.join("CHANGELOG.md");
        let new_content = if changelog_path.exists() {
            let existing = fs::read_to_string(&changelog_path)?;
            if existing.starts_with("# Changelog") {
                let rest = existing.trim_start_matches("# Changelog").trim_start();
                format!("# Changelog\n\n{}\n{}", plan.changelog_section.trim(), rest)
            } else {
                format!("{}\n{}", plan.changelog_section, existing)
            }
        } else {
            format!("# Changelog\n\nAll notable changes to this project will be documented in this file.\n\nThe format is based on [Keep a Changelog](https://keepachangelog.com/).\n\n{}", plan.changelog_section)
        };

        fs::write(&changelog_path, new_content)?;

        Ok(())
    }

    pub fn format_release_banner(plan: &ReleasePlan) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "{}\n",
            "=== SEMANTIC RELEASE PLAN ===".bold().cyan()
        ));
        out.push_str(&format!("  Current Version:  v{}\n", plan.current_version));
        out.push_str(&format!("  Bump Type:        {:?}\n", plan.bump));
        out.push_str(&format!(
            "  Next Version:     {}\n",
            format!("v{}", plan.next_version).green().bold()
        ));
        out.push_str(&format!("  Commits Analyzed: {}\n\n", plan.commits.len()));
        out.push_str(&format!(
            "{}\n",
            "--- Preview CHANGELOG.md Section ---".bold().yellow()
        ));
        out.push_str(&plan.changelog_section);
        out
    }
}

fn chrono_lite_today() -> String {
    let output = Command::new("date")
        .arg("+%Y-%m-%d")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "2026-11-04".to_string());
    if output.is_empty() {
        "2026-11-04".to_string()
    } else {
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_conventional_commit_parser() {
        let c1 =
            ReleaseOrchestrator::parse_conventional_commit("a1b2c3d", "feat(tui): add split view");
        assert_eq!(c1.commit_type, "feat");
        assert_eq!(c1.scope, Some("tui".to_string()));
        assert_eq!(c1.description, "add split view");
        assert!(!c1.is_breaking);

        let c2 = ReleaseOrchestrator::parse_conventional_commit(
            "e4f5g6h",
            "fix!: remove deprecated api",
        );
        assert_eq!(c2.commit_type, "fix");
        assert!(c2.is_breaking);
    }

    #[test]
    fn test_compute_bump_logic() {
        let commits_patch = vec![
            ReleaseOrchestrator::parse_conventional_commit("111", "fix: fix crash"),
            ReleaseOrchestrator::parse_conventional_commit("222", "docs: update manual"),
        ];
        assert_eq!(
            ReleaseOrchestrator::compute_bump(&commits_patch),
            SemverBump::Patch
        );

        let commits_minor = vec![
            ReleaseOrchestrator::parse_conventional_commit("111", "fix: fix crash"),
            ReleaseOrchestrator::parse_conventional_commit("222", "feat: new command"),
        ];
        assert_eq!(
            ReleaseOrchestrator::compute_bump(&commits_minor),
            SemverBump::Minor
        );

        let commits_major = vec![
            ReleaseOrchestrator::parse_conventional_commit("111", "feat: new command"),
            ReleaseOrchestrator::parse_conventional_commit("222", "feat!: breaking change"),
        ];
        assert_eq!(
            ReleaseOrchestrator::compute_bump(&commits_major),
            SemverBump::Major
        );
    }

    #[test]
    fn test_bump_semver_math() {
        assert_eq!(
            ReleaseOrchestrator::bump_semver("0.2.0", SemverBump::Patch).unwrap(),
            "0.2.1"
        );
        assert_eq!(
            ReleaseOrchestrator::bump_semver("0.2.0", SemverBump::Minor).unwrap(),
            "0.3.0"
        );
        assert_eq!(
            ReleaseOrchestrator::bump_semver("0.2.0", SemverBump::Major).unwrap(),
            "1.0.0"
        );
    }

    #[test]
    fn test_generate_changelog_section() {
        let commits = vec![
            ReleaseOrchestrator::parse_conventional_commit(
                "abc1234",
                "feat(cli): add /release command",
            ),
            ReleaseOrchestrator::parse_conventional_commit("def5678", "fix: resolve nil pointer"),
        ];
        let sec = ReleaseOrchestrator::generate_changelog_section("0.3.0", "2026-11-04", &commits);
        assert!(sec.contains("## [0.3.0] - 2026-11-04"));
        assert!(sec.contains("### Added"));
        assert!(sec.contains("add /release command"));
        assert!(sec.contains("### Fixed"));
        assert!(sec.contains("resolve nil pointer"));
    }
}
