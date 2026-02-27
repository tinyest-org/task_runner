# ArcRun - Issues & Improvements

This document tracks known issues, missing features, and improvements for the arcrun project.

---

## WHAT'S GOOD

### Architecture & Design
- **Clean separation of concerns**: handlers, db_operations, workers, actions, dtos are well separated
- **DAG model is solid**: Link table with `requires_success` flag is elegant
- **Cascading propagation**: Recursive failure/cancel propagation works correctly
- **Async throughout**: diesel-async, actix-web, tokio - proper async stack
- **Observability foundation**: Prometheus metrics, structured logging

### Code Quality
- **Comprehensive input validation** (`src/validation.rs`) - 300+ lines of validation
- **Good DTO design**: Separate DTOs for create, update, response, listing
- **Type-safe enums**: StatusKind, TriggerKind, ActionKindEnum
- **Integration tests with testcontainers**: Real PostgreSQL, proper isolation

### Features
- **Deduplication**: Skip duplicate tasks based on metadata fields
- **Concurrency rules**: Limit concurrent tasks by kind/metadata
- **Batch updates**: High-throughput endpoint for success/failure counters
- **DAG visualization**: Built-in Cytoscape.js UI

---

## CRITICAL - Fix Before Production

### 1. ~~No Authentication/Authorization~~ ✅ N/A
- **Location**: `src/main.rs` - all handlers
- **Status**: NOT APPLICABLE - Authentication/authorization is handled at the proxy layer (e.g., API gateway, reverse proxy). This service runs behind a secured proxy that handles auth.

