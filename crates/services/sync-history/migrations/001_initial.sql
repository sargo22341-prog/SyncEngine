CREATE TABLE profiles (
    profile TEXT PRIMARY KEY NOT NULL,
    binding TEXT NOT NULL,
    generation INTEGER NOT NULL DEFAULT 0 CHECK(generation >= 0),
    baseline TEXT
);
CREATE TABLE executions (
    execution TEXT PRIMARY KEY NOT NULL,
    profile TEXT NOT NULL REFERENCES profiles(profile),
    generation INTEGER NOT NULL,
    plan TEXT NOT NULL,
    started_at TEXT NOT NULL,
    finished_at TEXT,
    state TEXT NOT NULL CHECK(state IN ('running','finished','interrupted')),
    outcome TEXT,
    report TEXT
);
CREATE UNIQUE INDEX one_running_profile ON executions(profile) WHERE state = 'running';
CREATE INDEX execution_history ON executions(profile, state);
PRAGMA user_version = 1;
PRAGMA application_id = 1398361669;
