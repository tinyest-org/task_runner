//! Prometheus metrics for task runner observability.
//!
//! Provides custom metrics for tracking task execution, dependencies, and system health.

use prometheus::{
    Histogram, HistogramOpts, HistogramVec, IntCounter, IntCounterVec, IntGaugeVec, Opts, Registry,
};
use std::sync::LazyLock;

/// Global metrics registry
pub static REGISTRY: LazyLock<Registry> = LazyLock::new(Registry::new);

// ============================================================================
// Task Counters
// ============================================================================

/// Total number of tasks created
pub static TASKS_CREATED_TOTAL: LazyLock<IntCounter> = LazyLock::new(|| {
    let counter = IntCounter::new("tasks_created_total", "Total number of tasks created")
        .expect("metric can be created");
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
});

/// Total number of tasks by status transition
pub static TASK_STATUS_TRANSITIONS: LazyLock<IntCounterVec> = LazyLock::new(|| {
    let counter = IntCounterVec::new(
        Opts::new(
            "task_status_transitions_total",
            "Number of task status transitions",
        ),
        &["from_status", "to_status"],
    )
    .expect("metric can be created");
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
});

/// Tasks completed by outcome (success/failure)
pub static TASKS_COMPLETED_TOTAL: LazyLock<IntCounterVec> = LazyLock::new(|| {
    let counter = IntCounterVec::new(
        Opts::new("tasks_completed_total", "Total number of tasks completed"),
        &["outcome", "kind"],
    )
    .expect("metric can be created");
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
});

/// Tasks cancelled
pub static TASKS_CANCELLED_TOTAL: LazyLock<IntCounter> = LazyLock::new(|| {
    let counter = IntCounter::new("tasks_cancelled_total", "Total number of tasks cancelled")
        .expect("metric can be created");
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
});

/// Tasks timed out
pub static TASKS_TIMED_OUT_TOTAL: LazyLock<IntCounter> = LazyLock::new(|| {
    let counter = IntCounter::new(
        "tasks_timed_out_total",
        "Total number of tasks that timed out",
    )
    .expect("metric can be created");
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
});

// ============================================================================
// Task Gauges (current state)
// ============================================================================

