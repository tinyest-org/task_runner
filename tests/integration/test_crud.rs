use crate::common::*;

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
async fn test_create_task_with_expected_count() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    let tasks = vec![serde_json::json!({
        "id": "ec-task",
        "name": "Task with expected count",
        "kind": "test",
        "timeout": 60,
        "expected_count": 100,
        "on_start": webhook_action()
    })];
    let body = create_tasks_ok(&app, &tasks).await;

    assert_eq!(body.len(), 1);
    assert_eq!(body[0].expected_count, Some(100));

    // Verify via GET
    let found = get_task_ok(&app, body[0].id).await;
    assert_eq!(found.expected_count, Some(100));
}

#[tokio::test]
async fn test_create_task_without_expected_count() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    let tasks = vec![task_json("no-ec", "No expected count", "test")];
    let body = create_tasks_ok(&app, &tasks).await;

    assert_eq!(body.len(), 1);
    assert_eq!(body[0].expected_count, None);
}

#[tokio::test]
async fn test_update_expected_count_via_patch() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    // Create task
    let tasks = vec![task_json("patch-ec", "Patch EC", "test")];
    let created = create_tasks_ok(&app, &tasks).await;
    let task_id = created[0].id;

    // Claim and mark running
    {
        let mut conn = state.pool.get().await.unwrap();
        let claimed = task_runner::db_operation::claim_task(&mut conn, &task_id)
            .await
            .unwrap();
        assert!(claimed);
        let marked = task_runner::db_operation::mark_task_running(&mut conn, &task_id)
            .await
            .unwrap();
        assert!(marked);
    }

    // PATCH with expected_count
    let req = actix_web::test::TestRequest::patch()
        .uri(&format!("/task/{}", task_id))
        .set_json(serde_json::json!({
            "expected_count": 200
        }))
        .to_request();
    let resp = actix_web::test::call_service(&app, req).await;
    assert!(
        resp.status().is_success(),
        "PATCH should succeed, got {}",
        resp.status()
    );

    // Verify
    let found = get_task_ok(&app, task_id).await;
    assert_eq!(found.expected_count, Some(200));
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
