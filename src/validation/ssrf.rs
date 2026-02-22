use crate::config::SecurityConfig;
use std::net::IpAddr;
use url::Url;

use super::get_security_config;

/// Validates a webhook URL for security (SSRF prevention).
/// Blocks internal IP ranges, localhost, and non-HTTP schemes.
/// Can be skipped in debug builds via configuration.
pub fn validate_webhook_url(url_str: &str) -> Result<(), String> {
    validate_webhook_url_with_config(url_str, &get_security_config())
}

/// Validates a webhook URL with a specific security configuration.
/// Used internally and for testing.
fn validate_webhook_url_with_config(url_str: &str, config: &SecurityConfig) -> Result<(), String> {
    // Skip SSRF validation if configured (e.g., in debug builds)
    if config.skip_ssrf_validation {
        // Still validate URL format even when skipping SSRF checks
        Url::parse(url_str).map_err(|e| format!("Invalid URL format: {}", e))?;
        return Ok(());
    }

    // Parse the URL
    let url = Url::parse(url_str).map_err(|e| format!("Invalid URL format: {}", e))?;

    // Only allow http and https schemes
    match url.scheme() {
        "http" | "https" => {}
        scheme => {
            return Err(format!(
                "URL scheme '{}' not allowed, must be http or https",
                scheme
            ));
        }
    }

    // Get the host
    let host = url
        .host_str()
        .ok_or_else(|| "URL must have a host".to_string())?;

    let host_lower = host.to_lowercase();

    // Check against configurable blocked hostnames
    for blocked in &config.blocked_hostnames {
        let blocked_lower = blocked.to_lowercase();
        if host_lower == blocked_lower || host_lower.ends_with(&format!(".{}", blocked_lower)) {
            return Err(format!(
                "URL host '{}' is not allowed (internal/reserved)",
                host
            ));
        }
    }

    // Try to parse as IP address and check for internal ranges
    if let Ok(ip) = host.parse::<IpAddr>()
        && is_internal_ip(&ip)
    {
        return Err(format!(
            "URL points to internal IP address '{}' which is not allowed",
            ip
        ));
    }

    // Check against configurable blocked hostname suffixes
    for suffix in &config.blocked_hostname_suffixes {
        if host_lower.ends_with(suffix) {
            return Err(format!("URL host '{}' appears to be internal", host));
        }
    }

    Ok(())
}

