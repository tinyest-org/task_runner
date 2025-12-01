create table "link" (
    parent_id UUID not null REFERENCES task (id),
    child_id UUID not null REFERENCES task (id),
    requires_success boolean not null default false,
    PRIMARY KEY (parent_id, child_id)
);

/*
DAG Execution Model:

Example:
  ingest_1 ──┐
  ingest_2 ──┼──> cluster ──> refresh_summary
  ingest_3 ──┘

Task States:
  - Waiting: Has unresolved dependencies (wait_success > 0 or wait_finished > 0)
  - Pending: Dependencies resolved, ready for rule evaluation and execution

Counter Logic (on task table):
  - wait_success: count of parent tasks that must succeed
  - wait_finished: count of parent tasks that must finish (regardless of success/failure)

When a parent task completes:
  1. Decrement wait_finished for all children in Waiting status
  2. If parent succeeded: also decrement wait_success for children where link.requires_success = true
  3. If both counters reach 0: transition child from Waiting to Pending
  4. If a required parent fails (requires_success = true): mark child as Failure with propagated reason

Note: The link table with requires_success is primarily for visualization/debugging.
      The actual execution uses wait_success and wait_finished counters on the task.
*/
