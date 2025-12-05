//! End-to-end integration tests using testcontainers for PostgreSQL.
//!
//! These tests verify the HTTP API endpoints against a real PostgreSQL database
//! running in Docker. They test the full request/response cycle through actix-web.
//!
//! IMPORTANT: These tests use the same handlers as the production application,
//! ensuring that bugs in the handlers are caught by tests.

use actix_web::{App, test, web};
use diesel::{Connection, PgConnection, RunQueryDsl};
use diesel_async::AsyncPgConnection;
use diesel_async::pooled_connection::AsyncDieselConnectionManager;
use diesel_async::pooled_connection::bb8::Pool;
use diesel_migrations::{EmbeddedMigrations, MigrationHarness, embed_migrations};
use serde_json::json;
use std::sync::Arc;
use task_runner::DbPool;
use task_runner::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
use task_runner::config::Config;
use task_runner::dtos::{BasicTaskDto, TaskDto};
use task_runner::handlers::{AppState, configure_routes};
use task_runner::models::StatusKind;
use testcontainers::{ImageExt, runners::AsyncRunner};
use testcontainers_modules::postgres::Postgres;
use tokio::sync::mpsc;

pub const MIGRATIONS: EmbeddedMigrations = embed_migrations!("migrations");

// =============================================================================
// Test Infrastructure
// =============================================================================

/// Test application state (simplified version of main server's AppState)
struct TestApp {
    pool: DbPool,
    _container: testcontainers::ContainerAsync<Postgres>,
}

/// Setup a test database and return the pool
async fn setup_test_db() -> TestApp {
    let container = Postgres::default()
        .with_tag("18-alpine")
        .start()
        .await
        .unwrap();
    let host_port = container.get_host_port_ipv4(5432).await.unwrap();

    let database_url = format!(
        "postgres://postgres:postgres@127.0.0.1:{}/postgres",
        host_port
    );

    // Run migrations using sync connection
    run_migrations(&database_url);

    // Create async pool
    let config = AsyncDieselConnectionManager::<AsyncPgConnection>::new(&database_url);
    let pool = Pool::builder()
        .max_size(5)
        .build(config)
        .await
        .expect("Failed to create pool");

    TestApp {
        pool,
        _container: container,
    }
}

/// Convert ALTER TYPE ... ADD VALUE to use IF NOT EXISTS for idempotency
fn make_alter_type_idempotent(statement: &str) -> String {
    let upper = statement.to_uppercase();
    if upper.contains("IF NOT EXISTS") {
        // Already idempotent
        statement.to_string()
    } else {
        // Insert IF NOT EXISTS after ADD VALUE
        // Pattern: ALTER TYPE <name> ADD VALUE '<value>'
        // Convert to: ALTER TYPE <name> ADD VALUE IF NOT EXISTS '<value>'
        let add_value_pos = upper.find("ADD VALUE").unwrap_or(0);
        if add_value_pos > 0 {
            let before = &statement[..add_value_pos + 9]; // "ADD VALUE"
            let after = &statement[add_value_pos + 9..];
            format!("{} IF NOT EXISTS{}", before, after)
        } else {
            statement.to_string()
        }
    }
}

/// Run migrations, handling ALTER TYPE ... ADD VALUE specially
///
/// PostgreSQL's ALTER TYPE ... ADD VALUE cannot run inside a transaction block.
/// This function detects such migrations by reading their SQL content and runs
/// them outside of transactions.
fn run_migrations(database_url: &str) {
    use std::fs;
    use std::path::Path;

    let mut conn = PgConnection::establish(database_url).expect("Failed to connect for migrations");

    let pending = conn
        .pending_migrations(MIGRATIONS)
        .expect("Failed to get pending migrations");

    // Collect migration info first
    let migrations_to_run: Vec<_> = pending
        .iter()
        .map(|m| (m.name().to_string(), m.name().version().to_string()))
        .collect();

    // Build a map of migrations that contain ALTER TYPE ... ADD VALUE
    // by reading the actual migration SQL files
    let migrations_dir = Path::new("migrations");
    let alter_type_migrations: std::collections::HashMap<String, Vec<String>> =
        if migrations_dir.exists() {
            fs::read_dir(migrations_dir)
                .ok()
                .map(|entries| {
                    entries
                        .flatten()
                        .filter_map(|entry| {
                            let dir_name = entry.file_name().to_string_lossy().to_string();
                            let up_sql_path = entry.path().join("up.sql");

                            if up_sql_path.exists() {
                                let content = fs::read_to_string(&up_sql_path).ok()?;

                                // Find all ALTER TYPE ... ADD VALUE statements
                                let statements: Vec<String> = content
                                    .lines()
                                    .filter(|line| {
                                        let upper = line.to_uppercase();
                                        upper.contains("ALTER TYPE") && upper.contains("ADD VALUE")
                                    })
                                    .map(|s| make_alter_type_idempotent(s.trim()))
                                    .collect();

                                if !statements.is_empty() {
                                    return Some((dir_name, statements));
                                }
                            }
                            None
                        })
                        .collect()
                })
                .unwrap_or_default()
        } else {
            std::collections::HashMap::new()
        };

    // Run each migration
    for (migration_name, version) in &migrations_to_run {
        // Check if this migration contains ALTER TYPE ... ADD VALUE
        // by looking for matching directory name in our map
        let alter_statements: Option<&Vec<String>> = alter_type_migrations
            .iter()
            .find(|(dir_name, _)| migration_name.contains(dir_name.as_str()))
            .map(|(_, stmts)| stmts);

        if let Some(statements) = alter_statements {
            // This migration has ALTER TYPE ... ADD VALUE statements
            // Run them outside of a transaction

            // Ensure no transaction is active by reconnecting
            drop(conn);
            conn = PgConnection::establish(database_url)
                .expect("Failed to reconnect for ALTER TYPE migration");

            // Run each ALTER TYPE statement
            for stmt in statements {
                diesel::sql_query(stmt.clone())
                    .execute(&mut conn)
                    .unwrap_or_else(|e| {
                        // Log but don't fail - value might already exist in some PostgreSQL versions
                        eprintln!("Warning: ALTER TYPE statement may have failed (possibly already exists): {}", e);
                        0
                    });
            }

            // Mark migration as run in diesel's schema_migrations table
            diesel::sql_query(format!(
                "INSERT INTO __diesel_schema_migrations (version, run_on) VALUES ('{}', NOW()) ON CONFLICT DO NOTHING",
                version
            ))
            .execute(&mut conn)
            .expect("Failed to record migration");
        } else {
            // Run migration normally within a transaction
            let pending_now = conn
                .pending_migrations(MIGRATIONS)
                .expect("Failed to get pending migrations");

            for migration in pending_now {
                if migration.name().to_string() == *migration_name {
                    let _ = diesel::sql_query("COMMIT").execute(&mut conn);
                    conn.run_migration(&migration)
                        .expect(&format!("Failed to run migration: {}", migration_name));
                    break;
                }
            }
        }
    }
}

/// Create test configuration
fn test_config() -> Arc<Config> {
    Arc::new(Config {
        port: 8080,
        host_url: "http://localhost:8080".to_string(),
        database_url: "".to_string(),
        pool: task_runner::config::PoolConfig {
            max_size: 5,
            min_idle: 1,
            max_lifetime: std::time::Duration::from_secs(3600),
            idle_timeout: std::time::Duration::from_secs(120),
            connection_timeout: std::time::Duration::from_secs(30),
            acquire_retries: 3,
            retry_delay: std::time::Duration::from_millis(100),
        },
        pagination: task_runner::config::PaginationConfig {
            default_per_page: 50,
            max_per_page: 100,
        },
        worker: task_runner::config::WorkerConfig {
            loop_interval: std::time::Duration::from_secs(1),
            timeout_check_interval: std::time::Duration::from_secs(1),
            batch_flush_interval: std::time::Duration::from_millis(100),
            batch_channel_capacity: 100,
        },
        circuit_breaker: task_runner::config::CircuitBreakerConfig {
            enabled: true,
            failure_threshold: 5,
            failure_window_secs: 10,
            recovery_timeout_secs: 30,
            success_threshold: 2,
        },
        observability: task_runner::config::ObservabilityConfig {
            slow_query_threshold_ms: 100,
            tracing_enabled: false,
            otlp_endpoint: None,
            service_name: "task-runner-test".to_string(),
            sampling_ratio: 1.0,
        },
    })
}

