# Task Runner

A Rust service for orchestrating task execution with DAG (Directed Acyclic Graph) dependencies, concurrency control, and webhook-based actions.

## Features

- **DAG Dependencies**: Tasks can depend on other tasks, with support for `requires_success` flag
- **Cascading Propagation**: Failures and cancellations automatically propagate through the dependency chain
- **Concurrency Control**: Limit concurrent task execution per task kind using rules
- **Webhook Actions**: Execute webhooks on task start, success, failure, or cancellation
- **Task States**: Waiting, Pending, Claimed, Running, Success, Failure, Canceled, Paused
- **DAG Visualization**: Built-in web UI for visualizing task DAGs with auto-layout
- **Batch Updates**: Efficient batch update endpoint for high-throughput scenarios
- **Prometheus Metrics**: Built-in observability with custom metrics
- **Deduplication**: Skip duplicate tasks based on metadata fields
- **Input Validation**: Comprehensive validation with detailed error messages
- **Circuit Breaker**: Connection pool resilience with automatic recovery
- **Distributed Tracing**: OpenTelemetry support with OTLP export
- **SSRF Protection**: Webhook URL validation to prevent server-side request forgery
- **Health Checks**: Liveness and readiness probes for Kubernetes deployments

## API Endpoints

### Health

#### Health Check
```http
GET /health
```

Returns database connectivity status and connection pool stats.

Response `200 OK` (healthy) or `503 Service Unavailable` (degraded):
```json
{
  "status": "ok",
  "database": "healthy",
  "pool_size": 10,
  "pool_idle": 5
}
```

#### Readiness Check
```http
GET /ready
```

Stricter check - verifies pool is not exhausted and a connection can be acquired.

Response `200 OK` or `503 Service Unavailable`:
```json
{"status": "ready"}
```

### Tasks

#### Create Tasks
```http
POST /task
Content-Type: application/json

[
  {
    "id": "local-id-1",
    "name": "My Task",
    "kind": "data-processing",
    "timeout": 60,
    "metadata": {"key": "value"},
    "on_start": {
      "kind": "Webhook",
      "params": {
        "url": "https://example.com/webhook",
        "verb": "Post",
        "body": {"optional": "payload"},
        "headers": {"X-Custom": "header"}
      }
    },
    "dependencies": [
      {"id": "local-id-0", "requires_success": true}
    ],
    "on_success": [
      {"kind": "Webhook", "params": {"url": "https://example.com/done", "verb": "Post"}}
    ],
    "on_failure": [
      {"kind": "Webhook", "params": {"url": "https://example.com/failed", "verb": "Post"}}
    ],
    "rules": [
      {
        "type": "Concurency",
        "matcher": {"kind": "data-processing", "status": "Running", "fields": []},
        "max_concurency": 5
      }
    ],
    "dedupe_strategy": [
      {"kind": "data-processing", "status": "Pending", "fields": ["key"]}
    ]
  }
]
```

Fields:
- `timeout` (seconds): Maximum inactivity time. The task is marked as `Failure` if `last_updated` exceeds this duration. Batch counter updates (`PUT /task/{id}`) refresh `last_updated`, resetting the timeout clock.

Response: `201 Created` with array of created tasks, includes `X-Batch-ID` header.
If all tasks were deduplicated: `204 No Content` with `X-Batch-ID` header.

On validation failure: `400 Bad Request`:
```json
{
  "error": "Validation failed",
  "batch_id": "uuid",
  "details": ["error message 1", "error message 2"]
}
```

#### Get Task
```http
GET /task/{task_id}
```

Response: Full task details with all actions (fetched via single LEFT JOIN query).

```json
{
  "id": "uuid",
  "name": "My Task",
  "kind": "data-processing",
  "status": "Running",
  "timeout": 60,
  "rules": [],
  "metadata": {"key": "value"},
  "actions": [
    {"kind": "Webhook", "trigger": "Start", "params": {"url": "...", "verb": "Post"}}
  ],
  "created_at": "2024-01-01T00:00:00Z",
  "started_at": "2024-01-01T00:00:01Z",
  "ended_at": null,
  "last_updated": "2024-01-01T00:00:01Z",
  "success": 0,
  "failures": 0,
  "failure_reason": null,
  "batch_id": "uuid"
}
```

