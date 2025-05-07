CREATE EXTENSION IF NOT EXISTS "pgcrypto";

CREATE TYPE status_kind AS ENUM ('pending', 'running', 'failure', 'success');

-- Your SQL goes here
CREATE TABLE "task"(
	id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
	"name" TEXT,
	"kind" TEXT NOT NULL,
	"status" status_kind NOT NULL,
	"created_at"TIMESTAMP not null default now(),
	"timeout" INT4 NOT NULL,
	"last_updated" TIMESTAMP not null default now(),
	success INT4  NOT NULL default 0,
	failures INT4  NOT NULL default 0,
	metadata  jsonb not null default '{}'::jsonb,
	"ended_at" TIMESTAMP
);

