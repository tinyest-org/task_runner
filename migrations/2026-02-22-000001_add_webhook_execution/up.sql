CREATE TYPE webhook_execution_status AS ENUM ('pending', 'success', 'failure');

CREATE TABLE webhook_execution (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    task_id UUID NOT NULL REFERENCES task(id),
    trigger trigger_kind NOT NULL,
    condition trigger_condition NOT NULL,
    idempotency_key TEXT NOT NULL UNIQUE,
    status webhook_execution_status NOT NULL DEFAULT 'pending',
    attempts INT4 NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_webhook_execution_task_id ON webhook_execution(task_id);
CREATE INDEX idx_webhook_execution_status ON webhook_execution(status);
