use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct ExportTurn {
    pub role: String,
    pub content: String,
}

pub fn export_session_to_org(
    history: &[ExportTurn],
    total_tokens: u64,
    cached_tokens: u64,
    cost_usd: f64,
    workspace_dir: &Path,
    active_model: &str,
    target_path: Option<&Path>,
) -> Result<PathBuf> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let dest_file = match target_path {
        Some(p) => p.to_path_buf(),
        None => {
            let default_dir = dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("Org")
                .join("sessions");
            fs::create_dir_all(&default_dir)?;
            let proj_name = workspace_dir
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_else(|| "project".to_string());
            default_dir.join(format!("orbit_{}_{}.org", proj_name, now))
        }
    };

    let cache_pct = if total_tokens > 0 {
        (cached_tokens as f64 / total_tokens as f64) * 100.0
    } else {
        0.0
    };

    let mut doc = String::new();
    doc.push_str("#+TITLE: Orbit AI Coding Session Report\n");
    doc.push_str(&format!("#+DATE: [{}]\n", format_org_date(now)));
    doc.push_str("#+AUTHOR: Orbit CLI (Rust) & Rene\n");
    doc.push_str("#+FILETAGS: :orbit:ai:session:coding:report:\n");
    doc.push_str("#+STARTUP: showeverything\n\n");

    doc.push_str("* 🛰️ Session Executive Summary\n");
    doc.push_str(&format!("- *Workspace:* ={} =\n", workspace_dir.display()));
    doc.push_str(&format!("- *Active Model:* ={} =\n", active_model));
    doc.push_str(&format!("- *Total Turns:* {}\n", history.len()));
    doc.push_str(&format!("- *Total Tokens:* {}\n", total_tokens));
    doc.push_str(&format!(
        "- *Cached Tokens:* {} ({:.1}% cache discount)\n",
        cached_tokens, cache_pct
    ));
    doc.push_str(&format!("- *Total Cost:* ${:.4} USD\n\n", cost_usd));

    doc.push_str("* 💬 Conversation & Implementation Log\n\n");

    let redactor = crate::security::SecretRedactor::default();
    let mut turn_counter = 1;
    for msg in history {
        let clean_content = redactor.redact(&msg.content);
        if msg.role == "user" {
            doc.push_str(&format!("** Turn {}: User Request\n", turn_counter));
            doc.push_str(&format!("{}\n\n", clean_content.trim()));
            turn_counter += 1;
        } else if msg.role == "assistant" {
            doc.push_str("*** 🤖 Agent Response\n");
            let formatted_content = convert_markdown_code_to_org(&clean_content);
            doc.push_str(&format!("{}\n\n", formatted_content.trim()));
        } else if msg.role == "tool" || msg.role == "system" {
            doc.push_str(&format!("*** ⚙️ Context / Tool: {}\n", msg.role));
            doc.push_str("#+BEGIN_EXAMPLE\n");
            doc.push_str(clean_content.trim());
            doc.push_str("\n#+END_EXAMPLE\n\n");
        }
    }

    doc.push_str("* 🏁 End of Session Report\n");
    doc.push_str("/Generated autonomously by Orbit CLI (dwaddle/orbit-cli)/\n");

    fs::write(&dest_file, doc)?;
    Ok(dest_file)
}

fn format_org_date(timestamp: u64) -> String {
    format!("timestamp: {}", timestamp)
}

fn convert_markdown_code_to_org(content: &str) -> String {
    let mut out = String::new();
    let mut in_code = false;

    for line in content.lines() {
        if line.starts_with("```") {
            if !in_code {
                let lang = line.trim_start_matches('`').trim();
                let org_lang = if lang.is_empty() { "text" } else { lang };
                out.push_str(&format!("#+BEGIN_SRC {}\n", org_lang));
                in_code = true;
            } else {
                out.push_str("#+END_SRC\n");
                in_code = false;
            }
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }

    if in_code {
        out.push_str("#+END_SRC\n");
    }

    out
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_export_session_to_org_file() {
        let td = tempdir().unwrap();
        let target_file = td.path().join("session_test.org");

        let turns = vec![
            ExportTurn {
                role: "user".to_string(),
                content: "Can you fix the login crash?".to_string(),
            },
            ExportTurn {
                role: "assistant".to_string(),
                content: "Here is the fix with token: sk-1234567890abcdef1234567890\n```rust\nfn login() {}\n```".to_string(),
            },
        ];

        let path = export_session_to_org(
            &turns,
            1500,
            1200,
            0.0035,
            Path::new("/home/rene/projecten/demo"),
            "anthropic/claude-sonnet-4.5",
            Some(&target_file),
        )
        .unwrap();

        assert!(path.exists());
        let content = fs::read_to_string(path).unwrap();
        assert!(content.contains("#+TITLE: Orbit AI Coding Session Report"));
        assert!(content.contains("* 🛰️ Session Executive Summary"));
        assert!(content.contains("** Turn 1: User Request"));
        assert!(content.contains("#+BEGIN_SRC rust"));
        assert!(content.contains("#+END_SRC"));
        assert!(!content.contains("sk-1234567890"));
        assert!(content.contains("[REDACTED_SECRET]"));
    }
}