/// Create a test circuit breaker (with high thresholds so it doesn't trip during tests)
fn test_circuit_breaker() -> Arc<CircuitBreaker> {
    Arc::new(CircuitBreaker::new(CircuitBreakerConfig {
        failure_threshold: 100, // High threshold for tests
        failure_window: std::time::Duration::from_secs(60),
        recovery_timeout: std::time::Duration::from_secs(1),
        success_threshold: 1,
    }))
}

/// Create app state for tests (without batch updater)
fn create_test_state(pool: DbPool) -> AppState {
    let (sender, _receiver) = mpsc::channel(100);
    AppState {
        pool,
        sender,
        action_executor: task_runner::action::ActionExecutor {
            ctx: task_runner::action::ActionContext {
                host_address: "http://localhost:8080".to_string(),
            },
        },
        config: test_config(),
        circuit_breaker: test_circuit_breaker(),
    }
}

/// Create app state with batch updater running (for batch update tests)
fn create_test_state_with_batch_updater(pool: DbPool) -> AppState {
    let (sender, receiver) = mpsc::channel(100);

    // Spawn the batch updater background task
    let pool_clone = pool.clone();
    tokio::spawn(async move {
        task_runner::workers::batch_updater(pool_clone, receiver).await;
    });

    AppState {
        pool,
        sender,
        action_executor: task_runner::action::ActionExecutor {
            ctx: task_runner::action::ActionContext {
                host_address: "http://localhost:8080".to_string(),
            },
        },
        config: test_config(),
        circuit_breaker: test_circuit_breaker(),
    }
}

/// Helper to create a valid webhook action JSON
fn webhook_action(trigger: &str) -> serde_json::Value {
    json!({
        "kind": "Webhook",
        "trigger": trigger,
        "params": {
            "url": "https://example.com/webhook",
            "verb": "Post"
        }
    })
}

/// Helper to create a basic task JSON
fn task_json(id: &str, name: &str, kind: &str) -> serde_json::Value {
    json!({
        "id": id,
        "name": name,
        "kind": kind,
        "timeout": 60,
        "metadata": {"test": true},
        "on_start": webhook_action("Start")
    })
}

/// Helper to create a task with dependencies
fn task_with_deps(id: &str, name: &str, kind: &str, deps: Vec<(&str, bool)>) -> serde_json::Value {
    let dependencies: Vec<serde_json::Value> = deps
        .into_iter()
        .map(|(dep_id, requires_success)| {
            json!({
                "id": dep_id,
                "requires_success": requires_success
            })
        })
        .collect();

    json!({
        "id": id,
        "name": name,
        "kind": kind,
        "timeout": 60,
        "metadata": {"test": true},
        "on_start": webhook_action("Start"),
        "dependencies": dependencies
    })
}

// =============================================================================
// Health Check Tests
// =============================================================================

#[tokio::test]
async fn test_health_check() {
    let test_app = setup_test_db().await;
    let state = create_test_state(test_app.pool.clone());

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .configure(configure_routes),
    )
    .await;

    let req = test::TestRequest::get().uri("/health").to_request();
    let resp = test::call_service(&app, req).await;

    assert!(resp.status().is_success());

    let body: serde_json::Value = test::read_body_json(resp).await;
    assert_eq!(body["status"], "ok");
}

// =============================================================================
// Task CRUD Tests
// =============================================================================

#[tokio::test]
async fn test_create_single_task() {
    let test_app = setup_test_db().await;
    let state = create_test_state(test_app.pool.clone());

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .configure(configure_routes),
    )
    .await;

    let task = task_json("task-1", "Test Task", "test-kind");
    let req = test::TestRequest::post()
        .uri("/task")
        .insert_header(("requester", "test"))
        .set_json(&vec![task])
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), actix_web::http::StatusCode::CREATED);

    let body: Vec<TaskDto> = test::read_body_json(resp).await;
    assert_eq!(body.len(), 1);
    assert_eq!(body[0].name, "Test Task");
    assert_eq!(body[0].kind, "test-kind");
    assert_eq!(body[0].status, StatusKind::Pending);
}

#[tokio::test]
async fn test_create_multiple_tasks() {
    let test_app = setup_test_db().await;
    let state = create_test_state(test_app.pool.clone());

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .configure(configure_routes),
    )
    .await;

    let tasks: Vec<serde_json::Value> = (1..=5)
        .map(|i| task_json(&format!("task-{}", i), &format!("Task {}", i), "batch"))
        .collect();

    let req = test::TestRequest::post()
        .uri("/task")
        .insert_header(("requester", "test"))
        .set_json(&tasks)
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), actix_web::http::StatusCode::CREATED);

    let body: Vec<TaskDto> = test::read_body_json(resp).await;
    assert_eq!(body.len(), 5);
}

#[tokio::test]
async fn test_get_task_by_id() {
    let test_app = setup_test_db().await;
    let state = create_test_state(test_app.pool.clone());

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .configure(configure_routes),
    )
    .await;

    // Create a task first
    let task = task_json("find-me", "Findable Task", "findable");
    let create_req = test::TestRequest::post()
        .uri("/task")
        .insert_header(("requester", "test"))
        .set_json(&vec![task])
        .to_request();

    let create_resp = test::call_service(&app, create_req).await;
    let created: Vec<TaskDto> = test::read_body_json(create_resp).await;
    let task_id = created[0].id;

    // Fetch the task by ID
    let get_req = test::TestRequest::get()
        .uri(&format!("/task/{}", task_id))
        .to_request();

    let get_resp = test::call_service(&app, get_req).await;
    assert!(get_resp.status().is_success());

    let found: TaskDto = test::read_body_json(get_resp).await;
    assert_eq!(found.id, task_id);
    assert_eq!(found.name, "Findable Task");
}

#[tokio::test]
async fn test_get_nonexistent_task() {
    let test_app = setup_test_db().await;
    let state = create_test_state(test_app.pool.clone());

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .configure(configure_routes),
    )
    .await;

    let random_id = uuid::Uuid::new_v4();
    let req = test::TestRequest::get()
        .uri(&format!("/task/{}", random_id))
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), actix_web::http::StatusCode::NOT_FOUND);
}

// =============================================================================
// DAG Dependency Tests
// =============================================================================

#[tokio::test]
async fn test_task_with_single_dependency() {
    let test_app = setup_test_db().await;
    let state = create_test_state(test_app.pool.clone());

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .configure(configure_routes),
    )
    .await;

    // Create parent and child in same request
    let parent = task_json("parent", "Parent Task", "parent-kind");
    let child = task_with_deps("child", "Child Task", "child-kind", vec![("parent", true)]);

    let req = test::TestRequest::post()
        .uri("/task")
        .insert_header(("requester", "test"))
        .set_json(&vec![parent, child])
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), actix_web::http::StatusCode::CREATED);

    let body: Vec<TaskDto> = test::read_body_json(resp).await;
    assert_eq!(body.len(), 2);

    // Parent should be Pending
    assert_eq!(body[0].status, StatusKind::Pending);
    // Child should be Waiting (has dependency)
    assert_eq!(body[1].status, StatusKind::Waiting);
}

#[tokio::test]
async fn test_task_with_multiple_dependencies() {
    let test_app = setup_test_db().await;
    let state = create_test_state(test_app.pool.clone());

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .configure(configure_routes),
    )
    .await;

    // Create three parents and one child
    let tasks = vec![
        task_json("parent-1", "Parent 1", "ingest"),
        task_json("parent-2", "Parent 2", "ingest"),
        task_json("parent-3", "Parent 3", "ingest"),
        task_with_deps(
            "aggregator",
            "Aggregator Task",
            "aggregate",
            vec![
                ("parent-1", true),
                ("parent-2", true),
                ("parent-3", false), // Only needs to finish, not succeed
            ],
        ),
    ];

    let req = test::TestRequest::post()
        .uri("/task")
        .insert_header(("requester", "test"))
        .set_json(&tasks)
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), actix_web::http::StatusCode::CREATED);

    let body: Vec<TaskDto> = test::read_body_json(resp).await;
    assert_eq!(body.len(), 4);

    // Parents should be Pending
    assert_eq!(body[0].status, StatusKind::Pending);
    assert_eq!(body[1].status, StatusKind::Pending);
    assert_eq!(body[2].status, StatusKind::Pending);
    // Child should be Waiting
    assert_eq!(body[3].status, StatusKind::Waiting);
}

