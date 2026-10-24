-- Schedules gain the presentation-only `show_window` flag. Stored configurations
-- are re-serialized in Rust before this version bump (see `normalize_configs`).
PRAGMA user_version=3;
