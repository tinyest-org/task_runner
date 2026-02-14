# Claude Code Context for Task Runner

This file provides context for Claude Code when working on this project.

## Project Overview

Task Runner is a Rust-based task orchestration service that manages task execution with:
- DAG (Directed Acyclic Graph) dependencies between tasks
- Concurrency control via configurable rules
- Webhook-based action execution
- PostgreSQL persistence with async operations
- Circuit breaker for connection pool resilience
- Distributed tracing via OpenTelemetry
- SSRF protection on webhook URLs

## Key Concepts

### Task States
- `Pending` - Ready to run, waiting for worker to pick up
- `Running` - Currently executing (on_start webhook called)
- `Waiting` - Has unmet dependencies
- `Success` - Completed successfully
- `Failure` - Failed (timeout, explicit failure, or parent failure)
- `Canceled` - Manually canceled
- `Paused` - Manually paused

### Action Model
Actions are webhook calls triggered by task lifecycle events:
- **TriggerKind**: `Start`, `End`, `Cancel`
- **TriggerCondition**: `Success`, `Failure` (determines which end trigger fires)
- **ActionKindEnum**: `Webhook` (only kind currently)

The `on_start` action can return a `NewActionDto` in the response body to register a cancel action for the task.

### Dependency Propagation
When a task completes, `propagate_to_children` in `src/workers.rs` handles:
1. **Success**: Decrements `wait_finished` and `wait_success` counters on children
2. **Failure/Canceled**: Children with `requires_success=true` are marked as `Failure` (recursively)
3. **Transition to Pending**: When `wait_finished=0` and `wait_success=0`, child becomes `Pending`

