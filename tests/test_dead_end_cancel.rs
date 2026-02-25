#[macro_use]
mod common;
use common::*;

use serde_json::json;
use task_runner::models::StatusKind;

/// Test basic dead-end ancestor cancellation.
///
/// DAG: A1, A2, A3 → B (all with requires_success=true)
///
/// When A2 fails:
/// - B is marked Failure by downward propagation (required parent failed)
/// - A1 (Pending) and A3 (Pending) have B as their only child, and B is now terminal
/// - Dead-end detection should cancel A1 and A3
///
/// Note: A1 and A3 are "parents" of B in the DAG (B depends on them).
#[tokio::test]
async fn test_dead_end_cancel_basic() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    // Create: A1, A2, A3 (all Pending), B depends on all three (Waiting)
    let tasks = vec![
        task_json("a1", "Task A1", "worker"),
        task_json("a2", "Task A2", "worker"),
        task_json("a3", "Task A3", "worker"),
        task_with_deps(
            "b",
            "Task B",
            "aggregator",
            vec![("a1", true), ("a2", true), ("a3", true)],
        ),
    ];

    let created = create_tasks_ok(&app, &tasks).await;
    let a1_id = created[0].id;
    let a2_id = created[1].id;
    let a3_id = created[2].id;
    let b_id = created[3].id;

    assert_eq!(created[0].status, StatusKind::Pending);
    assert_eq!(created[1].status, StatusKind::Pending);
    assert_eq!(created[2].status, StatusKind::Pending);
    assert_eq!(created[3].status, StatusKind::Waiting);

    // Fail A2 → B should fail by propagation, then A1 and A3 should be dead-end canceled
    fail_task(&state, a2_id, "A2 intentional failure").await;

    assert_task_status(
        &app,
        a2_id,
        StatusKind::Failure,
        "A2 should be Failure (explicitly failed)",
    )
    .await;

    assert_task_status(
        &app,
        b_id,
        StatusKind::Failure,
        "B should be Failure (required parent A2 failed)",
    )
    .await;

    // A1 and A3 should be canceled by dead-end detection:
    // Their only child (B) is now terminal, so their work is useless.
    assert_task_status(
        &app,
        a1_id,
        StatusKind::Canceled,
        "A1 should be Canceled by dead-end detection (all children terminal)",
    )
    .await;

    assert_task_status(
        &app,
        a3_id,
        StatusKind::Canceled,
        "A3 should be Canceled by dead-end detection (all children terminal)",
    )
    .await;

    // Verify failure reason on dead-end canceled tasks
    let a1 = get_task_ok(&app, a1_id).await;
    assert!(
        a1.failure_reason
            .as_deref()
            .unwrap_or("")
            .contains("All child tasks already terminated"),
        "Dead-end canceled task should have descriptive failure reason"
    );
}

/// Test multi-level dead-end cascade upward.
/// A(Pending) → B(Waiting) → C(Waiting). Fail A → B fails (propagation) → C fails (propagation).
/// All tasks become terminal via downward propagation. No dead-end detection needed here.
///
/// Real multi-level dead-end test: separate DAGs where non-terminal ancestors exist.
/// X(Pending), Y(Pending) → Z(Waiting). Z depends on both.
/// Fail X → Z fails → Y (Pending, only child Z is terminal) should be dead-end canceled.
#[tokio::test]
async fn test_dead_end_cancel_multi_level() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    // P(Pending) → Q(Waiting) → R(Waiting)
    // P2(Pending) → Q (also a parent of Q)
    // When P fails: Q fails (propagation), R fails (cascade).
    // P2 has only child Q which is now terminal → P2 dead-end canceled.
    let tasks = vec![
        task_json("p", "Task P", "multi-level"),
        task_json("p2", "Task P2", "multi-level"),
        task_with_deps(
            "q",
            "Task Q",
            "multi-level",
            vec![("p", true), ("p2", true)],
        ),
        task_with_deps("r", "Task R", "multi-level", vec![("q", true)]),
    ];

    let created = create_tasks_ok(&app, &tasks).await;
    let p_id = created[0].id;
    let p2_id = created[1].id;
    let q_id = created[2].id;
    let r_id = created[3].id;

    // Fail P → Q fails → R fails
    // Then P2 (only child Q, which is terminal) → dead-end canceled
    fail_task(&state, p_id, "P failed").await;

    assert_task_status(&app, q_id, StatusKind::Failure, "Q fails by propagation").await;
    assert_task_status(&app, r_id, StatusKind::Failure, "R fails by cascade").await;
    assert_task_status(
        &app,
        p2_id,
        StatusKind::Canceled,
        "P2 dead-end canceled (only child Q is terminal)",
    )
    .await;
}

/// Test that dead-end cancellation does NOT happen when a parent has non-terminal children.
///
/// DAG: A → B, A → C. If B fails but C is still non-terminal, A should NOT be canceled.
#[tokio::test]
async fn test_dead_end_cancel_partial_children() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    let tasks = vec![
        task_json("a", "Task A", "partial-test"),
        task_with_deps("b", "Task B", "partial-test", vec![("a", false)]),
        task_with_deps("c", "Task C", "partial-test", vec![("a", false)]),
    ];

    let created = create_tasks_ok(&app, &tasks).await;
    let a_id = created[0].id;
    let b_id = created[1].id;
    let c_id = created[2].id;

    // Complete A → B and C become Pending
    succeed_task(&state, a_id).await;
    assert_task_status(&app, b_id, StatusKind::Pending, "B Pending").await;
    assert_task_status(&app, c_id, StatusKind::Pending, "C Pending").await;

    // Fail B. C is still Pending (non-terminal).
    // A is already Success, but even if it weren't, it should NOT be canceled
    // because C (its other child) is still non-terminal.
    fail_task(&state, b_id, "B failed").await;

    assert_task_status(&app, a_id, StatusKind::Success, "A stays Success").await;
    assert_task_status(
        &app,
        c_id,
        StatusKind::Pending,
        "C still Pending (not affected)",
    )
    .await;
}

