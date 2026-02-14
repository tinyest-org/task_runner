#[macro_use]
mod common;
use common::*;

use actix_web::{App, HttpResponse, HttpServer, web};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use task_runner::dtos::BasicTaskDto;
use task_runner::models::StatusKind;

#[tokio::test]
async fn test_start_loop_marks_task_failed_on_start_webhook_error() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    let task_payload = json!({
        "id": "start-fail",
        "name": "Start Fail Task",
        "kind": "start-fail-kind",
        "timeout": 60,
        "metadata": {"test": true},
        "on_start": {
            "kind": "Webhook",
            "params": {
                "url": "http://127.0.0.1:1/fail",
                "verb": "Post"
            }
        }
    });

    let created = create_tasks_ok(&app, &[task_payload]).await;
    let task_id = created[0].id;

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let evaluator = state.action_executor.clone();
    let pool = state.pool.clone();
    let handle = tokio::spawn(async move {
        task_runner::workers::start_loop(
            &evaluator,
            pool,
            std::time::Duration::from_millis(50),
            shutdown_rx,
        )
        .await;
    });

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let _ = shutdown_tx.send(true);
    let _ = handle.await;

    let updated = get_task_ok(&app, task_id).await;
    assert_eq!(
        updated.status,
        StatusKind::Failure,
        "on_start failure should mark task as Failure"
    );
    assert!(
        updated
            .failure_reason
            .as_deref()
            .unwrap_or("")
            .contains("on_start"),
        "failure_reason should mention on_start"
    );
}

#[tokio::test]
async fn test_timeout_loop_fires_on_failure_webhook() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    let hits = Arc::new(AtomicUsize::new(0));
    let (webhook_url, server_handle) = spawn_webhook_server(hits.clone());

    let on_failure = json!({
        "kind": "Webhook",
        "params": {
            "url": webhook_url,
            "verb": "Post"
        }
    });

    let task_payload = json!({
        "id": "timeout-fail",
        "name": "Timeout Failure Task",
        "kind": "timeout-kind",
        "timeout": 1,
        "metadata": {"test": true},
        "on_start": webhook_action("Start"),
        "on_failure": [on_failure]
    });

    let created = create_tasks_ok(&app, &[task_payload]).await;
    let task_id = created[0].id;

    let mut conn = state.pool.get().await.unwrap();
    use diesel::sql_query;
    use diesel::sql_types::{Timestamptz, Uuid as SqlUuid};

    let past = chrono::Utc::now() - chrono::Duration::seconds(120);
    {
        use diesel_async::RunQueryDsl;
        sql_query(
            "UPDATE task \
             SET status = 'Running', started_at = $1, last_updated = $2 \
             WHERE id = $3",
        )
        .bind::<Timestamptz, _>(past)
        .bind::<Timestamptz, _>(past)
        .bind::<SqlUuid, _>(task_id)
        .execute(&mut conn)
        .await
        .unwrap();
    }

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let evaluator = Arc::new(state.action_executor.clone());
    let pool = state.pool.clone();
    let handle = tokio::spawn(async move {
        task_runner::workers::timeout_loop(
            evaluator,
            pool,
            std::time::Duration::from_millis(50),
            shutdown_rx,
        )
        .await;
    });

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let _ = shutdown_tx.send(true);
    let _ = handle.await;

    let updated = get_task_ok(&app, task_id).await;
    assert_eq!(updated.status, StatusKind::Failure, "task should be failed");
    let hit_count = hits.load(Ordering::SeqCst);
    assert_eq!(
        hit_count, 1,
        "on_failure webhook should be called exactly once"
    );

    let _ = server_handle.stop(true).await;
}

#[tokio::test]
async fn test_pagination_large_page_does_not_overflow() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    let tasks = vec![task_json("page-overflow", "Page Overflow", "page-test")];
    create_tasks_ok(&app, &tasks).await;

    let req = actix_web::test::TestRequest::get()
        .uri(&format!("/task?page={}&page_size=100", i64::MAX))
        .to_request();
    let resp = actix_web::test::call_service(&app, req).await;
    assert_eq!(resp.status(), actix_web::http::StatusCode::OK);

    let list: Vec<BasicTaskDto> = actix_web::test::read_body_json(resp).await;
    assert!(
        list.is_empty(),
        "Huge page should return an empty list (no overflow)"
    );
}

fn spawn_webhook_server(hits: Arc<AtomicUsize>) -> (String, actix_web::dev::ServerHandle) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = HttpServer::new(move || {
        let hits = hits.clone();
        App::new().route(
            "/webhook",
            web::post().to(move || {
                hits.fetch_add(1, Ordering::SeqCst);
                async { HttpResponse::Ok() }
            }),
        )
    })
    .listen(listener)
    .unwrap()
    .run();

    let handle = server.handle();
    actix_web::rt::spawn(server);
    (format!("http://{}/webhook", addr), handle)
}
