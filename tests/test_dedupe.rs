#[macro_use]
mod common;
use common::*;

use serde_json::json;

#[tokio::test]
async fn test_dedupe_skip_existing() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    // Create first task with metadata
    let task1 = json!({
        "id": "dedupe-1",
        "name": "Dedupe Task 1",
        "kind": "dedupe-kind",
        "timeout": 60,
        "metadata": {"unique_key": "abc123"},
        "on_start": webhook_action("Start")
    });

    let create_req1 = actix_web::test::TestRequest::post()
        .uri("/task")
        .insert_header(("requester", "test"))
        .set_json(&vec![task1])
        .to_request();
    let create_resp1 = actix_web::test::call_service(&app, create_req1).await;
    assert_eq!(create_resp1.status(), actix_web::http::StatusCode::CREATED);

    // Create second task with same metadata but dedupe strategy
    let task2 = json!({
        "id": "dedupe-2",
        "name": "Dedupe Task 2",
        "kind": "dedupe-kind",
        "timeout": 60,
        "metadata": {"unique_key": "abc123"},
        "on_start": webhook_action("Start"),
        "dedupe_strategy": [{
            "kind": "dedupe-kind",
            "status": "Pending",
            "fields": ["unique_key"]
        }]
    });

    let create_req2 = actix_web::test::TestRequest::post()
        .uri("/task")
        .insert_header(("requester", "test"))
        .set_json(&vec![task2])
        .to_request();
    let create_resp2 = actix_web::test::call_service(&app, create_req2).await;
    // Should return NoContent because task was deduplicated
    assert_eq!(
        create_resp2.status(),
        actix_web::http::StatusCode::NO_CONTENT
    );
}

/// Bug #7: Over-aggressive dedupe when metadata is None.
#[tokio::test]
async fn test_bug7_dedupe_not_over_aggressive_when_metadata_is_none() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    // Step 1: Create first task WITH metadata
    let task1 = json!({
        "id": "dedupe-existing",
        "name": "Existing Task With Metadata",
        "kind": "dedupe-none-kind",
        "timeout": 60,
        "metadata": {"unique_key": "abc123", "project": "test"},
        "on_start": webhook_action("Start")
    });

    let create_req1 = actix_web::test::TestRequest::post()
        .uri("/task")
        .insert_header(("requester", "test"))
        .set_json(&vec![task1])
        .to_request();
    let create_resp1 = actix_web::test::call_service(&app, create_req1).await;
    assert_eq!(create_resp1.status(), actix_web::http::StatusCode::CREATED);

    // Step 2: Create second task WITHOUT metadata, WITH dedupe_strategy referencing fields.
    let task2 = json!({
        "id": "dedupe-no-meta",
        "name": "Task Without Metadata",
        "kind": "dedupe-none-kind",
        "timeout": 60,
        "on_start": webhook_action("Start"),
        "dedupe_strategy": [{
            "kind": "dedupe-none-kind",
            "status": "Pending",
            "fields": ["unique_key"]
        }]
    });

    let create_req2 = actix_web::test::TestRequest::post()
        .uri("/task")
        .insert_header(("requester", "test"))
        .set_json(&vec![task2])
        .to_request();
    let create_resp2 = actix_web::test::call_service(&app, create_req2).await;
    assert_eq!(
        create_resp2.status(),
        actix_web::http::StatusCode::CREATED,
        "Task without metadata should NOT be deduplicated against task with metadata"
    );
}

/// Bug #7 (variant): Two tasks with None metadata should not dedupe each other.
#[tokio::test]
async fn test_bug7_dedupe_two_tasks_both_no_metadata() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    // Create first task without metadata
    let task1 = json!({
        "id": "no-meta-1",
        "name": "No Metadata Task 1",
        "kind": "no-meta-dedupe-kind",
        "timeout": 60,
        "on_start": webhook_action("Start")
    });

    let create_req1 = actix_web::test::TestRequest::post()
        .uri("/task")
        .insert_header(("requester", "test"))
        .set_json(&vec![task1])
        .to_request();
    let create_resp1 = actix_web::test::call_service(&app, create_req1).await;
    assert_eq!(create_resp1.status(), actix_web::http::StatusCode::CREATED);

    // Create second task without metadata, with dedupe strategy referencing fields
    let task2 = json!({
        "id": "no-meta-2",
        "name": "No Metadata Task 2",
        "kind": "no-meta-dedupe-kind",
        "timeout": 60,
        "on_start": webhook_action("Start"),
        "dedupe_strategy": [{
            "kind": "no-meta-dedupe-kind",
            "status": "Pending",
            "fields": ["some_field"]
        }]
    });

    let create_req2 = actix_web::test::TestRequest::post()
        .uri("/task")
        .insert_header(("requester", "test"))
        .set_json(&vec![task2])
        .to_request();
    let create_resp2 = actix_web::test::call_service(&app, create_req2).await;
    assert_eq!(
        create_resp2.status(),
        actix_web::http::StatusCode::CREATED,
        "Second task (no metadata) should be created"
    );
}
