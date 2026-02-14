# Remediation Plan 2 (Detailed)

Scope: Fix all issues identified in the static audit (bugs and risks) with code changes and test coverage. This plan assumes the desired behavior is:
- DAG dependencies are **strict**: unknown IDs, duplicate local IDs, or out-of-order dependencies are **errors**.
- An `on_start` failure behaves like a full task failure: propagate to children and fire `on_failure` actions.

## Summary of Issues to Fix
1. Strict DAG validation missing (unknown IDs, duplicate local IDs, forward references).
2. `on_start` webhook failure does not propagate or fire `on_failure` actions.
3. SSRF: cancel actions from webhook are not validated; redirects/DNS rebinding risks.
4. Concurrency prefilter cache too broad (metadata-insensitive).
5. Listing excludes tasks with `metadata = NULL`.
6. `POOL_ACQUIRE_RETRIES=0` panics.
7. `timeout` filter is ignored in list endpoint.
8. `PUT /task/{id}` rejects otherwise valid counter updates due to `status` validation.

## Remediation Plan by Issue

### 1) Strict DAG Validation (Unknown IDs, Duplicate IDs, Forward References)
**Files**
- `src/validation.rs`
- `src/handlers.rs`

**Current Behavior**
- Unknown dependency IDs are silently ignored.
- Duplicate local IDs are allowed, later ones overwrite earlier ones in the mapping.
- Dependencies on tasks defined later are allowed (but ignored by mapping).

**Target Behavior**
- Validation errors if:
  - A dependency references a local ID that does not exist in the batch.
  - A task ID is duplicated within the batch.
  - A dependency references a task defined later in the array (forward reference).

**Fix Approach**
- Extend `validate_task_batch()`:
  - Track local IDs while iterating through tasks in order.
  - Error if a task ID already exists.
  - For each dependency, verify that the local ID has already been seen.
  - If dependency ID exists but appears later, error “dependency must appear earlier in the batch”.
  - If dependency ID not in the batch at all, error “unknown dependency”.
- Ensure these validation errors are returned by `POST /task` (already wired).

**Tests**
- New unit tests in `src/validation.rs`:
  - Duplicate IDs in same batch -> validation error.
  - Dependency to unknown ID -> validation error.
  - Dependency to later task -> validation error.
  - Dependency to earlier task -> OK.

**Notes**
- This makes the behavior deterministic and safer for DAG semantics.

---

### 2) `on_start` Failure Handling Must Propagate and Fire `on_failure`
**Files**
- `src/workers.rs`
- `src/db_operation.rs`

**Current Behavior**
- If `on_start` fails, task is marked `Failure`, but:
  - No propagation to children.
  - No `on_failure` webhooks fired.

**Target Behavior**
- Treat `on_start` failure as a full task failure:
  - Update task to `Failure`.
  - Fire `on_failure` actions (best effort).
  - Propagate failure to children.

**Fix Approach**
- Add a new helper in `db_operation.rs`, e.g. `fail_task_and_propagate(evaluator, conn, task_id, reason)`:
  - Update task status to `Failure` + `failure_reason`, set `ended_at`, `last_updated`.
  - Run `propagate_to_children` inside the same transaction.
  - After commit, call `fire_end_webhooks` with `Failure`.
- In `start_loop`, on `on_start` failure, call this helper instead of `mark_task_failed`.
- Ensure repeated failure calls are idempotent (only update if `status == Running`).

**Tests**
- Integration-style tests for:
  - `on_start` failure triggers child state transitions (child requiring success fails).
  - `on_failure` webhooks are fired on `on_start` failure.

---

### 3) SSRF Hardening for Cancel Actions + Redirect/DNS Rebinding
**Files**
- `src/validation.rs`
- `src/db_operation.rs`
- `src/workers.rs`
- `src/action.rs`
- `src/config.rs`

**Current Behavior**
- Cancel actions returned by `on_start` are persisted without validation.
- SSRF checks only look at hostname string and IP literals.
- Redirects are allowed by default (reqwest follows them), enabling bypass.

**Target Behavior**
- Validate **all** webhook URLs, including cancel actions.
- Reject redirect chains to internal targets (or disable redirects).
- Optionally resolve host to IP at execution time to prevent DNS rebinding.

**Fix Approach**
- Validate cancel actions before inserting:
  - Add `validate_new_action()` that parses `WebhookParams` and runs SSRF checks.
  - Apply in `save_cancel_actions`.
- Enforce safe redirect handling:
  - Configure `reqwest::Client` with `redirect::Policy::none()` or a custom policy that re-validates each redirect target.
- Add optional DNS resolution check (configurable):
  - Resolve hostname to IPs and verify none are internal ranges.
  - This can be a configurable flag to avoid DNS penalties if undesired.
