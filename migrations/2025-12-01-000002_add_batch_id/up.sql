-- Add batch_id column to track tasks created in the same request
-- This enables tracing/debugging of entire DAGs created together
ALTER TABLE task ADD COLUMN batch_id UUID;

-- Index for efficient queries by batch_id
CREATE INDEX idx_task_batch_id ON task(batch_id) WHERE batch_id IS NOT NULL;
