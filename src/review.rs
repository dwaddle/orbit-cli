use anyhow::Result;
use colored::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ReviewVerdict {
    Approved,
    Warnings,
    ChangesRequested,
}

impl ReviewVerdict {
    pub fn badge(&self) -> String {
        match self {
            ReviewVerdict::Approved => "✅ APPROVED".bold().green().to_string(),
            ReviewVerdict::Warnings => "⚠️ WARNINGS".bold().yellow().to_string(),
            ReviewVerdict::ChangesRequested => "❌ CHANGES REQUESTED".bold().red().to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewFinding {
    pub severity: String,
    pub title: String,
    pub description: String,
    pub suggestion: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewReport {
    pub verdict: ReviewVerdict,
    pub score: u8,
    pub summary: String,
    pub findings: Vec<ReviewFinding>,
    pub raw_response: String,
}

pub struct AdversarialReviewer;

impl AdversarialReviewer {
    /// Conducts a rigorous adversarial code review on code, patches, or files.
    pub async fn review_code(
        target_name: &str,
        content: &str,
        context_notes: &str,
        api_key: &str,
    ) -> Result<ReviewReport> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .connect_timeout(std::time::Duration::from_secs(5))
            .build()?;

        let sys_prompt = "You are an elite Adversarial Code Reviewer and Principal Security Engineer.\n\
        Your job is to ruthlessly critique proposed code changes and pull requests across 5 dimensions:\n\
        1. Correctness, edge cases, and regression risks\n\
        2. Security vulnerabilities (OWASP, SQLi, command injection, secret leakage, memory safety)\n\
        3. Error handling, unwrap/panic risks, nullability\n\
        4. Test coverage & verifiability\n\
        5. Performance, algorithmic complexity, and backward compatibility.\n\n\
        Provide your critique in structured markdown with:\n\
        - A Quality Score (0-100)\n\
        - Verdict: APPROVED (score >= 85 and 0 criticals), WARNINGS (score >= 70), or CHANGES_REQUESTED\n\
        - Bullet points of specific issues with severity [CRITICAL], [WARNING], or [NOTE]\n\
        - Concrete, copy-pasteable remediation suggestions.";

        let user_prompt = format!(
            "Review the following code/patch for '{}'.\n\n\
            Context / Requirements:\n{}\n\n\
            === CODE / DIFF TO REVIEW ===\n\
            {}\n\n\
            Conduct your adversarial review now.",
            target_name, context_notes, content
        );

        let default_fallbacks = [
            "openai/gpt-4o".to_string(),
            "deepseek/deepseek-r1".to_string(),
            "google/gemini-2.5-flash".to_string(),
        ];
        let primary_model = "anthropic/claude-sonnet-4.5";
        let candidates = crate::routing::get_failover_candidates(primary_model, &default_fallbacks);

        for (idx, candidate) in candidates.iter().enumerate() {
            let payload = serde_json::json!({
                "model": candidate,
                "messages": [
                    {
                        "role": "system",
                        "content": [
                            {
                                "type": "text",
                                "text": sys_prompt,
                                "cache_control": {
                                    "type": "ephemeral"
                                }
                            }
                        ]
                    },
                    {"role": "user", "content": user_prompt}
                ],
                "temperature": 0.1
            });

            let resp_res = client
                .post("https://openrouter.ai/api/v1/chat/completions")
                .header("Authorization", format!("Bearer {}", api_key))
                .header("Content-Type", "application/json")
                .header("HTTP-Referer", "https://github.com/dwaddle/orbit-cli")
                .header("X-Title", "Orbit-CLI")
                .json(&payload)
                .send()
                .await;

            let resp = match resp_res {
                Ok(r) => r,
                Err(e) => {
                    if idx + 1 < candidates.len() {
                        continue;
                    } else {
                        return Err(anyhow::anyhow!("Reviewer network error: {}", e));
                    }
                }
            };

            if !resp.status().is_success() {
                let status = resp.status();
                if crate::routing::is_retryable_status(status.as_u16())
                    && idx + 1 < candidates.len()
                {
                    continue;
                }
                let err = resp
                    .text()
                    .await
                    .unwrap_or_else(|_| "Unknown error".to_string());
                return Err(anyhow::anyhow!(
                    "Reviewer API error (HTTP {}): {}",
                    status.as_u16(),
                    err
                ));
            }

            let body: serde_json::Value = resp.json().await?;
            let response_text = body["choices"][0]["message"]["content"]
                .as_str()
                .unwrap_or("")
                .to_string();

            let report = parse_review_report(&response_text);
            return Ok(report);
        }

        Err(anyhow::anyhow!("All reviewer models exhausted."))
    }
}

pub fn parse_review_report(raw: &str) -> ReviewReport {
    let lower = raw.to_lowercase();
    let verdict = if lower.contains("changes_requested")
        || lower.contains("changes requested")
        || lower.contains("[critical]")
    {
        ReviewVerdict::ChangesRequested
    } else if lower.contains("warnings") || lower.contains("[warning]") {
        ReviewVerdict::Warnings
    } else {
        ReviewVerdict::Approved
    };

    let mut score = 80;
    // Extract score if present
    for line in raw.lines() {
        let l = line.to_lowercase();
        if l.contains("score") {
            for segment in l.split(|c: char| !c.is_numeric()) {
                if let Ok(parsed) = segment.parse::<u8>() {
                    if parsed <= 100 {
                        score = parsed;
                        break;
                    }
                }
            }
        }
    }

    let mut findings = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("- [CRITICAL]") || trimmed.starts_with("* [CRITICAL]") {
            findings.push(ReviewFinding {
                severity: "CRITICAL".to_string(),
                title: trimmed.trim_start_matches(['-', '*', ' ']).to_string(),
                description: String::new(),
                suggestion: None,
            });
        } else if trimmed.starts_with("- [WARNING]") || trimmed.starts_with("* [WARNING]") {
            findings.push(ReviewFinding {
                severity: "WARNING".to_string(),
                title: trimmed.trim_start_matches(['-', '*', ' ']).to_string(),
                description: String::new(),
                suggestion: None,
            });
        }
    }

    ReviewReport {
        verdict,
        score,
        summary: format!("Adversarial review completed with score {}/100.", score),
        findings,
        raw_response: raw.to_string(),
    }
}

pub fn format_review_banner(report: &ReviewReport) -> String {
    let mut out = format!(
        "=== 🛡️ ADVERSARIAL REVIEW: {} (Score: {}/100) ===\n\n",
        report.verdict.badge(),
        report.score
    );
    out.push_str(&report.raw_response);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_review_report_approved() {
        let raw =
            "Quality Score: 92/100\nVerdict: APPROVED\nEverything looks solid. No criticals found.";
        let rep = parse_review_report(raw);
        assert_eq!(rep.verdict, ReviewVerdict::Approved);
        assert_eq!(rep.score, 92);
    }

    #[test]
    fn test_parse_review_report_critical() {
        let raw = "Quality Score: 45/100\nVerdict: CHANGES_REQUESTED\n- [CRITICAL] SQL injection vulnerability in query builder\n- [WARNING] Missing error handling on file read";
        let rep = parse_review_report(raw);
        assert_eq!(rep.verdict, ReviewVerdict::ChangesRequested);
        assert_eq!(rep.score, 45);
        assert_eq!(rep.findings.len(), 2);
        assert_eq!(rep.findings[0].severity, "CRITICAL");
        assert_eq!(rep.findings[1].severity, "WARNING");
    }
}
