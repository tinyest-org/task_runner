#[macro_use]
mod common;
use common::*;

use serde_json::json;
use task_runner::dtos::TaskDto;

#[tokio::test]
async fn test_batch_update_increments_counters() {
    let (_g, test_state) = setup_test_app_with_batch_updater().await;
    let state = test_state.state;
    let app = test_service!(state);

    let tasks = vec![task_json("batch-test", "Batch Update Test", "batch-test")];
    let created = create_tasks_ok(&app, &tasks).await;
    let task_id = created[0].id;

    assert_eq!(created[0].success, 0);
    assert_eq!(created[0].failures, 0);

    // Send batch update via PUT endpoint
    let update_req = actix_web::test::TestRequest::put()
        .uri(&format!("/task/{}", task_id))
        .set_json(&json!({"new_success": 5, "new_failures": 2}))
        .to_request();
    let update_resp = actix_web::test::call_service(&app, update_req).await;
    assert_eq!(
        update_resp.status(),
        actix_web::http::StatusCode::ACCEPTED,
        "Batch update should be accepted"
    );

    // Wait for batch updater to process (runs every 100ms)
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;

    let updated: TaskDto = get_task_ok(&app, task_id).await;
    assert_eq!(
        updated.success, 5,
        "Success counter should be incremented to 5"
    );
    assert_eq!(
        updated.failures, 2,
        "Failures counter should be incremented to 2"
    );
}

#[tokio::test]
async fn test_batch_update_accumulates_multiple_updates() {
    let (_g, test_state) = setup_test_app_with_batch_updater().await;
    let state = test_state.state;
    let app = test_service!(state);

    let tasks = vec![task_json("batch-multi", "Batch Multi Test", "batch-test")];
    let created = create_tasks_ok(&app, &tasks).await;
    let task_id = created[0].id;

    // Send multiple rapid updates
    for _ in 0..10 {
        let update_req = actix_web::test::TestRequest::put()
            .uri(&format!("/task/{}", task_id))
            .set_json(&json!({"new_success": 1, "new_failures": 0}))
            .to_request();
        actix_web::test::call_service(&app, update_req).await;
    }

    // Wait for batch updater to process
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;

    let updated: TaskDto = get_task_ok(&app, task_id).await;
    assert_eq!(
        updated.success, 10,
        "Success counter should accumulate to 10 from 10 updates of +1 each"
    );
}

#[tokio::test]
async fn test_batch_update_rejects_zero_counters() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    let tasks = vec![task_json("batch-reject", "Batch Reject Test", "batch-test")];
    let created = create_tasks_ok(&app, &tasks).await;
    let task_id = created[0].id;

    // Try to send update with zero counters
    let update_req = actix_web::test::TestRequest::put()
        .uri(&format!("/task/{}", task_id))
        .set_json(&json!({"new_success": 0, "new_failures": 0}))
        .to_request();
    let update_resp = actix_web::test::call_service(&app, update_req).await;

    assert_eq!(
        update_resp.status(),
        actix_web::http::StatusCode::BAD_REQUEST,
        "Should reject updates with all zero counters"
    );
}
