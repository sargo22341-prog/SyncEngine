-- Bounded current interactive session, preserved across agent restarts.
CREATE TABLE resident_session (
 singleton INTEGER PRIMARY KEY CHECK(singleton=1),
 identity TEXT NOT NULL, logon_at INTEGER NOT NULL CHECK(logon_at>=0)
);
PRAGMA user_version=4;
