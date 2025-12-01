# Task Runner

A Rust service for orchestrating task execution with DAG (Directed Acyclic Graph) dependencies, concurrency control, and webhook-based actions.

## Features

- **DAG Dependencies**: Tasks can depend on other tasks, with support for `requires_success` flag
- **Concurrency Control**: Limit concurrent task execution per task kind using rules
- **Webhook Actions**: Execute webhooks on task start, success, or failure
- **Task States**: Pending, Running, Waiting, Success, Failure, Canceled, Paused
- **Batch Updates**: Efficient batch update endpoint for high-throughput scenarios
- **Prometheus Metrics**: Built-in observability with custom metrics
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
        "Concurency": {
          "matcher": {"kind": "data-processing", "status": "Running", "fields": []},
          "max_concurency": 5
        }
      }
    ]
  }
]
```

Response: `201 Created` with array of created tasks

#### Get Task
```http
GET /task/{task_id}
```

Response: Task details with all actions

#### List Tasks
```http
GET /task?page=0&per_page=50&status=Running
```

Query parameters:
- `page`: Page number (default: 0)
- `per_page`: Items per page (default: 50)
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

#### Pause Task
```http
PATCH /task/pause/{task_id}
```

#### Batch Update (High-throughput)
```http
PUT /task
Content-Type: application/json

{
  "task_id": "uuid",
  "new_success": 5,
  "new_failures": 0
}
```

### Metrics
```http
GET /metrics
```

Prometheus-format metrics including:
- `tasks_created_total` - Total tasks created
- `tasks_completed_total{outcome,kind}` - Tasks completed by outcome
- `task_status_transitions_total{from_status,to_status}` - Status transitions
- `tasks_blocked_by_concurrency_total` - Tasks blocked by rules
- `dependency_propagations_total{parent_outcome}` - Dependency propagations
- `worker_loop_duration_seconds` - Worker loop timing

## Task Lifecycle

```
         ┌─────────────────────────────────────┐
         │                                     │
         ▼                                     │
     ┌───────┐    ┌─────────┐    ┌─────────┐  │
────►│Pending│───►│ Running │───►│ Success │  │
     └───────┘    └─────────┘    └─────────┘  │
         │             │                       │
         │             │         ┌─────────┐  │
         │             └────────►│ Failure │  │
         │                       └─────────┘  │
         │                                     │
         │  (has dependencies)                 │
         ▼                                     │
     ┌───────┐                                 │
     │Waiting│─────────────────────────────────┘
     └───────┘  (all dependencies complete)
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

If a required parent fails, the child is automatically marked as failed.

## Concurrency Rules

Control concurrent execution with rules:

```json
{
  "rules": [
    {
      "Concurency": {
        "matcher": {
          "kind": "data-processing",
          "status": "Running",
          "fields": ["tenant_id"]
        },
        "max_concurency": 3
      }
    }
  ]
}
```

This limits to 3 concurrent `data-processing` tasks with the same `tenant_id` in metadata.

## Configuration

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

# Run tests
cargo test

# Run e2e tests (requires running server)
cd test && bun test
```

## Architecture

- **Actix-web**: HTTP server
- **Diesel + diesel-async**: Async PostgreSQL ORM
- **bb8**: Connection pooling
- **Prometheus**: Metrics
- **thiserror**: Error handling

## TODO

- [ ] Basic front-end UI
- [ ] Automatic rule reuse
- [ ] Automatic action reuse
- [ ] Failure count on actions for retries
