ALTER TABLE task ADD COLUMN priority INTEGER NOT NULL DEFAULT 0;
CREATE INDEX idx_task_priority ON task(status, priority DESC, created_at ASC);
