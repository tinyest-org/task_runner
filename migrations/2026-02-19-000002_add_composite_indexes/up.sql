-- Composite index for action lookups by task + trigger type.
-- Used in start_task, fire_end_webhooks, end_task, cancel_task.
-- Replaces the need to merge separate task_id and trigger indexes.
CREATE INDEX idx_action_task_id_trigger ON action(task_id, trigger);

-- GIN index for JSONB containment queries on task.metadata.
-- Used in filtering (list_task_filtered_paged), deduplication (handle_dedupe),
-- and concurrency rule checks (claim_task_with_rules).
CREATE INDEX idx_task_metadata_gin ON task USING gin(metadata);