#[tokio::test]
async fn test_diamond_dag_pattern() {
    let test_app = setup_test_db().await;
    let state = create_test_state(test_app.pool.clone());

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .configure(configure_routes),
    )
    .await;

    // Diamond pattern: A -> B, A -> C, B -> D, C -> D
    //       A
    //      / \
    //     B   C
    //      \ /
    //       D
    let tasks = vec![
        task_json("A", "Task A", "diamond"),
        task_with_deps("B", "Task B", "diamond", vec![("A", true)]),
        task_with_deps("C", "Task C", "diamond", vec![("A", true)]),
        task_with_deps("D", "Task D", "diamond", vec![("B", true), ("C", true)]),
    ];

    let req = test::TestRequest::post()
        .uri("/task")
        .insert_header(("requester", "test"))
        .set_json(&tasks)
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), actix_web::http::StatusCode::CREATED);

    let body: Vec<TaskDto> = test::read_body_json(resp).await;
    assert_eq!(body.len(), 4);

    // A is Pending (no dependencies)
    assert_eq!(body[0].status, StatusKind::Pending);
    // B, C, D are Waiting (have dependencies)
    assert_eq!(body[1].status, StatusKind::Waiting);
    assert_eq!(body[2].status, StatusKind::Waiting);
    assert_eq!(body[3].status, StatusKind::Waiting);
}

// =============================================================================
// Task Listing and Filtering Tests
// =============================================================================

#[tokio::test]
async fn test_list_tasks() {
    let test_app = setup_test_db().await;
    let state = create_test_state(test_app.pool.clone());

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .configure(configure_routes),
    )
    .await;

    // Create some tasks
    let tasks: Vec<serde_json::Value> = (1..=5)
        .map(|i| {
            task_json(
                &format!("list-{}", i),
                &format!("List Task {}", i),
                "listable",
            )
        })
        .collect();

    let create_req = test::TestRequest::post()
        .uri("/task")
        .insert_header(("requester", "test"))
        .set_json(&tasks)
        .to_request();
    test::call_service(&app, create_req).await;

    // List all tasks
    let list_req = test::TestRequest::get().uri("/task").to_request();
    let list_resp = test::call_service(&app, list_req).await;

    assert!(list_resp.status().is_success());

    let body: Vec<BasicTaskDto> = test::read_body_json(list_resp).await;
    assert_eq!(body.len(), 5, "Should return exactly 5 tasks");
}

#[tokio::test]
async fn test_filter_by_kind() {
    let test_app = setup_test_db().await;
    let state = create_test_state(test_app.pool.clone());

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .configure(configure_routes),
    )
    .await;

    // Create tasks with different kinds
    let tasks = vec![
        task_json("alpha-1", "Alpha 1", "alpha"),
        task_json("alpha-2", "Alpha 2", "alpha"),
        task_json("beta-1", "Beta 1", "beta"),
        task_json("gamma-1", "Gamma 1", "gamma"),
    ];

    let create_req = test::TestRequest::post()
        .uri("/task")
        .insert_header(("requester", "test"))
        .set_json(&tasks)
        .to_request();
    test::call_service(&app, create_req).await;

    // Filter by "alpha" kind
    let filter_req = test::TestRequest::get()
        .uri("/task?kind=alpha")
        .to_request();
    let filter_resp = test::call_service(&app, filter_req).await;

    assert!(filter_resp.status().is_success());

    let body: Vec<BasicTaskDto> = test::read_body_json(filter_resp).await;
    assert_eq!(body.len(), 2);
    for task in body {
        assert_eq!(task.kind, "alpha");
    }
}

#[tokio::test]
async fn test_filter_by_status() {
    let test_app = setup_test_db().await;
    let state = create_test_state(test_app.pool.clone());

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .configure(configure_routes),
    )
    .await;

    // Create parent and child (child will be Waiting)
    let tasks = vec![
        task_json("status-parent", "Status Parent", "status-test"),
        task_with_deps(
            "status-child",
            "Status Child",
            "status-test",
            vec![("status-parent", true)],
        ),
    ];

    let create_req = test::TestRequest::post()
        .uri("/task")
        .insert_header(("requester", "test"))
        .set_json(&tasks)
        .to_request();
    test::call_service(&app, create_req).await;

    // Filter by Pending status
    let pending_req = test::TestRequest::get()
        .uri("/task?status=Pending&kind=status-test")
        .to_request();
    let pending_resp = test::call_service(&app, pending_req).await;

    let pending_tasks: Vec<BasicTaskDto> = test::read_body_json(pending_resp).await;
    assert_eq!(pending_tasks.len(), 1);
    assert_eq!(pending_tasks[0].status, StatusKind::Pending);

    // Filter by Waiting status
    let waiting_req = test::TestRequest::get()
        .uri("/task?status=Waiting&kind=status-test")
        .to_request();
    let waiting_resp = test::call_service(&app, waiting_req).await;

    let waiting_tasks: Vec<BasicTaskDto> = test::read_body_json(waiting_resp).await;
    assert_eq!(waiting_tasks.len(), 1);
    assert_eq!(waiting_tasks[0].status, StatusKind::Waiting);
}

// =============================================================================
// Task Status Transition Tests
// =============================================================================

#[tokio::test]
async fn test_pause_task() {
    let test_app = setup_test_db().await;
    let state = create_test_state(test_app.pool.clone());

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .configure(configure_routes),
    )
    .await;

    // Create a task
    let task = task_json("pausable", "Pausable Task", "pausable-kind");
    let create_req = test::TestRequest::post()
        .uri("/task")
        .insert_header(("requester", "test"))
        .set_json(&vec![task])
        .to_request();

    let create_resp = test::call_service(&app, create_req).await;
    let created: Vec<TaskDto> = test::read_body_json(create_resp).await;
    let task_id = created[0].id;

    // Pause the task
    let pause_req = test::TestRequest::patch()
        .uri(&format!("/task/pause/{}", task_id))
        .to_request();

    let pause_resp = test::call_service(&app, pause_req).await;
    assert!(pause_resp.status().is_success());

    // Verify task is paused
    let get_req = test::TestRequest::get()
        .uri(&format!("/task/{}", task_id))
        .to_request();

    let get_resp = test::call_service(&app, get_req).await;
    let updated: TaskDto = test::read_body_json(get_resp).await;
    assert_eq!(updated.status, StatusKind::Paused);
}

#[tokio::test]
async fn test_cancel_pending_task() {
    let test_app = setup_test_db().await;
    let state = create_test_state(test_app.pool.clone());

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .configure(configure_routes),
    )
    .await;

    // Create a task
    let task = task_json("cancelable", "Cancelable Task", "cancelable-kind");
    let create_req = test::TestRequest::post()
        .uri("/task")
        .insert_header(("requester", "test"))
        .set_json(&vec![task])
        .to_request();

    let create_resp = test::call_service(&app, create_req).await;
    let created: Vec<TaskDto> = test::read_body_json(create_resp).await;
    let task_id = created[0].id;

    // Cancel the task
    let cancel_req = test::TestRequest::delete()
        .uri(&format!("/task/{}", task_id))
        .to_request();

    let cancel_resp = test::call_service(&app, cancel_req).await;
    assert!(cancel_resp.status().is_success());

    // Verify task is canceled
    let get_req = test::TestRequest::get()
        .uri(&format!("/task/{}", task_id))
        .to_request();

    let get_resp = test::call_service(&app, get_req).await;
    let updated: TaskDto = test::read_body_json(get_resp).await;
    assert_eq!(updated.status, StatusKind::Canceled);
}

// =============================================================================
// Deduplication Tests
// =============================================================================

