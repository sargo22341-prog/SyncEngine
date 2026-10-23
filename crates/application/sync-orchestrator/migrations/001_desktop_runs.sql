-- Separate from baseline state: all desktop modes share this attempt history.
CREATE TABLE runs(
    execution TEXT PRIMARY KEY,
    profile TEXT NOT NULL,
    started TEXT NOT NULL,
    state TEXT NOT NULL,
    plan TEXT NOT NULL,
    report TEXT
);
CREATE INDEX runs_profile ON runs(profile,started);
PRAGMA application_id=1398361681;
PRAGMA user_version=1;
