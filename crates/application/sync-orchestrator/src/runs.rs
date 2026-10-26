//! Durable application attempts, including preflight failures and interruptions.
use crate::{AppError, error, io};
use rusqlite::{Connection, params};
use serde::Serialize;
use std::path::Path;
use sync_core::{ExecutionId, ProfileId, Timestamp};
use sync_plan::ExecutionPlan;

/// Persisted attempt. `running` after restart means diagnosis is required;
/// it must never be presented as a completed successful run.
#[derive(Debug, Serialize)]
pub struct Run {
    /// Unique execution identity.
    pub execution: String,
    /// UTC start timestamp.
    pub started: String,
    /// Running, finished or error (report carries the actual execution status).
    pub state: String,
    /// Exact preview plan used for this attempt.
    pub plan: serde_json::Value,
    /// Structured report/error when completion was persisted.
    pub report: Option<serde_json::Value>,
}
pub(super) fn open(root: &Path) -> Result<Connection, AppError> {
    let mut db = Connection::open(root.join("desktop-runs.sqlite")).map_err(io)?;
    db.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(io)?;
    db.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL;")
        .map_err(io)?;
    let version: u32 = db
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(io)?;
    let app: u32 = db
        .query_row("PRAGMA application_id", [], |r| r.get(0))
        .map_err(io)?;
    // Ordinary reads must never acquire a writer reservation or scan the whole DB.
    if version == 2 && app == 1_398_361_681 {
        return Ok(db);
    }
    let tx = db
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(io)?;
    let version: u32 = tx
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(io)?;
    let app: u32 = tx
        .query_row("PRAGMA application_id", [], |r| r.get(0))
        .map_err(io)?;
    if version == 0 && app == 0 {
        let tables: i64 = tx
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'",
                [],
                |r| r.get(0),
            )
            .map_err(io)?;
        if tables != 0 {
            return Err(error("integrity", "historique étranger"));
        }
        tx.execute_batch(include_str!("../migrations/001_desktop_runs.sql"))
            .map_err(io)?;
    } else if !matches!(version, 1 | 2) || app != 1_398_361_681 {
        return Err(error("integrity", "version historique inconnue"));
    }
    if version < 2 {
        tx.execute_batch(include_str!("../migrations/002_problem_index.sql"))
            .map_err(io)?;
    }
    tx.commit().map_err(io)?;
    let mode: String = db
        .query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))
        .map_err(io)?;
    if mode != "wal" {
        return Err(error("unavailable", "WAL indisponible pour l’historique"));
    }
    Ok(db)
}

pub(super) fn initialize(root: &Path) -> Result<(), AppError> {
    let db = open(root)?;
    let check: String = db
        .query_row("PRAGMA quick_check", [], |r| r.get(0))
        .map_err(io)?;
    if check != "ok" {
        return Err(error("integrity", "historique corrompu"));
    }
    Ok(())
}
pub(super) fn begin(
    root: &Path,
    id: &ExecutionId,
    profile: &ProfileId,
    plan: &ExecutionPlan,
) -> Result<(), AppError> {
    open(root)?
        .execute(
            "INSERT INTO runs VALUES(?1,?2,?3,'running',?4,NULL)",
            params![
                id.as_str(),
                profile.as_str(),
                serde_json::to_string(&Timestamp::now().map_err(io)?).map_err(io)?,
                plan.to_json().map_err(io)?
            ],
        )
        .map_err(io)?;
    Ok(())
}
pub(super) fn finish(
    root: &Path,
    id: &ExecutionId,
    result: &Result<serde_json::Value, AppError>,
    plan: Option<&ExecutionPlan>,
) -> Result<(), AppError> {
    let mut db = open(root)?;
    let tx = db.transaction().map_err(io)?;
    let (state, report) = match result {
        Ok(value) => ("finished", value.clone()),
        Err(e) => ("error", serde_json::to_value(e).map_err(io)?),
    };
    tx.execute(
        "UPDATE runs SET state=?2,report=?3 WHERE execution=?1",
        params![
            id.as_str(),
            state,
            serde_json::to_string(&report).map_err(io)?
        ],
    )
    .map_err(io)?;
    crate::history::index_finished(&tx, id.as_str(), plan, &report)?;
    // Keep interrupted/running records; bound completed history per profile.
    // Match journal retention to the existing 100 completed attempts. Select
    // only persisted completed identities; never scan/delete arbitrary files.
    let expired: Vec<String> = {
        let mut query = tx.prepare("SELECT execution FROM runs WHERE profile=(SELECT profile FROM runs WHERE execution=?1) AND state!='running' ORDER BY rowid DESC LIMIT -1 OFFSET 100").map_err(io)?;
        query
            .query_map([id.as_str()], |row| row.get(0))
            .map_err(io)?
            .collect::<Result<_, _>>()
            .map_err(io)?
    };
    for expired in expired {
        let expired = ExecutionId::new(expired).map_err(|e| error("integrity", e))?;
        // ExecutionId excludes separators; the appended extension is a single
        // file in the application state directory. remove_file unlinks symlinks.
        match std::fs::remove_file(root.join(format!("{expired}.jsonl"))) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(error("journal_retention", e)),
        }
    }
    tx.execute("DELETE FROM runs WHERE profile=(SELECT profile FROM runs WHERE execution=?1) AND state!='running' AND execution NOT IN (SELECT execution FROM runs WHERE profile=(SELECT profile FROM runs WHERE execution=?1) AND state!='running' ORDER BY rowid DESC LIMIT 100)",[id.as_str()]).map_err(io)?;
    tx.commit().map_err(io)
}
pub(super) fn list(root: &Path, id: &ProfileId) -> Result<Vec<Run>, AppError> {
    list_view(root, id, false)
}
pub(super) fn overview(root: &Path, id: &ProfileId) -> Result<Vec<Run>, AppError> {
    list_view(root, id, true)
}
fn list_view(root: &Path, id: &ProfileId, overview: bool) -> Result<Vec<Run>, AppError> {
    let db = open(root)?;
    let query = if overview {
        "SELECT execution,started,state,'null',CASE WHEN report IS NULL THEN NULL ELSE json_object('status',COALESCE(json_extract(report,'$.status'),json_extract(report,'$.execution.status')),'verification_error',substr(json_extract(report,'$.verification_error'),1,4096),'baseline_updated',json_extract(report,'$.baseline_updated'),'code',json_extract(report,'$.code'),'message',substr(json_extract(report,'$.message'),1,4096),'totals',COALESCE(json_extract(report,'$.totals'),json_extract(report,'$.execution.totals')),'deletions_refused',(SELECT count(*) FROM json_each(runs.plan,'$.operations') WHERE json_extract(value,'$.reason.code') IN ('deletion_disabled','deletion_refused'))) END FROM runs WHERE profile=?1 ORDER BY (state='running') DESC,rowid DESC LIMIT 100"
    } else {
        "SELECT execution,started,state,plan,report FROM runs WHERE profile=?1 ORDER BY (state='running') DESC,rowid DESC LIMIT 100"
    };
    let mut stmt = db.prepare(query).map_err(io)?;
    let rows = stmt
        .query_map([id.as_str()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
            ))
        })
        .map_err(io)?;
    rows.map(|row| {
        let (execution, started, state, plan, report) = row.map_err(io)?;
        Ok(Run {
            execution,
            started,
            state,
            plan: serde_json::from_str(&plan).map_err(io)?,
            report: report
                .map(|r| serde_json::from_str(&r))
                .transpose()
                .map_err(io)?,
        })
    })
    .collect()
}

