use anyhow::{anyhow, Result};
use regex::Regex;
use reqwest::Url;
use std::net::{IpAddr, ToSocketAddrs};

pub struct DocFetcher;

impl DocFetcher {
    /// Validates that `url_str` is a safe external HTTP/HTTPS URL and does not target
    /// private networks (RFC 1918), loopback, link-local, or cloud metadata endpoints.
    pub fn validate_url_safe(url_str: &str) -> Result<Url> {
        let parsed = Url::parse(url_str).map_err(|e| anyhow!("Invalid URL format: {}", e))?;

        if parsed.scheme() != "http" && parsed.scheme() != "https" {
            return Err(anyhow!(
                "SSRF Guard: Unsupported URL scheme '{}'. Only HTTP/HTTPS allowed.",
                parsed.scheme()
            ));
        }

        let host = parsed
            .host_str()
            .ok_or_else(|| anyhow!("SSRF Guard: URL is missing a valid host"))?;

        // Fast-path hostname checks
        let lower_host = host.to_ascii_lowercase();
        if lower_host == "localhost"
            || lower_host.ends_with(".localhost")
            || lower_host.ends_with(".local")
            || lower_host.ends_with(".internal")
        {
            return Err(anyhow!(
                "SSRF Guard: Access to local domain '{}' is denied",
                host
            ));
        }

        // Check if host is direct IP address
        if let Ok(ip) = host.parse::<IpAddr>() {
            Self::check_ip_allowed(&ip)?;
        } else {
            // DNS resolution check: resolve hostname and ensure no resolved IP is private/loopback
            let port = parsed.port_or_known_default().unwrap_or(80);
            if let Ok(addrs) = (host, port).to_socket_addrs() {
                for addr in addrs {
                    Self::check_ip_allowed(&addr.ip())?;
                }
            }
        }

        Ok(parsed)
    }

    fn check_ip_allowed(ip: &IpAddr) -> Result<()> {
        if ip.is_loopback() || ip.is_unspecified() {
            return Err(anyhow!(
                "SSRF Guard: Loopback or unspecified IP address '{}' is blocked",
                ip
            ));
        }

        match ip {
            IpAddr::V4(ipv4) => {
                if ipv4.is_private() || ipv4.is_link_local() || ipv4.is_broadcast() {
                    return Err(anyhow!(
                        "SSRF Guard: Private or link-local IPv4 address '{}' is blocked",
                        ipv4
                    ));
                }
            }
            IpAddr::V6(ipv6) => {
                // Check for unique local fc00::/7 or link-local fe80::/10
                let seg = ipv6.segments();
                if (seg[0] & 0xfe00) == 0xfc00 || (seg[0] & 0xffc0) == 0xfe80 {
                    return Err(anyhow!(
                        "SSRF Guard: Private or link-local IPv6 address '{}' is blocked",
                        ipv6
                    ));
                }
            }
        }

        Ok(())
    }

    pub async fn fetch_and_clean(url: &str) -> Result<String> {
        let validated = Self::validate_url_safe(url)?;

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .user_agent("Orbit-Agent/0.2.0")
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() >= 5 {
                    return attempt.error(anyhow!("Too many redirects"));
                }
                if let Err(e) = DocFetcher::validate_url_safe(attempt.url().as_str()) {
                    return attempt.error(e);
                }
                attempt.follow()
            }))
            .build()?;

        let response = client.get(validated).send().await?;
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

    #[test]
    fn test_validate_url_safe_blocks_ssrf() {
        // Loopback IPv4
        assert!(DocFetcher::validate_url_safe("http://127.0.0.1:8080/admin").is_err());
        assert!(DocFetcher::validate_url_safe("http://localhost/secret").is_err());
        assert!(DocFetcher::validate_url_safe("http://my.localhost/").is_err());

        // Private IPv4 ranges
        assert!(DocFetcher::validate_url_safe("http://10.0.0.1/internal").is_err());
        assert!(DocFetcher::validate_url_safe("http://192.168.1.1/router").is_err());
        assert!(DocFetcher::validate_url_safe("http://172.16.0.5/api").is_err());

        // AWS/GCP cloud metadata link-local
        assert!(DocFetcher::validate_url_safe("http://169.254.169.254/latest/meta-data").is_err());

        // Unsupported schemes
        assert!(DocFetcher::validate_url_safe("file:///etc/passwd").is_err());
        assert!(DocFetcher::validate_url_safe("gopher://127.0.0.1").is_err());

        // Valid external public URL
        assert!(DocFetcher::validate_url_safe("https://docs.rs/tokio/latest").is_ok());
    }
}
