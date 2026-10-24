use anyhow::{anyhow, Result};
use regex::Regex;

pub struct DocFetcher;

impl DocFetcher {
    pub async fn fetch_and_clean(url: &str) -> Result<String> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .user_agent("Orbit-Agent/0.2.0")
            .build()?;

        let response = client.get(url).send().await?;
        if !response.status().is_success() {
            return Err(anyhow!(
                "HTTP error {} fetching URL: {}",
                response.status(),
                url
            ));
        }

        let body = response.text().await?;
        Ok(Self::clean_html_to_markdown(&body))
    }

    pub fn clean_html_to_markdown(html: &str) -> String {
        // Strip scripts and styles
        let re_script = Regex::new(r"(?is)<script.*?</script>").unwrap();
        let cleaned = re_script.replace_all(html, "");

        let re_style = Regex::new(r"(?is)<style.*?</style>").unwrap();
        let cleaned = re_style.replace_all(&cleaned, "");

        // Convert common block elements to newlines
        let re_blocks = Regex::new(r"(?i)</?(p|div|h[1-6]|li|tr|br/?)>").unwrap();
        let cleaned = re_blocks.replace_all(&cleaned, "\n");

        // Strip remaining HTML tags
        let re_tags = Regex::new(r"<[^>]+>").unwrap();
        let cleaned = re_tags.replace_all(&cleaned, "");

        // Decode common entities
        let cleaned = cleaned
            .replace("&amp;", "&")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&#39;", "'")
            .replace("&nbsp;", " ");

        // Collapse multiple blank lines
        let re_whitespace = Regex::new(r"\n\s*\n\s*\n").unwrap();
        let mut final_text = re_whitespace
            .replace_all(&cleaned, "\n\n")
            .trim()
            .to_string();

        const MAX_CHARS: usize = 12000;
        if final_text.len() > MAX_CHARS {
            final_text = format!(
                "{}\n\n... (truncated documentation, total {} characters)",
                &final_text[..MAX_CHARS],
                final_text.len()
            );
        }

        final_text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clean_html_to_markdown() {
        let html = r#"
            <html>
                <head><style>body { color: red; }</style></head>
                <body>
                    <h1>Rust API Reference</h1>
                    <script>console.log("bad");</script>
                    <p>Functions &amp; structs for Tokio.</p>
                </body>
            </html>
        "#;

        let cleaned = DocFetcher::clean_html_to_markdown(html);
        assert!(cleaned.contains("Rust API Reference"));
        assert!(cleaned.contains("Functions & structs for Tokio."));
        assert!(!cleaned.contains("color: red"));
        assert!(!cleaned.contains("console.log"));
    }
}
