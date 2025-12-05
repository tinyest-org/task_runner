# Task Runner

A Rust service for orchestrating task execution with DAG (Directed Acyclic Graph) dependencies, concurrency control, and webhook-based actions.

## Features

- **DAG Dependencies**: Tasks can depend on other tasks, with support for `requires_success` flag
- **Cascading Propagation**: Failures and cancellations automatically propagate through the dependency chain
- **Concurrency Control**: Limit concurrent task execution per task kind using rules
- **Webhook Actions**: Execute webhooks on task start, success, failure, or cancellation
- **Task States**: Pending, Running, Waiting, Success, Failure, Canceled, Paused
- **DAG Visualization**: Built-in web UI for visualizing task DAGs with auto-layout
- **Batch Updates**: Efficient batch update endpoint for high-throughput scenarios
- **Prometheus Metrics**: Built-in observability with custom metrics
- **Deduplication**: Skip duplicate tasks based on metadata fields
- **Input Validation**: Comprehensive validation with detailed error messages

## API Endpoints

### Tasks

#### Create Tasks
```http
POST /task
Content-Type: application/json
X-Requester: your-service-name

[
  {
    "id": "local-id-1",
    "name": "My Task",
    "kind": "data-processing",
    "timeout": 60,
    "metadata": {"key": "value"},
    "on_start": {
      "kind": "Webhook",
      "trigger": "Start",
      "params": {
        "url": "https://example.com/webhook",
        "verb": "Post"
      }
    },
    "dependencies": [
      {"id": "local-id-0", "requires_success": true}
    ],
    "on_success": [...],
    "on_failure": [...],
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

Response: `201 Created` with array of created tasks, includes `X-Batch-ID` header

#### Get Task
```http
GET /task/{task_id}
```

Response: Task details with all actions

#### List Tasks
```http
GET /task?page=0&page_size=50&status=Running&kind=data-processing
```

Query parameters:
- `page`: Page number (default: 0)
- `page_size`: Items per page (default: 50, max: 100)
- `status`: Filter by status (optional)
- `kind`: Filter by kind (optional)

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

### DAG Visualization

#### Get DAG Data
```http
GET /dag/{batch_id}
```

Returns tasks and links for a batch in JSON format for visualization.

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

Prometheus-format metrics including:
- `tasks_created_total` - Total tasks created
- `tasks_completed_total{outcome,kind}` - Tasks completed by outcome
- `task_status_transitions_total{from_status,to_status}` - Status transitions
- `tasks_blocked_by_concurrency_total` - Tasks blocked by rules
- `tasks_failed_by_dependency_total` - Tasks failed due to parent failure
- `tasks_cancelled_total` - Tasks cancelled
- `dependency_propagations_total{parent_outcome}` - Dependency propagations
- `worker_loop_duration_seconds` - Worker loop timing

## Task Lifecycle

```
         +-------------------------------------+
         |                                     |
         v                                     |
     +-------+    +---------+    +---------+  |
---->|Pending|--->| Running |--->| Success |  |
     +-------+    +---------+    +---------+  |
         |             |                       |
         |             |         +---------+  |
         |             +-------->| Failure |  |
         |                       +---------+  |
         |                                     |
         |  (has dependencies)                 |
         v                                     |
     +-------+                                 |
     |Waiting|---------------------------------+
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

Configuration via `config.toml` or environment variables:

```toml
port = 8085
host_url = "http://localhost:8085"
database_url = "postgres://user:pass@localhost/taskrunner"

[pool]
max_size = 10
min_idle = 2
max_lifetime = "30m"
idle_timeout = "10m"
connection_timeout = "30s"
acquire_retries = 3
retry_delay = "100ms"

[pagination]
default_per_page = 50
max_per_page = 100

[worker]
loop_interval = "1s"
timeout_check_interval = "5s"
batch_flush_interval = "100ms"
batch_channel_capacity = 100
```

Environment variables:
- `DATABASE_URL` - PostgreSQL connection string
- `HOST_URL` - Public URL for webhook callbacks
- `RUST_LOG` - Log level (default: info)

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

## Architecture

- **Actix-web**: HTTP server with async handlers
- **Diesel + diesel-async**: Async PostgreSQL ORM with bb8 connection pooling
- **Worker Loop**: Background task that:
  - Checks pending tasks against concurrency rules
  - Starts eligible tasks (executes on_start webhooks)
  - Checks for timeouts
  - Propagates completions to dependent children
- **Batch Updater**: High-throughput counter updates using:
  - `DashMap` for lock-free concurrent access (per-shard locking)
  - Atomic counters (`AtomicI32`) for success/failure counts
  - Automatic retry on DB failure (re-queues counts)
  - Periodic cleanup of zero-count entries
- **Prometheus**: Metrics exposition
- **Cytoscape.js + Dagre**: DAG visualization with auto-layout

## Project Structure

```
src/
├── main.rs          # HTTP server, routes, and handlers
├── models.rs        # Database models (Task, Action, Link)
├── dtos.rs          # API DTOs and validation
├── schema.rs        # Diesel schema (auto-generated)
├── db_operation.rs  # Database operations
├── workers.rs       # Background worker loop and propagation
├── action.rs        # Webhook action execution
├── config.rs        # Configuration loading
├── metrics.rs       # Prometheus metrics
└── validation.rs    # Input validation
static/
└── dag.html         # DAG visualization UI
test/
└── test.ts          # Manual testing script
migrations/          # Diesel migrations
tests/
└── integration_tests.rs  # Integration tests with testcontainers
```

## TODO

- [x] DAG visualization UI
- [x] Cascading failure propagation
- [x] Cancel propagation to children
- [ ] Automatic rule reuse
- [ ] Automatic action reuse
- [ ] Failure count on actions for retries
- [ ] Task retry with backoff