#[tokio::test]
async fn test_dedupe_skip_existing() {
    let test_app = setup_test_db().await;
    let state = create_test_state(test_app.pool.clone());

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .configure(configure_routes),
    )
    .await;

    // Create first task with metadata
    let task1 = json!({
        "id": "dedupe-1",
        "name": "Dedupe Task 1",
        "kind": "dedupe-kind",
        "timeout": 60,
        "metadata": {"unique_key": "abc123"},
        "on_start": webhook_action("Start")
    });

    let create_req1 = test::TestRequest::post()
        .uri("/task")
        .insert_header(("requester", "test"))
        .set_json(&vec![task1])
        .to_request();

    let create_resp1 = test::call_service(&app, create_req1).await;
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

    let create_req2 = test::TestRequest::post()
        .uri("/task")
        .insert_header(("requester", "test"))
        .set_json(&vec![task2])
        .to_request();

    let create_resp2 = test::call_service(&app, create_req2).await;
    // Should return NoContent because task was deduplicated
    assert_eq!(
        create_resp2.status(),
        actix_web::http::StatusCode::NO_CONTENT
    );
}

// =============================================================================
// Edge Cases
// =============================================================================

#[tokio::test]
async fn test_task_with_large_metadata() {
    let test_app = setup_test_db().await;
    let state = create_test_state(test_app.pool.clone());

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .configure(configure_routes),
    )
    .await;

    let large_array: Vec<i32> = (0..1000).collect();
    let task = json!({
        "id": "large-meta",
        "name": "Large Metadata Task",
        "kind": "large",
        "timeout": 60,
        "metadata": {
            "array": large_array,
            "nested": {
                "level1": {
                    "level2": {
                        "level3": "deep value"
                    }
                }
            }
        },
        "on_start": webhook_action("Start")
    });

    let req = test::TestRequest::post()
        .uri("/task")
        .insert_header(("requester", "test"))
        .set_json(&vec![task])
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), actix_web::http::StatusCode::CREATED);

    let body: Vec<TaskDto> = test::read_body_json(resp).await;
    assert!(body[0].metadata.get("array").is_some());
}

#[tokio::test]
async fn test_task_with_special_characters_in_name() {
    let test_app = setup_test_db().await;
    let state = create_test_state(test_app.pool.clone());

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .configure(configure_routes),
    )
    .await;

    let task = json!({
        "id": "special",
        "name": "Task with 'quotes' and \"double quotes\" and unicode: 日本語",
        "kind": "special-chars",
        "timeout": 60,
        "metadata": {"test": true},
        "on_start": webhook_action("Start")
    });

    let req = test::TestRequest::post()
        .uri("/task")
        .insert_header(("requester", "test"))
        .set_json(&vec![task])
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), actix_web::http::StatusCode::CREATED);

    let body: Vec<TaskDto> = test::read_body_json(resp).await;
    assert!(body[0].name.contains("quotes"));
    assert!(body[0].name.contains("日本語"));
}

// =============================================================================
// Concurrency Rules Storage Tests
// NOTE: These tests only verify rules are stored correctly.
// Runtime enforcement is done by the worker loop which is not tested here.
// =============================================================================

#[tokio::test]
async fn test_concurrency_rules_stored_on_task() {
    let test_app = setup_test_db().await;
    let state = create_test_state(test_app.pool.clone());

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .configure(configure_routes),
    )
    .await;

    // Rules use serde tag = "type", so format is {"type": "Concurency", ...}
    let task = json!({
        "id": "concurrent",
        "name": "Concurrent Task",
        "kind": "concurrent-kind",
        "timeout": 60,
        "metadata": {"test": true},
        "on_start": webhook_action("Start"),
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

    let req = test::TestRequest::post()
        .uri("/task")
        .insert_header(("requester", "test"))
        .set_json(&vec![task])
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), actix_web::http::StatusCode::CREATED);

    let body: Vec<TaskDto> = test::read_body_json(resp).await;
    assert_eq!(body[0].status, StatusKind::Pending);
    assert!(!body[0].rules.0.is_empty());
}

// =============================================================================
// Action Configuration Tests
// =============================================================================

#[tokio::test]
async fn test_task_with_success_and_failure_actions() {
    let test_app = setup_test_db().await;
    let state = create_test_state(test_app.pool.clone());

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .configure(configure_routes),
    )
    .await;

    let task = json!({
        "id": "with-actions",
        "name": "Task with Actions",
        "kind": "action-test",
        "timeout": 60,
        "metadata": {"test": true},
        "on_start": webhook_action("Start"),
        "on_success": [webhook_action("End")],
        "on_failure": [webhook_action("End")]
    });

    let req = test::TestRequest::post()
        .uri("/task")
        .insert_header(("requester", "test"))
        .set_json(&vec![task])
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), actix_web::http::StatusCode::CREATED);

    let body: Vec<TaskDto> = test::read_body_json(resp).await;
    // Should have 3 actions: on_start, on_success, on_failure
    assert_eq!(body[0].actions.len(), 3);
}

// =============================================================================
// Complex DAG Tests
// =============================================================================

#[tokio::test]
async fn test_linear_chain_dag() {
    let test_app = setup_test_db().await;
    let state = create_test_state(test_app.pool.clone());

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .configure(configure_routes),
    )
    .await;

    // Linear chain: A -> B -> C -> D
    let tasks = vec![
        task_json("chain-A", "Chain A", "chain"),
        task_with_deps("chain-B", "Chain B", "chain", vec![("chain-A", true)]),
        task_with_deps("chain-C", "Chain C", "chain", vec![("chain-B", true)]),
        task_with_deps("chain-D", "Chain D", "chain", vec![("chain-C", true)]),
    ];

    let req = test::TestRequest::post()
        .uri("/task")
        .insert_header(("requester", "test"))
        .set_json(&tasks)
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), actix_web::http::StatusCode::CREATED);

    let body: Vec<TaskDto> = test::read_body_json(resp).await;
    assert_eq!(body.len(), 4);

    // Only first task should be Pending
    assert_eq!(body[0].status, StatusKind::Pending);
    // All others should be Waiting
    assert_eq!(body[1].status, StatusKind::Waiting);
    assert_eq!(body[2].status, StatusKind::Waiting);
    assert_eq!(body[3].status, StatusKind::Waiting);
}

#[tokio::test]
async fn test_fan_out_dag() {
    let test_app = setup_test_db().await;
    let state = create_test_state(test_app.pool.clone());

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .configure(configure_routes),
    )
    .await;

    // Fan-out: A -> B, A -> C, A -> D, A -> E
    let tasks = vec![
        task_json("fanout-A", "Fanout A", "fanout"),
        task_with_deps("fanout-B", "Fanout B", "fanout", vec![("fanout-A", true)]),
        task_with_deps("fanout-C", "Fanout C", "fanout", vec![("fanout-A", true)]),
        task_with_deps("fanout-D", "Fanout D", "fanout", vec![("fanout-A", true)]),
        task_with_deps("fanout-E", "Fanout E", "fanout", vec![("fanout-A", true)]),
    ];

    let req = test::TestRequest::post()
        .uri("/task")
        .insert_header(("requester", "test"))
        .set_json(&tasks)
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), actix_web::http::StatusCode::CREATED);

    let body: Vec<TaskDto> = test::read_body_json(resp).await;
    assert_eq!(body.len(), 5);

    // Root is Pending
    assert_eq!(body[0].status, StatusKind::Pending);
    // All children are Waiting
    for i in 1..5 {
        assert_eq!(body[i].status, StatusKind::Waiting);
    }
}

#[tokio::test]
async fn test_fan_in_dag() {
    let test_app = setup_test_db().await;
    let state = create_test_state(test_app.pool.clone());

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .configure(configure_routes),
    )
    .await;

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

    let req = test::TestRequest::post()
        .uri("/task")
        .insert_header(("requester", "test"))
        .set_json(&tasks)
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), actix_web::http::StatusCode::CREATED);

    let body: Vec<TaskDto> = test::read_body_json(resp).await;
    assert_eq!(body.len(), 5);

    // All roots are Pending
    for i in 0..4 {
        assert_eq!(body[i].status, StatusKind::Pending);
    }
    // Final task is Waiting
    assert_eq!(body[4].status, StatusKind::Waiting);
}

