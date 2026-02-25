#[macro_use]
mod common;
use common::*;

use serde_json::json;
use std::collections::HashMap;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use task_runner::models::{TriggerCondition, TriggerKind};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Test that the start webhook idempotency guard prevents duplicate execution.
///
/// Scenario: A webhook_execution record already exists with status='success' for
/// the start trigger. Calling try_claim_webhook_execution again should return false.
#[tokio::test]
async fn test_start_webhook_idempotency() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    let tasks = vec![task_json("idem-start", "Idempotency Start", "idem-test")];
    let created = create_tasks_ok(&app, &tasks).await;
    let task_id = created[0].id;

    let mut conn = state.pool.get().await.unwrap();

    let key = task_runner::action::idempotency_key(
        task_id,
        &TriggerKind::Start,
        &TriggerCondition::Success,
    );

    // First claim should succeed
    let claimed = task_runner::db_operation::try_claim_webhook_execution(
        &mut conn,
        task_id,
        TriggerKind::Start,
        TriggerCondition::Success,
        &key,
        None,
    )
    .await
    .expect("try_claim should not error");
    assert!(claimed, "First claim should succeed");

    // Mark as success
    task_runner::db_operation::complete_webhook_execution(&mut conn, &key, true)
        .await
        .expect("complete should not error");

    // Second claim should be skipped (already succeeded)
    let claimed_again = task_runner::db_operation::try_claim_webhook_execution(
        &mut conn,
        task_id,
        TriggerKind::Start,
        TriggerCondition::Success,
        &key,
        None,
    )
    .await
    .expect("try_claim should not error");
    assert!(
        !claimed_again,
        "Second claim should be skipped because the first already succeeded"
    );
}

/// Test that the end webhook idempotency guard prevents duplicate execution.
///
/// Scenario: A webhook_execution record already exists with status='success' for
/// the end:success trigger. Calling try_claim_webhook_execution again should return false.
#[tokio::test]
async fn test_end_webhook_idempotency() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    let tasks = vec![task_json("idem-end", "Idempotency End", "idem-test")];
    let created = create_tasks_ok(&app, &tasks).await;
    let task_id = created[0].id;

    let mut conn = state.pool.get().await.unwrap();

    let key = task_runner::action::idempotency_key(
        task_id,
        &TriggerKind::End,
        &TriggerCondition::Success,
    );

    // First claim should succeed
    let claimed = task_runner::db_operation::try_claim_webhook_execution(
        &mut conn,
        task_id,
        TriggerKind::End,
        TriggerCondition::Success,
        &key,
        None,
    )
    .await
    .expect("try_claim should not error");
    assert!(claimed, "First claim should succeed");

    // Mark as success
    task_runner::db_operation::complete_webhook_execution(&mut conn, &key, true)
        .await
        .expect("complete should not error");

    // Second claim should be skipped
    let claimed_again = task_runner::db_operation::try_claim_webhook_execution(
        &mut conn,
        task_id,
        TriggerKind::End,
        TriggerCondition::Success,
        &key,
        None,
    )
    .await
    .expect("try_claim should not error");
    assert!(
        !claimed_again,
        "Second claim should be skipped because the first already succeeded"
    );
}

/// Test that a failed webhook execution can be retried (not skipped).
///
/// Scenario: A webhook_execution record exists with status='failure'.
/// Calling try_claim_webhook_execution again should return true (allow retry).
#[tokio::test]
async fn test_failed_webhook_allows_retry() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    let tasks = vec![task_json("idem-retry", "Idempotency Retry", "idem-test")];
    let created = create_tasks_ok(&app, &tasks).await;
    let task_id = created[0].id;

    let mut conn = state.pool.get().await.unwrap();

    let key = task_runner::action::idempotency_key(
        task_id,
        &TriggerKind::Start,
        &TriggerCondition::Success,
    );

    // First claim
    let claimed = task_runner::db_operation::try_claim_webhook_execution(
        &mut conn,
        task_id,
        TriggerKind::Start,
        TriggerCondition::Success,
        &key,
        None,
    )
    .await
    .unwrap();
    assert!(claimed);

    // Mark as failure
    task_runner::db_operation::complete_webhook_execution(&mut conn, &key, false)
        .await
        .unwrap();

    // Second claim should succeed (retry allowed for failed executions)
    let claimed_again = task_runner::db_operation::try_claim_webhook_execution(
        &mut conn,
        task_id,
        TriggerKind::Start,
        TriggerCondition::Success,
        &key,
        None,
    )
    .await
    .unwrap();
    assert!(claimed_again, "Failed webhook execution should allow retry");
}

