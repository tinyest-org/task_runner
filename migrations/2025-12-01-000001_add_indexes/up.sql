-- Add indexes for frequently queried columns to improve performance

-- Index on task.status - used constantly in worker loops to find pending/running tasks
CREATE INDEX idx_task_status ON task(status);

-- Index on task.kind - used in concurrency rule matching and filtering
CREATE INDEX idx_task_kind ON task(kind);

-- Index on task.created_at - used for ordering in list queries
CREATE INDEX idx_task_created_at ON task(created_at DESC);

-- Composite index for common query pattern: status + kind (concurrency checks)
CREATE INDEX idx_task_status_kind ON task(status, kind);

-- Index on action.task_id - used for foreign key lookups
CREATE INDEX idx_action_task_id ON action(task_id);

-- Index on action.trigger - used to filter actions by trigger type
CREATE INDEX idx_action_trigger ON action(trigger);

-- Index on link.child_id - used when propagating task completion to children
CREATE INDEX idx_link_child_id ON link(child_id);
