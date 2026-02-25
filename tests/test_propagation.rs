#[macro_use]
mod common;
use common::*;

use task_runner::models::StatusKind;

#[tokio::test]
async fn test_child_transitions_when_parent_succeeds() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    let tasks = vec![
        task_json("parent-success", "Parent Task", "transition-test"),
        task_with_deps(
            "child-success",
            "Child Task",
            "transition-test",
            vec![("parent-success", true)],
        ),
    ];

    let created = create_tasks_ok(&app, &tasks).await;
    let parent_id = created[0].id;
    let child_id = created[1].id;

    assert_eq!(created[0].status, StatusKind::Pending);
    assert_eq!(created[1].status, StatusKind::Waiting);

    succeed_task(&state, parent_id).await;

    assert_task_status(&app, parent_id, StatusKind::Success, "parent").await;
    assert_task_status(
        &app,
        child_id,
        StatusKind::Pending,
        "Child should transition from Waiting to Pending when parent succeeds",
    )
    .await;
}

#[tokio::test]
async fn test_child_fails_when_required_parent_fails() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    let tasks = vec![
        task_json("failing-parent", "Failing Parent", "fail-test"),
        task_with_deps(
            "dependent-child",
            "Dependent Child",
            "fail-test",
            vec![("failing-parent", true)],
        ),
    ];

    let created = create_tasks_ok(&app, &tasks).await;
    let parent_id = created[0].id;
    let child_id = created[1].id;

    fail_task(&state, parent_id, "Intentional test failure").await;

    assert_task_status(
        &app,
        child_id,
        StatusKind::Failure,
        "Child should be marked as Failure when required parent fails",
    )
    .await;
}

#[tokio::test]
async fn test_child_proceeds_when_optional_parent_fails() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    let tasks = vec![
        task_json("required-parent", "Required Parent", "optional-test"),
        task_json("optional-parent", "Optional Parent", "optional-test"),
        task_with_deps(
            "mixed-child",
            "Child with Mixed Deps",
            "optional-test",
            vec![
                ("required-parent", true),  // must succeed
                ("optional-parent", false), // just needs to finish
            ],
        ),
    ];

    let created = create_tasks_ok(&app, &tasks).await;
    let required_id = created[0].id;
    let optional_id = created[1].id;
    let child_id = created[2].id;

    // Fail the optional parent
    fail_task(&state, optional_id, "Optional failure").await;

    // Child should still be Waiting (required parent not done)
    assert_task_status(
        &app,
        child_id,
        StatusKind::Waiting,
        "Child should still be Waiting when only optional parent finished",
    )
    .await;

    // Complete the required parent
    succeed_task(&state, required_id).await;

    // Child should now be Pending
    assert_task_status(
        &app,
        child_id,
        StatusKind::Pending,
        "Child should be Pending after required parent succeeds (even if optional failed)",
    )
    .await;
}

#[tokio::test]
async fn test_multi_level_dag_propagation() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    // Create: ingest_1, ingest_2 -> cluster -> refresh
    let tasks = vec![
        task_json("ingest-1", "Ingest 1", "multi-level"),
        task_json("ingest-2", "Ingest 2", "multi-level"),
        task_with_deps(
            "cluster",
            "Cluster",
            "multi-level",
            vec![("ingest-1", true), ("ingest-2", true)],
        ),
        task_with_deps("refresh", "Refresh", "multi-level", vec![("cluster", true)]),
    ];

    let created = create_tasks_ok(&app, &tasks).await;
    let ingest1_id = created[0].id;
    let ingest2_id = created[1].id;
    let cluster_id = created[2].id;
    let refresh_id = created[3].id;

    assert_eq!(created[0].status, StatusKind::Pending);
    assert_eq!(created[1].status, StatusKind::Pending);
    assert_eq!(created[2].status, StatusKind::Waiting);
    assert_eq!(created[3].status, StatusKind::Waiting);

    // Complete ingest_1
    succeed_task(&state, ingest1_id).await;

    // Cluster should still be Waiting (ingest_2 not done)
    assert_task_status(
        &app,
        cluster_id,
        StatusKind::Waiting,
        "cluster still waiting",
    )
    .await;

    // Complete ingest_2
    succeed_task(&state, ingest2_id).await;

    // Cluster should now be Pending
    assert_task_status(&app, cluster_id, StatusKind::Pending, "cluster now pending").await;

    // Refresh should still be Waiting
    assert_task_status(
        &app,
        refresh_id,
        StatusKind::Waiting,
        "refresh still waiting",
    )
    .await;

    // Complete cluster
    succeed_task(&state, cluster_id).await;

    // Refresh should now be Pending
    assert_task_status(&app, refresh_id, StatusKind::Pending, "refresh now pending").await;
}

