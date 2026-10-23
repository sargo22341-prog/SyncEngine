CREATE TABLE schedules (
 profile TEXT PRIMARY KEY, config TEXT NOT NULL,
 last_slot INTEGER, running_slot INTEGER, result TEXT
);
PRAGMA application_id=1398361682;
PRAGMA user_version=1;
