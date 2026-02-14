CREATE INDEX idx_task_ended_at_terminal ON task(ended_at)
WHERE status IN ('success', 'failure', 'canceled');
