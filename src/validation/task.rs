use crate::dtos::{NewTaskDto, UpdateTaskDto};
use crate::models::StatusKind;

use super::action::validate_action_params;
use super::{ValidationError, ValidationResult};

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

    // Validate expected_count
    if let Some(expected_count) = dto.expected_count {
        if expected_count < 0 {
            errors.push(ValidationError {
                field: "expected_count".to_string(),
                message: "expected_count cannot be negative".to_string(),
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

    // Validate expected_count
    if let Some(expected_count) = dto.expected_count {
        if expected_count < 0 {
            errors.push(ValidationError {
                field: "expected_count".to_string(),
                message: "expected_count cannot be negative".to_string(),
            });
        }
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

/// Validates an update task DTO for the PUT (batch counter) endpoint.
/// Only validates counter fields — does NOT check status or failure_reason.
pub fn validate_update_task_counters(dto: &UpdateTaskDto) -> ValidationResult {
    let mut errors = Vec::new();

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

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Validates a batch of new tasks, checking for duplicate IDs, unknown/forward
/// dependency references, and circular dependencies.
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

    // Build task_id_to_idx incrementally, checking for duplicate IDs
    let mut task_id_to_idx: HashMap<&str, usize> = HashMap::new();
    for (i, task) in tasks.iter().enumerate() {
        if let Some(prev_idx) = task_id_to_idx.insert(task.id.as_str(), i) {
            errors.push(ValidationError {
                field: format!("tasks[{}].id", i),
                message: format!(
                    "duplicate task id: '{}' (first defined at index {})",
                    task.id, prev_idx
                ),
            });
        }
    }

    // Check for unknown and forward dependency references
    let mut seen: HashSet<&str> = HashSet::new();
    for (i, task) in tasks.iter().enumerate() {
        if let Some(ref deps) = task.dependencies {
            for dep in deps {
                if !task_id_to_idx.contains_key(dep.id.as_str()) {
                    errors.push(ValidationError {
                        field: format!("tasks[{}].dependencies", i),
                        message: format!("unknown dependency: '{}'", dep.id),
                    });
                } else if !seen.contains(dep.id.as_str()) {
                    errors.push(ValidationError {
                        field: format!("tasks[{}].dependencies", i),
                        message: format!(
                            "dependency '{}' must appear before task '{}' in the batch",
                            dep.id, task.id
                        ),
                    });
                }
            }
        }
        seen.insert(task.id.as_str());
    }

    // If we already have errors from duplicate IDs or unknown deps, skip cycle detection
    if !errors.is_empty() {
        return Err(errors);
    }

    // Build adjacency list for cycle detection
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
    for task in tasks {
        let deps: Vec<&str> = task
            .dependencies
            .as_ref()
            .map(|d| d.iter().map(|dep| dep.id.as_str()).collect())
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
    use crate::models::ActionKindEnum;

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
            expected_count: None,
            on_failure: None,
            on_success: None,
            dead_end_barrier: None,
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
                .any(|e| e.message.contains("must appear before"))
        );
    }

    #[test]
    fn test_longer_circular_dependency() {
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
            errors
                .iter()
                .any(|e| e.message.contains("must appear before")),
            "Expected forward reference error, got: {:?}",
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
}