/// Current number of tasks by status
pub static TASKS_BY_STATUS: LazyLock<IntGaugeVec> = LazyLock::new(|| {
    let gauge = IntGaugeVec::new(
        Opts::new("tasks_by_status", "Current number of tasks by status"),
        &["status"],
    )
    .expect("metric can be created");
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

/// Current number of running tasks by kind
pub static RUNNING_TASKS_BY_KIND: LazyLock<IntGaugeVec> = LazyLock::new(|| {
    let gauge = IntGaugeVec::new(
        Opts::new(
            "running_tasks_by_kind",
            "Current number of running tasks by kind",
        ),
        &["kind"],
    )
    .expect("metric can be created");
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

// ============================================================================
// Dependency Metrics
// ============================================================================

/// Tasks with dependencies created
pub static TASKS_WITH_DEPENDENCIES: LazyLock<IntCounter> = LazyLock::new(|| {
    let counter = IntCounter::new(
        "tasks_with_dependencies_total",
        "Total number of tasks created with dependencies",
    )
    .expect("metric can be created");
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
});

/// Dependency propagations (when parent task completes)
pub static DEPENDENCY_PROPAGATIONS: LazyLock<IntCounterVec> = LazyLock::new(|| {
    let counter = IntCounterVec::new(
        Opts::new(
            "dependency_propagations_total",
            "Number of dependency propagations when parent tasks complete",
        ),
        &["parent_outcome"],
    )
    .expect("metric can be created");
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
});

/// Tasks unblocked due to dependency completion
pub static TASKS_UNBLOCKED: LazyLock<IntCounter> = LazyLock::new(|| {
    let counter = IntCounter::new(
        "tasks_unblocked_total",
        "Total number of tasks unblocked after dependencies completed",
    )
    .expect("metric can be created");
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
});

/// Tasks failed due to dependency failure
pub static TASKS_FAILED_BY_DEPENDENCY: LazyLock<IntCounter> = LazyLock::new(|| {
    let counter = IntCounter::new(
        "tasks_failed_by_dependency_total",
        "Total number of tasks failed due to required dependency failure",
    )
    .expect("metric can be created");
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
});

/// Tasks where DB save failed after retries
pub static TASKS_DB_SAVE_FAILURES: LazyLock<IntCounter> = LazyLock::new(|| {
    let counter = IntCounter::new(
        "tasks_db_save_failures_total",
        "Total number of tasks where database save failed after max retries",
    )
    .expect("metric can be created");
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
});

/// Batch update failures (re-queued for retry)
pub static BATCH_UPDATE_FAILURES: LazyLock<IntCounter> = LazyLock::new(|| {
    let counter = IntCounter::new(
        "batch_update_failures_total",
        "Total number of batch update failures (counts re-queued for retry)",
    )
    .expect("metric can be created");
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
});

// ============================================================================
// Action Metrics
// ============================================================================

/// Webhook executions
pub static WEBHOOK_EXECUTIONS: LazyLock<IntCounterVec> = LazyLock::new(|| {
    let counter = IntCounterVec::new(
        Opts::new("webhook_executions_total", "Number of webhook executions"),
        &["trigger", "outcome"],
    )
    .expect("metric can be created");
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
});

/// Webhook execution duration in seconds
pub static WEBHOOK_DURATION_SECONDS: LazyLock<HistogramVec> = LazyLock::new(|| {
    let histogram = HistogramVec::new(
        HistogramOpts::new(
            "webhook_duration_seconds",
            "Webhook execution duration in seconds",
        )
        .buckets(vec![0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0]),
        &["trigger"],
    )
    .expect("metric can be created");
    REGISTRY.register(Box::new(histogram.clone())).unwrap();
    histogram
});

// ============================================================================
// Concurrency Metrics
// ============================================================================

/// Tasks blocked by concurrency rules
pub static TASKS_BLOCKED_BY_CONCURRENCY: LazyLock<IntCounter> = LazyLock::new(|| {
    let counter = IntCounter::new(
        "tasks_blocked_by_concurrency_total",
        "Total number of tasks blocked due to concurrency rules",
    )
    .expect("metric can be created");
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
});

// ============================================================================
// Duration Metrics
// ============================================================================

/// Task execution duration (from Running to completion)
pub static TASK_DURATION_SECONDS: LazyLock<HistogramVec> = LazyLock::new(|| {
    let histogram = HistogramVec::new(
        HistogramOpts::new(
            "task_duration_seconds",
            "Task execution duration in seconds from Running to completion",
        )
        .buckets(vec![
            1.0, 5.0, 10.0, 30.0, 60.0, 300.0, 600.0, 1800.0, 3600.0,
        ]),
        &["kind", "outcome"],
    )
    .expect("metric can be created");
    REGISTRY.register(Box::new(histogram.clone())).unwrap();
    histogram
});

/// Task wait time (from Pending to Running)
pub static TASK_WAIT_SECONDS: LazyLock<HistogramVec> = LazyLock::new(|| {
    let histogram = HistogramVec::new(
        HistogramOpts::new(
            "task_wait_seconds",
            "Task wait time in seconds from Pending to Running",
        )
        .buckets(vec![0.1, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0, 300.0]),
        &["kind"],
    )
    .expect("metric can be created");
    REGISTRY.register(Box::new(histogram.clone())).unwrap();
    histogram
});

// ============================================================================
// Worker Loop Metrics
// ============================================================================

/// Worker loop iterations
pub static WORKER_LOOP_ITERATIONS: LazyLock<IntCounter> = LazyLock::new(|| {
    let counter = IntCounter::new(
        "worker_loop_iterations_total",
        "Total number of worker loop iterations",
    )
    .expect("metric can be created");
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
});

/// Worker loop duration
pub static WORKER_LOOP_DURATION_SECONDS: LazyLock<Histogram> = LazyLock::new(|| {
    let histogram = Histogram::with_opts(
        HistogramOpts::new(
            "worker_loop_duration_seconds",
            "Duration of worker loop iterations in seconds",
        )
        .buckets(vec![0.001, 0.005, 0.01, 0.05, 0.1, 0.25, 0.5, 1.0]),
    )
    .expect("metric can be created");
    REGISTRY.register(Box::new(histogram.clone())).unwrap();
    histogram
});

/// Tasks processed per worker loop
pub static TASKS_PROCESSED_PER_LOOP: LazyLock<Histogram> = LazyLock::new(|| {
    let histogram = Histogram::with_opts(
        HistogramOpts::new(
            "tasks_processed_per_loop",
            "Number of tasks processed per worker loop iteration",
        )
        .buckets(vec![0.0, 1.0, 5.0, 10.0, 25.0, 50.0, 100.0]),
    )
    .expect("metric can be created");
    REGISTRY.register(Box::new(histogram.clone())).unwrap();
    histogram
});

// ============================================================================
// Database Metrics
// ============================================================================

/// Database query duration
pub static DB_QUERY_DURATION_SECONDS: LazyLock<HistogramVec> = LazyLock::new(|| {
    let histogram = HistogramVec::new(
        HistogramOpts::new(
            "db_query_duration_seconds",
            "Database query duration in seconds",
        )
        .buckets(vec![0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0]),
        &["query"],
    )
    .expect("metric can be created");
    REGISTRY.register(Box::new(histogram.clone())).unwrap();
    histogram
});

// ============================================================================
// Circuit Breaker Metrics
// ============================================================================

/// Circuit breaker state transitions
pub static CIRCUIT_BREAKER_STATE_TRANSITIONS: LazyLock<IntCounterVec> = LazyLock::new(|| {
    let counter = IntCounterVec::new(
        Opts::new(
            "circuit_breaker_state_transitions_total",
            "Number of circuit breaker state transitions",
        ),
        &["to_state"],
    )
    .expect("metric can be created");
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
});

/// Requests rejected by circuit breaker
pub static CIRCUIT_BREAKER_REJECTIONS: LazyLock<IntCounter> = LazyLock::new(|| {
    let counter = IntCounter::new(
        "circuit_breaker_rejections_total",
        "Total number of requests rejected by circuit breaker",
    )
    .expect("metric can be created");
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
});

// ============================================================================
// Retention Cleanup Metrics
// ============================================================================

/// Total number of tasks cleaned up by retention
pub static RETENTION_TASKS_CLEANED: LazyLock<IntCounter> = LazyLock::new(|| {
    let counter = IntCounter::new(
        "retention_tasks_cleaned_total",
        "Total number of tasks deleted by retention cleanup",
    )
    .expect("metric can be created");
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
});

/// Retention cleanup runs by outcome (success/error)
pub static RETENTION_CLEANUP_RUNS: LazyLock<IntCounterVec> = LazyLock::new(|| {
    let counter = IntCounterVec::new(
        Opts::new(
            "retention_cleanup_runs_total",
            "Number of retention cleanup runs by outcome",
        ),
        &["outcome"],
    )
    .expect("metric can be created");
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
});

/// Retention cleanup cycle duration in seconds
pub static RETENTION_CLEANUP_DURATION: LazyLock<Histogram> = LazyLock::new(|| {
    let histogram = Histogram::with_opts(
        HistogramOpts::new(
            "retention_cleanup_duration_seconds",
            "Duration of retention cleanup cycles in seconds",
        )
        .buckets(vec![0.1, 0.5, 1.0, 5.0, 10.0, 30.0, 60.0]),
    )
    .expect("metric can be created");
    REGISTRY.register(Box::new(histogram.clone())).unwrap();
    histogram
});

// ============================================================================
// Helper Functions
// ============================================================================

/// Record a task creation
pub fn record_task_created() {
    TASKS_CREATED_TOTAL.inc();
}

/// Record a task creation with dependencies
pub fn record_task_with_dependencies() {
    TASKS_WITH_DEPENDENCIES.inc();
}

/// Record a task status transition
pub fn record_status_transition(from: &str, to: &str) {
    TASK_STATUS_TRANSITIONS.with_label_values(&[from, to]).inc();
}

/// Record task completion
pub fn record_task_completed(outcome: &str, kind: &str) {
    TASKS_COMPLETED_TOTAL
        .with_label_values(&[outcome, kind])
        .inc();
}

/// Record task cancellation
pub fn record_task_cancelled() {
    TASKS_CANCELLED_TOTAL.inc();
}

/// Record task timeout
pub fn record_task_timeout() {
    TASKS_TIMED_OUT_TOTAL.inc();
}

/// Record dependency propagation
pub fn record_dependency_propagation(parent_outcome: &str) {
    DEPENDENCY_PROPAGATIONS
        .with_label_values(&[parent_outcome])
        .inc();
}

/// Record task unblocked
pub fn record_task_unblocked() {
    TASKS_UNBLOCKED.inc();
}

/// Record task failed by dependency
pub fn record_task_failed_by_dependency() {
    TASKS_FAILED_BY_DEPENDENCY.inc();
}

/// Record task DB save failure
pub fn record_task_db_save_failure() {
    TASKS_DB_SAVE_FAILURES.inc();
}

/// Record batch update failure (re-queued for retry)
pub fn record_batch_update_failure() {
    BATCH_UPDATE_FAILURES.inc();
}

/// Record webhook execution
pub fn record_webhook_execution(trigger: &str, outcome: &str, duration_secs: f64) {
    WEBHOOK_EXECUTIONS
        .with_label_values(&[trigger, outcome])
        .inc();
    WEBHOOK_DURATION_SECONDS
        .with_label_values(&[trigger])
        .observe(duration_secs);
}

/// Record task execution duration
pub fn record_task_duration(kind: &str, outcome: &str, duration_secs: f64) {
    TASK_DURATION_SECONDS
        .with_label_values(&[kind, outcome])
        .observe(duration_secs);
}

/// Record task blocked by concurrency
pub fn record_task_blocked_by_concurrency() {
    TASKS_BLOCKED_BY_CONCURRENCY.inc();
}

/// Update current tasks by status gauge
pub fn set_tasks_by_status(status: &str, count: i64) {
    TASKS_BY_STATUS.with_label_values(&[status]).set(count);
}

/// Update running tasks by kind gauge
pub fn set_running_tasks_by_kind(kind: &str, count: i64) {
    RUNNING_TASKS_BY_KIND.with_label_values(&[kind]).set(count);
}

/// Record worker loop iteration
pub fn record_worker_loop_iteration(duration_secs: f64, tasks_processed: usize) {
    WORKER_LOOP_ITERATIONS.inc();
    WORKER_LOOP_DURATION_SECONDS.observe(duration_secs);
    TASKS_PROCESSED_PER_LOOP.observe(tasks_processed as f64);
}

/// Record database query duration
pub fn record_db_query(query_name: &str, duration_secs: f64) {
    DB_QUERY_DURATION_SECONDS
        .with_label_values(&[query_name])
        .observe(duration_secs);
}

/// Record database query duration with slow query warning.
///
/// If the query duration exceeds the threshold, a warning is logged.
/// This helps operators identify performance issues in production.
pub fn record_db_query_with_slow_warning(query_name: &str, duration_secs: f64, threshold_ms: u64) {
    DB_QUERY_DURATION_SECONDS
        .with_label_values(&[query_name])
        .observe(duration_secs);

    let duration_ms = (duration_secs * 1000.0) as u64;
    if duration_ms > threshold_ms {
        log::warn!(
            "Slow query detected: {} took {}ms (threshold: {}ms)",
            query_name,
            duration_ms,
            threshold_ms
        );
        SLOW_QUERIES_TOTAL.with_label_values(&[query_name]).inc();
    }
}

/// Total number of slow queries
pub static SLOW_QUERIES_TOTAL: LazyLock<IntCounterVec> = LazyLock::new(|| {
    let counter = IntCounterVec::new(
        Opts::new(
            "slow_queries_total",
            "Total number of queries exceeding slow query threshold",
        ),
        &["query"],
    )
    .expect("metric can be created");
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
});

/// Record circuit breaker state transition
pub fn record_circuit_breaker_state(to_state: &str) {
    CIRCUIT_BREAKER_STATE_TRANSITIONS
        .with_label_values(&[to_state])
        .inc();
}

/// Record request rejected by circuit breaker
pub fn record_circuit_breaker_rejection() {
    CIRCUIT_BREAKER_REJECTIONS.inc();
}

/// Record retention cleanup cycle
pub fn record_retention_cleanup(outcome: &str, tasks_deleted: usize, duration_secs: f64) {
    RETENTION_CLEANUP_RUNS.with_label_values(&[outcome]).inc();
    RETENTION_CLEANUP_DURATION.observe(duration_secs);
    if tasks_deleted > 0 {
        RETENTION_TASKS_CLEANED.inc_by(tasks_deleted as u64);
    }
}

/// Initialize all metrics (call at startup to register them)
pub fn init_metrics() {
    // Force lazy initialization of all metrics
    let _ = &*TASKS_CREATED_TOTAL;
    let _ = &*TASK_STATUS_TRANSITIONS;
    let _ = &*TASKS_COMPLETED_TOTAL;
    let _ = &*TASKS_CANCELLED_TOTAL;
    let _ = &*TASKS_TIMED_OUT_TOTAL;
    let _ = &*TASKS_BY_STATUS;
    let _ = &*RUNNING_TASKS_BY_KIND;
    let _ = &*TASKS_WITH_DEPENDENCIES;
    let _ = &*DEPENDENCY_PROPAGATIONS;
    let _ = &*TASKS_UNBLOCKED;
    let _ = &*TASKS_FAILED_BY_DEPENDENCY;
    let _ = &*TASKS_DB_SAVE_FAILURES;
    let _ = &*BATCH_UPDATE_FAILURES;
    let _ = &*WEBHOOK_EXECUTIONS;
    let _ = &*WEBHOOK_DURATION_SECONDS;
    let _ = &*TASKS_BLOCKED_BY_CONCURRENCY;
    let _ = &*TASK_DURATION_SECONDS;
    let _ = &*TASK_WAIT_SECONDS;
    let _ = &*WORKER_LOOP_ITERATIONS;
    let _ = &*WORKER_LOOP_DURATION_SECONDS;
    let _ = &*TASKS_PROCESSED_PER_LOOP;
    let _ = &*DB_QUERY_DURATION_SECONDS;
    let _ = &*SLOW_QUERIES_TOTAL;
    let _ = &*CIRCUIT_BREAKER_STATE_TRANSITIONS;
    let _ = &*CIRCUIT_BREAKER_REJECTIONS;
    let _ = &*RETENTION_TASKS_CLEANED;
    let _ = &*RETENTION_CLEANUP_RUNS;
    let _ = &*RETENTION_CLEANUP_DURATION;
}
