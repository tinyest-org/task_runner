use crate::common::*;

use arcrun::dtos::BasicTaskDto;
use arcrun::models::StatusKind;

#[tokio::test]
async fn test_list_tasks() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    let tasks: Vec<serde_json::Value> = (1..=5)
        .map(|i| {
            task_json(
                &format!("list-{}", i),
                &format!("List Task {}", i),
                "listable",
            )
        })
        .collect();

    create_tasks_ok(&app, &tasks).await;

    let list_req = actix_web::test::TestRequest::get()
        .uri("/task")
        .to_request();
    let list_resp = actix_web::test::call_service(&app, list_req).await;

    assert!(list_resp.status().is_success());

    let body: Vec<BasicTaskDto> = actix_web::test::read_body_json(list_resp).await;
    assert_eq!(body.len(), 5, "Should return exactly 5 tasks");
}

#[tokio::test]
async fn test_filter_by_kind() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    let tasks = vec![
        task_json("alpha-1", "Alpha 1", "alpha"),
        task_json("alpha-2", "Alpha 2", "alpha"),
        task_json("beta-1", "Beta 1", "beta"),
        task_json("gamma-1", "Gamma 1", "gamma"),
    ];

    create_tasks_ok(&app, &tasks).await;

    let filter_req = actix_web::test::TestRequest::get()
        .uri("/task?kind=alpha")
        .to_request();
    let filter_resp = actix_web::test::call_service(&app, filter_req).await;

    assert!(filter_resp.status().is_success());

    let body: Vec<BasicTaskDto> = actix_web::test::read_body_json(filter_resp).await;
    assert_eq!(body.len(), 2);
    for task in body {
        assert_eq!(task.kind, "alpha");
    }
}

#[tokio::test]
async fn test_filter_by_status() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    let tasks = vec![
        task_json("status-parent", "Status Parent", "status-test"),
        task_with_deps(
            "status-child",
            "Status Child",
            "status-test",
            vec![("status-parent", true)],
        ),
    ];

    create_tasks_ok(&app, &tasks).await;

    // Filter by Pending status
    let pending_req = actix_web::test::TestRequest::get()
        .uri("/task?status=Pending&kind=status-test")
        .to_request();
    let pending_resp = actix_web::test::call_service(&app, pending_req).await;
    let pending_tasks: Vec<BasicTaskDto> = actix_web::test::read_body_json(pending_resp).await;
    assert_eq!(pending_tasks.len(), 1);
    assert_eq!(pending_tasks[0].status, StatusKind::Pending);

    // Filter by Waiting status
    let waiting_req = actix_web::test::TestRequest::get()
        .uri("/task?status=Waiting&kind=status-test")
        .to_request();
    let waiting_resp = actix_web::test::call_service(&app, waiting_req).await;
    let waiting_tasks: Vec<BasicTaskDto> = actix_web::test::read_body_json(waiting_resp).await;
    assert_eq!(waiting_tasks.len(), 1);
    assert_eq!(waiting_tasks[0].status, StatusKind::Waiting);
}

#[tokio::test]
async fn test_pagination_basic() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    let tasks: Vec<serde_json::Value> = (1..=60)
        .map(|i| {
            task_json(
                &format!("page-{}", i),
                &format!("Page Task {}", i),
                "pagination",
            )
        })
        .collect();

    create_tasks_ok(&app, &tasks).await;

    // Request first page
    let page1_req = actix_web::test::TestRequest::get()
        .uri("/task?page=0&kind=pagination")
        .to_request();
    let page1_resp = actix_web::test::call_service(&app, page1_req).await;
    let page1: Vec<BasicTaskDto> = actix_web::test::read_body_json(page1_resp).await;
    assert_eq!(
        page1.len(),
        50,
        "First page should have 50 tasks (hardcoded page_size)"
    );

    // Request second page
    let page2_req = actix_web::test::TestRequest::get()
        .uri("/task?page=1&kind=pagination")
        .to_request();
    let page2_resp = actix_web::test::call_service(&app, page2_req).await;
    let page2: Vec<BasicTaskDto> = actix_web::test::read_body_json(page2_resp).await;
    assert_eq!(
        page2.len(),
        10,
        "Second page should have remaining 10 tasks"
    );

    // Verify no overlap between pages
    let page1_ids: std::collections::HashSet<_> = page1.iter().map(|t| t.id).collect();
    let page2_ids: std::collections::HashSet<_> = page2.iter().map(|t| t.id).collect();
    let overlap: Vec<_> = page1_ids.intersection(&page2_ids).collect();
    assert!(overlap.is_empty(), "Pages should have no overlapping tasks");
}

#[tokio::test]
async fn test_pagination_last_page() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    let tasks: Vec<serde_json::Value> = (1..=55)
        .map(|i| {
            task_json(
                &format!("lastpage-{}", i),
                &format!("LastPage Task {}", i),
                "lastpage",
            )
        })
        .collect();

    create_tasks_ok(&app, &tasks).await;

    let page2_req = actix_web::test::TestRequest::get()
        .uri("/task?page=1&kind=lastpage")
        .to_request();
    let page2_resp = actix_web::test::call_service(&app, page2_req).await;
    let page2: Vec<BasicTaskDto> = actix_web::test::read_body_json(page2_resp).await;
    assert_eq!(page2.len(), 5, "Last page should have remaining 5 tasks");
}

#[tokio::test]
async fn test_pagination_empty_page() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    let empty_req = actix_web::test::TestRequest::get()
        .uri("/task?page=100&page_size=10&kind=nonexistent-kind")
        .to_request();
    let empty_resp = actix_web::test::call_service(&app, empty_req).await;
    let empty: Vec<BasicTaskDto> = actix_web::test::read_body_json(empty_resp).await;
    assert!(empty.is_empty(), "Page beyond data should be empty");
}
