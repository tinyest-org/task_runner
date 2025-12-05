//! Input validation for task creation and updates.
//!
//! This module provides validation functions for DTOs before they are processed.

use crate::action::WebhookParams;
use crate::config::SecurityConfig;
use crate::dtos::{NewTaskDto, UpdateTaskDto};
use crate::models::{ActionKindEnum, StatusKind};
use serde_json::Value;
use std::net::IpAddr;
use std::sync::OnceLock;
use url::Url;

/// Global security configuration for validation.
/// Must be initialized before validation is used.
static SECURITY_CONFIG: OnceLock<SecurityConfig> = OnceLock::new();

/// Initialize the security configuration for validation.
/// Should be called once at application startup.
pub fn init_security_config(config: SecurityConfig) {
    SECURITY_CONFIG
        .set(config)
        .expect("Security config already initialized");
}

/// Get the current security configuration.
/// Returns the default config if not initialized (useful for tests).
fn get_security_config() -> SecurityConfig {
    SECURITY_CONFIG
        .get()
        .cloned()
        .unwrap_or_else(SecurityConfig::default)
}

/// Validation error with details about what failed.
#[derive(Debug, Clone)]
pub struct ValidationError {
    pub field: String,
    pub message: String,
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.field, self.message)
    }
}

/// Result of validation - either Ok or a list of errors.
pub type ValidationResult = Result<(), Vec<ValidationError>>;

