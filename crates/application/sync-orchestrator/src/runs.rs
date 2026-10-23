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
    let check: String = db
        .query_row("PRAGMA quick_check", [], |r| r.get(0))
        .map_err(io)?;
    if check != "ok" {
        return Err(error("integrity", "historique corrompu"));
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
    } else if version != 1 || app != 1_398_361_681 {
        return Err(error("integrity", "version historique inconnue"));
    }
    tx.commit().map_err(io)?;
    db.execute_batch("PRAGMA synchronous=FULL;").map_err(io)?;
    Ok(db)
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
    // Keep interrupted/running records; bound completed history per profile.
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
        "SELECT execution,started,state,'null',CASE WHEN report IS NULL THEN NULL ELSE json_object('status',COALESCE(json_extract(report,'$.status'),json_extract(report,'$.execution.status')),'verification_error',substr(json_extract(report,'$.verification_error'),1,4096),'baseline_updated',json_extract(report,'$.baseline_updated'),'code',json_extract(report,'$.code'),'message',substr(json_extract(report,'$.message'),1,4096),'totals',COALESCE(json_extract(report,'$.totals'),json_extract(report,'$.execution.totals'))) END FROM runs WHERE profile=?1 ORDER BY (state='running') DESC,rowid DESC LIMIT 100"
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

pub(super) fn page(
    root: &Path,
    id: &ProfileId,
    execution: &str,
    offset: usize,
) -> Result<serde_json::Value, AppError> {
    let db = open(root)?;
    let total: i64 = db.query_row("SELECT json_array_length(plan,'$.operations') FROM runs WHERE profile=?1 AND execution=?2",params![id.as_str(),execution],|r|r.get(0)).map_err(io)?;
    let mut stmt = db.prepare("SELECT op.value,COALESCE(json_extract(r.report,'$.outcomes['||op.key||']'),json_extract(r.report,'$.execution.outcomes['||op.key||']')) FROM runs r,json_each(r.plan,'$.operations') op WHERE r.profile=?1 AND r.execution=?2 LIMIT 100 OFFSET ?3").map_err(io)?;
    let rows = stmt
        .query_map(params![id.as_str(), execution, offset as i64], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
        })
        .map_err(io)?;
    let mut values = Vec::new();
    for row in rows {
        let (operation, outcome) = row.map_err(io)?;
        values.push(serde_json::json!({"operation":serde_json::from_str::<serde_json::Value>(&operation).map_err(io)?,"outcome":outcome.map(|s|serde_json::from_str::<serde_json::Value>(&s)).transpose().map_err(io)?}));
    }
    Ok(serde_json::json!({"operations":values,"total":total}))
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
