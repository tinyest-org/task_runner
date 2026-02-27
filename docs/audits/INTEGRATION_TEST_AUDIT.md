# Integration Test Coverage Audit

**Date**: 2026-02-26

## Summary

The integration tests cover well the core business logic (CRUD, DAG propagation, batch updates, concurrency rules, deduplication, idempotency, timeout loop). The main gaps are around batch-level endpoints, end-to-end webhook flows, stale task requeue, and the retention cleanup loop.

---

## What IS Covered

### HTTP Endpoints
- **GET /health, GET /ready** — happy path (test_health.rs)
- **POST /task** — single, multiple, with/without deps, expected_count, dedup, validation errors (test_crud.rs, test_dag.rs, test_dedupe.rs, test_bug_audit1.rs, test_bug_audit2.rs)
- **GET /task/{id}** — existing task, 404 (test_crud.rs)
- **GET /task** (list) — all, filter by kind/status, pagination, empty/overflow pages, null metadata, timeout filter (test_filtering.rs, test_bug_audit2.rs)
- **PATCH /task/{id}** — Success/Failure transitions, failure_reason required, invalid transitions, negative counters, double-update 404, non-running 404, expected_count update (test_status.rs, test_crud.rs, test_bug_audit1.rs, test_bug_audit2.rs)
- **PUT /task/{id}** (batch counter) — increments, accumulation, zero counter rejection, persistence on terminal tasks, ignores status validation (test_batch_update.rs, test_bug_audit2.rs)
- **DELETE /task/{id}** — cancel Pending, cancel with cascade to children (test_status.rs)
- **PATCH /task/pause/{id}** — basic pause on Pending (test_status.rs)
- **GET /dag/{batch_id}** — correct tasks and links (test_dag.rs)
- **PATCH /batch/{batch_id}/rules** — update by kind, skip terminal, empty rules removes, 404, validation errors (test_batch_rules.rs)

### Worker Loops
- **start_loop** — on_start webhook failure marks task as Failed (test_regressions.rs), idempotency headers sent (test_idempotency.rs), requeue skips already-executed on_start (test_idempotency.rs)
- **timeout_loop** — fires on_failure webhook, respects last_updated not started_at, propagates failure to children (test_regressions.rs)

### Propagation
- Parent success unblocks child (test_propagation.rs)
- Parent failure cascades to requires_success children (test_propagation.rs)
- Optional parent failure does not cascade (test_propagation.rs)
- Multi-level DAG propagation (test_propagation.rs)
- Failure propagation through chain (recursive) (test_propagation.rs)
- CI/CD pipeline failure cascade (test_propagation.rs)
- Cancel propagation to children (test_status.rs)

### Concurrency Rules
- Storage on task (test_concurrency.rs)
- Per-project with different metadata (test_concurrency.rs)
- Enforcement via `claim_task_with_rules`: blocks at limit, allows after completion, different metadata not blocked (test_concurrency.rs)
- Capacity rules: storage, under/at-limit, large candidate, progress tracking, expected_count required, Running required, zero/null expected_count, metadata fields, combined concurrency+capacity (test_concurrency.rs)

### Batch Updater
- Counter increments via PUT (test_batch_update.rs)
- Accumulation of multiple rapid updates (test_batch_update.rs)
- Rejection of zero counters (test_batch_update.rs)
- Persistence on terminal tasks (test_batch_update.rs)

### Dead-end Cancellation
- Basic dead-end detection, multi-level cascade upward, partial children prevents cancellation, paused tasks eligible, leaf tasks unaffected, barrier stops upward propagation (test_dead_end_cancel.rs)

### Deduplication
- Skip existing match, None metadata not over-aggressive, two tasks with no metadata not deduped (test_dedupe.rs)

### Idempotency
- Start/end webhook idempotency, failed webhook allows retry, different triggers independent, pending blocks until stale, headers sent, requeued Claimed task does not re-fire on_start (test_idempotency.rs)

### Validation (via integration tests)
- Duplicate local IDs rejected, unknown dependency references rejected, forward dependency references rejected, invalid status transitions, failure without reason, negative counters (test_bug_audit1.rs, test_bug_audit2.rs)

---

## What is NOT Covered

### Priority 1 — Critical

#### 1. DELETE /batch/{batch_id} (stop_batch)
Production-critical operation for killing runaway batches. ~~Zero tests exist.~~
- [x] Waiting, Pending, Claimed, Running, and Paused tasks all correctly canceled (test_stop_batch.rs)
- [x] Cancel webhooks fire for formerly-Running tasks (test_stop_batch.rs)
- [x] `already_terminal` count in the response is correct (test_stop_batch.rs)
- [x] 404 for non-existent batch_id (test_stop_batch.rs)

#### 2. Cancel webhook flow end-to-end
The on_start webhook can return a `NewActionDto` to register a cancel action. Then if the task is canceled, the cancel webhook should fire. ~~This entire lifecycle is untested.~~
- [x] on_start returns cancel action -> action is saved (test_cancel_webhook.rs)
- [x] Task is canceled -> cancel webhook fires with correct params (test_cancel_webhook.rs)
- [ ] Cancel action URL is validated (SSRF) — covered by unit tests in `src/validation/ssrf.rs` (`test_ssrf_cloud_metadata_blocked`); cannot integration-test because `SKIP_SSRF_VALIDATION` defaults to `true` in debug builds via `OnceLock` and `is_internal_ip()` blocks `127.0.0.0/8`

#### 3. Requeue of stale Claimed tasks (timeout loop)
`requeue_stale_claimed_tasks` is called in the timeout loop but never tested. Simulates worker crash leaving tasks in Claimed state.
- [x] Claimed task older than threshold is requeued to Pending (test_requeue_stale.rs)
- [x] Recently claimed task is NOT requeued (test_requeue_stale.rs)
- [x] Requeued task can be picked up again by start_loop (test_requeue_stale.rs)

