use serde_json::json;
use task_runner::dtos::UpdateTaskDto;
use task_runner::models::StatusKind;

/// Helper to create a valid webhook action JSON.
/// The trigger is determined by context (on_start, on_success, on_failure),
/// not by the action payload itself.
pub fn webhook_action() -> serde_json::Value {
    json!({
        "kind": "Webhook",
        "params": {
            "url": "https://example.com/webhook",
            "verb": "Post"
        }
    })
}

/// Helper to create a basic task JSON
pub fn task_json(id: &str, name: &str, kind: &str) -> serde_json::Value {
    json!({
        "id": id,
        "name": name,
        "kind": kind,
        "timeout": 60,
        "metadata": {"test": true},
        "on_start": webhook_action()
    })
}

/// Helper to create a task with dependencies
pub fn task_with_deps(
    id: &str,
    name: &str,
    kind: &str,
    deps: Vec<(&str, bool)>,
) -> serde_json::Value {
    let dependencies: Vec<serde_json::Value> = deps
        .into_iter()
        .map(|(dep_id, requires_success)| {
            json!({
                "id": dep_id,
                "requires_success": requires_success
            })
        })
        .collect();

    json!({
        "id": id,
        "name": name,
        "kind": kind,
        "timeout": 60,
        "metadata": {"test": true},
        "on_start": webhook_action(),
        "dependencies": dependencies
    })
}

/// Helper to create a task with custom metadata.
pub fn task_with_metadata(
    id: &str,
    name: &str,
    kind: &str,
    metadata: serde_json::Value,
) -> serde_json::Value {
    json!({
        "id": id,
        "name": name,
        "kind": kind,
        "timeout": 60,
        "metadata": metadata,
        "on_start": webhook_action()
    })
}

/// Helper to create a task with concurrency/capacity rules.
pub fn task_with_rules(
    id: &str,
    name: &str,
    kind: &str,
    rules: serde_json::Value,
) -> serde_json::Value {
    json!({
        "id": id,
        "name": name,
        "kind": kind,
        "timeout": 60,
        "metadata": {"test": true},
        "on_start": webhook_action(),
        "rules": rules
    })
}

/// Helper to build an UpdateTaskDto that marks a task as Success.
pub fn success_dto() -> UpdateTaskDto {
    UpdateTaskDto {
        status: Some(StatusKind::Success),
        metadata: None,
        new_success: None,
        new_failures: None,
        failure_reason: None,
        expected_count: None,
    }
}

/// Helper to build an UpdateTaskDto that marks a task as Failure with a reason.
pub fn failure_dto(reason: &str) -> UpdateTaskDto {
    UpdateTaskDto {
        status: Some(StatusKind::Failure),
        metadata: None,
        new_success: None,
        new_failures: None,
        failure_reason: Some(reason.to_string()),
        expected_count: None,
    }
}
