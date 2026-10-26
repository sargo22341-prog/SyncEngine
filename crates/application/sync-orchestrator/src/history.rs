//! Filtered, bounded views over the existing durable reports (no migration).
use crate::{AppError, error, io};
use rusqlite::params;
use std::path::Path;
use sync_core::ProfileId;
mod index;
pub(super) use index::finished as index_finished;

/// Selection applied before pagination, never to the executable plan itself.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum HistoryFilter {
    /// Every operation, including successful ones.
    #[default]
    All,
    /// Failures, refusals, interruptions, unknown outcomes, skips and conflicts.
    Problems,
    /// Failed operations only.
    Failed,
    /// Mutations refused by execution preconditions.
    Refused,
    /// Cancelled, not-started or unrecorded operations.
    Unfinished,
    /// Planned skips and conflicts, even when no mutation was attempted.
    Skipped,
}
impl HistoryFilter {
    fn code(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Problems => "problems",
            Self::Failed => "failed",
            Self::Refused => "refused",
            Self::Unfinished => "unfinished",
            Self::Skipped => "skipped",
        }
    }
}

// Walk report outcomes once to filter on their own path/action/status. Looking
// up every outcome by its index in a huge JSON array would be quadratic.
// Parse the plan once into SQLite's binary JSON for the selected index lookups.
const ROWS: &str = r#"
WITH chosen AS MATERIALIZED (
 SELECT jsonb(plan) plan, COALESCE(json_extract(report,'$.outcomes'),json_extract(report,'$.execution.outcomes')) outcomes
 FROM runs WHERE profile=?1 AND execution=?2
), records AS (
 SELECT CAST(j.key AS INTEGER) ordinal, j.value outcome,
        json_extract(j.value,'$.path') path, json_extract(j.value,'$.action') action,
        COALESCE(json_extract(j.value,'$.status.status'),'') status
 FROM chosen r,json_each(r.outcomes) j
 UNION ALL
 SELECT CAST(j.key AS INTEGER),NULL,json_extract(j.value,'$.path'),json_extract(j.value,'$.action'),''
 FROM chosen r,json_each(r.plan,'$.operations') j WHERE r.outcomes IS NULL
), matches AS (
 SELECT * FROM records WHERE
 (?3='all' OR (?3='failed' AND status='failed') OR (?3='refused' AND status='refused')
 OR (?3='unfinished' AND status IN ('cancelled','not_started',''))
 OR (?3='skipped' AND action IN ('skip','conflict'))
 OR (?3='problems' AND (status IN ('failed','refused','cancelled','not_started','') OR action IN ('skip','conflict'))))
 AND (?4='' OR instr(lower(path),lower(?4))>0 OR instr(lower(COALESCE(outcome,'')),lower(?4))>0)
)
"#;