#### 4. DELETE /task/{id} on a Running task
Canceling a Running task should trigger cancel webhooks. Only Pending/Waiting cancel is tested.
- [x] Running task is canceled (test_cancel_webhook.rs)
- [x] Cancel webhook fires (test_cancel_webhook.rs)
- [x] Children are propagated (failure cascade) (test_cancel_webhook.rs)

### Priority 2 — Important

#### 5. GET /batch/{batch_id} (batch stats)
Monitoring endpoint returning aggregated counters. ~~Zero tests.~~
- [x] Correct aggregation of success/failure counters across tasks (test_batch_stats.rs)
- [x] Per-status counts correctness (test_batch_stats.rs)
- [x] 404 for non-existent batch_id (test_batch_stats.rs)

#### 6. GET /batches (batch listing)
Batch listing with aggregated statistics, filtering, and pagination. ~~Zero tests.~~
- [x] Lists batches with correct stats (test_batch_stats.rs)
- [ ] Filtering works
- [x] Pagination works (test_batch_stats.rs)

#### 7. on_success webhook actually fires
~~No integration test verifies on_success webhooks are called with a real webhook server when a task completes successfully.~~
- [x] Task with on_success action completes -> webhook called (test_webhook_flows.rs)
- [x] Webhook receives correct ?handle= query param and diagnostic headers (test_webhook_flows.rs)

#### 8. on_failure webhook on explicit failure (via PATCH)
Currently only tested via timeout path.
- [x] Task explicitly marked as Failure via PATCH -> on_failure webhook fires (test_webhook_flows.rs)

#### 9. start_loop with webhook returning cancel action
The main happy-path of the worker loop is not tested E2E.
- [x] Task goes Pending -> Claimed -> Running via start_loop (test_webhook_flows.rs)
- [x] on_start webhook is called (test_webhook_flows.rs)
- [x] Cancel action from response is saved (test_cancel_webhook.rs)

#### 10. PATCH /task/{id} metadata-only update (no status change)
The no-status-change path takes a different code path (no transaction).
- [x] Update only metadata without status -> 200 (test_webhook_flows.rs)
- [x] Metadata is persisted correctly (test_webhook_flows.rs)
- [x] last_updated is refreshed (test_webhook_flows.rs)

### Priority 3 — Moderate

#### 11. Circuit breaker integration with HTTP handlers
Unit tests exist but no integration test verifies the 503 response when circuit breaker is Open.
- [ ] `get_conn_with_retry` returns 503 when Open
- [ ] Connection retry logic (multiple attempts)
- [ ] Circuit breaker records successes/failures

#### 13. POST /task with empty array
- [x] Empty `[]` body -> appropriate response (204 No Content) (test_validation_e2e.rs)

#### 14. DELETE /task/{id} on terminal task
- [x] Cancel a Success/Failure/Canceled task -> 400 (test_validation_e2e.rs)

#### 15. PATCH /task/pause on various states
Only Pending is tested.
- [ ] Pause a Running task
- [x] Pause a Waiting task (test_validation_e2e.rs)
- [x] Pause an already-paused task -> 400 (test_validation_e2e.rs)
- [x] Pause a terminal task -> 400 (test_validation_e2e.rs)

#### 16. Health check degraded mode
- [ ] GET /health when DB is unreachable -> 503
- [ ] GET /ready when pool is exhausted -> 503

#### 17. Diamond DAG with mixed success/failure
A diamond DAG is created in tests but never completed with mixed results.
- [x] One path succeeds, one fails -> verify final task status based on requires_success (test_validation_e2e.rs)

#### 18. Retention cleanup loop
Completely untested. `retention_cleanup_loop` in `src/workers/retention.rs`.
- [ ] Tasks older than retention_days are deleted
- [ ] FK constraints handled (actions, links, webhook_executions deleted first)
- [ ] batch_size limiting works
- [ ] Disabled config returns immediately

### Priority 4 — Low

#### 19. Malformed JSON in requests
- [x] POST /task with invalid JSON -> 400 (test_validation_e2e.rs)
- [x] PATCH /task/{id} with invalid JSON -> 400 (test_validation_e2e.rs)
- [x] PUT /task/{id} with invalid JSON -> 400 (test_validation_e2e.rs)

#### 20. Invalid UUID in path parameters
- [ ] GET /task/not-a-uuid -> 400

#### 21. Webhook with custom headers/body
- [x] Custom headers are forwarded to webhook endpoint (test_webhook_flows.rs)
- [x] Custom body is sent to webhook endpoint (test_webhook_flows.rs)

#### 22. Webhook redirect rejection
- [x] Webhook target returns 3xx -> treated as failure (SSRF protection) (test_webhook_flows.rs)

#### 23. Webhook with non-2xx response
- [x] Webhook server returns 500 -> on_start failure path triggered (test_webhook_flows.rs)

#### 25. Transaction rollback on partial batch failure
- [ ] If one task in a batch fails to insert, entire transaction rolls back

#### 27. Propagation: Canceled parent with requires_success=false child
- [x] Parent Canceled, child has requires_success=false -> child transitions to Pending (test_propagation_edge.rs)

#### 28. Deduplication against non-Pending tasks
- [x] Dedup matches against Running tasks when matcher status=Running (test_dedupe.rs)
- [x] Dedup with status=Pending does NOT match Running tasks (test_dedupe.rs)

#### 29. PATCH /batch/{batch_id}/rules with Capacity rule but no expected_count
- [x] Tasks without expected_count are blocked by capacity rule; tasks with expected_count are claimable (test_batch_rules.rs)