- Add blocked hostname/IP handling for IPv6-mapped IPv4 and other reserved ranges (if not already).

**Tests**
- Unit tests for URL validation:
  - Cancel action to `localhost` rejected.
  - Redirect to internal URL rejected (if redirect handling is enabled).
- Integration tests for `save_cancel_actions`:
  - Invalid URL results in error and no inserted cancel actions.

---

### 4) Concurrency Prefilter Cache Is Too Broad (Metadata-Insensitive)
**Files**
- `src/workers.rs`
- `src/db_operation.rs`

**Current Behavior**
- The prefilter caches `Strategy` objects, so a rule blocked for one metadata value blocks all tasks sharing the same rule, even with different metadata.

**Target Behavior**
- Cache should be scoped by rule + metadata values (fields + values).

**Fix Approach**
- Replace `HashSet<Strategy>` with a set of computed keys:
  - Use the same `concurrency_lock_key()` logic (or create a new helper) to derive a unique key for rule + metadata.
- During prefilter check, compute keys for current task and check cache.
- When a rule blocks, insert the computed keys (not the raw rule).

**Tests**
- Unit test for prefilter:
  - Two tasks with same rule but different metadata; block one should not block the other.

---

### 5) `GET /task` Excludes Tasks With `metadata = NULL`
**Files**
- `src/db_operation.rs`

**Current Behavior**
- `metadata.contains({})` is always applied. For `NULL`, this is false; tasks without metadata are hidden.

**Target Behavior**
- When no metadata filter is provided, include tasks regardless of `metadata` being `NULL`.

**Fix Approach**
- Conditional filter:
  - If `filter.metadata` is `None`, skip metadata filter entirely.
  - If provided, apply `metadata.contains(...)`.
- Ensure DB column default is `NULL` vs `jsonb 'null'` is handled (match actual schema).

**Tests**
- List tasks with `metadata = NULL` and ensure they appear when no metadata filter is provided.

---

### 6) Panic When `POOL_ACQUIRE_RETRIES=0`
**Files**
- `src/handlers.rs`

**Current Behavior**
- `0..max_retries` runs zero times and `last_error.unwrap()` panics.

**Target Behavior**
- Graceful failure without panic.

**Fix Approach**
- Handle `max_retries == 0` explicitly:
  - Attempt exactly once or return a clear error before the loop.
- Ensure `last_error` exists before unwrap (or replace with default error).

**Tests**
- Unit test for `get_conn_with_retry` with `max_retries=0` -> returns `503` and no panic.

---

### 7) `timeout` Filter Ignored
**Files**
- `src/db_operation.rs`

**Current Behavior**
- `FilterDto.timeout` is unused in query filters.

**Target Behavior**
- Apply exact match on `timeout` when filter is provided.

**Fix Approach**
- Extend query builder: add `.and(timeout.eq(dto_timeout))` where `dto_timeout` exists.

**Tests**
- List with `timeout` filter only returns matching tasks.

---

### 8) `PUT /task/{id}` Over-Validates `status`
**Files**
- `src/handlers.rs`
- `src/validation.rs`

**Current Behavior**
- `PUT` uses `validate_update_task` which requires `status` and `failure_reason` consistency, even though `PUT` ignores `status`.

**Target Behavior**
- `PUT` only validates non-negative counters and non-zero update.
- `PATCH` keeps strict status validation.

**Fix Approach**
- Split validation into:
  - `validate_update_task_patch()` for `PATCH` (status + failure_reason rules).
  - `validate_update_task_put()` for `PUT` (only counter checks).
- Update handlers to use the correct validator.

**Tests**
- `PUT` with `status=Failure` but no `failure_reason` should succeed (status ignored).
- `PATCH` still rejects invalid status or missing `failure_reason`.

---

## Cross-Cutting Improvements (Optional but Recommended)
- Add structured error types for validation failures (easier to test).
- Add logging context for batch validation errors.
- Document strict DAG validation behavior in `readme.md`.

## Execution Order
1. Strict DAG validation (Issue 1) — prevents data inconsistencies early.
2. on_start failure propagation (Issue 2) — correctness of DAG state.
3. SSRF hardening (Issue 3) — security risk mitigation.
4. Concurrency prefilter fix (Issue 4) — correctness/perf.
5. Listing metadata fix (Issue 5) — API correctness.
6. Retry panic fix (Issue 6) — stability.
7. Timeout filter (Issue 7) — API correctness.
8. PUT validation split (Issue 8) — API semantics.

## Deliverables
- Code changes across the files listed above.
- Unit tests for validation, listing, and prefilter logic.
- Integration tests for `on_start` failure propagation and SSRF validation.
- Updated documentation for strict DAG validation and PUT/PATCH semantics.

