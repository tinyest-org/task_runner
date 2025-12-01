# Task Runner - Production Readiness TODO

This file tracks improvements needed for a production-ready codebase.

## Completed

- [x] **8. Database Indexes** - Add indexes on frequently queried columns
  - `migrations/2025-12-01-000001_add_indexes/up.sql`
  - Indexes on task.status, task.kind, task.created_at
  - Composite index on task(status, kind)
  - Indexes on action.task_id, action.trigger, link.child_id

- [x] **9. Input Validation** - Add comprehensive input validation
  - `src/validation.rs`
  - Validates task names, kinds, timeouts, metadata size
  - Validates action parameters (webhook URLs)
  - Validates dependencies (circular, self-reference, duplicates)
  - Validates concurrency rules
  - Batch validation with detailed error messages

- [x] **13. Prometheus Metrics** - Add custom metrics for observability
  - `src/metrics.rs`
  - Task counters (created, completed, cancelled, timed out)
  - Status transition tracking
  - Dependency metrics (propagations, unblocked, failed)
  - Worker loop metrics (iterations, duration)
  - Webhook execution metrics

- [x] **14. Error Types** - Create proper error types with thiserror
  - `src/error.rs`
  - TaskRunnerError, ValidationError, ApiError enums
  - HTTP response mapping
  - Automatic error conversion

- [x] **15. Documentation** - Add API documentation and update README
  - `readme.md` with full API docs
  - Task lifecycle diagram
  - Dependency and concurrency examples

- [x] **3. Proper Logging** - Replace println with structured logging
  - `src/workers.rs` - replaced println with log::debug
  - All logging now uses appropriate log levels

- [x] **4. Connection Pool Error Handling** - Handle pool exhaustion gracefully
  - `src/main.rs` - `get_conn_with_retry()` function
  - Configurable retry count and delay
  - Metrics for pool exhaustion events
  - Returns 503 Service Unavailable on failure

- [x] **6. Graceful Shutdown** - Handle SIGTERM/SIGINT properly
  - `src/main.rs` - `.shutdown_timeout(30)` on HttpServer
  - Actix handles graceful shutdown automatically
  - Waits up to 30 seconds for in-flight requests

- [x] **7. Health Check Endpoint** - Add /health and /ready endpoints
  - `GET /health` - checks database connectivity, returns pool status
  - `GET /ready` - stricter check for load balancer readiness
  - Returns 503 when degraded

- [x] **12. Pagination Limits** - Enforce maximum page size
  - `src/main.rs` - `enforce_pagination_limits()` function
  - Configurable default (50) and max (100) via env vars
  - Validates page is non-negative
  - Caps page_size to configured maximum

- [x] **16. Configuration Management** - Use typed configuration
  - `src/config.rs` - Config struct with all settings
  - Loads from environment variables with defaults
  - Validates configuration at startup
  - Supports: database, pool, pagination, worker settings

---

## Pending

### High Priority - Code Quality

- [x] **1. Transaction Safety** - Ensure rollback on all error paths
  - Added explicit ROLLBACK on insert_new_task failure
  - Added explicit ROLLBACK on COMMIT failure
  - All error paths now properly rollback before returning connection to pool

- [ ] **2. Async Transaction Support** - Use diesel-async transaction support (optional)
  - Could migrate from raw SQL to `AsyncConnection::transaction` for cleaner code
  - Current implementation is correct but verbose

### Medium Priority - Robustness

- [ ] **5. Webhook Retry Logic** - Add exponential backoff for failed webhooks
  - Implement retry with backoff in ActionExecutor
  - Track retry count per action
  - Add dead-letter handling for permanently failed webhooks

- [ ] **10. Rate Limiting** - Add rate limiting to API endpoints
  - Use actix-ratelimit or similar
  - Configure per-IP or per-requester limits
  - Return 429 Too Many Requests appropriately

- [x] **11. Request Tracing (Batch ID)** - Add batch_id for DAG tracing
  - `migrations/2025-12-01-000002_add_batch_id/` - Added batch_id column
  - Generate UUID batch_id for each task creation request
  - All tasks in same request share batch_id
  - Included in all log statements: `[batch_id=xxx]`
  - Returned in `X-Batch-ID` response header
  - Filter tasks by `?batch_id=xxx` in list endpoint

### Lower Priority - Operations

- [x] **17. Database Connection Retry** - ~~Retry on startup if DB unavailable~~ NOT NEEDED
  - Using Docker with auto-restart policy instead
  - Fail-fast behavior preferred: `initialize_db_pool()` uses `.expect()` to panic on failure
  - Server exits immediately if DB is unavailable, Docker handles restart

- [ ] **18. Worker Backpressure** - Handle overload scenarios
  - Limit in-flight webhook calls
  - Add backpressure to batch updater channel
  - Shed load gracefully when overwhelmed

- [ ] **19. Audit Logging** - Track who did what
  - Log task creation with requester
  - Log status changes with timestamp
  - Consider separate audit table

- [ ] **20. Soft Delete** - Keep task history
  - Add deleted_at column
  - Filter out deleted tasks in queries
  - Add purge job for old deleted tasks

---

## Feature Enhancements

- [ ] **F1. Basic Front-end UI** - Simple web interface
  - Task list view with filtering
  - Task detail view
  - Manual task actions (cancel, pause)

- [ ] **F2. Automatic Rule Reuse** - Deduplicate rules
  - Store rules in separate table
  - Reference by ID in tasks
  - UI for managing rules

- [ ] **F3. Automatic Action Reuse** - Deduplicate actions
  - Store action templates
  - Reference by ID in tasks
  - UI for managing action templates

- [ ] **F4. Retry Support** - Automatic task retry on failure
  - Add max_retries field to tasks
  - Add retry_count tracking
  - Implement exponential backoff between retries

- [ ] **F5. Scheduled Tasks** - Support for delayed/scheduled execution
  - Add scheduled_at field
  - Worker loop checks scheduled time
  - Support for cron-like schedules

- [ ] **F6. Task Priority** - Priority-based execution
  - Add priority field (0-100)
  - Higher priority tasks start first
  - Preemption for critical tasks

- [ ] **F7. Task Groups** - Logical grouping of tasks
  - Add group_id field
  - Cancel/pause entire groups
  - Group-level metrics

- [ ] **F8. Webhooks V2** - Enhanced webhook features
  - Response body parsing
  - Conditional actions based on response
  - Custom headers from task metadata

---

## Technical Debt

- [ ] **T1. Remove old validate_form** - Clean up db_operation.rs
  - Remove unused `validate_form` function
  - Remove unused `validate_params` function

- [ ] **T2. Improve task completion metrics** - Include task kind
  - Fetch task before update to get kind
  - Or pass kind in update DTO

- [ ] **T3. Consolidate error handling** - Use new error types everywhere
  - Replace DbError with TaskRunnerError
  - Use ApiError in all handlers
  - Remove Box<dyn Error> usages

- [x] **T4. Add integration tests** - Test database operations
  - `tests/integration_tests.rs`
  - Uses testcontainers for PostgreSQL
  - Tests: CRUD, DAG dependencies, pagination, filtering, transactions, deduplication
  - 20+ test cases covering core functionality

- [ ] **T5. Benchmark performance** - Establish baselines
  - Measure task throughput
  - Measure webhook latency
  - Identify bottlenecks
