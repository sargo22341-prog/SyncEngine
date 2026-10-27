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
/// Completed attempts kept per profile, whatever their size.
const KEPT_RUNS: i64 = 100;
/// Most recent completed attempts kept per profile, whatever the byte budget.
const MIN_KEPT_RUNS: i64 = 3;
/// Stored plan and report bytes per profile beyond which older completed
/// attempts are retired. A plan holds every inventoried entry, so a large
/// mirror otherwise grows the database by hundreds of MiB per run.
const PROFILE_BUDGET: i64 = 512 * 1024 * 1024;

/// Bounded presentation summary of a report. `report` is a SQL expression for
/// its JSON text, `deletions` one for its count of refused or disabled deletions.
fn summary(report: &str, deletions: &str) -> String {
    format!(
        "json_object('status',COALESCE(json_extract({report},'$.status'),json_extract({report},'$.execution.status')),'verification_error',substr(json_extract({report},'$.verification_error'),1,4096),'baseline_updated',json_extract({report},'$.baseline_updated'),'code',json_extract({report},'$.code'),'message',substr(json_extract({report},'$.message'),1,4096),'totals',COALESCE(json_extract({report},'$.totals'),json_extract({report},'$.execution.totals')),'deletions_refused',{deletions})"
    )
}
/// Deletions refused or disabled by a stored plan (walks the whole plan).
fn stored_deletions(plan: &str) -> String {
    format!(
        "(SELECT count(*) FROM json_each({plan},'$.operations') WHERE json_extract(value,'$.reason.code') IN ('deletion_disabled','deletion_refused'))"
    )
}
/// Recomputes the overview of the selected attempts from their stored JSON.
fn refresh_overview(
    db: &Connection,
    condition: &str,
    params: impl rusqlite::Params,
) -> Result<(), AppError> {
    db.execute(
        &format!(
            "INSERT OR REPLACE INTO run_overview SELECT execution,length(CAST(plan AS BLOB))+COALESCE(length(CAST(report AS BLOB)),0),CASE WHEN report IS NULL THEN NULL ELSE {} END FROM runs WHERE {condition}",
            summary("report", &stored_deletions("runs.plan"))
        ),
        params,
    )
    .map_err(io)?;
    Ok(())
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
    if version == 3 && app == 1_398_361_681 {
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
    } else if !matches!(version, 1..=3) || app != 1_398_361_681 {
        return Err(error("integrity", "version historique inconnue"));
    }
    if version < 2 {
        tx.execute_batch(include_str!("../migrations/002_problem_index.sql"))
            .map_err(io)?;
    }
    if version < 3 {
        // One-time walk of the saved JSON; later overviews read only this table.
        tx.execute_batch(include_str!("../migrations/003_run_overview.sql"))
            .map_err(io)?;
        refresh_overview(&tx, "1", [])?;
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

/// Free space worth giving back to the file system (and not re-reading).
const COMPACT_FREE_BYTES: i64 = 256 * 1024 * 1024;

/// Rewrites the file once retired plans leave it mostly empty. SQLite reuses
/// free pages but never shrinks the file by itself. Another open connection
/// makes VACUUM busy: it is then retried at the next opening, the database
/// staying valid either way.
fn compact(db: &Connection) -> Result<(), AppError> {
    let pages: i64 = db
        .query_row("PRAGMA page_count", [], |r| r.get(0))
        .map_err(io)?;
    let free: i64 = db
        .query_row("PRAGMA freelist_count", [], |r| r.get(0))
        .map_err(io)?;
    let size: i64 = db
        .query_row("PRAGMA page_size", [], |r| r.get(0))
        .map_err(io)?;
    if free.saturating_mul(size) < COMPACT_FREE_BYTES || free.saturating_mul(2) < pages {
        return Ok(());
    }
    match db.execute_batch("VACUUM") {
        Ok(()) => Ok(()),
        Err(rusqlite::Error::SqliteFailure(code, _))
            if matches!(
                code.code,
                rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
            ) =>
        {
            Ok(())
        }
        Err(e) => Err(io(e)),
    }
}

/// Opens, maintains and checks the attempt history. With the identities of
/// the saved profiles, completed attempts of removed profiles (unreachable
/// from any view) are retired; interrupted ones stay for diagnosis.
pub(super) fn initialize(root: &Path, profiles: Option<&[ProfileId]>) -> Result<(), AppError> {
    let mut db = open(root)?;
    if let Some(profiles) = profiles {
        let known = serde_json::to_string(profiles).map_err(io)?;
        let tx = db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(io)?;
        let orphans: Vec<String> = {
            let mut query = tx
                .prepare("SELECT execution FROM runs WHERE state!='running' AND profile NOT IN (SELECT value FROM json_each(?1))")
                .map_err(io)?;
            query
                .query_map([known], |row| row.get(0))
                .map_err(io)?
                .collect::<Result<_, _>>()
                .map_err(io)?
        };
        retire(&tx, root, &orphans)?;
        tx.commit().map_err(io)?;
    }
    compact(&db)?;
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
    // Compact JSON: same model as `ExecutionPlan::to_json`, without the
    // indentation that nearly doubles a large plan.
    let plan = serde_json::to_string(plan).map_err(io)?;
    let mut db = open(root)?;
    let tx = db.transaction().map_err(io)?;
    tx.execute(
        "INSERT INTO runs VALUES(?1,?2,?3,'running',?4,NULL)",
        params![
            id.as_str(),
            profile.as_str(),
            serde_json::to_string(&Timestamp::now().map_err(io)?).map_err(io)?,
            plan
        ],
    )
    .map_err(io)?;
    tx.execute(
        "INSERT INTO run_overview VALUES(?1,?2,NULL)",
        params![id.as_str(), i64::try_from(plan.len()).map_err(io)?],
    )
    .map_err(io)?;
    tx.commit().map_err(io)
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
    let text = serde_json::to_string(&report).map_err(io)?;
    tx.execute(
        "UPDATE runs SET state=?2,report=?3 WHERE execution=?1",
        params![id.as_str(), state, text],
    )
    .map_err(io)?;
    // Plan bytes were recorded by `begin`; the in-memory plan avoids walking it.
    let deletions = match plan {
        Some(plan) => plan.summary().deletions_refused.to_string(),
        None => "(SELECT count(*) FROM runs,json_each(runs.plan,'$.operations') WHERE runs.execution=?1 AND json_extract(value,'$.reason.code') IN ('deletion_disabled','deletion_refused'))".to_owned(),
    };
    tx.execute(
        &format!(
            "INSERT OR REPLACE INTO run_overview VALUES(?1,COALESCE((SELECT bytes FROM run_overview WHERE execution=?1),0)+?3,{})",
            summary("?2", &deletions)
        ),
        params![id.as_str(), text, i64::try_from(text.len()).map_err(io)?],
    )
    .map_err(io)?;
    crate::history::index_finished(&tx, id.as_str(), plan, &report)?;
    // Keep interrupted/running records; bound completed history per profile,
    // by count and by stored bytes (the newest attempts always stay).
    // Journals follow the same retention. Select only persisted completed
    // identities; never scan/delete arbitrary files.
    let expired: Vec<String> = {
        let mut query = tx.prepare("SELECT execution FROM (SELECT r.execution,row_number() OVER w n,sum(COALESCE(o.bytes,0)) OVER w total FROM runs r LEFT JOIN run_overview o ON o.execution=r.execution WHERE r.profile=(SELECT profile FROM runs WHERE execution=?1) AND r.state!='running' WINDOW w AS (ORDER BY r.rowid DESC)) WHERE n>?2 OR (n>?3 AND total>?4)").map_err(io)?;
        query
            .query_map(
                params![id.as_str(), KEPT_RUNS, MIN_KEPT_RUNS, PROFILE_BUDGET],
                |row| row.get(0),
            )
            .map_err(io)?
            .collect::<Result<_, _>>()
            .map_err(io)?
    };
    retire(&tx, root, &expired)?;
    tx.commit().map_err(io)
}
/// Removes completed attempts selected from the database, with their journals.
fn retire(db: &Connection, root: &Path, expired: &[String]) -> Result<(), AppError> {
    for expired in expired {
        let expired = ExecutionId::new(expired.as_str()).map_err(|e| error("integrity", e))?;
        // ExecutionId excludes separators; the appended extension is a single
        // file in the application state directory. remove_file unlinks symlinks.
        match std::fs::remove_file(root.join(format!("{expired}.jsonl"))) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(error("journal_retention", e)),
        }
    }
    for expired in expired {
        db.execute("DELETE FROM runs WHERE execution=?1", [expired])
            .map_err(io)?;
    }
    Ok(())
}
pub(super) fn list(root: &Path, id: &ProfileId) -> Result<Vec<Run>, AppError> {
    list_view(root, id, false)
}
pub(super) fn overview(root: &Path, id: &ProfileId) -> Result<Vec<Run>, AppError> {
    list_view(root, id, true)
}
fn list_view(root: &Path, id: &ProfileId, overview: bool) -> Result<Vec<Run>, AppError> {
    let db = open(root)?;
    // The overview reads the stored summary; a row without one (written
    // outside `finish`) falls back to walking its saved JSON.
    let query = if overview {
        format!(
            "SELECT r.execution,r.started,r.state,'null',CASE WHEN o.overview IS NOT NULL THEN o.overview WHEN r.report IS NULL THEN NULL ELSE {} END FROM runs r LEFT JOIN run_overview o ON o.execution=r.execution WHERE r.profile=?1 ORDER BY (r.state='running') DESC,r.rowid DESC LIMIT 100",
            summary("r.report", &stored_deletions("r.plan"))
        )
    } else {
        "SELECT execution,started,state,plan,report FROM runs WHERE profile=?1 ORDER BY (state='running') DESC,rowid DESC LIMIT 100".to_owned()
    };
    let mut stmt = db.prepare(&query).map_err(io)?;
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
    let mut db = open(root)?;
    let tx = db.transaction().map_err(io)?;
    let acknowledged: Vec<String> = {
        let mut query = tx
            .prepare("SELECT execution FROM runs WHERE profile=?1 AND state='running'")
            .map_err(io)?;
        query
            .query_map([id.as_str()], |row| row.get(0))
            .map_err(io)?
            .collect::<Result<_, _>>()
            .map_err(io)?
    };
    tx.execute(
        "UPDATE runs SET state='error',report=?2 WHERE profile=?1 AND state='running'",
        params![id.as_str(), report.to_string()],
    )
    .map_err(io)?;
    for execution in &acknowledged {
        refresh_overview(&tx, "execution=?1", [execution])?;
    }
    tx.commit().map_err(io)
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
    #[test]
    fn stored_bytes_retire_older_completed_runs_but_keep_the_newest() -> Result<(), Failure> {
        let root = tempfile::tempdir()?;
        let db = open(root.path())?;
        for n in 0..6 {
            let id = format!("big-{n}");
            db.execute(
                "INSERT INTO runs VALUES(?1,'p','0','finished','{}','{}')",
                [&id],
            )?;
            db.execute(
                "INSERT INTO run_overview VALUES(?1,?2,'{}')",
                params![id, 200 * 1024 * 1024_i64],
            )?;
            std::fs::write(root.path().join(format!("{id}.jsonl")), b"journal")?;
        }
        db.execute(
            "INSERT INTO runs VALUES('other','q','0','finished','{}','{}')",
            [],
        )?;
        db.execute(
            "INSERT INTO run_overview VALUES('other',?1,'{}')",
            [900 * 1024 * 1024_i64],
        )?;
        db.execute(
            "INSERT INTO runs VALUES('current','p','0','running','{}',NULL)",
            [],
        )?;
        // Given 1.2 GiB of completed history, When a run finishes,
        // Then only the three newest attempts of that profile remain.
        finish(
            root.path(),
            &ExecutionId::new("current")?,
            &Ok(serde_json::json!({"status":"completed"})),
            None,
        )?;
        let kept: Vec<String> = db
            .prepare("SELECT execution FROM runs ORDER BY rowid")?
            .query_map([], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        assert_eq!(kept, ["big-4", "big-5", "other", "current"]);
        assert!(!root.path().join("big-3.jsonl").exists());
        assert!(root.path().join("big-4.jsonl").exists());
        let orphans: i64 = db.query_row(
            "SELECT count(*) FROM run_overview WHERE execution NOT IN (SELECT execution FROM runs)",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(orphans, 0);
        Ok(())
    }
}

#[cfg(test)]
mod overview_tests {
    use super::*;
    use serde_json::json;
    type Failure = Box<dyn std::error::Error>;
    #[test]
    fn version_2_history_is_summarized_once_and_read_without_its_plans() -> Result<(), Failure> {
        let root = tempfile::tempdir()?;
        let old = Connection::open(root.path().join("desktop-runs.sqlite"))?;
        old.execute_batch(include_str!("../migrations/001_desktop_runs.sql"))?;
        old.execute_batch(include_str!("../migrations/002_problem_index.sql"))?;
        let plan = json!({"operations":[{"path":"a","action":"skip","reason":{"code":"deletion_refused"}},{"path":"b","action":"keep","reason":{"code":"equivalent"}}]});
        let report = json!({"execution":{"status":"completed","totals":{"done":1}}});
        old.execute(
            "INSERT INTO runs VALUES('e','p','0','finished',?1,?2)",
            params![plan.to_string(), report.to_string()],
        )?;
        old.execute(
            "INSERT INTO runs VALUES('r','p','0','running',?1,NULL)",
            [plan.to_string()],
        )?;
        drop(old);
        let db = open(root.path())?;
        assert_eq!(
            db.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))?,
            3
        );
        assert_eq!(
            db.query_row("SELECT count(*) FROM run_overview", [], |r| r
                .get::<_, i64>(0))?,
            2
        );
        // Corrupting the saved plan proves the overview no longer parses it.
        db.execute("UPDATE runs SET plan='not json' WHERE execution='e'", [])?;
        let runs = overview(root.path(), &ProfileId::new("p")?)?;
        assert_eq!(runs[0].execution, "r");
        assert!(runs[0].report.is_none());
        let done = runs[1].report.as_ref().ok_or("summary")?;
        assert_eq!(done["status"], "completed");
        assert_eq!(done["totals"]["done"], 1);
        assert_eq!(done["deletions_refused"], 1);
        // An acknowledged interruption gets its summary in the same write.
        acknowledge(root.path(), &ProfileId::new("p")?)?;
        let stored: Option<String> = db.query_row(
            "SELECT overview FROM run_overview WHERE execution='r'",
            [],
            |r| r.get(0),
        )?;
        assert!(
            stored
                .ok_or("acknowledged summary")?
                .contains("interruption_acknowledged")
        );
        Ok(())
    }
    #[test]
    fn removed_profiles_lose_completed_history_but_not_interruptions() -> Result<(), Failure> {
        let root = tempfile::tempdir()?;
        let db = open(root.path())?;
        for (execution, profile, state) in [
            ("kept", "p", "finished"),
            ("orphan", "gone", "finished"),
            ("interrupted", "gone", "running"),
        ] {
            db.execute(
                "INSERT INTO runs VALUES(?1,?2,'0',?3,'{}','{}')",
                params![execution, profile, state],
            )?;
            std::fs::write(root.path().join(format!("{execution}.jsonl")), b"journal")?;
        }
        // Unknown saved set: nothing is retired.
        initialize(root.path(), None)?;
        assert_eq!(
            db.query_row("SELECT count(*) FROM runs", [], |r| r.get::<_, i64>(0))?,
            3
        );
        initialize(root.path(), Some(&[ProfileId::new("p")?]))?;
        let kept: Vec<String> = db
            .prepare("SELECT execution FROM runs ORDER BY rowid")?
            .query_map([], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        assert_eq!(kept, ["kept", "interrupted"]);
        assert!(!root.path().join("orphan.jsonl").exists());
        assert!(root.path().join("interrupted.jsonl").exists());
        Ok(())
    }
}
