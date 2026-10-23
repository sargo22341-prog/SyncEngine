-- Only applied to an empty database under an IMMEDIATE transaction.
CREATE TABLE profiles(id TEXT PRIMARY KEY, config TEXT NOT NULL);
CREATE TABLE preferences(id INTEGER PRIMARY KEY CHECK(id=1), config TEXT NOT NULL);
PRAGMA application_id=1398361680;
PRAGMA user_version=1;