#### List Tasks
```http
GET /task?page=0&page_size=50&status=Running&kind=data-processing&batch_id=uuid&name=my&metadata={"key":"value"}
```

Query parameters:
- `page`: Page number (default: 0)
- `page_size`: Items per page (default: 50, max: 100)
- `status`: Filter by status (optional)
- `kind`: Filter by kind (substring match, optional)
- `name`: Filter by name (substring match, optional)
- `batch_id`: Filter by batch ID (optional)
- `metadata`: Filter by metadata JSON containment (optional)

Response: `200 OK` with array of `BasicTaskDto`.

#### Update Task
```http
PATCH /task/{task_id}
Content-Type: application/json

{
  "status": "Success",
  "new_success": 10,
  "new_failures": 2,
  "metadata": {"updated": true},
  "failure_reason": "Error message (required if status=Failure)"
}
```

Only `Success` or `Failure` status transitions are allowed. Setting status triggers end actions and dependency propagation. Failed tasks cannot be updated further.

#### Cancel Task
```http
DELETE /task/{task_id}
```

Cancels a pending or running task. For running tasks, executes cancel actions.
**Cancellation propagates** to dependent children that require success.

#### Pause Task
```http
PATCH /task/pause/{task_id}
```

Pauses a task (sets status to `Paused`).

#### Batch Update (High-throughput)
```http
PUT /task/{task_id}
Content-Type: application/json

{
  "new_success": 5,
  "new_failures": 2
}
```

This endpoint efficiently batches counter updates using a lock-free `DashMap` architecture for high concurrency. At least one of `new_success` or `new_failures` must be non-zero. Returns `202 Accepted` when the update is queued.

Each batch update refreshes the task's `last_updated` timestamp, which resets the timeout clock. This prevents active tasks (still receiving updates) from being incorrectly timed out.

### DAG Visualization

#### Get DAG Data
```http
GET /dag/{batch_id}
```

Returns tasks and links for a batch in JSON format:
```json
{
  "tasks": [
    {"id": "uuid", "name": "...", "kind": "...", "status": "Running", ...}
  ],
  "links": [
    {"parent_id": "uuid", "child_id": "uuid", "requires_success": true}
  ]
}
```

#### View DAG UI
```http
GET /view?batch={batch_id}
```

Opens the built-in DAG visualization UI with:
- Cytoscape.js with Dagre auto-layout
- Color-coded nodes by status
- Click on nodes for task details
- Auto-refresh option (5s interval)
- Pan, zoom, and fit controls

### Metrics
```http
GET /metrics
```

