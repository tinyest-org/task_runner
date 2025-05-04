-- Your SQL goes here

CREATE TABLE "action"(
	"id" UUID NOT NULL PRIMARY KEY,
	"kind" TEXT NOT NULL,
	"task_id" UUID NOT NULL
);

