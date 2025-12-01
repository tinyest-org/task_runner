CREATE EXTENSION IF NOT EXISTS "pgcrypto";

CREATE TYPE status_kind AS ENUM (
	'waiting',
	'pending',
	'running',
	'failure',
	'success',
	'paused'
);

CREATE TABLE
	"task" (
		-- should move to uuid v7
		id UUID PRIMARY KEY DEFAULT gen_random_uuid (), 
		"name" TEXT NOT NULL,
		"kind" TEXT NOT NULL,
		"status" status_kind NOT NULL,
		"timeout" INT4 NOT NULL,
		"created_at" TIMESTAMPTZ not null default now (),
		"started_at" TIMESTAMPTZ,
		"last_updated" TIMESTAMPTZ not null default now (),
		"metadata" jsonb not null,
		"ended_at" TIMESTAMPTZ,
		"start_condition" jsonb not null,

		"wait_success" INT4 NOT NULL default 0,
		"wait_finished" INT4 NOT NULL default 0,
		
		"success" INT4 NOT NULL default 0,
		"failures" INT4 NOT NULL default 0,
		"failure_reason" TEXT
	);