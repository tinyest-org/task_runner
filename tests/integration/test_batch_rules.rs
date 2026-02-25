use crate::common::*;
use serde_json::json;
use task_runner::dtos::UpdateBatchRulesResponseDto;

/// PATCH /batch/{batch_id}/rules updates only tasks of the specified kind.
#[tokio::test]
async fn test_update_batch_rules_by_kind() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    // Create a batch with tasks of two different kinds
    let tasks = vec![
        json!({
            "id": "build",
            "name": "Build Task",
            "kind": "ci",
            "timeout": 60,
            "expected_count": 100,
            "metadata": {"project": "test"},
            "on_start": webhook_action()
        }),
        json!({
            "id": "deploy",
            "name": "Deploy Task",
            "kind": "deploy",
            "timeout": 60,
            "expected_count": 200,
            "metadata": {"project": "test"},
            "on_start": webhook_action(),
            "dependencies": [{"id": "build", "requires_success": true}]
        }),
        json!({
            "id": "build2",
            "name": "Build Task 2",
            "kind": "ci",
            "timeout": 60,
            "expected_count": 300,
            "metadata": {"project": "test"},
            "on_start": webhook_action(),
            "dependencies": [{"id": "build", "requires_success": true}]
        }),
    ];

    let created = create_tasks_ok(&app, &tasks).await;
    let batch_id = created[0].batch_id.unwrap();

    // Update rules only for kind "ci"
    let new_rules = json!({
        "kind": "ci",
        "rules": [{
            "type": "Capacity",
            "max_capacity": 500,
            "matcher": {
                "kind": "ci",
                "status": "Running",
                "fields": ["project"]
            }
        }]
    });

    let req = actix_web::test::TestRequest::patch()
        .uri(&format!("/batch/{}/rules", batch_id))
        .set_json(&new_rules)
        .to_request();
    let resp = actix_web::test::call_service(&app, req).await;
    assert_eq!(resp.status(), actix_web::http::StatusCode::OK);

    let body: UpdateBatchRulesResponseDto = actix_web::test::read_body_json(resp).await;
    assert_eq!(body.batch_id, batch_id);
    assert_eq!(body.kind, "ci");
    assert_eq!(
        body.updated_count, 2,
        "Only the 2 'ci' tasks should be updated"
    );

    // Verify "ci" tasks have the new rule
    let build = get_task_ok(&app, created[0].id).await;
    assert_eq!(build.rules.0.len(), 1, "ci task should have 1 rule");
    match &build.rules.0[0] {
        task_runner::rule::Strategy::Capacity(rule) => {
            assert_eq!(rule.max_capacity, 500);
        }
        other => panic!("Expected Capacity rule, got {:?}", other),
    }

    let build2 = get_task_ok(&app, created[2].id).await;
    assert_eq!(build2.rules.0.len(), 1, "second ci task should have 1 rule");

    // Verify "deploy" task was NOT updated
    let deploy = get_task_ok(&app, created[1].id).await;
    assert!(
        deploy.rules.0.is_empty(),
        "deploy task should have no rules"
    );
}

/// Terminal tasks are skipped even when their kind matches.
#[tokio::test]
async fn test_update_batch_rules_skips_terminal_tasks() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    let tasks = vec![
        json!({
            "id": "will-succeed",
            "name": "Will Succeed",
            "kind": "rule-terminal",
            "timeout": 60,
            "metadata": {},
            "on_start": webhook_action()
        }),
        json!({
            "id": "will-wait",
            "name": "Will Wait",
            "kind": "rule-terminal",
            "timeout": 60,
            "metadata": {},
            "on_start": webhook_action(),
            "dependencies": [{"id": "will-succeed", "requires_success": true}]
        }),
    ];

    let created = create_tasks_ok(&app, &tasks).await;
    let batch_id = created[0].batch_id.unwrap();

    // Complete the first task -> it becomes Success (terminal)
    succeed_task(&state, created[0].id).await;

    // Now update batch rules for kind "rule-terminal"
    let new_rules = json!({
        "kind": "rule-terminal",
        "rules": [{
            "type": "Concurency",
            "max_concurency": 2,
            "matcher": {
                "kind": "rule-terminal",
                "status": "Running",
                "fields": []
            }
        }]
    });

    let req = actix_web::test::TestRequest::patch()
        .uri(&format!("/batch/{}/rules", batch_id))
        .set_json(&new_rules)
        .to_request();
    let resp = actix_web::test::call_service(&app, req).await;
    assert_eq!(resp.status(), actix_web::http::StatusCode::OK);

    let body: UpdateBatchRulesResponseDto = actix_web::test::read_body_json(resp).await;
    assert_eq!(
        body.updated_count, 1,
        "Only 1 non-terminal task should be updated"
    );

    // Verify the succeeded task still has no rules
    let succeeded = get_task_ok(&app, created[0].id).await;
    assert!(
        succeeded.rules.0.is_empty(),
        "Terminal task should keep its original rules"
    );

    // Verify the waiting child got the new rule
    let child = get_task_ok(&app, created[1].id).await;
    assert_eq!(
        child.rules.0.len(),
        1,
        "Waiting child should have the new rule"
    );
}

