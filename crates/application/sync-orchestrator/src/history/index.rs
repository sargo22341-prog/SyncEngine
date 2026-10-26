//! Indexed projections of immutable completed reports, never execution truth.
use crate::{AppError, error, io};
use rusqlite::{Connection, params};
use serde_json::Value;
use sync_plan::ExecutionPlan;

pub(crate) fn finished(
    db: &Connection,
    execution: &str,
    plan: Option<&ExecutionPlan>,
    report: &Value,
) -> Result<(), AppError> {
    db.execute("DELETE FROM problem_rows WHERE execution=?1", [execution])
        .map_err(io)?;
    db.execute(
        "DELETE FROM problem_indexed WHERE execution=?1",
        [execution],
    )
    .map_err(io)?;
    let report = report
        .get("execution")
        .filter(|v| v.is_object())
        .unwrap_or(report);
    let (Some(plan), Some(outcomes)) = (plan, report["outcomes"].as_array()) else {
        return Ok(());
    };
    if plan.operations().len() != outcomes.len() {
        return Err(error("integrity", "nombre de résultats incohérent"));
    }
    let mut insert = db
        .prepare("INSERT INTO problem_rows VALUES(?1,?2,?3,?4,?5,?6,?7)")
        .map_err(io)?;
    for (ordinal, (operation, outcome)) in plan.operations().iter().zip(outcomes).enumerate() {
        if outcome["path"].as_str() != Some(operation.path().as_str()) {
            return Err(error("integrity", "chemin du résultat incohérent"));
        }
        let status = outcome["status"]["status"].as_str().unwrap_or("");
        let action = outcome["action"].as_str().unwrap_or("");
        if matches!(
            status,
            "failed" | "refused" | "cancelled" | "not_started" | ""
        ) || matches!(action, "skip" | "conflict")
        {
            insert
                .execute(params![
                    execution,
                    ordinal as i64,
                    action,
                    status,
                    operation.path().as_str(),
                    serde_json::to_string(operation).map_err(io)?,
                    outcome.to_string()
                ])
                .map_err(io)?;
        }
    }
    db.execute("INSERT INTO problem_indexed VALUES(?1)", [execution])
        .map_err(io)?;
    Ok(())
}

pub(super) fn ensure(
    db: &mut Connection,
    profile: &str,
    execution: &str,
) -> Result<bool, AppError> {
    if ready(db, execution)? {
        return Ok(true);
    }
    // Build the legacy projection in TEMP storage without reserving the main
    // writer. Walk the plan once, with integer primary-key probes per ordinal.
    db.execute_batch("CREATE TEMP TABLE history_candidates(ordinal INTEGER PRIMARY KEY,action TEXT,status TEXT,path TEXT,outcome TEXT); CREATE TEMP TABLE history_ready(ordinal INTEGER PRIMARY KEY,action TEXT,status TEXT,path TEXT,operation TEXT NOT NULL,outcome TEXT);").map_err(io)?;
    let rows = super::ROWS.replace("SELECT jsonb(plan) plan,", "SELECT plan,");
    db.execute(&format!("{rows} INSERT INTO history_candidates SELECT ordinal,action,status,path,outcome FROM matches"),params![profile,execution,"problems",""]).map_err(io)?;
    let inserted=db.execute("INSERT INTO history_ready SELECT p.ordinal,p.action,p.status,p.path,CASE WHEN json_extract(j.value,'$.path')=p.path THEN j.value ELSE NULL END,p.outcome FROM runs r CROSS JOIN json_each(r.plan,'$.operations') j CROSS JOIN history_candidates p WHERE r.profile=?1 AND r.execution=?2 AND p.ordinal=CAST(j.key AS INTEGER)",params![profile,execution]).map_err(io)?;
    let expected: i64 = db
        .query_row("SELECT count(*) FROM history_candidates", [], |r| r.get(0))
        .map_err(io)?;
    if i64::try_from(inserted).map_err(io)? != expected {
        return Err(error("integrity", "index historique incomplet"));
    }
    // Cache publication is optional; a busy writer must not deny a valid read.
    db.busy_timeout(std::time::Duration::from_millis(50))
        .map_err(io)?;
    let tx = match db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate) {
        Ok(tx) => tx,
        Err(error) if busy(&error) => return Ok(false),
        Err(error) => return Err(io(error)),
    };
    // Concurrent readers can race only on this first legacy backfill.
    if !ready(&tx, execution)? {
        tx.execute("INSERT INTO problem_rows SELECT ?1,ordinal,action,status,path,operation,outcome FROM history_ready",[execution]).map_err(io)?;
        tx.execute("INSERT INTO problem_indexed VALUES(?1)", [execution])
            .map_err(io)?;
    }
    match tx.commit() {
        Ok(()) => Ok(true),
        Err(error) if busy(&error) => Ok(false),
        Err(error) => Err(io(error)),
    }
}
fn busy(error: &rusqlite::Error) -> bool {
    matches!(error,rusqlite::Error::SqliteFailure(code,_) if matches!(code.code,rusqlite::ErrorCode::DatabaseBusy|rusqlite::ErrorCode::DatabaseLocked))
}
fn ready(db: &Connection, execution: &str) -> Result<bool, AppError> {
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM problem_indexed WHERE execution=?1)",
        [execution],
        |r| r.get(0),
    )
    .map_err(io)
}

pub(super) const MATCHES: &str = "FROM problem_rows WHERE execution=?1 AND (?2='problems' OR (?2='failed' AND status='failed') OR (?2='refused' AND status='refused') OR (?2='unfinished' AND status IN ('cancelled','not_started','')) OR (?2='skipped' AND action IN ('skip','conflict'))) AND (?3='' OR instr(lower(path),lower(?3))>0 OR instr(lower(COALESCE(outcome,'')),lower(?3))>0)";
