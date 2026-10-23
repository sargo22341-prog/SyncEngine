-- Preserve existing schedules and receipts; remember native events across ticks.
CREATE TABLE session_wakes (event TEXT PRIMARY KEY, occurred_at INTEGER NOT NULL);
PRAGMA user_version=2;
