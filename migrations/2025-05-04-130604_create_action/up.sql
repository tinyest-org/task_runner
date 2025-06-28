-- Your SQL goes here

CREATE TYPE action_kind AS ENUM ('webhook');
CREATE TYPE trigger_kind AS ENUM ('start', 'end', 'cancel');

CREATE TABLE "action"(
	id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
	"task_id" UUID NOT NULL references task(id),
	"kind" action_kind NOT NULL,
	params  jsonb not null default '{}'::jsonb,
	trigger trigger_kind NOT NULL,
	"success" boolean
);