#[tokio::test]
async fn test_mixed_dependency_requirements() {
    let test_app = setup_test_db().await;
    let state = create_test_state(test_app.pool.clone());

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .configure(configure_routes),
    )
    .await;

    // Mixed requirements: some require success, some just need to finish
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

    let req = test::TestRequest::post()
        .uri("/task")
        .insert_header(("requester", "test"))
        .set_json(&tasks)
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), actix_web::http::StatusCode::CREATED);

    let body: Vec<TaskDto> = test::read_body_json(resp).await;
    assert_eq!(body.len(), 3);
    assert_eq!(body[2].status, StatusKind::Waiting);
}

// =============================================================================
// Status Transition Tests (E2E behavior simulation)
// =============================================================================

#[tokio::test]
async fn test_child_transitions_when_parent_succeeds() {
    let test_app = setup_test_db().await;
    let state = create_test_state(test_app.pool.clone());

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state.clone()))
            .configure(configure_routes),
    )
    .await;

    // Create parent and child
    let tasks = vec![
        task_json("parent-success", "Parent Task", "transition-test"),
        task_with_deps(
            "child-success",
            "Child Task",
            "transition-test",
            vec![("parent-success", true)],
        ),
    ];

    let req = test::TestRequest::post()
        .uri("/task")
        .insert_header(("requester", "test"))
        .set_json(&tasks)
        .to_request();

    let resp = test::call_service(&app, req).await;
    let created: Vec<TaskDto> = test::read_body_json(resp).await;

    let parent_id = created[0].id;
    let child_id = created[1].id;

    // Verify initial states
    assert_eq!(created[0].status, StatusKind::Pending);
    assert_eq!(created[1].status, StatusKind::Waiting);

    // Complete the parent task with Success status
    let mut conn = state.pool.get().await.unwrap();
    let update_dto = task_runner::dtos::UpdateTaskDto {
        status: Some(StatusKind::Success),
        metadata: None,
        new_success: None,
        new_failures: None,
        failure_reason: None,
    };

    task_runner::db_operation::update_running_task(
        &state.action_executor,
        &mut conn,
        parent_id,
        update_dto,
    )
    .await
    .unwrap();

    // Verify parent is now Success
    let get_parent = test::TestRequest::get()
        .uri(&format!("/task/{}", parent_id))
        .to_request();
    let parent_resp = test::call_service(&app, get_parent).await;
    let parent_after: TaskDto = test::read_body_json(parent_resp).await;
    assert_eq!(parent_after.status, StatusKind::Success);

    // Verify child transitioned from Waiting to Pending
    let get_child = test::TestRequest::get()
        .uri(&format!("/task/{}", child_id))
        .to_request();
    let child_resp = test::call_service(&app, get_child).await;
    let child_after: TaskDto = test::read_body_json(child_resp).await;
    assert_eq!(
        child_after.status,
        StatusKind::Pending,
        "Child should transition from Waiting to Pending when parent succeeds"
    );
}

#[tokio::test]
async fn test_child_fails_when_required_parent_fails() {
    let test_app = setup_test_db().await;
    let state = create_test_state(test_app.pool.clone());

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state.clone()))
            .configure(configure_routes),
    )
    .await;

    // Create parent and child with required dependency
    let tasks = vec![
        task_json("failing-parent", "Failing Parent", "fail-test"),
        task_with_deps(
            "dependent-child",
            "Dependent Child",
            "fail-test",
            vec![("failing-parent", true)], // requires_success = true
        ),
    ];

    let req = test::TestRequest::post()
        .uri("/task")
        .insert_header(("requester", "test"))
        .set_json(&tasks)
        .to_request();

    let resp = test::call_service(&app, req).await;
    let created: Vec<TaskDto> = test::read_body_json(resp).await;

    let parent_id = created[0].id;
    let child_id = created[1].id;

    // Fail the parent task
    let mut conn = state.pool.get().await.unwrap();
    let update_dto = task_runner::dtos::UpdateTaskDto {
        status: Some(StatusKind::Failure),
        metadata: None,
        new_success: None,
        new_failures: None,
        failure_reason: Some("Intentional test failure".to_string()),
    };

    task_runner::db_operation::update_running_task(
        &state.action_executor,
        &mut conn,
        parent_id,
        update_dto,
    )
    .await
    .unwrap();

    // Verify child is marked as Failure due to required parent failure
    let get_child = test::TestRequest::get()
        .uri(&format!("/task/{}", child_id))
        .to_request();
    let child_resp = test::call_service(&app, get_child).await;
    let child_after: TaskDto = test::read_body_json(child_resp).await;
    assert_eq!(
        child_after.status,
        StatusKind::Failure,
        "Child should be marked as Failure when required parent fails"
    );
}

#[tokio::test]
async fn test_child_proceeds_when_optional_parent_fails() {
    let test_app = setup_test_db().await;
    let state = create_test_state(test_app.pool.clone());

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state.clone()))
            .configure(configure_routes),
    )
    .await;

    // Create two parents (required and optional) and one child
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

    let req = test::TestRequest::post()
        .uri("/task")
        .insert_header(("requester", "test"))
        .set_json(&tasks)
        .to_request();

    let resp = test::call_service(&app, req).await;
    let created: Vec<TaskDto> = test::read_body_json(resp).await;

    let required_id = created[0].id;
    let optional_id = created[1].id;
    let child_id = created[2].id;

    let mut conn = state.pool.get().await.unwrap();

    // Fail the optional parent
    let fail_optional = task_runner::dtos::UpdateTaskDto {
        status: Some(StatusKind::Failure),
        metadata: None,
        new_success: None,
        new_failures: None,
        failure_reason: Some("Optional failure".to_string()),
    };
    task_runner::db_operation::update_running_task(
        &state.action_executor,
        &mut conn,
        optional_id,
        fail_optional,
    )
    .await
    .unwrap();

    // Child should still be Waiting (required parent not done)
    let get_child1 = test::TestRequest::get()
        .uri(&format!("/task/{}", child_id))
        .to_request();
    let child_resp1 = test::call_service(&app, get_child1).await;
    let child_status1: TaskDto = test::read_body_json(child_resp1).await;
    assert_eq!(
        child_status1.status,
        StatusKind::Waiting,
        "Child should still be Waiting when only optional parent finished"
    );

    // Complete the required parent
    let success_required = task_runner::dtos::UpdateTaskDto {
        status: Some(StatusKind::Success),
        metadata: None,
        new_success: None,
        new_failures: None,
        failure_reason: None,
    };
    task_runner::db_operation::update_running_task(
        &state.action_executor,
        &mut conn,
        required_id,
        success_required,
    )
    .await
    .unwrap();

    // Child should now be Pending
    let get_child2 = test::TestRequest::get()
        .uri(&format!("/task/{}", child_id))
        .to_request();
    let child_resp2 = test::call_service(&app, get_child2).await;
    let child_status2: TaskDto = test::read_body_json(child_resp2).await;
    assert_eq!(
        child_status2.status,
        StatusKind::Pending,
        "Child should be Pending after required parent succeeds (even if optional failed)"
    );
}

