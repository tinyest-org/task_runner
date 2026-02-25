use crate::common::*;

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

/// Batch updates must persist counters even on terminal tasks.
/// Previously, the batch updater SQL filtered out terminal statuses, which caused
/// a race condition: if PUT (batch counter) was in-flight when PATCH (status=Success)
/// landed, the buffered counts were silently dropped — resulting in tasks at "Success"
/// with incomplete counters.
#[tokio::test]
async fn test_batch_update_persists_counters_on_terminal_tasks() {
    let (_g, test_state) = setup_test_app_with_batch_updater().await;
    let state = test_state.state;
    let app = test_service!(state);

    // Success case: counters must still be applied
    let tasks = vec![task_json(
        "terminal-success",
        "Terminal Success",
        "batch-test",
    )];
    let created = create_tasks_ok(&app, &tasks).await;
    let task_id = created[0].id;

    succeed_task(&state, task_id).await;

    let update_req = actix_web::test::TestRequest::put()
        .uri(&format!("/task/{}", task_id))
        .set_json(&json!({"new_success": 3, "new_failures": 1}))
        .to_request();
    let update_resp = actix_web::test::call_service(&app, update_req).await;
    assert_eq!(
        update_resp.status(),
        actix_web::http::StatusCode::ACCEPTED,
        "Batch update should be accepted even if task is terminal"
    );

    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    let updated: TaskDto = get_task_ok(&app, task_id).await;
    assert_eq!(
        updated.success, 3,
        "Success counter must be persisted even on terminal task"
    );
    assert_eq!(
        updated.failures, 1,
        "Failure counter must be persisted even on terminal task"
    );

    // Canceled case: counters must still be applied
    let tasks = vec![task_json(
        "terminal-canceled",
        "Terminal Canceled",
        "batch-test",
    )];
    let created = create_tasks_ok(&app, &tasks).await;
    let cancel_id = created[0].id;

    let cancel_req = actix_web::test::TestRequest::delete()
        .uri(&format!("/task/{}", cancel_id))
        .to_request();
    let cancel_resp = actix_web::test::call_service(&app, cancel_req).await;
    assert!(cancel_resp.status().is_success());

    let update_req = actix_web::test::TestRequest::put()
        .uri(&format!("/task/{}", cancel_id))
        .set_json(&json!({"new_success": 2, "new_failures": 2}))
        .to_request();
    let update_resp = actix_web::test::call_service(&app, update_req).await;
    assert_eq!(
        update_resp.status(),
        actix_web::http::StatusCode::ACCEPTED,
        "Batch update should be accepted even if task is canceled"
    );

    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    let updated: TaskDto = get_task_ok(&app, cancel_id).await;
    assert_eq!(
        updated.success, 2,
        "Success counter must be persisted even on canceled task"
    );
    assert_eq!(
        updated.failures, 2,
        "Failure counter must be persisted even on canceled task"
    );
}
