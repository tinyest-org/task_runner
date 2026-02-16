# Integration Tests

This directory contains end-to-end integration tests for the Task Runner service. These tests use **testcontainers** to spin up a real PostgreSQL database in Docker, ensuring tests run against actual database behavior rather than mocks.

## Overview

The integration tests verify:

1. **HTTP API endpoints** - Full request/response cycle through actix-web
2. **Database operations** - CRUD operations, transactions, and data integrity
3. **DAG dependencies** - Task dependency resolution and status propagation
4. **Concurrency rules** - Task scheduling based on concurrency limits
5. **Status transitions** - Lifecycle state machine behavior

## Prerequisites

### Required Software

- **Rust** (edition 2024) - The project's programming language
- **Docker** - Required for testcontainers to run PostgreSQL
- **Docker daemon running** - Ensure Docker is started before running tests

### Verify Docker is Running

```bash
docker info
```

If Docker isn't running, start it:
- **macOS**: Open Docker Desktop
- **Linux**: `sudo systemctl start docker`

## Running the Tests

### Run All Integration Tests

```bash
cargo test --test integration_tests
```

### Run with Output (see test progress)

```bash
cargo test --test integration_tests -- --nocapture
```

### Run a Specific Test

```bash
cargo test --test integration_tests test_health_check
```

### Run Tests Matching a Pattern

```bash
# Run all DAG-related tests
cargo test --test integration_tests dag

# Run all pagination tests
cargo test --test integration_tests pagination

# Run all status transition tests
cargo test --test integration_tests transition
```

### Run Tests in Parallel (default) or Serial

```bash
# Parallel (default, faster)
cargo test --test integration_tests

# Serial (useful for debugging)
cargo test --test integration_tests -- --test-threads=1
```

## Test Categories

### Health Check Tests
- `test_health_check` - Verifies `/health` endpoint returns OK when DB is accessible

### Task CRUD Tests
- `test_create_single_task` - Create a single task
- `test_create_multiple_tasks` - Create multiple tasks in one batch
- `test_get_task_by_id` - Retrieve task by UUID
- `test_get_nonexistent_task` - Returns 404 for unknown task ID

### DAG Dependency Tests
- `test_task_with_single_dependency` - Parent -> Child relationship
- `test_task_with_multiple_dependencies` - Multiple parents -> Child
- `test_diamond_dag_pattern` - A -> B, A -> C, B -> D, C -> D
- `test_linear_chain_dag` - A -> B -> C -> D
- `test_fan_out_dag` - A -> B, C, D, E
- `test_fan_in_dag` - A, B, C, D -> E
- `test_mixed_dependency_requirements` - Required vs optional dependencies

### Status Transition Tests (E2E Behavior)
- `test_child_transitions_when_parent_succeeds` - Child goes Waiting -> Pending
- `test_child_fails_when_required_parent_fails` - Child marked as Failure
- `test_child_proceeds_when_optional_parent_fails` - Child still proceeds
- `test_multi_level_dag_propagation` - Status flows through multi-level DAG
- `test_failure_propagation_through_chain` - Failure cascades through DAG

### Task Listing and Filtering Tests
- `test_list_tasks` - List all tasks
- `test_filter_by_kind` - Filter by task kind
- `test_filter_by_status` - Filter by task status

### Pagination Tests
- `test_pagination_basic` - Basic page navigation
- `test_pagination_last_page` - Partial last page
- `test_pagination_empty_page` - Empty result for out-of-range page

### Task Status Transition Tests
- `test_pause_task` - Pause a pending task
- `test_cancel_pending_task` - Cancel a pending task
- `test_cancel_task_with_waiting_children` - Cancel affects children
- `test_update_task_status_via_api` - Update via PATCH endpoint
- `test_update_task_with_failure_reason` - Include failure reason

### Deduplication Tests
- `test_dedupe_skip_existing` - Skip duplicate task creation

### Concurrency Rules Tests
- `test_task_with_concurrency_rules` - Task with concurrency constraints
- `test_concurrency_rules_per_project` - Per-project concurrency limits
- `test_concurrency_different_projects_parallel` - Different projects run in parallel

### Action Configuration Tests
- `test_task_with_success_and_failure_actions` - Multiple action triggers

### Edge Cases
- `test_task_with_large_metadata` - Large JSON metadata
- `test_task_with_special_characters_in_name` - Unicode and quotes
- `test_concurrent_task_creation` - Batch creation atomicity