### 2. ~~SSRF Vulnerability - Unvalidated Webhook URLs~~ ✅ FIXED
- **Location**: `src/validation.rs`
- **Status**: FIXED - Added comprehensive URL validation that blocks:
  - Internal IP ranges (10.x, 172.16.x, 192.168.x, 127.x, 169.254.x)
  - Localhost and loopback addresses
  - Cloud metadata endpoints (169.254.169.254, metadata.google.internal, etc.)
  - Non-HTTP(S) schemes (file://, ftp://, etc.)

### 3. ~~Hardcoded Pagination Size~~ ✅ FIXED
- **Location**: `src/db_operation.rs:224`
- **Status**: FIXED - Changed from `let page_size = 50;` to `pagination.page_size.unwrap_or(50)` to respect user-provided pagination

### 4. ~~Race Condition in Dependency Propagation~~ ✅ NOT AN ISSUE
- **Location**: `src/workers.rs:376-415`
- **Status**: VERIFIED SAFE - The current implementation handles race conditions correctly:
  - Counter decrement is atomic (single UPDATE)
  - Status transition uses WHERE clause checking `wait_finished = 0 AND wait_success = 0`
  - Concurrent parent completions are safe: only the last one triggers transition
  - Duplicate transitions prevented by also checking `status = Waiting` in WHERE clause

### 5. ~~Lost Batch Updates~~ ✅ FIXED
- **Location**: `src/workers.rs:505-582`
- **Status**: FIXED - Refactored batch updater with DashMap for concurrent access:
  - Replaced `Mutex<HashMap>` with `DashMap` - per-shard locking, no global contention
  - Receiver and updater can operate concurrently on different task IDs
  - Atomically swap counts before persistence, re-add on failure
  - Added `record_batch_update_failure()` metric for monitoring
  - Added unit tests for Entry accumulation, swap/requeue, concurrent access, and cleanup

### 6. ~~Hardcoded "e" in Error Message~~ ✅ FIXED
- **Location**: `src/action.rs:79-96`
- **Status**: FIXED - Capture status before consuming response body, then use actual `status` in error message. Also improved log message to include status code.

---

## HIGH - Significant Issues

### 7. No Webhook Retry Mechanism
- **Location**: `src/action.rs:56-98`
- **Impact**: Transient network failure causes permanent task failure
- **Details**: Webhook failures are not retried. No exponential backoff.
- **Recommendation**: Implement retry with configurable limits and exponential backoff

### 8. Action Execution Status Never Persisted
- **Location**: `src/workers.rs:249-267`
- **Impact**: No audit trail of action execution; can't debug task failures
- **Details**: Actions execute but results (success/failure) never saved. `action.success` field exists but never updated.
- **Recommendation**: Persist action execution results in database

### 9. ~~Worker Loop Breaks on Single Task Failure~~ ✅ FIXED
- **Location**: `src/workers.rs:92-165`
- **Status**: FIXED - Changed `break 'outer` to `break` (inner loop) so the worker continues processing other tasks. Added metric `record_task_db_save_failure()` for monitoring.

### 10. ~~N+1 Query Pattern in Task Listing~~ ✅ FIXED
- **Location**: `src/db_operation.rs:192-217`
- **Status**: FIXED - `find_detailed_task_by_id` now uses LEFT JOIN to fetch task and actions in a single query instead of 2 separate queries.

### 11. ~~Incomplete Circular Dependency Detection~~ ✅ FIXED
- **Location**: `src/validation.rs:365-466`
- **Status**: FIXED - Implemented proper DFS cycle detection with path tracking. Now detects cycles of any length (A→B→C→A, etc.), not just direct A→B→A cycles.

### 12. ~~Connection Pool Exhaustion Handling~~ ✅ FIXED
- **Location**: `src/circuit_breaker.rs`, `src/handlers.rs`
- **Status**: FIXED - Implemented circuit breaker pattern for connection pool resilience:
  - Three states: Closed (normal), Open (fast-fail), HalfOpen (probe)
  - Configurable failure threshold, failure window, recovery timeout, success threshold
  - Fast-fails requests when circuit is open, preventing thundering herd
  - Prometheus metrics for state transitions and rejections
  - Environment variables: `CIRCUIT_BREAKER_ENABLED`, `CIRCUIT_BREAKER_FAILURE_THRESHOLD`, etc.

### 13. expect() Panics in Action Executor
- **Location**: `src/action.rs:43`
- **Impact**: Application crashes on HTTP client initialization failure
- **Details**: Multiple `.expect()` calls will panic instead of graceful error handling
- **Recommendation**: Return `Result` type and handle errors at call site

---

## MEDIUM - Should Fix

### 14. Pause Feature Doesn't Work
- **Location**: `src/workers.rs`
- **Impact**: Pause endpoint exists but has no effect
- **Details**: Worker loop doesn't check for Paused status. No resume capability.
- **Recommendation**: Implement pause/resume logic in worker loop

### 15. No Task Retry Mechanism
- **Impact**: Failed tasks must be manually recreated
- **Details**: No built-in retry support with configurable backoff
- **Recommendation**: Add `retry_count`, `max_retries`, `retry_strategy` to task model

### 16. Timeout Doesn't Cancel External Work
- **Location**: `src/db_operation.rs:87-114`
- **Impact**: Wasted resources; external tasks keep running after timeout
- **Details**: Task marked failed but nothing tells executor to stop
- **Recommendation**: Trigger cancel actions on timeout

### 17. ~~No Rate Limiting~~ ✅ N/A
- **Location**: `src/main.rs`, `src/action.rs`
- **Status**: NOT APPLICABLE - Rate limiting is handled at the proxy layer (e.g., API gateway, reverse proxy). This service runs behind a secured proxy that handles rate limiting.

### 18. Generic DbError Type
- **Location**: `src/db_operation.rs:16`
- **Impact**: Difficult to handle specific error types
- **Details**: `Box<dyn Error>` loses error type information
- **Recommendation**: Create strongly-typed error enums (use existing `error.rs`)

### 19. unreachable!() Panic on Metadata
- **Location**: `src/workers.rs:200`
- **Impact**: Application panics on incomplete user input
- **Details**: `unreachable!("None should't be there")` - panics if metadata field missing
- **Recommendation**: Return proper validation error instead of panicking

### 20. ~~Cancel Actions Never Executed~~ ✅ VERIFIED WORKING
- **Location**: `src/workers.rs:446-466`
- **Status**: VERIFIED WORKING - Cancel actions ARE executed for `Running` tasks in `cancel_task()`. The code loads `TriggerKind::Cancel` actions and executes them via the webhook executor. Tasks in `Pending`/`Paused` states don't need cancel actions (nothing to stop).

### 21. Integer Overflow in Counters
- **Location**: `src/db_operation.rs:159-160`
- **Impact**: Data corruption for very high-volume tasks
- **Details**: Using `i32` for success/failure counters without overflow protection
- **Recommendation**: Use `i64` or implement overflow guards

### 22. ~~Silently Dropped Validation Errors~~ ✅ FIXED
- **Location**: `src/validation.rs`
- **Status**: FIXED - The old `validate_params`/`validate_form` functions (returning `bool`) were dead code. The new validation system in `src/validation.rs` already returns detailed `ValidationError` with field names and messages. Removed the dead code from `db_operation.rs`.

---

## TESTING GAPS

### 23. No Worker Loop Integration Tests
- **Impact**: Core business logic untested in E2E scenarios
- **Details**: Tests exist for API endpoints but not for worker loop (task starting, propagation, timeouts)
- **Recommendation**: Add tests simulating worker loop execution with mocked time

### 24. No Concurrency Rule Enforcement Tests
- **Location**: `src/workers.rs:167-236`
- **Impact**: Feature could be completely broken
- **Details**: Only storage is tested, not runtime enforcement
- **Recommendation**: Add tests for concurrency rule enforcement

### 25. No Webhook Execution Tests
- **Location**: `src/action.rs`
- **Impact**: Webhook logic untested
- **Details**: Uses real HTTP client; no mocking
- **Recommendation**: Implement tests with mocked HTTP client

### 26. No Error Path Tests
- **Impact**: Error handling quality unknown
- **Details**: Most tests check happy paths only
- **Recommendation**: Add comprehensive error scenario tests

### 27. No Stress/Load Tests
- **Impact**: System behavior under load unknown
- **Details**: No tests for high load (1000+ tasks, wide DAGs, concurrent webhooks)
- **Recommendation**: Add load/stress tests

---

## OBSERVABILITY GAPS

### 28. ~~No Distributed Tracing~~ ✅ FIXED
- **Location**: `src/tracing.rs`, `src/main.rs`
- **Status**: FIXED - Implemented OpenTelemetry distributed tracing:
  - New `src/tracing.rs` module with W3C Trace Context support
  - OTLP exporter for trace export to collectors (Jaeger, Zipkin, etc.)
  - Configurable sampling ratio and service name
  - Environment variables: `TRACING_ENABLED`, `OTEL_EXPORTER_OTLP_ENDPOINT`, `OTEL_SERVICE_NAME`, `OTEL_SAMPLING_RATIO`
  - Helper functions for trace context extraction/injection in HTTP headers

### 29. ~~Insufficient Metric Labels~~ ✅ FIXED
- **Location**: `src/db_operation.rs:113-150`
- **Status**: FIXED - Task kind is now fetched before recording completion metrics:
  - Added query to fetch task kind before update
  - `record_task_completed(outcome, task_kind)` now uses actual task kind instead of "unknown"
  - Enables proper drill-down by task kind in metrics dashboards

### 30. ~~No Slow Query Warnings~~ ✅ FIXED
- **Location**: `src/metrics.rs`, `src/config.rs`
- **Status**: FIXED - Added slow query warning system:
  - New `record_db_query_with_slow_warning()` function logs warnings when queries exceed threshold
  - New `slow_queries_total` Prometheus counter metric by query name
  - Configurable threshold via `SLOW_QUERY_THRESHOLD_MS` environment variable (default: 100ms)
  - Added `ObservabilityConfig` to config for centralized observability settings

---

## MISSING FEATURES

### 31. No Task Priority
- **Impact**: All tasks have equal priority
- **Recommendation**: Add priority field; process higher priority first

### 32. No Task Scheduling (Delayed Start)
- **Impact**: Can't schedule tasks for future execution
- **Recommendation**: Add `start_after` timestamp field

### 33. No Task Grouping/Namespaces
- **Impact**: No multi-tenancy support
- **Recommendation**: Add namespace/tenant field for isolation

### 34. No Dynamic Webhook Headers from Task
- **Impact**: Can't pass task-specific headers to webhooks
- **Recommendation**: Allow header templates with task metadata interpolation

### 35. No Critical Path Analysis
- **Impact**: Can't identify bottleneck tasks in DAG
- **Recommendation**: Add endpoint for critical path calculation

---

## CONFIGURATION ISSUES

### 36. No Database URL Validation at Startup
- **Location**: `src/config.rs:150-158`
- **Impact**: Invalid URLs fail at runtime, not startup
- **Details**: Only checks if env var exists, not if it's valid
- **Recommendation**: Try test connection at startup

### 37. No Graceful Shutdown Handler
- **Location**: `src/main.rs:553-556`
- **Impact**: May lose in-flight requests on shutdown
- **Details**: Shutdown timeout hardcoded, no SIGTERM/SIGINT handling
- **Recommendation**: Implement proper signal handling and drain logic

---

## SUMMARY

| Category | Critical | High | Medium | Total |
|----------|----------|------|--------|-------|
| Security | ~~2~~ 0 | 1 | ~~1~~ 0 | ~~4~~ 1 |
| Bugs | ~~3~~ 0 | ~~4~~ 0 | ~~4~~ 3 | ~~11~~ 3 |
| Error Handling | ~~1~~ 0 | 2 | ~~2~~ 1 | ~~5~~ 3 |
| Performance | - | ~~2~~ 1 | - | ~~2~~ 1 |
| Testing | - | 4 | 1 | 5 |
| Observability | - | - | ~~3~~ 0 | ~~3~~ 0 |
| Missing Features | - | - | 5 | 5 |
| Configuration | - | - | 2 | 2 |
| **Total** | ~~6~~ **0** | ~~13~~ **8** | ~~18~~ **12** | ~~37~~ **20** |

**Fixed/Resolved Issues:**
- ✅ #1 - Auth/Authorization (N/A - handled at proxy layer)
- ✅ #2 - SSRF vulnerability (Critical Security)
- ✅ #3 - Pagination hardcoding (Critical Bug)
- ✅ #4 - Race condition verified safe (was Critical Bug, not actually an issue)
- ✅ #5 - Lost batch updates (Critical Bug)
- ✅ #6 - Hardcoded "e" error (Critical Error Handling)
- ✅ #9 - Worker loop resilience (High Bug)
- ✅ #10 - N+1 query pattern (High Performance)
- ✅ #11 - Circular dependency detection (High Bug)
- ✅ #12 - Connection pool exhaustion (High Bug) - Circuit breaker implemented
- ✅ #17 - Rate limiting (N/A - handled at proxy layer)
- ✅ #20 - Cancel actions (Verified working - executes for Running tasks)
- ✅ #22 - Silently dropped validation errors (Removed dead code, new system has detailed errors)
- ✅ #28 - No distributed tracing (Observability) - OpenTelemetry tracing with OTLP export
- ✅ #29 - Insufficient metric labels (Observability) - Task kind now included in completion metrics
- ✅ #30 - No slow query warnings (Observability) - Configurable threshold with logging and metrics

---

## PRIORITY ORDER

### Immediate (before production):
- [x] #1 - Authentication/Authorization ✅ (handled at proxy layer)
- [x] #2 - SSRF vulnerability fix ✅
- [x] #3 - Pagination hardcoding ✅
- [x] #4 - Race condition in propagation ✅ (verified safe)
- [x] #5 - Lost batch updates ✅
- [x] #6 - Hardcoded "e" error ✅

### High Priority (before v1.0):
- [ ] #7 - Webhook retry mechanism
- [ ] #8 - Action status persistence
- [x] #9 - Worker loop resilience ✅
- [x] #10 - N+1 query fix ✅
- [x] #11 - Circular dependency detection ✅
- [ ] #23 - Worker loop tests
- [ ] #24 - Concurrency rule tests

### Medium Priority:
- [ ] #14 - Pause/resume
- [ ] #15 - Task retry mechanism
- [x] #28 - Distributed tracing ✅
- [x] #29 - Insufficient metric labels ✅
- [x] #30 - Slow query warnings ✅
- [ ] Remaining items