/// Test that Paused tasks are eligible for dead-end cancellation.
///
/// X(Paused) → Y(Pending). Fail Y → X should be canceled (it's Paused but its only child is terminal).
#[tokio::test]
async fn test_dead_end_cancel_paused() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    let tasks = vec![
        task_json("x", "Parent X", "paused-test"),
        task_with_deps("y", "Child Y", "paused-test", vec![("x", true)]),
    ];

    let created = create_tasks_ok(&app, &tasks).await;
    let x_id = created[0].id;
    let y_id = created[1].id;

    // Pause X via HTTP API
    let req = actix_web::test::TestRequest::patch()
        .uri(&format!("/task/pause/{}", x_id))
        .to_request();
    let resp = actix_web::test::call_service(&app, req).await;
    assert!(resp.status().is_success(), "Pause should succeed");
    assert_task_status(&app, x_id, StatusKind::Paused, "X Paused").await;

    // Force Y to Pending (bypass wait counters via SQL)
    let mut conn = state.pool.get().await.unwrap();
    {
        use diesel_async::RunQueryDsl;
        diesel::sql_query(
            "UPDATE task SET status = 'pending', wait_finished = 0, wait_success = 0 WHERE id = $1",
        )
        .bind::<diesel::sql_types::Uuid, _>(y_id)
        .execute(&mut *conn)
        .await
        .unwrap();
    }

    // Fail Y → dead-end should detect X (Paused) has only terminal child and cancel it
    fail_task(&state, y_id, "Y failed").await;

    assert_task_status(
        &app,
        x_id,
        StatusKind::Canceled,
        "Paused parent X should be canceled by dead-end detection",
    )
    .await;
}

/// Test that leaf tasks (no children) are never affected by dead-end detection.
#[tokio::test]
async fn test_dead_end_cancel_no_children() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    let tasks = vec![task_json("leaf", "Leaf Task", "leaf-test")];

    let created = create_tasks_ok(&app, &tasks).await;
    let leaf_id = created[0].id;

    // Fail it
    fail_task(&state, leaf_id, "Leaf failed").await;

    // It should just be Failure (no dead-end cascade since it has no parents)
    assert_task_status(&app, leaf_id, StatusKind::Failure, "Leaf is Failure").await;
}

/// Test dead-end barrier: G(Pending) → A(barrier, Pending) → B(Waiting).
/// When we make B terminal and A is the dead-end:
/// - A IS canceled (its only child B is terminal)
/// - But G is NOT checked because A is a barrier
///
/// To test with non-terminal parents, we use requires_success=false links
/// and force states via SQL.
#[tokio::test]
async fn test_dead_end_cancel_barrier() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    // G(Pending) → A(barrier, Waiting) → B(Waiting)
    // Use requires_success=false so that G failing doesn't cascade to A.
    let g_task = task_json("g", "Gate", "barrier-test");

    let mut a_task = task_with_deps("a", "Barrier", "barrier-test", vec![("g", false)]);
    a_task["dead_end_barrier"] = json!(true);

    let b_task = task_with_deps("b", "Leaf", "barrier-test", vec![("a", false)]);

    let tasks = vec![g_task, a_task, b_task];

    let created = create_tasks_ok(&app, &tasks).await;
    let g_id = created[0].id;
    let a_id = created[1].id;
    let b_id = created[2].id;

    // Verify A has dead_end_barrier set
    let a_detail = get_task_ok(&app, a_id).await;
    assert!(
        a_detail.dead_end_barrier,
        "A should have dead_end_barrier=true"
    );

    // Complete G → A becomes Pending (wait_finished decremented, becomes 0)
    succeed_task(&state, g_id).await;
    assert_task_status(&app, a_id, StatusKind::Pending, "A Pending").await;

    // Force A to Running and B to Pending via SQL
    let mut conn = state.pool.get().await.unwrap();
    {
        use diesel_async::RunQueryDsl;
        diesel::sql_query(
            "UPDATE task SET status = 'running', started_at = now(), last_updated = now() WHERE id = $1",
        )
        .bind::<diesel::sql_types::Uuid, _>(a_id)
        .execute(&mut *conn)
        .await
        .unwrap();
        diesel::sql_query(
            "UPDATE task SET status = 'pending', wait_finished = 0, wait_success = 0 WHERE id = $1",
        )
        .bind::<diesel::sql_types::Uuid, _>(b_id)
        .execute(&mut *conn)
        .await
        .unwrap();
    }

    // Fail B → dead-end detection:
    // A (Running, barrier) has only child B which is terminal → A gets canceled
    // But A is a barrier → G is NOT checked (even though G is already terminal here)
    fail_task(&state, b_id, "B failed").await;

    assert_task_status(
        &app,
        a_id,
        StatusKind::Canceled,
        "A (barrier) should be canceled by dead-end detection",
    )
    .await;

    // G should remain Success (was already terminal, and barrier would have stopped cascade anyway)
    assert_task_status(
        &app,
        g_id,
        StatusKind::Success,
        "G should NOT be affected (barrier stops cascade)",
    )
    .await;
}