/// Empty rules array removes all rules for the specified kind.
#[tokio::test]
async fn test_update_batch_rules_with_empty_removes_rules() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    let tasks = vec![json!({
        "id": "has-rules",
        "name": "Task With Rules",
        "kind": "clear-rules",
        "timeout": 60,
        "expected_count": 100,
        "metadata": {},
        "on_start": webhook_action(),
        "rules": [{
            "type": "Capacity",
            "max_capacity": 500,
            "matcher": { "kind": "clear-rules", "status": "Running", "fields": [] }
        }]
    })];

    let created = create_tasks_ok(&app, &tasks).await;
    let batch_id = created[0].batch_id.unwrap();

    // Verify task has rule
    let task = get_task_ok(&app, created[0].id).await;
    assert_eq!(task.rules.0.len(), 1);

    // Clear rules for kind "clear-rules"
    let req = actix_web::test::TestRequest::patch()
        .uri(&format!("/batch/{}/rules", batch_id))
        .set_json(&json!({ "kind": "clear-rules", "rules": [] }))
        .to_request();
    let resp = actix_web::test::call_service(&app, req).await;
    assert_eq!(resp.status(), actix_web::http::StatusCode::OK);

    let task = get_task_ok(&app, created[0].id).await;
    assert!(
        task.rules.0.is_empty(),
        "Rules should be empty after clearing"
    );
}

/// Returns 404 for non-existent batch.
#[tokio::test]
async fn test_update_batch_rules_not_found() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    let fake_batch_id = uuid::Uuid::new_v4();
    let req = actix_web::test::TestRequest::patch()
        .uri(&format!("/batch/{}/rules", fake_batch_id))
        .set_json(&json!({ "kind": "anything", "rules": [] }))
        .to_request();
    let resp = actix_web::test::call_service(&app, req).await;
    assert_eq!(resp.status(), actix_web::http::StatusCode::NOT_FOUND);
}

/// Returns 400 for invalid rules or empty kind.
#[tokio::test]
async fn test_update_batch_rules_validation_error() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    let tasks = vec![json!({
        "id": "valid-task",
        "name": "Valid Task",
        "kind": "validate-rules",
        "timeout": 60,
        "metadata": {},
        "on_start": webhook_action()
    })];

    let created = create_tasks_ok(&app, &tasks).await;
    let batch_id = created[0].batch_id.unwrap();

    // Invalid: empty kind
    let req = actix_web::test::TestRequest::patch()
        .uri(&format!("/batch/{}/rules", batch_id))
        .set_json(&json!({ "kind": "", "rules": [] }))
        .to_request();
    let resp = actix_web::test::call_service(&app, req).await;
    assert_eq!(resp.status(), actix_web::http::StatusCode::BAD_REQUEST);

    // Invalid: Capacity rule with matcher.status = Pending
    let req = actix_web::test::TestRequest::patch()
        .uri(&format!("/batch/{}/rules", batch_id))
        .set_json(&json!({
            "kind": "validate-rules",
            "rules": [{
                "type": "Capacity",
                "max_capacity": 500,
                "matcher": { "kind": "validate-rules", "status": "Pending", "fields": [] }
            }]
        }))
        .to_request();
    let resp = actix_web::test::call_service(&app, req).await;
    assert_eq!(resp.status(), actix_web::http::StatusCode::BAD_REQUEST);

    // Invalid: Concurency rule with max_concurency = 0
    let req = actix_web::test::TestRequest::patch()
        .uri(&format!("/batch/{}/rules", batch_id))
        .set_json(&json!({
            "kind": "validate-rules",
            "rules": [{
                "type": "Concurency",
                "max_concurency": 0,
                "matcher": { "kind": "validate-rules", "status": "Running", "fields": [] }
            }]
        }))
        .to_request();
    let resp = actix_web::test::call_service(&app, req).await;
    assert_eq!(resp.status(), actix_web::http::StatusCode::BAD_REQUEST);
}
