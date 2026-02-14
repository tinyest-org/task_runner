#[macro_use]
mod common;
use common::*;

use task_runner::models::StatusKind;

#[tokio::test]
async fn test_task_with_single_dependency() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    let tasks = vec![
        task_json("parent", "Parent Task", "parent-kind"),
        task_with_deps("child", "Child Task", "child-kind", vec![("parent", true)]),
    ];

    let body = create_tasks_ok(&app, &tasks).await;
    assert_eq!(body.len(), 2);
    assert_eq!(body[0].status, StatusKind::Pending);
    assert_eq!(body[1].status, StatusKind::Waiting);
}

#[tokio::test]
async fn test_task_with_multiple_dependencies() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    let tasks = vec![
        task_json("parent-1", "Parent 1", "ingest"),
        task_json("parent-2", "Parent 2", "ingest"),
        task_json("parent-3", "Parent 3", "ingest"),
        task_with_deps(
            "aggregator",
            "Aggregator Task",
            "aggregate",
            vec![("parent-1", true), ("parent-2", true), ("parent-3", false)],
        ),
    ];

    let body = create_tasks_ok(&app, &tasks).await;
    assert_eq!(body.len(), 4);
    assert_eq!(body[0].status, StatusKind::Pending);
    assert_eq!(body[1].status, StatusKind::Pending);
    assert_eq!(body[2].status, StatusKind::Pending);
    assert_eq!(body[3].status, StatusKind::Waiting);
}

#[tokio::test]
async fn test_diamond_dag_pattern() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    // Diamond pattern: A -> B, A -> C, B -> D, C -> D
    let tasks = vec![
        task_json("A", "Task A", "diamond"),
        task_with_deps("B", "Task B", "diamond", vec![("A", true)]),
        task_with_deps("C", "Task C", "diamond", vec![("A", true)]),
        task_with_deps("D", "Task D", "diamond", vec![("B", true), ("C", true)]),
    ];

    let body = create_tasks_ok(&app, &tasks).await;
    assert_eq!(body.len(), 4);
    assert_eq!(body[0].status, StatusKind::Pending);
    assert_eq!(body[1].status, StatusKind::Waiting);
    assert_eq!(body[2].status, StatusKind::Waiting);
    assert_eq!(body[3].status, StatusKind::Waiting);
}

#[tokio::test]
async fn test_linear_chain_dag() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    // Linear chain: A -> B -> C -> D
    let tasks = vec![
        task_json("chain-A", "Chain A", "chain"),
        task_with_deps("chain-B", "Chain B", "chain", vec![("chain-A", true)]),
        task_with_deps("chain-C", "Chain C", "chain", vec![("chain-B", true)]),
        task_with_deps("chain-D", "Chain D", "chain", vec![("chain-C", true)]),
    ];

    let body = create_tasks_ok(&app, &tasks).await;
    assert_eq!(body.len(), 4);
    assert_eq!(body[0].status, StatusKind::Pending);
    assert_eq!(body[1].status, StatusKind::Waiting);
    assert_eq!(body[2].status, StatusKind::Waiting);
    assert_eq!(body[3].status, StatusKind::Waiting);
}

#[tokio::test]
async fn test_fan_out_dag() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    // Fan-out: A -> B, A -> C, A -> D, A -> E
    let tasks = vec![
        task_json("fanout-A", "Fanout A", "fanout"),
        task_with_deps("fanout-B", "Fanout B", "fanout", vec![("fanout-A", true)]),
        task_with_deps("fanout-C", "Fanout C", "fanout", vec![("fanout-A", true)]),
        task_with_deps("fanout-D", "Fanout D", "fanout", vec![("fanout-A", true)]),
        task_with_deps("fanout-E", "Fanout E", "fanout", vec![("fanout-A", true)]),
    ];

    let body = create_tasks_ok(&app, &tasks).await;
    assert_eq!(body.len(), 5);
    assert_eq!(body[0].status, StatusKind::Pending);
    for i in 1..5 {
        assert_eq!(body[i].status, StatusKind::Waiting);
    }
}

#[tokio::test]
async fn test_fan_in_dag() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    // Fan-in: A, B, C, D -> E
    let tasks = vec![
        task_json("fanin-A", "Fanin A", "fanin"),
        task_json("fanin-B", "Fanin B", "fanin"),
        task_json("fanin-C", "Fanin C", "fanin"),
        task_json("fanin-D", "Fanin D", "fanin"),
        task_with_deps(
            "fanin-E",
            "Fanin E",
            "fanin",
            vec![
                ("fanin-A", true),
                ("fanin-B", true),
                ("fanin-C", true),
                ("fanin-D", true),
            ],
        ),
    ];

    let body = create_tasks_ok(&app, &tasks).await;
    assert_eq!(body.len(), 5);
    for i in 0..4 {
        assert_eq!(body[i].status, StatusKind::Pending);
    }
    assert_eq!(body[4].status, StatusKind::Waiting);
}

#[tokio::test]
async fn test_mixed_dependency_requirements() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    let tasks = vec![
        task_json("mixed-A", "Mixed A", "mixed"),
        task_json("mixed-B", "Mixed B", "mixed"),
        task_with_deps(
            "mixed-C",
            "Mixed C",
            "mixed",
            vec![
                ("mixed-A", true),  // requires success
                ("mixed-B", false), // just needs to finish
            ],
        ),
    ];

    let body = create_tasks_ok(&app, &tasks).await;
    assert_eq!(body.len(), 3);
    assert_eq!(body[2].status, StatusKind::Waiting);
}