/// Test that different trigger types don't interfere with each other.
///
/// Scenario: A start trigger is claimed and succeeded. An end trigger for the
/// same task should still be claimable.
#[tokio::test]
async fn test_different_triggers_independent() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    let tasks = vec![task_json(
        "idem-indep",
        "Idempotency Independent",
        "idem-test",
    )];
    let created = create_tasks_ok(&app, &tasks).await;
    let task_id = created[0].id;

    let mut conn = state.pool.get().await.unwrap();

    // Claim and succeed the start trigger
    let start_key = task_runner::action::idempotency_key(
        task_id,
        &TriggerKind::Start,
        &TriggerCondition::Success,
    );
    task_runner::db_operation::try_claim_webhook_execution(
        &mut conn,
        task_id,
        TriggerKind::Start,
        TriggerCondition::Success,
        &start_key,
        None,
    )
    .await
    .unwrap();
    task_runner::db_operation::complete_webhook_execution(&mut conn, &start_key, true)
        .await
        .unwrap();

    // End trigger should be independently claimable
    let end_key = task_runner::action::idempotency_key(
        task_id,
        &TriggerKind::End,
        &TriggerCondition::Success,
    );
    let claimed = task_runner::db_operation::try_claim_webhook_execution(
        &mut conn,
        task_id,
        TriggerKind::End,
        TriggerCondition::Success,
        &end_key,
        None,
    )
    .await
    .unwrap();
    assert!(
        claimed,
        "End trigger should be claimable independently of start trigger"
    );
}

/// Test that pending executions block retries until they become stale.
#[tokio::test]
async fn test_pending_webhook_blocks_until_stale() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    let tasks = vec![task_json(
        "idem-pending",
        "Idempotency Pending",
        "idem-test",
    )];
    let created = create_tasks_ok(&app, &tasks).await;
    let task_id = created[0].id;

    let mut conn = state.pool.get().await.unwrap();
    let key = task_runner::action::idempotency_key(
        task_id,
        &TriggerKind::Start,
        &TriggerCondition::Success,
    );

    let claimed = task_runner::db_operation::try_claim_webhook_execution(
        &mut conn,
        task_id,
        TriggerKind::Start,
        TriggerCondition::Success,
        &key,
        None,
    )
    .await
    .unwrap();
    assert!(claimed, "First claim should succeed");

    let claimed_again = task_runner::db_operation::try_claim_webhook_execution(
        &mut conn,
        task_id,
        TriggerKind::Start,
        TriggerCondition::Success,
        &key,
        None,
    )
    .await
    .unwrap();
    assert!(
        !claimed_again,
        "Pending execution should block retries without stale timeout"
    );

    // Force the pending record to look stale
    use diesel::sql_query;
    use diesel::sql_types::Text;
    use diesel_async::RunQueryDsl;
    sql_query(
        "UPDATE webhook_execution SET updated_at = now() - interval '5 seconds' \
         WHERE idempotency_key = $1",
    )
    .bind::<Text, _>(&key)
    .execute(&mut conn)
    .await
    .unwrap();

    let claimed_stale = task_runner::db_operation::try_claim_webhook_execution(
        &mut conn,
        task_id,
        TriggerKind::Start,
        TriggerCondition::Success,
        &key,
        Some(std::time::Duration::from_secs(1)),
    )
    .await
    .unwrap();
    assert!(claimed_stale, "Stale pending execution should allow retry");
}

/// Test that the idempotency headers are sent with the webhook request.
#[tokio::test]
async fn test_webhook_includes_idempotency_headers() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    let (webhook_url, headers_rx, shutdown_server) = spawn_header_capture_server();

    let task_payload = json!({
        "id": "idem-headers",
        "name": "Idempotency Headers",
        "kind": "idem-test",
        "timeout": 60,
        "metadata": {"test": true},
        "on_start": {
            "kind": "Webhook",
            "params": {
                "url": webhook_url,
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
            true,
            shutdown_rx,
        )
        .await;
    });

    let headers = tokio::time::timeout(std::time::Duration::from_secs(2), headers_rx)
        .await
        .expect("timed out waiting for webhook headers")
        .expect("failed to receive webhook headers");

    let _ = shutdown_tx.send(true);
    let _ = handle.await;
    let _ = shutdown_server.send(());

    let idempotency_key = headers.get("idempotency-key").cloned().unwrap_or_default();
    let trigger = headers.get("x-task-trigger").cloned().unwrap_or_default();
    let task_header = headers.get("x-task-id").cloned().unwrap_or_default();

    assert_eq!(idempotency_key, format!("{}:start", task_id));
    assert_eq!(trigger, "start");
    assert_eq!(task_header, task_id.to_string());
}

