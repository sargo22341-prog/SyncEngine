-- Bounded projection read at start-up, never execution truth. Kept apart from
-- `runs` so that listing attempts never walks the overflow pages of saved plans.
CREATE TABLE run_overview (
 execution TEXT PRIMARY KEY REFERENCES runs(execution) ON DELETE CASCADE,
 bytes INTEGER NOT NULL,
 overview TEXT
);
PRAGMA user_version=3;
