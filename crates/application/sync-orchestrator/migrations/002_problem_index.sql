-- Derived views only: original plans/reports remain authoritative.
CREATE TABLE problem_rows (
 execution TEXT NOT NULL REFERENCES runs(execution) ON DELETE CASCADE,
 ordinal INTEGER NOT NULL,
 action TEXT NOT NULL,
 status TEXT NOT NULL,
 path TEXT NOT NULL,
 operation TEXT NOT NULL,
 outcome TEXT,
 PRIMARY KEY(execution,ordinal)
);
CREATE INDEX problem_status ON problem_rows(execution,status,ordinal);
CREATE INDEX problem_action ON problem_rows(execution,action,ordinal);
CREATE TABLE problem_indexed (
 execution TEXT PRIMARY KEY REFERENCES runs(execution) ON DELETE CASCADE
);
PRAGMA user_version=2;
