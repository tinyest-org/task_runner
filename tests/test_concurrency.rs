#[macro_use]
mod common;
use common::*;

use serde_json::json;
use task_runner::models::StatusKind;

#[tokio::test]
async fn test_concurrency_rules_stored_on_task() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    let task = json!({
        "id": "concurrent",
        "name": "Concurrent Task",
        "kind": "concurrent-kind",
        "timeout": 60,
        "metadata": {"test": true},
        "on_start": webhook_action("Start"),
        "rules": [{
            "type": "Concurency",
            "matcher": {
                "kind": "concurrent-kind",
                "status": "Running",
                "fields": []
            },
            "max_concurency": 2
        }]
    });

    let body = create_tasks_ok(&app, &[task]).await;
    assert_eq!(body[0].status, StatusKind::Pending);
    assert!(!body[0].rules.0.is_empty());
}

#[tokio::test]
async fn test_concurrency_rules_stored_per_project() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    let project_id = 12345;

    let tasks = vec![
        json!({
            "id": "proj-task-1",
            "name": "Project Task 1",
            "kind": "project-concurrent",
            "timeout": 60,
            "metadata": {"projectId": project_id},
            "on_start": webhook_action("Start"),
            "rules": [{
                "type": "Concurency",
                "matcher": {
                    "kind": "project-concurrent",
                    "status": "Running",
                    "fields": ["projectId"]
                },
                "max_concurency": 1
            }]
        }),
        json!({
            "id": "proj-task-2",
            "name": "Project Task 2",
            "kind": "project-concurrent",
            "timeout": 60,
            "metadata": {"projectId": project_id},
            "on_start": webhook_action("Start"),
            "rules": [{
                "type": "Concurency",
                "matcher": {
                    "kind": "project-concurrent",
                    "status": "Running",
                    "fields": ["projectId"]
                },
                "max_concurency": 1
            }]
        }),
    ];

    let created = create_tasks_ok(&app, &tasks).await;
    assert_eq!(created.len(), 2);
    assert_eq!(created[0].status, StatusKind::Pending);
    assert_eq!(created[1].status, StatusKind::Pending);
    assert!(!created[0].rules.0.is_empty());
    assert!(!created[1].rules.0.is_empty());
}

#[tokio::test]
async fn test_concurrency_rules_stored_different_projects() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    let tasks = vec![
        json!({
            "id": "diff-proj-1",
            "name": "Project 1 Task",
            "kind": "diff-project",
            "timeout": 60,
            "metadata": {"projectId": 111},
            "on_start": webhook_action("Start"),
            "rules": [{
                "type": "Concurency",
                "matcher": {
                    "kind": "diff-project",
                    "status": "Running",
                    "fields": ["projectId"]
                },
                "max_concurency": 1
            }]
        }),
        json!({
            "id": "diff-proj-2",
            "name": "Project 2 Task",
            "kind": "diff-project",
            "timeout": 60,
            "metadata": {"projectId": 222},
            "on_start": webhook_action("Start"),
            "rules": [{
                "type": "Concurency",
                "matcher": {
                    "kind": "diff-project",
                    "status": "Running",
                    "fields": ["projectId"]
                },
                "max_concurency": 1
            }]
        }),
    ];

    let created = create_tasks_ok(&app, &tasks).await;
    assert_eq!(created.len(), 2);
    assert_eq!(created[0].status, StatusKind::Pending);
    assert_eq!(created[1].status, StatusKind::Pending);
    assert!(!created[0].rules.0.is_empty());
    assert!(!created[1].rules.0.is_empty());
}
