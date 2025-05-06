CREATE EXTENSION IF NOT EXISTS "pgcrypto";
-- Your SQL goes here
CREATE TABLE "task"(
	id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
	"name" TEXT NOT NULL,
	"kind" TEXT NOT NULL,
	"status" TEXT NOT NULL,
	"timeout" INT4 NOT NULL,
	"last_updated" TIMESTAMP not null default now(),
	success INT4  NOT NULL default 0,
	failures INT4  NOT NULL default 0,
	metadata  jsonb not null default '{}'::jsonb
);