/// Validates a new task DTO before creation.
pub fn validate_new_task(dto: &NewTaskDto) -> ValidationResult {
    let mut errors = Vec::new();

    // Validate id (local reference)
    if dto.id.trim().is_empty() {
        errors.push(ValidationError {
            field: "id".to_string(),
            message: "Task local ID cannot be empty".to_string(),
        });
    }

    // Validate name
    if dto.name.trim().is_empty() {
        errors.push(ValidationError {
            field: "name".to_string(),
            message: "Task name cannot be empty".to_string(),
        });
    } else if dto.name.len() > 255 {
        errors.push(ValidationError {
            field: "name".to_string(),
            message: "Task name cannot exceed 255 characters".to_string(),
        });
    }

    // Validate kind
    if dto.kind.trim().is_empty() {
        errors.push(ValidationError {
            field: "kind".to_string(),
            message: "Task kind cannot be empty".to_string(),
        });
    } else if dto.kind.len() > 100 {
        errors.push(ValidationError {
            field: "kind".to_string(),
            message: "Task kind cannot exceed 100 characters".to_string(),
        });
    }

    // Validate timeout
    if let Some(timeout) = dto.timeout {
        if timeout <= 0 {
            errors.push(ValidationError {
                field: "timeout".to_string(),
                message: "Timeout must be a positive integer".to_string(),
            });
        } else if timeout > 86400 {
            // Max 24 hours
            errors.push(ValidationError {
                field: "timeout".to_string(),
                message: "Timeout cannot exceed 86400 seconds (24 hours)".to_string(),
            });
        }
    }

    // Validate metadata size (prevent extremely large payloads)
    if let Some(ref metadata) = dto.metadata {
        let metadata_str = serde_json::to_string(metadata).unwrap_or_default();
        if metadata_str.len() > 65536 {
            // 64KB limit
            errors.push(ValidationError {
                field: "metadata".to_string(),
                message: "Metadata cannot exceed 64KB".to_string(),
            });
        }
    }

    // Validate on_start action
    if let Err(e) = validate_action_params(&dto.on_start.kind, &dto.on_start.params) {
        errors.push(ValidationError {
            field: "on_start.params".to_string(),
            message: e,
        });
    }

    // Validate on_failure actions
    if let Some(ref actions) = dto.on_failure {
        for (i, action) in actions.iter().enumerate() {
            if let Err(e) = validate_action_params(&action.kind, &action.params) {
                errors.push(ValidationError {
                    field: format!("on_failure[{}].params", i),
                    message: e,
                });
            }
        }
    }

    // Validate on_success actions
    if let Some(ref actions) = dto.on_success {
        for (i, action) in actions.iter().enumerate() {
            if let Err(e) = validate_action_params(&action.kind, &action.params) {
                errors.push(ValidationError {
                    field: format!("on_success[{}].params", i),
                    message: e,
                });
            }
        }
    }

    // Validate dependencies
    if let Some(ref deps) = dto.dependencies {
        for (i, dep) in deps.iter().enumerate() {
            if dep.id.trim().is_empty() {
                errors.push(ValidationError {
                    field: format!("dependencies[{}].id", i),
                    message: "Dependency ID cannot be empty".to_string(),
                });
            }
        }

        // Check for self-referencing dependency
        if deps.iter().any(|d| d.id == dto.id) {
            errors.push(ValidationError {
                field: "dependencies".to_string(),
                message: "Task cannot depend on itself".to_string(),
            });
        }

        // Check for duplicate dependencies
        let mut seen_ids: Vec<&str> = Vec::new();
        for dep in deps {
            if seen_ids.contains(&dep.id.as_str()) {
                errors.push(ValidationError {
                    field: "dependencies".to_string(),
                    message: format!("Duplicate dependency: {}", dep.id),
                });
            } else {
                seen_ids.push(&dep.id);
            }
        }
    }

    // Validate rules (concurrency conditions)
    if let Some(ref rules) = dto.rules {
        for (i, rule) in rules.0.iter().enumerate() {
            match rule {
                crate::rule::Strategy::Concurency(concurrency_rule) => {
                    if concurrency_rule.max_concurency <= 0 {
                        errors.push(ValidationError {
                            field: format!("rules[{}].max_concurency", i),
                            message: "max_concurency must be a positive integer".to_string(),
                        });
                    }
                    if concurrency_rule.matcher.kind.trim().is_empty() {
                        errors.push(ValidationError {
                            field: format!("rules[{}].matcher.kind", i),
                            message: "Matcher kind cannot be empty".to_string(),
                        });
                    }
                }
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Validates action parameters based on action kind.
fn validate_action_params(kind: &ActionKindEnum, params: &Value) -> Result<(), String> {
    match kind {
        ActionKindEnum::Webhook => {
            let webhook: Result<WebhookParams, _> = serde_json::from_value(params.clone());
            match webhook {
                Ok(w) => validate_webhook_url(&w.url),
                Err(e) => Err(format!("Invalid webhook params: {}", e)),
            }
        }
    }
}

/// Validates a webhook URL for security (SSRF prevention).
/// Blocks internal IP ranges, localhost, and non-HTTP schemes.
/// Can be skipped in debug builds via configuration.
fn validate_webhook_url(url_str: &str) -> Result<(), String> {
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

/// Validates an update task DTO.
pub fn validate_update_task(dto: &UpdateTaskDto) -> ValidationResult {
    let mut errors = Vec::new();

    // Validate status transition (only Success or Failure allowed via API)
    if let Some(ref status) = dto.status {
        match status {
            StatusKind::Success | StatusKind::Failure => {}
            _ => {
                errors.push(ValidationError {
                    field: "status".to_string(),
                    message: "Status can only be updated to Success or Failure".to_string(),
                });
            }
        }
    }

    // Validate counters are non-negative
    if let Some(success) = dto.new_success
        && success < 0
    {
        errors.push(ValidationError {
            field: "new_success".to_string(),
            message: "new_success cannot be negative".to_string(),
        });
    }

    if let Some(failures) = dto.new_failures
        && failures < 0
    {
        errors.push(ValidationError {
            field: "new_failures".to_string(),
            message: "new_failures cannot be negative".to_string(),
        });
    }

    // Validate failure_reason is provided when status is Failure
    if let Some(ref status) = dto.status
        && *status == StatusKind::Failure
        && dto.failure_reason.is_none()
    {
        errors.push(ValidationError {
            field: "failure_reason".to_string(),
            message: "failure_reason is required when status is Failure".to_string(),
        });
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Validates a batch of new tasks, checking for circular dependencies.
pub fn validate_task_batch(tasks: &[NewTaskDto]) -> ValidationResult {
    use std::collections::{HashMap, HashSet};

    let mut errors = Vec::new();

    // First validate each task individually
    for (i, task) in tasks.iter().enumerate() {
        if let Err(task_errors) = validate_new_task(task) {
            for e in task_errors {
                errors.push(ValidationError {
                    field: format!("tasks[{}].{}", i, e.field),
                    message: e.message,
                });
            }
        }
    }

    // Build adjacency list for cycle detection (task_id -> list of dependency ids)
    let task_id_to_idx: HashMap<&str, usize> = tasks
        .iter()
        .enumerate()
        .map(|(i, t)| (t.id.as_str(), i))
        .collect();

    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
    for task in tasks {
        let deps: Vec<&str> = task
            .dependencies
            .as_ref()
            .map(|d| {
                d.iter()
                    .filter(|dep| task_id_to_idx.contains_key(dep.id.as_str()))
                    .map(|dep| dep.id.as_str())
                    .collect()
            })
            .unwrap_or_default();
        adj.insert(task.id.as_str(), deps);
    }

    // DFS cycle detection with path tracking
    // States: 0 = unvisited, 1 = visiting (in current path), 2 = visited (done)
    let mut state: HashMap<&str, u8> = tasks.iter().map(|t| (t.id.as_str(), 0u8)).collect();
    let mut path: Vec<&str> = Vec::new();

    fn dfs<'a>(
        node: &'a str,
        adj: &HashMap<&'a str, Vec<&'a str>>,
        state: &mut HashMap<&'a str, u8>,
        path: &mut Vec<&'a str>,
        cycles: &mut HashSet<String>,
    ) {
        state.insert(node, 1); // Mark as visiting
        path.push(node);

        if let Some(neighbors) = adj.get(node) {
            for &neighbor in neighbors {
                match state.get(neighbor) {
                    Some(1) => {
                        // Found a cycle - neighbor is in current path
                        // Find where the cycle starts in the path
                        if let Some(cycle_start) = path.iter().position(|&n| n == neighbor) {
                            let cycle_path: Vec<&str> = path[cycle_start..].to_vec();
                            let cycle_str = format!("{} -> {}", cycle_path.join(" -> "), neighbor);
                            cycles.insert(cycle_str);
                        }
                    }
                    Some(0) | None => {
                        // Unvisited - recurse
                        dfs(neighbor, adj, state, path, cycles);
                    }
                    _ => {} // Already visited (state 2), skip
                }
            }
        }

        path.pop();
        state.insert(node, 2); // Mark as visited (done)
    }

    let mut cycles: HashSet<String> = HashSet::new();
    for task in tasks {
        if state.get(task.id.as_str()) == Some(&0) {
            dfs(task.id.as_str(), &adj, &mut state, &mut path, &mut cycles);
        }
    }

    // Report all detected cycles
    for cycle in cycles {
        errors.push(ValidationError {
            field: "dependencies".to_string(),
            message: format!("Circular dependency detected: {}", cycle),
        });
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dtos::{Dependency, NewActionDto};

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

    fn make_valid_action() -> NewActionDto {
        NewActionDto {
            kind: ActionKindEnum::Webhook,
            params: serde_json::json!({
                "url": "https://example.com/webhook",
                "verb": "Post"
            }),
        }
    }

    fn make_valid_task(id: &str) -> NewTaskDto {
        NewTaskDto {
            id: id.to_string(),
            name: "Test Task".to_string(),
            kind: "test".to_string(),
            timeout: Some(60),
            metadata: None,
            dedupe_strategy: None,
            rules: None,
            on_start: make_valid_action(),
            dependencies: None,
            on_failure: None,
            on_success: None,
        }
    }

    #[test]
    fn test_valid_task() {
        let task = make_valid_task("task1");
        assert!(validate_new_task(&task).is_ok());
    }

    #[test]
    fn test_empty_name() {
        let mut task = make_valid_task("task1");
        task.name = "".to_string();
        let result = validate_new_task(&task);
        assert!(result.is_err());
        assert!(result.unwrap_err().iter().any(|e| e.field == "name"));
    }

    #[test]
    fn test_empty_kind() {
        let mut task = make_valid_task("task1");
        task.kind = "".to_string();
        let result = validate_new_task(&task);
        assert!(result.is_err());
        assert!(result.unwrap_err().iter().any(|e| e.field == "kind"));
    }

    #[test]
    fn test_negative_timeout() {
        let mut task = make_valid_task("task1");
        task.timeout = Some(-1);
        let result = validate_new_task(&task);
        assert!(result.is_err());
        assert!(result.unwrap_err().iter().any(|e| e.field == "timeout"));
    }

    #[test]
    fn test_self_dependency() {
        let mut task = make_valid_task("task1");
        task.dependencies = Some(vec![Dependency {
            id: "task1".to_string(),
            requires_success: true,
        }]);
        let result = validate_new_task(&task);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .iter()
                .any(|e| e.message.contains("itself"))
        );
    }

    #[test]
    fn test_circular_dependency() {
        let mut task1 = make_valid_task("task1");
        task1.dependencies = Some(vec![Dependency {
            id: "task2".to_string(),
            requires_success: true,
        }]);

        let mut task2 = make_valid_task("task2");
        task2.dependencies = Some(vec![Dependency {
            id: "task1".to_string(),
            requires_success: true,
        }]);

        let result = validate_task_batch(&[task1, task2]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .iter()
                .any(|e| e.message.contains("Circular"))
        );
    }

    #[test]
    fn test_longer_circular_dependency() {
        // Test A -> B -> C -> A cycle detection
        let mut task_a = make_valid_task("A");
        task_a.dependencies = Some(vec![Dependency {
            id: "B".to_string(),
            requires_success: true,
        }]);

        let mut task_b = make_valid_task("B");
        task_b.dependencies = Some(vec![Dependency {
            id: "C".to_string(),
            requires_success: true,
        }]);

        let mut task_c = make_valid_task("C");
        task_c.dependencies = Some(vec![Dependency {
            id: "A".to_string(),
            requires_success: true,
        }]);

        let result = validate_task_batch(&[task_a, task_b, task_c]);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(
            errors.iter().any(|e| e.message.contains("Circular")),
            "Expected circular dependency error, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_invalid_webhook_url() {
        let mut task = make_valid_task("task1");
        task.on_start.params = serde_json::json!({
            "url": "not-a-url",
            "verb": "Post"
        });
        let result = validate_new_task(&task);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .iter()
                .any(|e| e.field.contains("on_start"))
        );
    }

    // SSRF prevention tests (use validate_url_strict to ensure SSRF checks run regardless of build mode)
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
