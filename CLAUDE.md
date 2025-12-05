# Claude Code Context for Task Runner

This file provides context for Claude Code when working on this project.

## Project Overview

Task Runner is a Rust-based task orchestration service that manages task execution with:
- DAG (Directed Acyclic Graph) dependencies between tasks
- Concurrency control via configurable rules
- Webhook-based action execution
- PostgreSQL persistence with async operations

## Key Concepts

### Task States
- `Pending` - Ready to run, waiting for worker to pick up
- `Running` - Currently executing (on_start webhook called)
- `Waiting` - Has unmet dependencies
- `Success` - Completed successfully
- `Failure` - Failed (timeout, explicit failure, or parent failure)
- `Canceled` - Manually canceled
- `Paused` - Manually paused

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

### Database Models (`src/models.rs`)
- `Task` - Main task entity with status, metadata, counters
- `Action` - Webhook actions (on_start, on_success, on_failure, on_cancel)
- `Link` - Parent-child dependency relationships

### DTOs (`src/dtos.rs`)
- `NewTaskDto` - Input for creating tasks
- `TaskDto` - Full task response with actions
- `BasicTaskDto` - Lightweight task for listings
- `DagDto` - Tasks + links for visualization
- `UpdateTaskDto` - Task update payload

### Key Functions

**`src/db_operation.rs`**:
- `insert_new_task` - Creates task with dependencies and actions
- `update_running_task` - Updates status, calls `end_task` and `propagate_to_children`
- `get_dag_for_batch` - Fetches tasks + links for DAG visualization

**`src/workers.rs`**:
- `propagate_to_children` - Handles dependency propagation (recursive for failures)
- `cancel_task` - Cancels task and propagates to children
- `check_concurrency` - Evaluates concurrency rules
- `start_task` - Executes on_start webhooks
- `end_task` - Executes on_success/on_failure webhooks
- `batch_updater` - Batches success/failure count updates to database (see below)

### Batch Updater Architecture

The batch updater (`src/workers.rs`) efficiently handles high-throughput success/failure counter updates:

```
┌──────────────┐     channel      ┌─────────────────────────────────────┐
│   Handlers   │ ──────────────► │          Receiver Task               │
│ (HTTP reqs)  │   UpdateEvent   │  - Accumulates counts in DashMap     │
└──────────────┘                  │  - No blocking, per-shard locks      │
                                  └─────────────────────────────────────┘
                                                   │
                                                   │ DashMap (concurrent)
                                                   ▼
                                  ┌─────────────────────────────────────┐
                                  │         Updater Loop                 │
                                  │  - Swaps counts atomically           │
                                  │  - Persists to DB                    │
                                  │  - Re-queues on failure              │
                                  └─────────────────────────────────────┘
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
1. Add handler function in `src/main.rs`
2. Register route in `App::new().service(...)`
3. Add any new DTOs in `src/dtos.rs`
4. Add database operations in `src/db_operation.rs`

### Adding a New Task Status
1. Add variant to `StatusKind` enum in `src/models.rs`
2. Add migration for the enum value
3. Update `propagate_to_children` if it affects propagation
4. Update `cancel_task` if it should be cancelable from this state

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

## Database Schema

```sql
-- Core tables
task (id, name, kind, status, metadata, timeout, batch_id, ...)
action (id, task_id, kind, trigger, params, ...)
link (parent_id, child_id, requires_success)

-- Key indexes
task_status_kind_idx ON task(status, kind)
task_batch_id_idx ON task(batch_id)
link_parent_id_idx ON link(parent_id)
link_child_id_idx ON link(child_id)
```

## Metrics

Prometheus metrics in `src/metrics.rs`:
- Task lifecycle: created, completed, cancelled, failed by dependency
- Status transitions
- Concurrency blocks
- Worker loop duration
- Dependency propagations

## Configuration

`config.toml` with sections:
- Root: port, host_url, database_url
- `[pool]`: Connection pool settings
- `[pagination]`: Default/max page sizes
- `[worker]`: Loop intervals and timeouts