### Worker Loop
The worker loop in `src/workers.rs`:
1. Finds `Pending` tasks
2. Checks concurrency rules against running tasks
3. Starts eligible tasks (calls on_start webhooks)
4. Handles timeouts
5. Does NOT handle propagation (that's done in `update_running_task` and `cancel_task`)

## Code Architecture

### Entry Points
- `src/main.rs` - HTTP server startup, migration, worker spawning
- `src/test_server.rs` - Test server binary for integration tests
- `src/cache_helper.rs` - Cache utility binary

### HTTP Handlers (`src/handlers.rs`)
All HTTP handler functions and route configuration:
- `configure_routes` - Registers all routes on the Actix `ServiceConfig`
- `health_check` / `readiness_check` - Health and readiness probes
- `add_task` - POST /task (batch create)
- `get_task` - GET /task/{task_id}
- `list_task` - GET /task (filtered, paginated)
- `update_task` - PATCH /task/{task_id}
- `batch_task_updater` - PUT /task/{task_id} (high-throughput counter updates)
- `cancel_task` - DELETE /task/{task_id}
- `pause_task` - PATCH /task/pause/{task_id}
- `get_dag` - GET /dag/{batch_id}
- `view_dag_page` - GET /view (serves static HTML)

### Database Models (`src/models.rs`)
- `Task` - Main task entity with status, metadata, counters
- `Action` - Webhook actions with kind, trigger, condition, params
- `Link` - Parent-child dependency relationships
- `NewTask` / `NewAction` - Insertable structs
- Enums: `StatusKind`, `ActionKindEnum`, `TriggerKind`, `TriggerCondition`

### DTOs (`src/dtos.rs`)
- `NewTaskDto` - Input for creating tasks (includes local `id` for dependency resolution)
- `TaskDto` - Full task response with actions
- `BasicTaskDto` - Lightweight task for listings
- `DagDto` - Tasks + links for visualization
- `UpdateTaskDto` - Task update payload
- `NewActionDto` - Action input (kind + params, trigger determined by context)
- `ActionDto` - Action output (includes trigger)
- `PaginationDto` / `FilterDto` - Query parameters

### Key Functions

**`src/db_operation.rs`**:
- `insert_new_task` - Creates task with dependencies and actions
- `update_running_task` - Updates status, calls `end_task` and `propagate_to_children`
- `find_detailed_task_by_id` - Single query with LEFT JOIN for task + actions
- `list_task_filtered_paged` - Filtered listing with pagination
- `get_dag_for_batch` - Fetches tasks + links for DAG visualization
- `pause_task` - Sets task status to Paused
- `set_started_task` - Atomically transitions Pending -> Running

**`src/workers.rs`**:
- `propagate_to_children` - Handles dependency propagation (recursive for failures)
- `cancel_task` - Cancels task and propagates to children
- `check_concurrency` - Evaluates concurrency rules
- `start_task` - Executes on_start webhooks
- `end_task` - Executes on_success/on_failure webhooks
- `batch_updater` - Batches success/failure count updates to database (see below)

**`src/action.rs`**:
- `ActionExecutor` - Executes webhook actions, passes `?handle=<host>/task/<id>` query param
- `WebhookParams` - URL, HTTP verb, optional body and headers

**`src/circuit_breaker.rs`**:
- `CircuitBreaker` - State machine (Closed -> Open -> HalfOpen) for DB pool resilience
- Records successes/failures and trips when threshold exceeded

**`src/validation.rs`**:
- `validate_task_batch` - Validates entire batch before insertion
- SSRF protection on webhook URLs
- Circular dependency detection

**`src/rule.rs`**:
- `Strategy::Concurency` - Concurrency rule with matcher and max count
- `Matcher` - Matches on status, kind, and metadata fields

### Batch Updater Architecture

The batch updater (`src/workers.rs`) efficiently handles high-throughput success/failure counter updates:

```
+----------------+     channel      +-------------------------------------+
|   Handlers     | ---------------> |          Receiver Task               |
| (HTTP reqs)    |   UpdateEvent    |  - Accumulates counts in DashMap     |
+----------------+                  |  - No blocking, per-shard locks      |
                                    +-------------------------------------+
                                                   |
                                                   | DashMap (concurrent)
                                                   v
                                    +-------------------------------------+
                                    |         Updater Loop                 |
                                    |  - Swaps counts atomically           |
                                    |  - Persists to DB                    |
                                    |  - Re-queues on failure              |
                                    +-------------------------------------+
```

Key design decisions:
- **DashMap**: Lock-free concurrent HashMap with per-shard locking
- **Atomic counters**: `AtomicI32` for success/failures within each entry
- **No data loss**: Failed DB updates re-add counts for retry
- **Cleanup**: Zero-count entries removed periodically

## Testing

### Unit Tests
Located in test modules within source files:
- **`src/workers.rs`**: Batch updater tests (Entry accumulation, swap/requeue, DashMap concurrency, cleanup)
- **`src/validation.rs`**: Input validation, SSRF protection, circular dependency detection
- **`src/config.rs`**: Configuration defaults
- **`src/error.rs`**: Error handling

### Integration Tests (`tests/integration_tests.rs`)
Uses testcontainers for PostgreSQL. Key test categories:
- **CRUD tests**: Create, read, update, delete tasks
- **DAG creation tests**: Verify correct initial states for dependencies
- **Propagation tests**: Verify status transitions when parents complete
- **Cascading tests**: Verify failure/cancel propagation through chain

### Manual Testing (`test/test.ts`)
Bun script for manual API testing:
```bash
bun test.ts dag      # Create CI/CD pipeline DAG
bun test.ts single   # Create single task
bun test.ts list     # List tasks
bun test.ts update <id> Success|Failure
bun test.ts view <batch_id>
```

## Common Tasks

### Adding a New Endpoint
1. Add handler function in `src/handlers.rs`
2. Register route in `configure_routes`
3. Add any new DTOs in `src/dtos.rs`
4. Add database operations in `src/db_operation.rs`

### Adding a New Task Status
1. Add variant to `StatusKind` enum in `src/models.rs`
2. Add migration for the enum value
3. Update `propagate_to_children` if it affects propagation
4. Update `cancel_task` if it should be cancelable from this state

### Fixing a Bug
When fixing a bug, always add an integration test in `tests/integration_tests.rs` that reproduces the bug scenario and verifies the fix. The test should:
1. Be named `test_bug<N>_<short_description>` (e.g. `test_bug7_dedupe_not_over_aggressive_when_metadata_is_none`)
2. Include a doc comment explaining the original bug, the fix, and what the test asserts
3. Assert the **correct** behavior (test passes when the fix is in place, fails if reverted)

Existing bug regression tests are grouped under the "Bug Regression Tests" section at the bottom of `tests/integration_tests.rs`.

### Modifying Propagation Logic
Key file: `src/workers.rs`, function `propagate_to_children`
- `parent_succeeded` - Check if parent was successful
- `parent_failed` - Check if parent failed OR was canceled
- Recursive call for cascading failures

## Important Invariants

1. **Propagation is recursive**: When a child is marked as failed due to parent, it must propagate to its own children
2. **Canceled = Failed for propagation**: `Canceled` status is treated like `Failure` when propagating to children
3. **wait_finished vs wait_success**:
   - `wait_finished` counts ALL dependencies
   - `wait_success` counts only `requires_success=true` dependencies
4. **Actions are per-task**: Each task has its own action records, not shared
5. **Route configuration is centralized**: All routes in `handlers::configure_routes`, shared by main server and test server

## Database Schema

```sql
-- Core tables
task (id, name, kind, status, metadata, timeout, batch_id, start_condition,
      wait_success, wait_finished, success, failures, failure_reason,
      created_at, started_at, ended_at, last_updated)
action (id, task_id, kind, trigger, condition, params, success)
link (parent_id, child_id, requires_success)

-- Key indexes
task_status_kind_idx ON task(status, kind)
task_batch_id_idx ON task(batch_id)
link_parent_id_idx ON link(parent_id)
link_child_id_idx ON link(child_id)
```

## Metrics

Prometheus metrics in `src/metrics.rs`:
- Task lifecycle: created, completed, cancelled, timed out, failed by dependency
- Status transitions and current status gauges
- Concurrency blocks
- Worker loop duration, iterations, tasks processed per loop
- Webhook executions and duration
- Task execution duration and wait time
- Dependency propagations, unblocked tasks
- Batch update failures, DB save failures
- Database query duration and slow query detection
- Circuit breaker state transitions and rejections

## Configuration

All configuration is via environment variables (loaded in `src/config.rs`):

**Required:**
- `DATABASE_URL` - PostgreSQL connection string
- `HOST_URL` - Public URL for webhook callbacks

**Optional:**
- `PORT` (default: 8085) - Server port
- `POOL_MAX_SIZE` (default: 10) - Max pool connections
- `POOL_MIN_IDLE` (default: 5) - Min idle connections
- `POOL_ACQUIRE_RETRIES` (default: 3) - Connection acquire retries
- `POOL_TIMEOUT_SECS` (default: 30) - Connection timeout
- `PAGINATION_DEFAULT` (default: 50) - Default items per page
- `PAGINATION_MAX` (default: 100) - Max items per page
- `WORKER_LOOP_INTERVAL_MS` (default: 1000) - Worker loop interval
- `BATCH_CHANNEL_CAPACITY` (default: 100) - Batch update channel size
- `CIRCUIT_BREAKER_ENABLED` (default: 1) - Enable circuit breaker
- `CIRCUIT_BREAKER_FAILURE_THRESHOLD` (default: 5) - Failures before opening
- `CIRCUIT_BREAKER_FAILURE_WINDOW_SECS` (default: 10) - Failure counting window
- `CIRCUIT_BREAKER_RECOVERY_TIMEOUT_SECS` (default: 30) - Time before half-open
- `CIRCUIT_BREAKER_SUCCESS_THRESHOLD` (default: 2) - Successes to close
- `SLOW_QUERY_THRESHOLD_MS` (default: 100) - Slow query warning threshold
- `TRACING_ENABLED` (default: 0) - Enable distributed tracing
- `OTEL_EXPORTER_OTLP_ENDPOINT` - OTLP endpoint URL
- `OTEL_SERVICE_NAME` (default: task-runner) - Service name for traces
- `OTEL_SAMPLING_RATIO` (default: 1.0) - Sampling ratio
- `SKIP_SSRF_VALIDATION` (default: 1 in debug, 0 in release) - Skip SSRF checks
- `BLOCKED_HOSTNAMES` - Comma-separated blocked hostnames
- `BLOCKED_HOSTNAME_SUFFIXES` - Comma-separated blocked hostname suffixes
- `RUST_LOG` (default: info) - Log level

## Project Structure

```
src/
+-- main.rs           # HTTP server, routes, and startup
+-- test_server.rs    # Test server binary
+-- cache_helper.rs   # Cache utility binary
+-- lib.rs            # Module declarations, DB pool initialization
+-- handlers.rs       # HTTP handlers and route configuration
+-- models.rs         # Database models (Task, Action, Link, enums)
+-- dtos.rs           # API DTOs and query parameters
+-- schema.rs         # Diesel schema (auto-generated)
+-- db_operation.rs   # Database operations
+-- workers.rs        # Background worker loop, propagation, batch updater
+-- action.rs         # Webhook action execution
+-- rule.rs           # Concurrency rules and matchers
+-- config.rs         # Configuration loading from env vars
+-- metrics.rs        # Prometheus metrics
+-- validation.rs     # Input validation and SSRF protection
+-- error.rs          # Typed error definitions
+-- circuit_breaker.rs # Circuit breaker for DB pool resilience
+-- tracing.rs        # OpenTelemetry distributed tracing
+-- helper.rs         # Internal helpers
static/
+-- dag.html          # DAG visualization UI
test/
+-- test.ts           # Manual testing script (Bun)
migrations/           # Diesel migrations
tests/
+-- integration_tests.rs  # Integration tests with testcontainers
```