/// Test that requeued Claimed tasks do not re-fire on_start when idempotency succeeded.
#[tokio::test]
async fn test_requeue_claimed_skips_on_start_when_idempotent() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    let hits = Arc::new(AtomicUsize::new(0));
    let (webhook_url, shutdown_server) = spawn_counting_webhook_server(hits.clone());

    let task_payload = json!({
        "id": "idem-requeue",
        "name": "Idempotency Requeue",
        "kind": "idem-test",
        "timeout": 60,
        "metadata": {"test": true},
        "on_start": {
            "kind": "Webhook",
            "params": {
                "url": webhook_url,
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
            true,
            shutdown_rx,
        )
        .await;
    });

    let mut attempts = 0;
    while hits.as_ref().load(Ordering::SeqCst) == 0 && attempts < 20 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        attempts += 1;
    }

    let _ = shutdown_tx.send(true);
    let _ = handle.await;
    assert_eq!(
        hits.as_ref().load(Ordering::SeqCst),
        1,
        "Expected exactly one start webhook call"
    );

    let mut conn = state.pool.get().await.unwrap();
    use diesel::sql_query;
    use diesel::sql_types::{Timestamptz, Uuid as SqlUuid};
    use diesel_async::RunQueryDsl;
    let past = chrono::Utc::now() - chrono::Duration::seconds(120);
    sql_query(
        "UPDATE task \
         SET status = 'claimed', last_updated = $1, started_at = NULL \
         WHERE id = $2",
    )
    .bind::<Timestamptz, _>(past)
    .bind::<SqlUuid, _>(task_id)
    .execute(&mut conn)
    .await
    .unwrap();

    // Simulate requeue by moving the task back to Pending.
    sql_query(
        "UPDATE task \
         SET status = 'pending', last_updated = now() \
         WHERE id = $1",
    )
    .bind::<SqlUuid, _>(task_id)
    .execute(&mut conn)
    .await
    .unwrap();

    let (shutdown_tx2, shutdown_rx2) = tokio::sync::watch::channel(false);
    let evaluator = state.action_executor.clone();
    let pool = state.pool.clone();
    let handle2 = tokio::spawn(async move {
        task_runner::workers::start_loop(
            &evaluator,
            pool,
            std::time::Duration::from_millis(50),
            true,
            shutdown_rx2,
        )
        .await;
    });

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let _ = shutdown_tx2.send(true);
    let _ = handle2.await;
    let _ = shutdown_server.send(());

    assert_eq!(
        hits.as_ref().load(Ordering::SeqCst),
        1,
        "Requeued task should not re-fire on_start webhook"
    );
}

fn spawn_counting_webhook_server(
    hits: Arc<AtomicUsize>,
) -> (String, tokio::sync::oneshot::Sender<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let listener = tokio::net::TcpListener::from_std(listener).unwrap();

    let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    tokio::spawn(async move {
        loop {
            tokio::select! {
                result = listener.accept() => {
                    if let Ok((mut stream, _)) = result {
                        let hits = hits.clone();
                        tokio::spawn(async move {
                            let mut buf = [0u8; 1024];
                            let _ = stream.read(&mut buf).await;
                            hits.fetch_add(1, Ordering::SeqCst);
                            let response = "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
                            let _ = stream.write_all(response.as_bytes()).await;
                        });
                    }
                }
                _ = &mut shutdown_rx => break,
            }
        }
    });

    (format!("http://{}/webhook", addr), shutdown_tx)
}

fn spawn_header_capture_server() -> (
    String,
    tokio::sync::oneshot::Receiver<HashMap<String, String>>,
    tokio::sync::oneshot::Sender<()>,
) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let listener = tokio::net::TcpListener::from_std(listener).unwrap();

    let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let (headers_tx, headers_rx) = tokio::sync::oneshot::channel::<HashMap<String, String>>();
    let headers_tx = Arc::new(tokio::sync::Mutex::new(Some(headers_tx)));

    tokio::spawn(async move {
        loop {
            tokio::select! {
                result = listener.accept() => {
                    if let Ok((mut stream, _)) = result {
                        let headers_tx = headers_tx.clone();
                        tokio::spawn(async move {
                            let mut data = Vec::new();
                            let mut buf = [0u8; 1024];
                            loop {
                                let read = match stream.read(&mut buf).await {
                                    Ok(n) => n,
                                    Err(_) => 0,
                                };
                                if read == 0 {
                                    break;
                                }
                                data.extend_from_slice(&buf[..read]);
                                if data.windows(4).any(|w| w == b"\r\n\r\n") {
                                    break;
                                }
                            }

                            let mut headers = HashMap::new();
                            let text = String::from_utf8_lossy(&data);
                            for line in text.lines().skip(1) {
                                let line = line.trim_end();
                                if line.is_empty() {
                                    break;
                                }
                                if let Some((k, v)) = line.split_once(':') {
                                    headers.insert(
                                        k.trim().to_ascii_lowercase(),
                                        v.trim().to_string(),
                                    );
                                }
                            }

                            if let Some(tx) = headers_tx.lock().await.take() {
                                let _ = tx.send(headers);
                            }

                            let response = "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
                            let _ = stream.write_all(response.as_bytes()).await;
                        });
                    }
                }
                _ = &mut shutdown_rx => break,
            }
        }
    });

    (format!("http://{}/webhook", addr), headers_rx, shutdown_tx)
}