pub(super) fn page(
    root: &Path,
    id: &ProfileId,
    execution: &str,
    offset: usize,
    filter: HistoryFilter,
    query: &str,
) -> Result<serde_json::Value, AppError> {
    if offset > i64::MAX as usize || query.len() > 512 {
        return Err(error("invalid_input", "historical query out of range"));
    }
    let mut db = crate::runs::open(root)?;
    let state: String = db
        .query_row(
            "SELECT state FROM runs WHERE profile=?1 AND execution=?2",
            params![id.as_str(), execution],
            |r| r.get(0),
        )
        .map_err(io)?;
    if filter != HistoryFilter::All && state != "running" {
        let persisted = index::ensure(&mut db, id.as_str(), execution)?;
        let matches = if persisted {
            index::MATCHES.to_owned()
        } else {
            index::MATCHES.replace(
                "FROM problem_rows",
                "FROM (SELECT ?1 execution,* FROM history_ready)",
            )
        };
        let total: i64 = db
            .query_row(
                &format!("SELECT count(*) {matches}"),
                params![execution, filter.code(), query],
                |r| r.get(0),
            )
            .map_err(io)?;
        let mut stmt = db
            .prepare(&format!(
                "SELECT operation,outcome {} ORDER BY ordinal LIMIT 100 OFFSET ?4",
                matches
            ))
            .map_err(io)?;
        let rows = stmt
            .query_map(
                params![execution, filter.code(), query, offset as i64],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
            )
            .map_err(io)?;
        let mut operations = Vec::new();
        for row in rows {
            let (op, outcome) = row.map_err(io)?;
            operations.push(serde_json::json!({"operation":serde_json::from_str::<serde_json::Value>(&op).map_err(io)?,"outcome":outcome.map(|s|serde_json::from_str::<serde_json::Value>(&s)).transpose().map_err(io)?}));
        }
        return Ok(serde_json::json!({"operations":operations,"total":total}));
    }
    let total: i64 = db
        .query_row(
            &format!("{ROWS} SELECT count(*) FROM matches"),
            params![id.as_str(), execution, filter.code(), query],
            |r| r.get(0),
        )
        .map_err(io)?;
    let mut stmt = db.prepare(&format!("{ROWS}, page AS MATERIALIZED (SELECT * FROM matches ORDER BY ordinal LIMIT 100 OFFSET ?5) SELECT json_extract(r.plan,'$.operations['||p.ordinal||']'),p.outcome FROM chosen r,page p ORDER BY p.ordinal")).map_err(io)?;
    let rows = stmt
        .query_map(
            params![id.as_str(), execution, filter.code(), query, offset as i64],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
        )
        .map_err(io)?;
    let mut values = Vec::new();
    for row in rows {
        let (operation, outcome) = row.map_err(io)?;
        values.push(serde_json::json!({"operation":serde_json::from_str::<serde_json::Value>(&operation).map_err(io)?,"outcome":outcome.map(|s|serde_json::from_str::<serde_json::Value>(&s)).transpose().map_err(io)?}));
    }
    Ok(serde_json::json!({"operations":values,"total":total}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn legacy_history_migrates_and_indexed_reads_do_not_reserve_the_writer()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let old = rusqlite::Connection::open(root.path().join("desktop-runs.sqlite"))?;
        old.execute_batch(include_str!("../migrations/001_desktop_runs.sql"))?;
        old.execute("INSERT INTO runs VALUES('e','p','0','finished',?1,?2)",params![json!({"operations":[{"path":"a","action":"copy"}]}).to_string(),json!({"outcomes":[{"path":"a","action":"copy","status":{"status":"refused","refusal":{"cause":"source_changed"}}}]}).to_string()])?;
        drop(old);
        let id = ProfileId::new("p")?;
        assert_eq!(
            page(root.path(), &id, "e", 0, HistoryFilter::Refused, "")?["total"],
            1
        );
        let mut writer = crate::runs::open(root.path())?;
        writer.execute("INSERT INTO runs SELECT 'uncached',profile,started,state,plan,report FROM runs WHERE execution='e'",[])?;
        assert_eq!(
            writer.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))?,
            2
        );
        assert_eq!(
            writer.query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0))?,
            "wal"
        );
        let tx = writer.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute("UPDATE runs SET started='1' WHERE execution='e'", [])?;
        // Given a cached completed run and another writer, When filtering,
        // Then reads finish without waiting for that writer to release its lock.
        let start = std::time::Instant::now();
        assert_eq!(
            page(root.path(), &id, "e", 0, HistoryFilter::Problems, "")?["total"],
            1
        );
        assert_eq!(
            page(root.path(), &id, "uncached", 0, HistoryFilter::Refused, "")?["total"],
            1
        );
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
        tx.rollback()?;
        writer.execute("DELETE FROM runs WHERE execution='e'", [])?;
        assert_eq!(
            writer.query_row("SELECT count(*) FROM problem_rows", [], |r| r
                .get::<_, u32>(0))?,
            0
        );
        Ok(())
    }
    #[test]
    fn filters_precede_pagination_and_preserve_plan_outcome_pairs()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let db = crate::runs::open(root.path())?;
        let mut operations = Vec::new();
        let mut outcomes = Vec::new();
        for n in 0..250 {
            let status = if n % 2 == 0 { "refused" } else { "done" };
            operations.push(json!({"path":format!("f{n:03}"),"action":"copy"}));
            outcomes.push(json!({"path":format!("f{n:03}"),"action":"copy","status":{"status":status,"refusal":{"cause":"source_changed"}}}));
        }
        operations.push(json!({"path":"ignored","action":"skip","reason":{"code":"uncertain"}}));
        outcomes.push(json!({"path":"ignored","action":"skip","status":{"status":"no_action"}}));
        operations.push(json!({"path":"failed","action":"copy"}));
        outcomes.push(json!({"path":"failed","action":"copy","status":{"status":"failed","error":{"message":"disk error"}}}));
        let plan = json!({"operations":operations}).to_string();
        for (execution, report) in [
            ("direct", json!({"outcomes":outcomes})),
            ("wrapped", json!({"execution":{"outcomes":outcomes}})),
        ] {
            db.execute(
                "INSERT INTO runs VALUES(?1,'p','0','finished',?2,?3)",
                params![execution, plan, report.to_string()],
            )?;
            let id = ProfileId::new("p")?;
            let first = page(root.path(), &id, execution, 0, HistoryFilter::Refused, "")?;
            assert_eq!(first["total"], 125);
            assert_eq!(first["operations"].as_array().ok_or("array")?.len(), 100);
            let second = page(root.path(), &id, execution, 100, HistoryFilter::Refused, "")?;
            assert_eq!(second["operations"].as_array().ok_or("array")?.len(), 25);
            assert_eq!(second["operations"][0]["operation"]["path"], "f200");
            assert_eq!(second["operations"][0]["outcome"]["path"], "f200");
            assert_eq!(
                page(root.path(), &id, execution, 0, HistoryFilter::Problems, "")?["total"],
                127
            );
            assert_eq!(
                page(
                    root.path(),
                    &id,
                    execution,
                    0,
                    HistoryFilter::Failed,
                    "disk error"
                )?["total"],
                1
            );
            assert_eq!(
                page(
                    root.path(),
                    &id,
                    execution,
                    0,
                    HistoryFilter::Skipped,
                    "ignored"
                )?["total"],
                1
            );
            assert_eq!(
                page(
                    root.path(),
                    &id,
                    execution,
                    0,
                    HistoryFilter::Refused,
                    "f004"
                )?["total"],
                1
            );
            assert!(
                page(
                    root.path(),
                    &ProfileId::new("foreign")?,
                    execution,
                    0,
                    HistoryFilter::All,
                    ""
                )
                .is_err()
            );
        }
        db.execute(
            "INSERT INTO runs VALUES('interrupted','p','0','running',?1,NULL)",
            [plan],
        )?;
        assert_eq!(
            page(
                root.path(),
                &ProfileId::new("p")?,
                "interrupted",
                0,
                HistoryFilter::Unfinished,
                ""
            )?["total"],
            252
        );
        assert!(
            page(
                root.path(),
                &ProfileId::new("p")?,
                "direct",
                0,
                HistoryFilter::All,
                &"a".repeat(513)
            )
            .is_err()
        );
        Ok(())
    }
}