Prometheus-format metrics (see [Metrics](#metrics-1) section).

## Webhook Execution

When a task starts, the `on_start` webhook is called with a `?handle=<host_url>/task/<task_id>` query parameter. This allows the webhook target to update the task status directly.

Webhook params:
```json
{
  "url": "https://example.com/webhook",
  "verb": "Post",
  "body": {"optional": "json payload"},
  "headers": {"X-Custom": "header value"}
}
```

Supported HTTP verbs: `Get`, `Post`, `Put`, `Patch`, `Delete`.

The runner sends these headers on webhook requests:
- `Idempotency-Key`: `"<task_id>:start"`, `"<task_id>:end:success"`, `"<task_id>:end:failure"`, `"<task_id>:cancel"`
- `X-Task-Id`: task UUID
- `X-Task-Trigger`: `start`, `end`, or `cancel`

The `on_start` webhook response body can optionally contain a `NewActionDto` JSON to register a cancel action for the task.

## Task Lifecycle

```
         +-----------------------------------------------------+
         |                                                     |
         v                                                     |
     +-------+   +---------+   +---------+   +---------+      |
---->|Pending|-->| Claimed |-->| Running |-->| Success |      |
     +-------+   +---------+   +---------+   +---------+      |
         |             |             |                         |
         |             |             |         +---------+     |
         |             |             +-------->| Failure |     |
         |             |                       +---------+     |
         |             |                                     |
         | (has deps)  | (start timeout -> requeue)          |
         v             |                                     |
     +-------+         |                                     |
     |Waiting|---------+-------------------------------------+
     +-------+  (all dependencies complete)

     +----------+     +----------+
     | Canceled |     |  Paused  |
     +----------+     +----------+
```

## Dependencies

Tasks can specify dependencies using local IDs within the same batch:

```json
[
  {"id": "ingest-1", "name": "Ingest A", ...},
  {"id": "ingest-2", "name": "Ingest B", ...},
  {
    "id": "cluster",
    "name": "Cluster",
    "dependencies": [
      {"id": "ingest-1", "requires_success": true},
      {"id": "ingest-2", "requires_success": false}
    ],
    ...
  }
]
```

- `requires_success: true` - Parent must succeed for child to proceed
- `requires_success: false` - Parent just needs to finish (success or failure)

### Propagation Behavior

- **Parent succeeds**: Children with all dependencies met transition to `Pending`
- **Parent fails**: Children with `requires_success: true` are marked as `Failure` (cascades recursively)
- **Parent canceled**: Treated like failure - children with `requires_success: true` are marked as `Failure`

## Concurrency Rules

Control concurrent execution with rules:

```json
{
  "rules": [
    {
      "type": "Concurency",
      "matcher": {
        "kind": "data-processing",
        "status": "Running",
        "fields": ["tenant_id"]
      },
      "max_concurency": 3
    }
  ]
}
```

This limits to 3 concurrent `data-processing` tasks with the same `tenant_id` in metadata.

## Capacity Rules

Capacity rules limit total remaining work across Running (and Claimed) tasks that match the same `kind` and metadata `fields`. Remaining work is computed as `max(coalesce(expected_count, 0) - success - failures, 0)`; tasks without `expected_count` contribute `0`. Tasks that use Capacity rules must set `expected_count`, and `matcher.status` must be `Running`.

```json
{
  "rules": [
    {
      "type": "Capacity",
      "matcher": {
        "kind": "data-processing",
        "status": "Running",
        "fields": ["tenant_id"]
      },
      "max_capacity": 500
    }
  ],
  "expected_count": 1000
}
```

## Deduplication

Skip creating duplicate tasks based on metadata fields:

```json
{
  "dedupe_strategy": [
    {
      "kind": "data-processing",
      "status": "Pending",
      "fields": ["project_id"]
    }
  ]
}
```

If a task with the same `kind`, `status`, and matching `project_id` exists, the new task is skipped.

## Configuration

All configuration is via environment variables.

### Required

| Variable | Description |
|----------|-------------|
| `DATABASE_URL` | PostgreSQL connection string |
| `HOST_URL` | Public URL for webhook callbacks (must start with `http://` or `https://`) |

### Optional

| Variable | Default | Description |
|----------|---------|-------------|
| `PORT` | `8085` | Server port |
| `RUST_LOG` | `info` | Log level |

### Connection Pool

| Variable | Default | Description |
|----------|---------|-------------|
| `POOL_MAX_SIZE` | `10` | Maximum connections |
| `POOL_MIN_IDLE` | `5` | Minimum idle connections |
| `POOL_ACQUIRE_RETRIES` | `3` | Connection acquire retries |
| `POOL_TIMEOUT_SECS` | `30` | Connection timeout in seconds |

### Pagination

| Variable | Default | Description |
|----------|---------|-------------|
| `PAGINATION_DEFAULT` | `50` | Default items per page |
| `PAGINATION_MAX` | `100` | Maximum items per page |

### Worker

| Variable | Default | Description |
|----------|---------|-------------|
| `WORKER_LOOP_INTERVAL_MS` | `1000` | Worker loop interval in ms |
| `WORKER_CLAIM_TIMEOUT_SECS` | `30` | Max time a task can stay Claimed before requeue |
| `BATCH_CHANNEL_CAPACITY` | `100` | Batch update channel size |

### Circuit Breaker

| Variable | Default | Description |
|----------|---------|-------------|
| `CIRCUIT_BREAKER_ENABLED` | `1` | Enable circuit breaker (0 to disable) |
| `CIRCUIT_BREAKER_FAILURE_THRESHOLD` | `5` | Failures before circuit opens |
| `CIRCUIT_BREAKER_FAILURE_WINDOW_SECS` | `10` | Time window for counting failures |
| `CIRCUIT_BREAKER_RECOVERY_TIMEOUT_SECS` | `30` | Time before trying half-open |
| `CIRCUIT_BREAKER_SUCCESS_THRESHOLD` | `2` | Successes in half-open to close |

### Observability

| Variable | Default | Description |
|----------|---------|-------------|
| `SLOW_QUERY_THRESHOLD_MS` | `100` | Slow query warning threshold in ms |
| `TRACING_ENABLED` | `0` | Enable OpenTelemetry distributed tracing |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | - | OTLP endpoint URL (e.g., `http://localhost:4317`) |
| `OTEL_SERVICE_NAME` | `task-runner` | Service name for traces |
| `OTEL_SAMPLING_RATIO` | `1.0` | Sampling ratio (0.0 to 1.0) |

### Security

| Variable | Default | Description |
|----------|---------|-------------|
| `SKIP_SSRF_VALIDATION` | `1` (debug) / `0` (release) | Skip SSRF validation on webhook URLs |
| `BLOCKED_HOSTNAMES` | `localhost,127.0.0.1,::1,0.0.0.0,local,internal` | Comma-separated blocked hostnames |
| `BLOCKED_HOSTNAME_SUFFIXES` | `.local,.internal,.localdomain,.localhost` | Comma-separated blocked hostname suffixes |

## Metrics

Prometheus metrics exposed at `GET /metrics`:

### Task Counters
| Metric | Labels | Description |
|--------|--------|-------------|
| `tasks_created_total` | - | Total tasks created |
| `tasks_completed_total` | `outcome`, `kind` | Tasks completed by outcome and kind |
| `tasks_cancelled_total` | - | Tasks cancelled |
| `tasks_timed_out_total` | - | Tasks timed out |
| `task_status_transitions_total` | `from_status`, `to_status` | Status transitions |

### Task Gauges
| Metric | Labels | Description |
|--------|--------|-------------|
| `tasks_by_status` | `status` | Current tasks by status |
| `running_tasks_by_kind` | `kind` | Running tasks by kind |

### Dependencies
| Metric | Labels | Description |
|--------|--------|-------------|
| `tasks_with_dependencies_total` | - | Tasks created with dependencies |
| `dependency_propagations_total` | `parent_outcome` | Dependency propagations |
| `tasks_unblocked_total` | - | Tasks unblocked after dependencies completed |
| `tasks_failed_by_dependency_total` | - | Tasks failed due to parent failure |

### Webhooks
| Metric | Labels | Description |
|--------|--------|-------------|
| `webhook_executions_total` | `trigger`, `outcome` | Webhook calls |
| `webhook_attempts_total` | `trigger`, `outcome` | Webhook attempts (includes failures) |
| `webhook_duration_seconds` | `trigger` | Webhook duration histogram |
| `webhook_idempotent_skips_total` | `trigger` | Webhook executions skipped due to idempotency |
| `webhook_idempotent_conflicts_total` | - | Idempotency conflicts when claiming executions |

### Concurrency
| Metric | Labels | Description |
|--------|--------|-------------|
| `tasks_blocked_by_concurrency_total` | - | Tasks blocked by rules |

### Duration
| Metric | Labels | Description |
|--------|--------|-------------|
| `task_duration_seconds` | `kind`, `outcome` | Task execution duration |
| `task_wait_seconds` | `kind` | Time from Pending to Running (includes Claimed) |

### Worker
| Metric | Labels | Description |
|--------|--------|-------------|
| `worker_loop_iterations_total` | - | Worker loop iterations |
| `worker_loop_duration_seconds` | - | Worker loop duration |
| `tasks_processed_per_loop` | - | Tasks processed per iteration |

### Database
| Metric | Labels | Description |
|--------|--------|-------------|
| `db_query_duration_seconds` | `query` | Query duration |
| `slow_queries_total` | `query` | Queries exceeding threshold |
| `tasks_db_save_failures_total` | - | DB save failures after retries |
| `batch_update_failures_total` | - | Batch update failures (re-queued) |

### Circuit Breaker
| Metric | Labels | Description |
|--------|--------|-------------|
| `circuit_breaker_state_transitions_total` | `to_state` | State transitions |
| `circuit_breaker_rejections_total` | - | Requests rejected |

## Development

```bash
# Run migrations
diesel migration run

# Start server
cargo run --bin server

# Run tests (requires Docker for testcontainers)
cargo test

# Run integration tests only
cargo test --test integration_tests

# Manual testing with bun
cd test && bun test.ts dag
```

### Test Commands

```bash
# Create a CI/CD pipeline DAG
bun test.ts dag

# Create a single task
bun test.ts single

# List all tasks
bun test.ts list

# Update a task status
bun test.ts update <task_id> Success
bun test.ts update <task_id> Failure

# View DAG data as JSON
bun test.ts view <batch_id>
```

## Releasing

Create a new release by pushing a Git tag:

```bash
# Create and push a version tag
git tag v1.0.0
git push origin v1.0.0
```

This triggers the CI pipeline which builds multi-arch Docker images (amd64 + arm64) and pushes them to DockerHub with the following tags:

| Tag | Example | Description |
|-----|---------|-------------|
| `{version}` | `1.0.0` | Full semantic version |
| `{major}.{minor}` | `1.0` | Major.minor version |
| `{major}` | `1` | Major version (not created for v0.x) |
| `sha-{commit}` | `sha-abc1234` | Git commit SHA |
| `latest` | `latest` | Updated on main/master branch pushes |

Pull the image:
```bash
docker pull plawn/task-runner:1.0.0
# or
docker pull plawn/task-runner:latest
```

## Architecture

- **Actix-web**: HTTP server with async handlers
- **Diesel + diesel-async**: Async PostgreSQL ORM with bb8 connection pooling
- **Start Loop**: Background loop that:
  - Checks pending tasks against concurrency rules
  - Claims and starts eligible tasks (executes on_start webhooks)
  - Propagates completions to dependent children
- **Timeout Loop**: Background loop that:
  - Finds running tasks where `last_updated` exceeds the timeout duration
  - Marks them as failed, propagates to children, fires on_failure webhooks
- **Batch Updater**: High-throughput counter updates using:
  - `DashMap` for lock-free concurrent access (per-shard locking)
  - Atomic counters (`AtomicI32`) for success/failure counts
  - Automatic retry on DB failure (re-queues counts)
  - Periodic cleanup of zero-count entries
- **Circuit Breaker**: Connection pool resilience with states:
  - Closed (normal) -> Open (rejecting) -> HalfOpen (testing recovery)
- **OpenTelemetry**: Distributed tracing with OTLP export
- **Prometheus**: Metrics exposition with custom registry
- **Cytoscape.js + Dagre**: DAG visualization with auto-layout

## Project Structure

```
src/
+-- main.rs              # HTTP server, startup, worker spawning
+-- test_server.rs       # Test server binary
+-- cache_helper.rs      # Cache utility binary
+-- lib.rs               # Module declarations, DB pool initialization
+-- handlers.rs          # HTTP handlers and route configuration
+-- models.rs            # Database models (Task, Action, Link, enums)
+-- dtos.rs              # API DTOs and query parameters
+-- schema.rs            # Diesel schema (auto-generated)
+-- db_operation.rs      # Database operations
+-- workers.rs           # Background worker loop, propagation, batch updater
+-- action.rs            # Webhook action execution
+-- rule.rs              # Concurrency rules and matchers
+-- config.rs            # Configuration loading from env vars
+-- metrics.rs           # Prometheus metrics
+-- validation.rs        # Input validation and SSRF protection
+-- error.rs             # Typed error definitions
+-- circuit_breaker.rs   # Circuit breaker for DB pool resilience
+-- tracing.rs           # OpenTelemetry distributed tracing
+-- helper.rs            # Internal helpers
static/
+-- dag.html             # DAG visualization UI
test/
+-- test.ts              # Manual testing script (Bun)
migrations/              # Diesel migrations
tests/
+-- integration_tests.rs # Integration tests with testcontainers
```

## TODO

- [x] DAG visualization UI
- [x] Cascading failure propagation
- [x] Cancel propagation to children
- [x] Circuit breaker for connection pool
- [x] Distributed tracing
- [x] SSRF protection
- [x] Health and readiness endpoints
- [ ] Automatic rule reuse
- [ ] Automatic action reuse
- [ ] Failure count on actions for retries
- [ ] Task retry with backoff
