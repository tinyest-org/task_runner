use serde_json::json;

/// Helper to create a valid webhook action JSON
pub fn webhook_action(trigger: &str) -> serde_json::Value {
    json!({
        "kind": "Webhook",
        "trigger": trigger,
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
        "on_start": webhook_action("Start")
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
        "on_start": webhook_action("Start"),
        "dependencies": dependencies
    })
}
