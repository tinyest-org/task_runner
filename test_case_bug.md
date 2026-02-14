# Test Cases That Should Trigger Identified Bugs

## Scope
Test case proposals intended to reproduce the issues listed in `AUDIT_1_CODEX.md`. These are **not implemented yet**; they describe scenarios and expected failure modes.

## High Severity
1. **Duplicate webhook execution across multiple workers (multi-instance race).**
   - Setup: Create a Pending task with an `on_start` webhook that increments an external counter.
   - Action: Run two instances of the worker loop concurrently against the same database (or simulate by running two async loops in tests) and let both fetch Pending tasks.
   - Expected bug: The webhook is executed twice for the same task because actions are executed before a single instance claims the task.
   - Observed signals: External counter increments twice; task status may still end up `Running` or `Success` once, hiding the double action.

2. **Repeated end-task propagation causing negative counters.**
   - Setup: A parent task with one child that has `wait_finished=1` and `wait_success=1` (requires success). Ensure child is `Waiting`.
   - Action: Call `PATCH /task/{parent_id}` twice with `status=Success` (or call `update_running_task` twice).
   - Expected bug: `wait_finished` and/or `wait_success` decremented twice; counters can become negative or child transitions to `Pending` prematurely.
   - Observed signals: Child can move to `Pending` before all parents finish; counters < 0 when re-read from DB.

3. **End-task actions executed multiple times.**
   - Setup: Task with an `on_success` webhook that appends to a list or creates a side-effect.
   - Action: Call update endpoint twice to set status `Success`.
   - Expected bug: Webhook executes twice for the same task completion.
   - Observed signals: Duplicate side-effects, duplicated events, webhook called twice.

4. **Update endpoint allows invalid transitions and returns misleading 404.**
   - Setup: Create a task in DB.
   - Action: `PATCH /task/{id}` with invalid status (e.g. `Running` or `Pending`) or negative counters, or failure_reason with status Success.
   - Expected bug: Request succeeds or fails with `404 Not Found` instead of `400 Bad Request` and correct validation errors.
   - Observed signals: Task updated incorrectly or handler returns 404 despite task existing.

## Medium Severity
5. **Crash when `POOL_ACQUIRE_RETRIES=0`.**
   - Setup: Start server with env `POOL_ACQUIRE_RETRIES=0` and DB not reachable.
   - Action: Call any endpoint that uses `get_conn_with_retry`.
   - Expected bug: Panic due to `last_error.unwrap()`; server crashes.
   - Observed signals: Process panic, service unavailable.

6. **Panic on missing metadata fields in concurrency rules.**
   - Setup: Create task with a concurrency rule matcher requiring fields not present in metadata.
   - Action: Run start loop (evaluate rules).
   - Expected bug: `unreachable!` panic, worker crashes.
   - Observed signals: Process crash, panic log.

7. **Over-aggressive dedupe when metadata is `None`.**
   - Setup: Two distinct tasks with same `kind/status`, and dedupe_strategy referencing metadata fields, but metadata is `None`.
   - Action: Insert both tasks in batch.
   - Expected bug: Second task is dropped because `metadata.contains({})` matches all.
   - Observed signals: Only one task created; dedupe eliminates unrelated task.

8. **Config values ignored for pool / worker intervals.**
   - Setup: Configure `POOL_MAX_SIZE=1` and `WORKER_LOOP_INTERVAL_MS=5000` and `BATCH_CHANNEL_CAPACITY=1`.
   - Action: Start server and observe pool size and worker/batch loop intervals.
   - Expected bug: Hardcoded defaults still used (pool size 10, intervals 1s/100ms).
   - Observed signals: More than one connection used; logs show loop every 1s; batching loop runs every 100ms.

## Notes
- For concurrency-related tests, prefer running against a real Postgres instance (e.g. testcontainers) with two worker loops concurrently.
- For webhook side-effects, use a local HTTP mock server (or a test handler) that increments a counter for verification.
- For negative counter checks, read `wait_finished` and `wait_success` directly from DB after each update.
