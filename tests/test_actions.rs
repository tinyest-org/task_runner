#[macro_use]
mod common;
use common::*;

use serde_json::json;

#[tokio::test]
async fn test_task_with_success_and_failure_actions() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    let task = json!({
        "id": "with-actions",
        "name": "Task with Actions",
        "kind": "action-test",
        "timeout": 60,
        "metadata": {"test": true},
        "on_start": webhook_action("Start"),
        "on_success": [webhook_action("End")],
        "on_failure": [webhook_action("End")]
    });

    let body = create_tasks_ok(&app, &[task]).await;
    // Should have 3 actions: on_start, on_success, on_failure
    assert_eq!(body[0].actions.len(), 3);
}
