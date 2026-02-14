#[macro_use]
mod common;
use common::*;

use task_runner::models::StatusKind;

#[tokio::test]
async fn test_create_single_task() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    let tasks = vec![task_json("task-1", "Test Task", "test-kind")];
    let body = create_tasks_ok(&app, &tasks).await;

    assert_eq!(body.len(), 1);
    assert_eq!(body[0].name, "Test Task");
    assert_eq!(body[0].kind, "test-kind");
    assert_eq!(body[0].status, StatusKind::Pending);
}

#[tokio::test]
async fn test_create_multiple_tasks() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    let tasks: Vec<serde_json::Value> = (1..=5)
        .map(|i| task_json(&format!("task-{}", i), &format!("Task {}", i), "batch"))
        .collect();

    let body = create_tasks_ok(&app, &tasks).await;
    assert_eq!(body.len(), 5);
}

#[tokio::test]
async fn test_get_task_by_id() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    let tasks = vec![task_json("find-me", "Findable Task", "findable")];
    let created = create_tasks_ok(&app, &tasks).await;
    let task_id = created[0].id;

    let found = get_task_ok(&app, task_id).await;
    assert_eq!(found.id, task_id);
    assert_eq!(found.name, "Findable Task");
}

#[tokio::test]
async fn test_get_nonexistent_task() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    let random_id = uuid::Uuid::new_v4();
    let req = actix_web::test::TestRequest::get()
        .uri(&format!("/task/{}", random_id))
        .to_request();

    let resp = actix_web::test::call_service(&app, req).await;
    assert_eq!(resp.status(), actix_web::http::StatusCode::NOT_FOUND);
}