pub(super) fn acknowledge(root: &Path, id: &ProfileId) -> Result<(), AppError> {
    let report = serde_json::json!({"code":"interruption_acknowledged", "profile":id});
    open(root)?
        .execute(
            "UPDATE runs SET state='error',report=?2 WHERE profile=?1 AND state='running'",
            params![id.as_str(), report.to_string()],
        )
        .map_err(io)?;
    Ok(())
}

pub(super) fn unfinished(root: &Path, id: &ProfileId) -> Result<bool, AppError> {
    open(root)?
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM runs WHERE profile=?1 AND state='running')",
            [id.as_str()],
            |r| r.get(0),
        )
        .map_err(io)
}

#[cfg(test)]
mod retention_tests {
    use super::*;
    type Failure = Box<dyn std::error::Error>;
    fn fixture() -> Result<tempfile::TempDir, Failure> {
        let root = tempfile::tempdir()?;
        let db = open(root.path())?;
        for n in 0..103 {
            let id = format!("run-{n}");
            db.execute(
                "INSERT INTO runs VALUES(?1,'p','0','finished','{}','{}')",
                [&id],
            )?;
            std::fs::write(
                root.path().join(format!("{id}.jsonl")),
                b"old completed journal",
            )?;
        }
        db.execute(
            "INSERT INTO runs VALUES('run-current','p','0','running','{}',NULL)",
            [],
        )?;
        db.execute(
            "INSERT INTO runs VALUES('run-interrupted','p','0','running','{}',NULL)",
            [],
        )?;
        std::fs::write(root.path().join("run-interrupted.jsonl"), b"diagnose")?;
        std::fs::write(root.path().join("foreign.jsonl"), b"keep")?;
        Ok(root)
    }
    #[test]
    fn completed_journals_follow_retention_but_interrupted_and_foreign_stay() -> Result<(), Failure>
    {
        let root = fixture()?;
        finish(
            root.path(),
            &ExecutionId::new("run-current")?,
            &Ok(serde_json::json!({})),
            None,
        )?;
        for n in 0..4 {
            assert!(!root.path().join(format!("run-{n}.jsonl")).exists());
        }
        assert!(root.path().join("run-4.jsonl").exists());
        assert_eq!(
            std::fs::read(root.path().join("run-interrupted.jsonl"))?,
            b"diagnose"
        );
        assert_eq!(std::fs::read(root.path().join("foreign.jsonl"))?, b"keep");
        Ok(())
    }
    #[test]
    fn retention_refuses_malicious_identity_and_reports_cleanup_failure() -> Result<(), Failure> {
        let root = fixture()?;
        open(root.path())?.execute(
            "UPDATE runs SET execution='../outside' WHERE execution='run-0'",
            [],
        )?;
        let id = ExecutionId::new("run-current")?;
        assert_eq!(
            finish(root.path(), &id, &Ok(serde_json::json!({})), None)
                .err()
                .ok_or("error")?
                .code,
            "integrity"
        );
        open(root.path())?.execute(
            "UPDATE runs SET execution='run-0' WHERE execution='../outside'",
            [],
        )?;
        std::fs::remove_file(root.path().join("run-0.jsonl"))?;
        std::fs::create_dir(root.path().join("run-0.jsonl"))?;
        assert_eq!(
            finish(root.path(), &id, &Ok(serde_json::json!({})), None)
                .err()
                .ok_or("error")?
                .code,
            "journal_retention"
        );
        assert!(unfinished(root.path(), &ProfileId::new("p")?)?);
        assert!(root.path().join("run-interrupted.jsonl").exists());
        Ok(())
    }
}