#[tokio::test]
async fn test_multi_level_dag_propagation() {
    let test_app = setup_test_db().await;
    let state = create_test_state(test_app.pool.clone());

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state.clone()))
            .configure(configure_routes),
    )
    .await;

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

    let req = test::TestRequest::post()
        .uri("/task")
        .insert_header(("requester", "test"))
        .set_json(&tasks)
        .to_request();

    let resp = test::call_service(&app, req).await;
    let created: Vec<TaskDto> = test::read_body_json(resp).await;

    let ingest1_id = created[0].id;
    let ingest2_id = created[1].id;
    let cluster_id = created[2].id;
    let refresh_id = created[3].id;

    // Verify initial states
    assert_eq!(created[0].status, StatusKind::Pending);
    assert_eq!(created[1].status, StatusKind::Pending);
    assert_eq!(created[2].status, StatusKind::Waiting);
    assert_eq!(created[3].status, StatusKind::Waiting);

    let mut conn = state.pool.get().await.unwrap();

    // Helper to create success update DTO
    let make_success = || task_runner::dtos::UpdateTaskDto {
        status: Some(StatusKind::Success),
        metadata: None,
        new_success: None,
        new_failures: None,
        failure_reason: None,
    };

    // Complete ingest_1
    task_runner::db_operation::update_running_task(
        &state.action_executor,
        &mut conn,
        ingest1_id,
        make_success(),
    )
    .await
    .unwrap();

    // Cluster should still be Waiting (ingest_2 not done)
    let get_cluster1 = test::TestRequest::get()
        .uri(&format!("/task/{}", cluster_id))
        .to_request();
    let cluster_resp1 = test::call_service(&app, get_cluster1).await;
    let cluster1: TaskDto = test::read_body_json(cluster_resp1).await;
    assert_eq!(cluster1.status, StatusKind::Waiting);

    // Complete ingest_2
    task_runner::db_operation::update_running_task(
        &state.action_executor,
        &mut conn,
        ingest2_id,
        make_success(),
    )
    .await
    .unwrap();

    // Cluster should now be Pending
    let get_cluster2 = test::TestRequest::get()
        .uri(&format!("/task/{}", cluster_id))
        .to_request();
    let cluster_resp2 = test::call_service(&app, get_cluster2).await;
    let cluster2: TaskDto = test::read_body_json(cluster_resp2).await;
    assert_eq!(cluster2.status, StatusKind::Pending);

    // Refresh should still be Waiting
    let get_refresh1 = test::TestRequest::get()
        .uri(&format!("/task/{}", refresh_id))
        .to_request();
    let refresh_resp1 = test::call_service(&app, get_refresh1).await;
    let refresh1: TaskDto = test::read_body_json(refresh_resp1).await;
    assert_eq!(refresh1.status, StatusKind::Waiting);

    // Complete cluster
    task_runner::db_operation::update_running_task(
        &state.action_executor,
        &mut conn,
        cluster_id,
        make_success(),
    )
    .await
    .unwrap();

    // Refresh should now be Pending
    let get_refresh2 = test::TestRequest::get()
        .uri(&format!("/task/{}", refresh_id))
        .to_request();
    let refresh_resp2 = test::call_service(&app, get_refresh2).await;
    let refresh2: TaskDto = test::read_body_json(refresh_resp2).await;
    assert_eq!(refresh2.status, StatusKind::Pending);
}

#[tokio::test]
async fn test_failure_propagation_through_chain() {
    let test_app = setup_test_db().await;
    let state = create_test_state(test_app.pool.clone());

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state.clone()))
            .configure(configure_routes),
    )
    .await;

    // Create: root -> middle -> leaf (all require success)
    let tasks = vec![
        task_json("root", "Root Task", "propagation"),
        task_with_deps("middle", "Middle Task", "propagation", vec![("root", true)]),
        task_with_deps("leaf", "Leaf Task", "propagation", vec![("middle", true)]),
    ];

    let req = test::TestRequest::post()
        .uri("/task")
        .insert_header(("requester", "test"))
        .set_json(&tasks)
        .to_request();

    let resp = test::call_service(&app, req).await;
    let created: Vec<TaskDto> = test::read_body_json(resp).await;

    let root_id = created[0].id;
    let middle_id = created[1].id;
    let leaf_id = created[2].id;

    let mut conn = state.pool.get().await.unwrap();

    // Fail the root task
    let fail = task_runner::dtos::UpdateTaskDto {
        status: Some(StatusKind::Failure),
        metadata: None,
        new_success: None,
        new_failures: None,
        failure_reason: Some("Root failure".to_string()),
    };
    task_runner::db_operation::update_running_task(
        &state.action_executor,
        &mut conn,
        root_id,
        fail,
    )
    .await
    .unwrap();

    // Middle should be marked as Failure
    let get_middle = test::TestRequest::get()
        .uri(&format!("/task/{}", middle_id))
        .to_request();
    let middle_resp = test::call_service(&app, get_middle).await;
    let middle: TaskDto = test::read_body_json(middle_resp).await;
    assert_eq!(middle.status, StatusKind::Failure);

    // Leaf should also be marked as Failure (cascading propagation)
    let get_leaf = test::TestRequest::get()
        .uri(&format!("/task/{}", leaf_id))
        .to_request();
    let leaf_resp = test::call_service(&app, get_leaf).await;
    let leaf: TaskDto = test::read_body_json(leaf_resp).await;
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
    let test_app = setup_test_db().await;
    let state = create_test_state(test_app.pool.clone());

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state.clone()))
            .configure(configure_routes),
    )
    .await;

    // CI/CD Pipeline DAG:
    //       Build
    //      /  |  \
    //   Unit Int  Lint
    //      \  |  /
    //      Deploy
    let tasks = vec![
        task_json("build", "Build Project", "build"),
        task_with_deps("unit", "Unit Tests", "test", vec![("build", true)]),
        task_with_deps(
            "integration",
            "Integration Tests",
            "test",
            vec![("build", true)],
        ),
        task_with_deps("lint", "Lint Check", "lint", vec![("build", false)]), // doesn't require success
        task_with_deps(
            "deploy",
            "Deploy to Staging",
            "deploy",
            vec![
                ("unit", true),        // requires success
                ("integration", true), // requires success
                ("lint", false),       // just needs to finish
            ],
        ),
    ];

    let req = test::TestRequest::post()
        .uri("/task")
        .insert_header(("requester", "test"))
        .set_json(&tasks)
        .to_request();

    let resp = test::call_service(&app, req).await;
    let created: Vec<TaskDto> = test::read_body_json(resp).await;

    assert_eq!(created.len(), 5);
    let build_id = created[0].id;
    let unit_id = created[1].id;
    let integration_id = created[2].id;
    let lint_id = created[3].id;
    let deploy_id = created[4].id;

    // Verify initial states
    assert_eq!(created[0].status, StatusKind::Pending); // build
    assert_eq!(created[1].status, StatusKind::Waiting); // unit
    assert_eq!(created[2].status, StatusKind::Waiting); // integration
    assert_eq!(created[3].status, StatusKind::Waiting); // lint
    assert_eq!(created[4].status, StatusKind::Waiting); // deploy

    let mut conn = state.pool.get().await.unwrap();

    // Fail the build task
    let fail_build = task_runner::dtos::UpdateTaskDto {
        status: Some(StatusKind::Failure),
        metadata: None,
        new_success: None,
        new_failures: None,
        failure_reason: Some("Build compilation failed".to_string()),
    };
    task_runner::db_operation::update_running_task(
        &state.action_executor,
        &mut conn,
        build_id,
        fail_build,
    )
    .await
    .unwrap();

    // Unit Tests should be Failure (required build success)
    let get_unit = test::TestRequest::get()
        .uri(&format!("/task/{}", unit_id))
        .to_request();
    let unit_resp = test::call_service(&app, get_unit).await;
    let unit: TaskDto = test::read_body_json(unit_resp).await;
    assert_eq!(
        unit.status,
        StatusKind::Failure,
        "Unit Tests should fail because required parent (build) failed"
    );

    // Integration Tests should be Failure (required build success)
    let get_integration = test::TestRequest::get()
        .uri(&format!("/task/{}", integration_id))
        .to_request();
    let integration_resp = test::call_service(&app, get_integration).await;
    let integration: TaskDto = test::read_body_json(integration_resp).await;
    assert_eq!(
        integration.status,
        StatusKind::Failure,
        "Integration Tests should fail because required parent (build) failed"
    );

    // Lint should still be Waiting (doesn't require build success, just finish)
    // Its wait_finished was decremented but it hasn't transitioned yet
    let get_lint = test::TestRequest::get()
        .uri(&format!("/task/{}", lint_id))
        .to_request();
    let lint_resp = test::call_service(&app, get_lint).await;
    let lint: TaskDto = test::read_body_json(lint_resp).await;
    assert_eq!(
        lint.status,
        StatusKind::Pending,
        "Lint should be Pending (build finished, doesn't require success)"
    );

    // Deploy should be Failure because Unit and Integration failed (and it required their success)
    let get_deploy = test::TestRequest::get()
        .uri(&format!("/task/{}", deploy_id))
        .to_request();
    let deploy_resp = test::call_service(&app, get_deploy).await;
    let deploy: TaskDto = test::read_body_json(deploy_resp).await;
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

