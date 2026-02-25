#[macro_use]
mod common;
use common::*;

use serde_json::json;
use task_runner::db_operation::{ClaimResult, claim_task_with_rules};
use task_runner::models::{StatusKind, Task};
use task_runner::rule::{ConcurencyRule, Matcher, Rules, Strategy};

/// Helper: build a minimal Task struct from a TaskDto and known metadata/rules.
/// Only the fields used by `claim_task_with_rules` matter (id, kind, metadata, start_condition).
fn make_task_for_claim(
    id: uuid::Uuid,
    kind: &str,
    metadata: serde_json::Value,
    rules: Rules,
) -> Task {
    Task {
        id,
        name: String::new(),
        kind: kind.to_string(),
        status: StatusKind::Pending,
        timeout: 60,
        created_at: chrono::Utc::now(),
        started_at: None,
        last_updated: chrono::Utc::now(),
        metadata,
        ended_at: None,
        start_condition: rules,
        wait_success: 0,
        wait_finished: 0,
        success: 0,
        failures: 0,
        failure_reason: None,
        batch_id: None,
        expected_count: None,
        dead_end_barrier: false,
    }
}

/// Helper: build the Rules for a concurrency rule with max_concurency and optional metadata fields.
fn concurrency_rules(kind: &str, max: i32, fields: Vec<&str>) -> Rules {
    Rules(vec![Strategy::Concurency(ConcurencyRule {
        max_concurency: max,
        matcher: Matcher {
            kind: kind.to_string(),
            status: StatusKind::Running,
            fields: fields.into_iter().map(String::from).collect(),
        },
    })])
}

// =============================================================================
// Storage tests
// =============================================================================

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
        "on_start": webhook_action(),
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