## Test Architecture

### Test Infrastructure

```
┌─────────────────────────────────────────────────────────────┐
│                     Integration Test                         │
├─────────────────────────────────────────────────────────────┤
│  ┌─────────────┐    ┌──────────────┐    ┌────────────────┐ │
│  │ Test Client │───>│ actix-web    │───>│ db_operation   │ │
│  │ (requests)  │    │ Test Server  │    │ (database ops) │ │
│  └─────────────┘    └──────────────┘    └────────────────┘ │
│                                                │            │
│                                                ▼            │
│                           ┌────────────────────────────┐   │
│                           │  PostgreSQL (testcontainer)│   │
│                           │  - Fresh DB per test       │   │
│                           │  - Migrations auto-run     │   │
│                           └────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
```

### How Tests Work

1. **Setup**: Each test calls `setup_test_db()` which:
   - Starts a PostgreSQL container via testcontainers
   - Runs all migrations (with special handling for enum modifications)
   - Creates an async connection pool

2. **Test Execution**: Tests use `actix_web::test` to:
   - Create a test application with routes
   - Send HTTP requests
   - Verify responses

3. **Cleanup**: When the test completes:
   - The testcontainer is automatically stopped
   - All data is discarded (fresh state for each test)

### Migration Handling

PostgreSQL's `ALTER TYPE ... ADD VALUE` statements cannot run inside a transaction block. The test infrastructure automatically handles this by:

1. **Scanning migration files** - Reads each migration's `up.sql` looking for `ALTER TYPE ... ADD VALUE` patterns
2. **Running outside transactions** - For affected migrations, reconnects to ensure no active transaction
3. **Adding idempotency** - Automatically adds `IF NOT EXISTS` to prevent errors on reruns
4. **Recording completion** - Manually inserts into `__diesel_schema_migrations`

This means you can add new enum values in future migrations without modifying test infrastructure.

### Key Components

| Component | Purpose |
|-----------|---------|
| `TestApp` | Holds pool and container reference |
| `AppState` | Application state for handlers |
| `setup_test_db()` | Creates fresh database per test |
| `create_test_state()` | Builds app state with pool |
| `configure_test_app()` | Registers all routes |

## Writing New Tests

### Basic Test Template

```rust
#[tokio::test]
async fn test_my_feature() {
    // 1. Setup
    let test_app = setup_test_db().await;
    let state = create_test_state(test_app.pool.clone());
    let app = test::init_service(
        App::new().configure(|cfg| configure_test_app(cfg, state)),
    ).await;

    // 2. Create test data
    let task = task_json("my-task", "My Task", "my-kind");
    let req = test::TestRequest::post()
        .uri("/task")
        .set_json(&vec![task])
        .to_request();
    let resp = test::call_service(&app, req).await;

    // 3. Assert
    assert_eq!(resp.status(), actix_web::http::StatusCode::CREATED);
}
```

### Helper Functions

| Function | Description |
|----------|-------------|
| `task_json(id, name, kind)` | Creates basic task JSON |
| `task_with_deps(id, name, kind, deps)` | Creates task with dependencies |
| `webhook_action()` | Creates webhook action JSON |

## Troubleshooting

### Docker Not Running

```
Error: Docker is not running
```

**Solution**: Start Docker Desktop or the Docker daemon.

### Container Startup Timeout

```
Error: Timeout waiting for container
```

**Solution**:
- Ensure Docker has enough resources
- Try running fewer tests in parallel
- Check Docker logs: `docker logs <container_id>`

### Database Connection Errors

```
Error: Failed to connect to database
```

**Solution**:
- Verify PostgreSQL image is available: `docker pull postgres`
- Check Docker network connectivity
- Increase connection timeout if needed

### Slow Tests

Tests are slow because each creates a fresh PostgreSQL container.

**Tips**:
- Run specific tests instead of the full suite
- Use `--test-threads=1` for sequential execution (helps debugging)
- The container is cached after first pull

## Performance Notes

- **First run**: Slower due to Docker image download
- **Subsequent runs**: ~2-5 seconds per test for container startup
- **Parallel tests**: Faster overall but higher resource usage
- **Serial tests**: Slower but more predictable

## Related Files

- `tests/integration_tests.rs` - Main test file
- `test/e2e_test.ts` - TypeScript E2E tests (requires running server)
- `test/test.ts` - Manual testing script