/// Checks if an IP address is in a private/internal range.
fn is_internal_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(ipv4) => {
            let octets = ipv4.octets();
            // 10.0.0.0/8 (private)
            if octets[0] == 10 {
                return true;
            }
            // 172.16.0.0/12 (private)
            if octets[0] == 172 && (16..=31).contains(&octets[1]) {
                return true;
            }
            // 192.168.0.0/16 (private)
            if octets[0] == 192 && octets[1] == 168 {
                return true;
            }
            // 127.0.0.0/8 (loopback)
            if octets[0] == 127 {
                return true;
            }
            // 169.254.0.0/16 (link-local / cloud metadata)
            if octets[0] == 169 && octets[1] == 254 {
                return true;
            }
            // 0.0.0.0/8
            if octets[0] == 0 {
                return true;
            }
            false
        }
        IpAddr::V6(ipv6) => {
            // ::1 (loopback)
            if ipv6.is_loopback() {
                return true;
            }
            // fe80::/10 (link-local)
            let segments = ipv6.segments();
            if (segments[0] & 0xffc0) == 0xfe80 {
                return true;
            }
            // fc00::/7 (unique local)
            if (segments[0] & 0xfe00) == 0xfc00 {
                return true;
            }
            // :: (unspecified)
            if ipv6.is_unspecified() {
                return true;
            }
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Creates a strict security config for testing SSRF protection.
    /// Always validates URLs regardless of debug/release mode.
    fn strict_security_config() -> SecurityConfig {
        SecurityConfig {
            skip_ssrf_validation: false,
            ..SecurityConfig::default()
        }
    }

    /// Helper to validate webhook URL with strict config for tests.
    fn validate_url_strict(url: &str) -> Result<(), String> {
        validate_webhook_url_with_config(url, &strict_security_config())
    }

    #[test]
    fn test_ssrf_localhost_blocked() {
        assert!(validate_url_strict("http://localhost/api").is_err());
        assert!(validate_url_strict("http://localhost:8080/api").is_err());
        assert!(validate_url_strict("https://localhost/api").is_err());
    }

    #[test]
    fn test_ssrf_loopback_ip_blocked() {
        assert!(validate_url_strict("http://127.0.0.1/api").is_err());
        assert!(validate_url_strict("http://127.0.0.1:8080/api").is_err());
        assert!(validate_url_strict("http://127.1.2.3/api").is_err());
    }

    #[test]
    fn test_ssrf_private_ip_10_blocked() {
        assert!(validate_url_strict("http://10.0.0.1/api").is_err());
        assert!(validate_url_strict("http://10.255.255.255/api").is_err());
    }

    #[test]
    fn test_ssrf_private_ip_172_blocked() {
        assert!(validate_url_strict("http://172.16.0.1/api").is_err());
        assert!(validate_url_strict("http://172.31.255.255/api").is_err());
        // 172.15.x.x and 172.32.x.x should be allowed (not in private range)
        assert!(validate_url_strict("http://172.15.0.1/api").is_ok());
        assert!(validate_url_strict("http://172.32.0.1/api").is_ok());
    }

    #[test]
    fn test_ssrf_private_ip_192_168_blocked() {
        assert!(validate_url_strict("http://192.168.0.1/api").is_err());
        assert!(validate_url_strict("http://192.168.255.255/api").is_err());
    }

    #[test]
    fn test_ssrf_cloud_metadata_blocked() {
        assert!(validate_url_strict("http://169.254.169.254/latest/meta-data/").is_err());
        assert!(
            validate_url_strict("http://metadata.google.internal/computeMetadata/v1/").is_err()
        );
    }

    #[test]
    fn test_ssrf_file_scheme_blocked() {
        assert!(validate_url_strict("file:///etc/passwd").is_err());
    }

    #[test]
    fn test_ssrf_internal_domains_blocked() {
        assert!(validate_url_strict("http://service.local/api").is_err());
        assert!(validate_url_strict("http://app.internal/api").is_err());
        assert!(validate_url_strict("http://host.localdomain/api").is_err());
    }

    #[test]
    fn test_valid_external_urls() {
        assert!(validate_url_strict("https://example.com/webhook").is_ok());
        assert!(validate_url_strict("https://api.github.com/repos").is_ok());
        assert!(validate_url_strict("http://httpbin.org/post").is_ok());
        assert!(validate_url_strict("https://8.8.8.8/api").is_ok());
    }

    #[test]
    fn test_skip_ssrf_validation_allows_localhost() {
        let config = SecurityConfig {
            skip_ssrf_validation: true,
            ..SecurityConfig::default()
        };
        // With skip_ssrf_validation=true, localhost should be allowed
        assert!(validate_webhook_url_with_config("http://localhost/api", &config).is_ok());
        assert!(validate_webhook_url_with_config("http://127.0.0.1/api", &config).is_ok());
        // But invalid URLs should still fail
        assert!(validate_webhook_url_with_config("not-a-url", &config).is_err());
    }

    #[test]
    fn test_custom_blocked_hostnames() {
        let config = SecurityConfig {
            skip_ssrf_validation: false,
            blocked_hostnames: vec!["myblocked.com".to_string()],
            blocked_hostname_suffixes: vec![".blocked".to_string()],
        };
        // Custom blocked hostname
        assert!(validate_webhook_url_with_config("http://myblocked.com/api", &config).is_err());
        // Custom blocked suffix
        assert!(validate_webhook_url_with_config("http://service.blocked/api", &config).is_err());
        // Default blocked hostnames should not be blocked with custom config
        assert!(validate_webhook_url_with_config("http://localhost/api", &config).is_ok());
        // But internal IPs are still blocked (hardcoded check)
        assert!(validate_webhook_url_with_config("http://10.0.0.1/api", &config).is_err());
    }
}
