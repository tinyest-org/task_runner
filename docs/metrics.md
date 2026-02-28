# Prometheus Metrics

All metrics are exposed at `GET /metrics` in Prometheus format.

## Task Counters

| Metric | Labels | Description |
|--------|--------|-------------|
| `tasks_created_total` | - | Total tasks created |
| `tasks_completed_total` | `outcome`, `kind` | Tasks completed by outcome and kind |
| `tasks_cancelled_total` | - | Tasks cancelled |
| `tasks_timed_out_total` | - | Tasks timed out |
| `task_status_transitions_total` | `from_status`, `to_status` | Status transitions |

## Task Gauges

| Metric | Labels | Description |
|--------|--------|-------------|
| `tasks_by_status` | `status` | Current tasks by status |
| `running_tasks_by_kind` | `kind` | Running tasks by kind |

## Dependencies

| Metric | Labels | Description |
|--------|--------|-------------|
| `tasks_with_dependencies_total` | - | Tasks created with dependencies |
| `dependency_propagations_total` | `parent_outcome` | Dependency propagations |
| `tasks_unblocked_total` | - | Tasks unblocked after dependencies completed |
| `tasks_failed_by_dependency_total` | - | Tasks failed due to parent failure |

## Webhooks

| Metric | Labels | Description |
|--------|--------|-------------|
| `webhook_executions_total` | `trigger`, `outcome` | Webhook calls |
| `webhook_attempts_total` | `trigger`, `outcome` | Webhook attempts (includes failures) |
| `webhook_duration_seconds` | `trigger` | Webhook duration histogram |
| `webhook_idempotent_skips_total` | `trigger` | Webhook executions skipped due to idempotency |
| `webhook_idempotent_conflicts_total` | - | Idempotency conflicts when claiming executions |

## Concurrency

| Metric | Labels | Description |
|--------|--------|-------------|
| `tasks_blocked_by_concurrency_total` | - | Tasks blocked by rules |

## Duration

| Metric | Labels | Description |
|--------|--------|-------------|
| `task_duration_seconds` | `kind`, `outcome` | Task execution duration |
| `task_wait_seconds` | `kind` | Time from Pending to Running (includes Claimed) |

## Worker

| Metric | Labels | Description |
|--------|--------|-------------|
| `worker_loop_iterations_total` | - | Worker loop iterations |
| `worker_loop_duration_seconds` | - | Worker loop duration |
| `tasks_processed_per_loop` | - | Tasks processed per iteration |

## Database

| Metric | Labels | Description |
|--------|--------|-------------|
| `db_query_duration_seconds` | `query` | Query duration |
| `slow_queries_total` | `query` | Queries exceeding threshold |
| `tasks_db_save_failures_total` | - | DB save failures after retries |
| `batch_update_failures_total` | - | Batch update failures (re-queued) |

## Circuit Breaker

| Metric | Labels | Description |
|--------|--------|-------------|
| `circuit_breaker_state_transitions_total` | `to_state` | State transitions |
| `circuit_breaker_rejections_total` | - | Requests rejected |
