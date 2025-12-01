-- Remove indexes
DROP INDEX IF EXISTS idx_task_status;
DROP INDEX IF EXISTS idx_task_kind;
DROP INDEX IF EXISTS idx_task_created_at;
DROP INDEX IF EXISTS idx_task_status_kind;
DROP INDEX IF EXISTS idx_action_task_id;
DROP INDEX IF EXISTS idx_action_trigger;
DROP INDEX IF EXISTS idx_link_child_id;
