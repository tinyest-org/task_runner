# Task Runner - Task Orchestration Service

A webhook-based task scheduler that executes DAGs (Directed Acyclic Graphs) of tasks with concurrency control.

## Core Workflow

1. **Create a batch** of tasks via `POST /task` with an array of `NewTaskDto`. Each task has a local `id` (string, your choice) used to wire dependencies within the batch. The server assigns real UUIDs and returns them. The response includes an `X-Batch-ID` header grouping all tasks.
2. **The worker loop** (server-side, automatic) picks up `Pending` tasks, checks concurrency rules, and calls each task's `on_start` webhook. The webhook URL receives a `?handle=<host>/task/<uuid>` query parameter -- this is the callback URL your service must use to report completion.
3. **Report completion** by calling `PATCH /task/{task_id}` with `{"status": "Success"}` or `{"status": "Failure", "failure_reason": "..."}`. Only `Success` and `Failure` are valid status transitions via the API.
4. **Dependency propagation** happens automatically: when a parent succeeds, its children's counters decrement; when all counters reach zero, the child becomes `Pending`. When a parent fails and `requires_success` was true on the link, the child (and its descendants) are marked as `Failure`.

## Task States

| State | Meaning |
|-------|---------|
| `Waiting` | Has unmet dependencies -- will transition to `Pending` automatically when parents complete |
| `Pending` | Ready to run -- the worker loop will pick it up and call its `on_start` webhook |
| `Running` | `on_start` webhook has been called -- waiting for external completion report |
| `Success` | Completed successfully (set via API) |
| `Failure` | Failed -- either reported via API, timed out, or failed due to parent failure |
| `Paused` | Manually paused via `PATCH /task/pause/{task_id}` -- worker ignores it |
| `Canceled` | Manually canceled via `DELETE /task/{task_id}` -- treated like Failure for propagation |

## PATCH vs PUT for Updates

- **`PATCH /task/{task_id}`**: Synchronous update. Use this to set final status (`Success`/`Failure`). Triggers `on_success`/`on_failure` webhooks and dependency propagation immediately.
- **`PUT /task/{task_id}`**: Batched counter update. Use this for high-throughput incremental `new_success`/`new_failures` counter bumps. Updates are accumulated and flushed periodically (not immediately). At least one of `new_success` or `new_failures` must be non-zero.

## Webhooks

When the worker starts a task, it calls the `on_start` webhook with a `?handle=<callback_url>` query parameter. Your webhook handler can optionally return a `NewActionDto` JSON body to register a cancel action.

Each webhook request includes these headers for idempotency and diagnostics:
- `Idempotency-Key`: `"<task_id>:start"`, `"<task_id>:end:success"`, `"<task_id>:end:failure"`, or `"<task_id>:cancel"`
- `X-Task-Id`: task UUID
- `X-Task-Trigger`: `start`, `end`, or `cancel`

Webhook params are stored as JSON in the `params` field of `NewActionDto`:
```json
{"kind": "Webhook", "params": {"url": "https://my-service.com/run", "verb": "Post", "body": {"key": "value"}, "headers": {"Authorization": "Bearer xxx"}}}
```

## Concurrency Rules

Tasks can have `rules` (array of `Strategy`) that limit concurrent execution. A `Concurency` rule specifies: match currently `Running` tasks by `kind` and metadata `fields`, and block if `max_concurency` is reached. Fields are keys in the task's `metadata` JSON -- two tasks match if they share the same values for all listed fields.

## Deduplication

`dedupe_strategy` on `NewTaskDto` allows skipping task creation if a matching task already exists. Each `Matcher` specifies a `status`, `kind`, and `fields` (metadata keys) to match against.

## Minimal Example

Create two tasks where `deploy` depends on `build`:
```json
[
  {
    "id": "build",
    "name": "Build App",
    "kind": "ci",
    "timeout": 300,
    "on_start": {"kind": "Webhook", "params": {"url": "https://ci.example.com/build", "verb": "Post"}}
  },
  {
    "id": "deploy",
    "name": "Deploy App",
    "kind": "cd",
    "timeout": 600,
    "on_start": {"kind": "Webhook", "params": {"url": "https://cd.example.com/deploy", "verb": "Post"}},
    "dependencies": [{"id": "build", "requires_success": true}],
    "on_success": [{"kind": "Webhook", "params": {"url": "https://slack.example.com/notify", "verb": "Post", "body": {"text": "Deploy succeeded"}}}]
  }
]
```
The `build` task starts as `Pending` (no deps), `deploy` starts as `Waiting`. Once `build` succeeds, `deploy` transitions to `Pending` and the worker starts it.