/// Merged test: rules are stored per-project AND different projects get independent rules.
#[tokio::test]
async fn test_concurrency_rules_stored_per_project() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    let tasks = vec![
        json!({
            "id": "proj-task-1",
            "name": "Project 1 Task",
            "kind": "project-concurrent",
            "timeout": 60,
            "metadata": {"projectId": 111},
            "on_start": webhook_action(),
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
            "name": "Project 1 Task 2 (same project)",
            "kind": "project-concurrent",
            "timeout": 60,
            "metadata": {"projectId": 111},
            "on_start": webhook_action(),
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
            "id": "proj-task-3",
            "name": "Project 2 Task (different project)",
            "kind": "project-concurrent",
            "timeout": 60,
            "metadata": {"projectId": 222},
            "on_start": webhook_action(),
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
    assert_eq!(created.len(), 3);
    for t in &created {
        assert_eq!(t.status, StatusKind::Pending);
        assert!(!t.rules.0.is_empty(), "task {} should have rules", t.name);
    }
}

// =============================================================================
// Enforcement tests
// =============================================================================

/// Concurrency enforcement: with max_concurency=1 and same kind,
/// the second task should be blocked when the first is Running.
#[tokio::test]
async fn test_concurrency_blocks_when_limit_reached() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    let rules_json = json!([{
        "type": "Concurency",
        "matcher": {
            "kind": "enforce-kind",
            "status": "Running",
            "fields": []
        },
        "max_concurency": 1
    }]);

    let tasks = vec![
        json!({
            "id": "enforce-1",
            "name": "Enforce Task 1",
            "kind": "enforce-kind",
            "timeout": 60,
            "metadata": {"test": true},
            "on_start": webhook_action(),
            "rules": rules_json.clone()
        }),
        json!({
            "id": "enforce-2",
            "name": "Enforce Task 2",
            "kind": "enforce-kind",
            "timeout": 60,
            "metadata": {"test": true},
            "on_start": webhook_action(),
            "rules": rules_json.clone()
        }),
    ];

    let created = create_tasks_ok(&app, &tasks).await;
    let id1 = created[0].id;
    let id2 = created[1].id;

    let rules = concurrency_rules("enforce-kind", 1, vec![]);
    let metadata = json!({"test": true});

    // Claim first task — should succeed
    let mut conn = state.pool.get().await.unwrap();
    let t1 = make_task_for_claim(id1, "enforce-kind", metadata.clone(), rules.clone());
    let result1 = claim_task_with_rules(&mut conn, &t1).await.unwrap();
    assert_eq!(
        result1,
        ClaimResult::Claimed,
        "First task should be claimed"
    );

    // Try to claim second task — should be blocked (max_concurency=1, one already Claimed)
    let t2 = make_task_for_claim(id2, "enforce-kind", metadata.clone(), rules.clone());
    let result2 = claim_task_with_rules(&mut conn, &t2).await.unwrap();
    assert_eq!(
        result2,
        ClaimResult::RuleBlocked,
        "Second task should be blocked by concurrency rule"
    );

    // Verify statuses
    assert_task_status(&app, id1, StatusKind::Claimed, "first task Claimed").await;
    assert_task_status(&app, id2, StatusKind::Pending, "second task still Pending").await;

    // Mark first task as Running then complete it
    task_runner::db_operation::mark_task_running(&mut conn, &id1)
        .await
        .unwrap();
    let update_result = task_runner::db_operation::update_running_task(
        &state.action_executor,
        &mut conn,
        id1,
        success_dto(),
        true,
    )
    .await
    .unwrap();
    assert_eq!(
        update_result,
        task_runner::db_operation::UpdateTaskResult::Updated,
        "First task should be updated to Success"
    );

    // Now claim second task — should succeed
    let result3 = claim_task_with_rules(&mut conn, &t2).await.unwrap();
    assert_eq!(
        result3,
        ClaimResult::Claimed,
        "Second task should be claimable after first completes"
    );

    assert_task_status(&app, id2, StatusKind::Claimed, "second task now Claimed").await;
}

/// Concurrency enforcement with metadata fields: tasks with different metadata
/// should NOT block each other even with max_concurency=1.
#[tokio::test]
async fn test_concurrency_allows_different_metadata() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    let make_task = |id: &str, project_id: i32| {
        json!({
            "id": id,
            "name": format!("Project {} Task", project_id),
            "kind": "meta-enforce",
            "timeout": 60,
            "metadata": {"projectId": project_id},
            "on_start": webhook_action(),
            "rules": [{
                "type": "Concurency",
                "matcher": {
                    "kind": "meta-enforce",
                    "status": "Running",
                    "fields": ["projectId"]
                },
                "max_concurency": 1
            }]
        })
    };

    let tasks = vec![make_task("meta-1", 111), make_task("meta-2", 222)];
    let created = create_tasks_ok(&app, &tasks).await;
    let id1 = created[0].id;
    let id2 = created[1].id;

    let rules = concurrency_rules("meta-enforce", 1, vec!["projectId"]);

    // Claim first task (project 111)
    let mut conn = state.pool.get().await.unwrap();
    let t1 = make_task_for_claim(
        id1,
        "meta-enforce",
        json!({"projectId": 111}),
        rules.clone(),
    );
    let result1 = claim_task_with_rules(&mut conn, &t1).await.unwrap();
    assert_eq!(result1, ClaimResult::Claimed);

    // Claim second task (project 222) — different metadata, should NOT be blocked
    let t2 = make_task_for_claim(
        id2,
        "meta-enforce",
        json!({"projectId": 222}),
        rules.clone(),
    );
    let result2 = claim_task_with_rules(&mut conn, &t2).await.unwrap();
    assert_eq!(
        result2,
        ClaimResult::Claimed,
        "Tasks with different projectId should not block each other"
    );

    assert_task_status(&app, id1, StatusKind::Claimed, "task 1 Claimed").await;
    assert_task_status(&app, id2, StatusKind::Claimed, "task 2 Claimed").await;
}

/// Concurrency enforcement with metadata fields: tasks with the SAME metadata
/// should block each other with max_concurency=1.
#[tokio::test]
async fn test_concurrency_blocks_same_metadata() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    let make_task = |id: &str| {
        json!({
            "id": id,
            "name": format!("Same Project Task {}", id),
            "kind": "same-meta",
            "timeout": 60,
            "metadata": {"projectId": 999},
            "on_start": webhook_action(),
            "rules": [{
                "type": "Concurency",
                "matcher": {
                    "kind": "same-meta",
                    "status": "Running",
                    "fields": ["projectId"]
                },
                "max_concurency": 1
            }]
        })
    };

    let tasks = vec![make_task("same-1"), make_task("same-2")];
    let created = create_tasks_ok(&app, &tasks).await;
    let id1 = created[0].id;
    let id2 = created[1].id;

    let rules = concurrency_rules("same-meta", 1, vec!["projectId"]);
    let metadata = json!({"projectId": 999});

    let mut conn = state.pool.get().await.unwrap();
    let t1 = make_task_for_claim(id1, "same-meta", metadata.clone(), rules.clone());
    let result1 = claim_task_with_rules(&mut conn, &t1).await.unwrap();
    assert_eq!(result1, ClaimResult::Claimed);

    let t2 = make_task_for_claim(id2, "same-meta", metadata.clone(), rules.clone());
    let result2 = claim_task_with_rules(&mut conn, &t2).await.unwrap();
    assert_eq!(
        result2,
        ClaimResult::RuleBlocked,
        "Tasks with same projectId should block each other"
    );
}
