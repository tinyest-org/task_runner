-- Your SQL goes here

CREATE TYPE action_kind AS ENUM ('webhook');

CREATE TABLE "action"(
	id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
	"task_id" UUID NOT NULL,
	"kind" action_kind NOT NULL,
	params  jsonb not null default '{}'::jsonb
);