// =============================================================================
// Pagination Tests
// =============================================================================

#[tokio::test]
async fn test_pagination_basic() {
    let test_app = setup_test_db().await;
    let state = create_test_state(test_app.pool.clone());

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .configure(configure_routes),
    )
    .await;

    // Note: The db_operation uses hardcoded page_size=50, so create enough tasks
    // to test pagination works by checking offset between pages
    // Create 60 tasks to span multiple pages
    let tasks: Vec<serde_json::Value> = (1..=60)
        .map(|i| {
            task_json(
                &format!("page-{}", i),
                &format!("Page Task {}", i),
                "pagination",
            )
        })
        .collect();

    let create_req = test::TestRequest::post()
        .uri("/task")
        .insert_header(("requester", "test"))
        .set_json(&tasks)
        .to_request();
    test::call_service(&app, create_req).await;

    // Request first page (page=0) - should get up to 50 tasks (hardcoded page_size)
    let page1_req = test::TestRequest::get()
        .uri("/task?page=0&kind=pagination")
        .to_request();
    let page1_resp = test::call_service(&app, page1_req).await;
    let page1: Vec<BasicTaskDto> = test::read_body_json(page1_resp).await;
    assert_eq!(
        page1.len(),
        50,
        "First page should have 50 tasks (hardcoded page_size)"
    );

    // Request second page (page=1) - should get remaining 10 tasks
    let page2_req = test::TestRequest::get()
        .uri("/task?page=1&kind=pagination")
        .to_request();
    let page2_resp = test::call_service(&app, page2_req).await;
    let page2: Vec<BasicTaskDto> = test::read_body_json(page2_resp).await;
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
    let test_app = setup_test_db().await;
    let state = create_test_state(test_app.pool.clone());

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .configure(configure_routes),
    )
    .await;

    // Create 55 tasks to test partial last page with hardcoded page_size=50
    let tasks: Vec<serde_json::Value> = (1..=55)
        .map(|i| {
            task_json(
                &format!("lastpage-{}", i),
                &format!("LastPage Task {}", i),
                "lastpage",
            )
        })
        .collect();

    let create_req = test::TestRequest::post()
        .uri("/task")
        .insert_header(("requester", "test"))
        .set_json(&tasks)
        .to_request();
    test::call_service(&app, create_req).await;

    // Request second page (page=1) - should have 5 remaining tasks
    let page2_req = test::TestRequest::get()
        .uri("/task?page=1&kind=lastpage")
        .to_request();
    let page2_resp = test::call_service(&app, page2_req).await;
    let page2: Vec<BasicTaskDto> = test::read_body_json(page2_resp).await;
    assert_eq!(page2.len(), 5, "Last page should have remaining 5 tasks");
}

#[tokio::test]
async fn test_pagination_empty_page() {
    let test_app = setup_test_db().await;
    let state = create_test_state(test_app.pool.clone());

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .configure(configure_routes),
    )
    .await;

    // Request page with no data
    let empty_req = test::TestRequest::get()
        .uri("/task?page=100&page_size=10&kind=nonexistent-kind")
        .to_request();
    let empty_resp = test::call_service(&app, empty_req).await;
    let empty: Vec<BasicTaskDto> = test::read_body_json(empty_resp).await;
    assert!(empty.is_empty(), "Page beyond data should be empty");
}

// =============================================================================
// Cancel Task with Children Tests
// =============================================================================

#[tokio::test]
async fn test_cancel_task_with_waiting_children() {
    let test_app = setup_test_db().await;
    let state = create_test_state(test_app.pool.clone());

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .configure(configure_routes),
    )
    .await;

    // Create parent with two children
    let tasks = vec![
        task_json("cancel-parent", "Cancel Parent", "cancel-cascade"),
        task_with_deps(
            "cancel-child1",
            "Cancel Child 1",
            "cancel-cascade",
            vec![("cancel-parent", true)],
        ),
        task_with_deps(
            "cancel-child2",
            "Cancel Child 2",
            "cancel-cascade",
            vec![("cancel-parent", true)],
        ),
    ];

    let req = test::TestRequest::post()
        .uri("/task")
        .insert_header(("requester", "test"))
        .set_json(&tasks)
        .to_request();

    let resp = test::call_service(&app, req).await;
    let created: Vec<TaskDto> = test::read_body_json(resp).await;

    let parent_id = created[0].id;
    let child1_id = created[1].id;
    let child2_id = created[2].id;

    // Verify initial states
    assert_eq!(created[0].status, StatusKind::Pending);
    assert_eq!(created[1].status, StatusKind::Waiting);
    assert_eq!(created[2].status, StatusKind::Waiting);

    // Cancel the parent
    let cancel_req = test::TestRequest::delete()
        .uri(&format!("/task/{}", parent_id))
        .to_request();
    let cancel_resp = test::call_service(&app, cancel_req).await;
    assert!(cancel_resp.status().is_success());

    // Verify parent is canceled
    let get_parent = test::TestRequest::get()
        .uri(&format!("/task/{}", parent_id))
        .to_request();
    let parent_resp = test::call_service(&app, get_parent).await;
    let parent: TaskDto = test::read_body_json(parent_resp).await;
    assert_eq!(parent.status, StatusKind::Canceled);

    // Children with requires_success=true should be marked as Failure
    // because their required parent was canceled (treated like failure)
    let get_child1 = test::TestRequest::get()
        .uri(&format!("/task/{}", child1_id))
        .to_request();
    let child1_resp = test::call_service(&app, get_child1).await;
    let child1: TaskDto = test::read_body_json(child1_resp).await;
    assert_eq!(
        child1.status,
        StatusKind::Failure,
        "Child 1 should be Failure because required parent was canceled"
    );
    assert!(
        child1.failure_reason.is_some(),
        "Child 1 should have a failure reason"
    );

    let get_child2 = test::TestRequest::get()
        .uri(&format!("/task/{}", child2_id))
        .to_request();
    let child2_resp = test::call_service(&app, get_child2).await;
    let child2: TaskDto = test::read_body_json(child2_resp).await;
    assert_eq!(
        child2.status,
        StatusKind::Failure,
        "Child 2 should be Failure because required parent was canceled"
    );
    assert!(
        child2.failure_reason.is_some(),
        "Child 2 should have a failure reason"
    );
}

// =============================================================================
// Update Task Status Tests
// =============================================================================

#[tokio::test]
async fn test_update_task_status_via_api() {
    let test_app = setup_test_db().await;
    let state = create_test_state(test_app.pool.clone());

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .configure(configure_routes),
    )
    .await;

    // Create a task
    let task = task_json("updatable", "Updatable Task", "update-test");
    let create_req = test::TestRequest::post()
        .uri("/task")
        .insert_header(("requester", "test"))
        .set_json(&vec![task])
        .to_request();

    let create_resp = test::call_service(&app, create_req).await;
    let created: Vec<TaskDto> = test::read_body_json(create_resp).await;
    let task_id = created[0].id;

    // Update task to Success via PATCH
    let update_req = test::TestRequest::patch()
        .uri(&format!("/task/{}", task_id))
        .set_json(&json!({"status": "Success"}))
        .to_request();

    let update_resp = test::call_service(&app, update_req).await;
    assert!(update_resp.status().is_success());

    // Verify task is now Success
    let get_req = test::TestRequest::get()
        .uri(&format!("/task/{}", task_id))
        .to_request();
    let get_resp = test::call_service(&app, get_req).await;
    let updated: TaskDto = test::read_body_json(get_resp).await;
    assert_eq!(updated.status, StatusKind::Success);
}

