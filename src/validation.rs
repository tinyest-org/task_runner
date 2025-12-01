//! Input validation for task creation and updates.
//!
//! This module provides validation functions for DTOs before they are processed.

use crate::action::WebhookParams;
use crate::dtos::{NewTaskDto, UpdateTaskDto};
use crate::models::{ActionKindEnum, StatusKind};
use serde_json::Value;

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
                Ok(w) => {
                    // Validate URL
                    if w.url.trim().is_empty() {
                        return Err("Webhook URL cannot be empty".to_string());
                    }
                    // Basic URL validation
                    if !w.url.starts_with("http://") && !w.url.starts_with("https://") {
                        return Err("Webhook URL must start with http:// or https://".to_string());
                    }
                    Ok(())
                }
                Err(e) => Err(format!("Invalid webhook params: {}", e)),
            }
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
    if let Some(success) = dto.new_success {
        if success < 0 {
            errors.push(ValidationError {
                field: "new_success".to_string(),
                message: "new_success cannot be negative".to_string(),
            });
        }
    }

    if let Some(failures) = dto.new_failures {
        if failures < 0 {
            errors.push(ValidationError {
                field: "new_failures".to_string(),
                message: "new_failures cannot be negative".to_string(),
            });
        }
    }

    // Validate failure_reason is provided when status is Failure
    if let Some(ref status) = dto.status {
        if *status == StatusKind::Failure && dto.failure_reason.is_none() {
            errors.push(ValidationError {
                field: "failure_reason".to_string(),
                message: "failure_reason is required when status is Failure".to_string(),
            });
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Validates a batch of new tasks, checking for circular dependencies.
pub fn validate_task_batch(tasks: &[NewTaskDto]) -> ValidationResult {
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

    // Check for circular dependencies within the batch
    let task_ids: Vec<&str> = tasks.iter().map(|t| t.id.as_str()).collect();

    for (i, task) in tasks.iter().enumerate() {
        if let Some(ref deps) = task.dependencies {
            for dep in deps {
                // Check if dependency references a task that comes after this one
                // (forward reference is ok, but we need to check for cycles)
                if let Some(dep_idx) = task_ids.iter().position(|&id| id == dep.id) {
                    // Check if the dependency also depends on this task (direct cycle)
                    if let Some(ref dep_deps) = tasks[dep_idx].dependencies {
                        if dep_deps.iter().any(|d| d.id == task.id) {
                            errors.push(ValidationError {
                                field: format!("tasks[{}].dependencies", i),
                                message: format!(
                                    "Circular dependency detected between '{}' and '{}'",
                                    task.id, dep.id
                                ),
                            });
                        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dtos::{ActionDto, Dependency};
    use crate::models::TriggerKind;

    fn make_valid_action() -> ActionDto {
        ActionDto {
            kind: ActionKindEnum::Webhook,
            trigger: TriggerKind::Start,
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
}