#[tokio::test]
async fn test_failure_propagation_through_chain() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    // Create: root -> middle -> leaf (all require success)
    let tasks = vec![
        task_json("root", "Root Task", "propagation"),
        task_with_deps("middle", "Middle Task", "propagation", vec![("root", true)]),
        task_with_deps("leaf", "Leaf Task", "propagation", vec![("middle", true)]),
    ];

    let created = create_tasks_ok(&app, &tasks).await;
    let root_id = created[0].id;
    let middle_id = created[1].id;
    let leaf_id = created[2].id;

    // Fail the root task
    fail_task(&state, root_id, "Root failure").await;

    // Middle should be marked as Failure
    assert_task_status(&app, middle_id, StatusKind::Failure, "middle should fail").await;

    // Leaf should also be marked as Failure (cascading propagation)
    let leaf = get_task_ok(&app, leaf_id).await;
    assert_eq!(
        leaf.status,
        StatusKind::Failure,
        "Leaf should be Failure due to cascading failure propagation from root -> middle -> leaf"
    );
    assert!(
        leaf.failure_reason.is_some(),
        "Leaf should have a failure reason explaining why it failed"
    );
}

/// Test the exact DAG pattern from the CI/CD pipeline:
/// Build -> (Unit Tests, Integration Tests, Lint) -> Deploy
/// When Build fails, all downstream tasks should fail via cascading propagation
#[tokio::test]
async fn test_cicd_pipeline_failure_cascade() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    let tasks = vec![
        task_json("build", "Build Project", "build"),
        task_with_deps("unit", "Unit Tests", "test", vec![("build", true)]),
        task_with_deps(
            "integration",
            "Integration Tests",
            "test",
            vec![("build", true)],
        ),
        task_with_deps("lint", "Lint Check", "lint", vec![("build", false)]),
        task_with_deps(
            "deploy",
            "Deploy to Staging",
            "deploy",
            vec![("unit", true), ("integration", true), ("lint", false)],
        ),
    ];

    let created = create_tasks_ok(&app, &tasks).await;
    assert_eq!(created.len(), 5);
    let build_id = created[0].id;
    let unit_id = created[1].id;
    let integration_id = created[2].id;
    let lint_id = created[3].id;
    let deploy_id = created[4].id;

    assert_eq!(created[0].status, StatusKind::Pending);
    assert_eq!(created[1].status, StatusKind::Waiting);
    assert_eq!(created[2].status, StatusKind::Waiting);
    assert_eq!(created[3].status, StatusKind::Waiting);
    assert_eq!(created[4].status, StatusKind::Waiting);

    // Fail the build task
    fail_task(&state, build_id, "Build compilation failed").await;

    assert_task_status(
        &app,
        unit_id,
        StatusKind::Failure,
        "Unit Tests should fail because required parent (build) failed",
    )
    .await;
    assert_task_status(
        &app,
        integration_id,
        StatusKind::Failure,
        "Integration Tests should fail because required parent (build) failed",
    )
    .await;
    // Lint would normally become Pending (build finished, doesn't require success),
    // but dead-end detection cancels it because its only child (deploy) is already terminal.
    assert_task_status(
        &app,
        lint_id,
        StatusKind::Canceled,
        "Lint should be Canceled (dead-end: its only child deploy is already terminal)",
    )
    .await;

    let deploy = get_task_ok(&app, deploy_id).await;
    assert_eq!(
        deploy.status,
        StatusKind::Failure,
        "Deploy should fail via cascading: build failed -> unit/integration failed -> deploy failed"
    );
    assert!(
        deploy.failure_reason.is_some(),
        "Deploy should have failure reason"
    );
}
