-- Your SQL goes here

CREATE TYPE action_kind AS ENUM ('webhook');

CREATE TABLE "action"(
	"id" UUID NOT NULL PRIMARY KEY,
	"task_id" UUID NOT NULL,
	"kind" action_kind NOT NULL
);