#[tokio::test]
async fn test_update_task_with_failure_reason() {
    let test_app = setup_test_db().await;
    let state = create_test_state(test_app.pool.clone());

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .configure(configure_routes),
    )
    .await;

    // Create a task
    let task = task_json("fail-reason", "Fail Reason Task", "fail-reason-test");
    let create_req = test::TestRequest::post()
        .uri("/task")
        .insert_header(("requester", "test"))
        .set_json(&vec![task])
        .to_request();

    let create_resp = test::call_service(&app, create_req).await;
    let created: Vec<TaskDto> = test::read_body_json(create_resp).await;
    let task_id = created[0].id;

    // Update task to Failure with reason
    let update_req = test::TestRequest::patch()
        .uri(&format!("/task/{}", task_id))
        .set_json(&json!({
            "status": "Failure",
            "failure_reason": "Connection timeout after 30 seconds"
        }))
        .to_request();

    let update_resp = test::call_service(&app, update_req).await;
    assert!(update_resp.status().is_success());

    // Verify task has failure reason
    let get_req = test::TestRequest::get()
        .uri(&format!("/task/{}", task_id))
        .to_request();
    let get_resp = test::call_service(&app, get_req).await;
    let updated: TaskDto = test::read_body_json(get_resp).await;
    assert_eq!(updated.status, StatusKind::Failure);
    assert_eq!(
        updated.failure_reason,
        Some("Connection timeout after 30 seconds".to_string())
    );
}

#[tokio::test]
async fn test_concurrency_rules_stored_per_project() {
    let test_app = setup_test_db().await;
    let state = create_test_state(test_app.pool.clone());

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .configure(configure_routes),
    )
    .await;

    let project_id = 12345;

    // Create two tasks with same project, concurrency limit of 1
    // Rules use serde tag = "type", so format is {"type": "Concurency", ...}
    let tasks = vec![
        json!({
            "id": "proj-task-1",
            "name": "Project Task 1",
            "kind": "project-concurrent",
            "timeout": 60,
            "metadata": {"projectId": project_id},
            "on_start": webhook_action("Start"),
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
            "name": "Project Task 2",
            "kind": "project-concurrent",
            "timeout": 60,
            "metadata": {"projectId": project_id},
            "on_start": webhook_action("Start"),
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

    let req = test::TestRequest::post()
        .uri("/task")
        .insert_header(("requester", "test"))
        .set_json(&tasks)
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), actix_web::http::StatusCode::CREATED);

    let created: Vec<TaskDto> = test::read_body_json(resp).await;
    assert_eq!(created.len(), 2);

    // Both tasks created with Pending status and have rules stored
    // NOTE: Actual enforcement of max_concurency=1 happens in worker loop
    assert_eq!(created[0].status, StatusKind::Pending);
    assert_eq!(created[1].status, StatusKind::Pending);
    assert!(!created[0].rules.0.is_empty());
    assert!(!created[1].rules.0.is_empty());
}

#[tokio::test]
async fn test_concurrency_rules_stored_different_projects() {
    let test_app = setup_test_db().await;
    let state = create_test_state(test_app.pool.clone());

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .configure(configure_routes),
    )
    .await;

    // Verify tasks with different project IDs can both have concurrency rules stored
    // NOTE: This only tests storage, not runtime enforcement
    let tasks = vec![
        json!({
            "id": "diff-proj-1",
            "name": "Project 1 Task",
            "kind": "diff-project",
            "timeout": 60,
            "metadata": {"projectId": 111},
            "on_start": webhook_action("Start"),
            "rules": [{
                "type": "Concurency",
                "matcher": {
                    "kind": "diff-project",
                    "status": "Running",
                    "fields": ["projectId"]
                },
                "max_concurency": 1
            }]
        }),
        json!({
            "id": "diff-proj-2",
            "name": "Project 2 Task",
            "kind": "diff-project",
            "timeout": 60,
            "metadata": {"projectId": 222},
            "on_start": webhook_action("Start"),
            "rules": [{
                "type": "Concurency",
                "matcher": {
                    "kind": "diff-project",
                    "status": "Running",
                    "fields": ["projectId"]
                },
                "max_concurency": 1
            }]
        }),
    ];

    let req = test::TestRequest::post()
        .uri("/task")
        .insert_header(("requester", "test"))
        .set_json(&tasks)
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), actix_web::http::StatusCode::CREATED);

    let created: Vec<TaskDto> = test::read_body_json(resp).await;
    assert_eq!(created.len(), 2);

    // Both tasks created with Pending status and have rules stored
    assert_eq!(created[0].status, StatusKind::Pending);
    assert_eq!(created[1].status, StatusKind::Pending);
    assert!(!created[0].rules.0.is_empty());
    assert!(!created[1].rules.0.is_empty());
}

// =============================================================================
// Batch Update Tests
// =============================================================================

#[tokio::test]
async fn test_batch_update_increments_counters() {
    let test_app = setup_test_db().await;
    let state = create_test_state_with_batch_updater(test_app.pool.clone());

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .configure(configure_routes),
    )
    .await;

    // Create a task
    let task = task_json("batch-test", "Batch Update Test", "batch-test");
    let create_req = test::TestRequest::post()
        .uri("/task")
        .insert_header(("requester", "test"))
        .set_json(&vec![task])
        .to_request();
    let create_resp = test::call_service(&app, create_req).await;
    let created: Vec<TaskDto> = test::read_body_json(create_resp).await;
    let task_id = created[0].id;

    // Verify initial counters are 0
    assert_eq!(created[0].success, 0);
    assert_eq!(created[0].failures, 0);

    // Send batch update via PUT endpoint
    let update_req = test::TestRequest::put()
        .uri(&format!("/task/{}", task_id))
        .set_json(&json!({
            "new_success": 5,
            "new_failures": 2
        }))
        .to_request();
    let update_resp = test::call_service(&app, update_req).await;
    assert_eq!(
        update_resp.status(),
        actix_web::http::StatusCode::ACCEPTED,
        "Batch update should be accepted"
    );

    // Wait for batch updater to process (runs every 100ms)
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;

    // Verify counters were updated
    let get_req = test::TestRequest::get()
        .uri(&format!("/task/{}", task_id))
        .to_request();
    let get_resp = test::call_service(&app, get_req).await;
    let updated: TaskDto = test::read_body_json(get_resp).await;

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
    let test_app = setup_test_db().await;
    let state = create_test_state_with_batch_updater(test_app.pool.clone());

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .configure(configure_routes),
    )
    .await;

    // Create a task
    let task = task_json("batch-multi", "Batch Multi Test", "batch-test");
    let create_req = test::TestRequest::post()
        .uri("/task")
        .insert_header(("requester", "test"))
        .set_json(&vec![task])
        .to_request();
    let create_resp = test::call_service(&app, create_req).await;
    let created: Vec<TaskDto> = test::read_body_json(create_resp).await;
    let task_id = created[0].id;

    // Send multiple rapid updates
    for _ in 0..10 {
        let update_req = test::TestRequest::put()
            .uri(&format!("/task/{}", task_id))
            .set_json(&json!({
                "new_success": 1,
                "new_failures": 0
            }))
            .to_request();
        test::call_service(&app, update_req).await;
    }

    // Wait for batch updater to process
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;

    // Verify all updates were accumulated
    let get_req = test::TestRequest::get()
        .uri(&format!("/task/{}", task_id))
        .to_request();
    let get_resp = test::call_service(&app, get_req).await;
    let updated: TaskDto = test::read_body_json(get_resp).await;

    assert_eq!(
        updated.success, 10,
        "Success counter should accumulate to 10 from 10 updates of +1 each"
    );
}

#[tokio::test]
async fn test_batch_update_rejects_zero_counters() {
    let test_app = setup_test_db().await;
    let state = create_test_state(test_app.pool.clone()); // Don't need batch updater for this

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .configure(configure_routes),
    )
    .await;

    // Create a task
    let task = task_json("batch-reject", "Batch Reject Test", "batch-test");
    let create_req = test::TestRequest::post()
        .uri("/task")
        .insert_header(("requester", "test"))
        .set_json(&vec![task])
        .to_request();
    let create_resp = test::call_service(&app, create_req).await;
    let created: Vec<TaskDto> = test::read_body_json(create_resp).await;
    let task_id = created[0].id;

    // Try to send update with zero counters
    let update_req = test::TestRequest::put()
        .uri(&format!("/task/{}", task_id))
        .set_json(&json!({
            "new_success": 0,
            "new_failures": 0
        }))
        .to_request();
    let update_resp = test::call_service(&app, update_req).await;

    assert_eq!(
        update_resp.status(),
        actix_web::http::StatusCode::BAD_REQUEST,
        "Should reject updates with all zero counters"
    );
}
