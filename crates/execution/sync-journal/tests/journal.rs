//! Journal durable : écriture, relecture, reprise.

#![expect(
    clippy::unwrap_used,
    reason = "aides de test : un échec est un échec de test"
)]

use sync_core::{ExecutionId, ExecutionStatus, OperationId, PlanId, ResourcePath};
use sync_journal::{
    FileJournal, JournalError, JournalRecord, JournalSink, JournalTotals, MemoryJournal,
    OutcomeCode, parse_journal, read_journal,
};
use sync_plan::Action;
use tempfile::TempDir;

fn op(n: u32) -> OperationId {
    OperationId::new(format!("op-{n:06}")).unwrap()
}

fn p(value: &str) -> ResourcePath {
    ResourcePath::parse(value).unwrap()
}

fn started() -> JournalRecord {
    JournalRecord::ExecutionStarted {
        execution: ExecutionId::new("exec-1").unwrap(),
        plan: PlanId::new("plan-1").unwrap(),
        profile: None,
        operations: 3,
        mutations: 2,
        source: "C:/source".to_owned(),
        destination: "C:/destination".to_owned(),
        versions: None,
    }
}

fn finished(n: u32, path: &str, outcome: OutcomeCode, bytes: u64) -> JournalRecord {
    JournalRecord::OperationFinished {
        operation: op(n),
        path: p(path),
        action: Action::Copy,
        outcome,
        bytes,
        error_class: None,
        detail: None,
    }
}

fn interrupted_run(journal: &mut dyn JournalSink) {
    journal.append(started()).unwrap();
    journal
        .append(JournalRecord::OperationStarted {
            operation: op(1),
            path: p("a.txt"),
            action: Action::Copy,
        })
        .unwrap();
    journal
        .append(finished(1, "a.txt", OutcomeCode::Done, 5))
        .unwrap();
    journal
        .append(JournalRecord::OperationStarted {
            operation: op(2),
            path: p("b.txt"),
            action: Action::Replace,
        })
        .unwrap();
    journal
        .append(JournalRecord::StagingCreated {
            operation: op(2),
            staging: p(".syncengine-1-2-3.partial"),
        })
        .unwrap();
}

#[test]
fn file_journal_round_trip_and_recovery_of_an_interrupted_run() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("exec.jsonl");
    let mut journal = FileJournal::create(&path).unwrap();
    interrupted_run(&mut journal);
    drop(journal);

    let replay = read_journal(&path).unwrap();
    assert_eq!(replay.lines().len(), 5);
    assert!(!replay.truncated_tail());
    let report = replay.recovery();
    assert!(report.is_interrupted());
    assert_eq!(report.execution.unwrap().as_str(), "exec-1");
    assert_eq!(report.mutations_planned, 2);
    assert_eq!(report.totals.done, 1);
    assert_eq!(report.totals.bytes, 5);
    assert_eq!(report.in_flight.len(), 1);
    let in_flight = &report.in_flight[0];
    assert_eq!(in_flight.path, p("b.txt"));
    assert_eq!(in_flight.action, Action::Replace);
    assert_eq!(
        in_flight.staging.as_ref().unwrap().as_str(),
        ".syncengine-1-2-3.partial"
    );
}

#[test]
fn completed_run_is_not_interrupted() {
    let mut journal = MemoryJournal::new();
    journal.append(started()).unwrap();
    journal
        .append(finished(1, "a.txt", OutcomeCode::NoAction, 0))
        .unwrap();
    journal
        .append(JournalRecord::ExecutionFinished {
            status: ExecutionStatus::Succeeded,
            totals: JournalTotals {
                no_action: 1,
                ..JournalTotals::default()
            },
        })
        .unwrap();
    let report = journal.replay().recovery();
    assert_eq!(report.finished, Some(ExecutionStatus::Succeeded));
    assert!(report.in_flight.is_empty());
    assert_eq!(report.totals.no_action, 1);
}

#[test]
fn an_existing_journal_is_never_overwritten() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("exec.jsonl");
    std::fs::write(&path, b"precieux").unwrap();
    assert!(matches!(
        FileJournal::create(&path),
        Err(JournalError::AlreadyExists)
    ));
    assert_eq!(std::fs::read(&path).unwrap(), b"precieux");
}

#[test]
fn truncated_last_line_is_tolerated_but_corruption_is_not() {
    let mut journal = MemoryJournal::new();
    interrupted_run(&mut journal);
    let mut text = String::new();
    for line in journal.lines() {
        text.push_str(&serde_json::to_string(line).unwrap());
        text.push('\n');
    }
    // Arrêt pendant l'écriture de la ligne suivante.
    let truncated = format!("{text}{{\"format_version\":1,\"seq");
    let replay = parse_journal(&truncated).unwrap();
    assert!(replay.truncated_tail());
    assert_eq!(replay.lines().len(), 5);
    assert!(replay.recovery().truncated_tail);

    // Ligne illisible au milieu : refus.
    let corrupt = text.replacen("\"sequence\":1", "\"sequence\":1,,", 1);
    assert!(matches!(
        parse_journal(&corrupt),
        Err(JournalError::Corrupt { line: 2 })
    ));
    // Ligne perdue : séquence non consécutive.
    let mut kept: Vec<&str> = text.lines().collect();
    kept.remove(1);
    let gap = kept.join("\n") + "\n";
    assert!(matches!(
        parse_journal(&gap),
        Err(JournalError::Sequence {
            expected: 1,
            found: 2
        })
    ));
    // Version inconnue : refus.
    let future = text.replace("\"format_version\":1", "\"format_version\":9");
    assert!(matches!(
        parse_journal(&future),
        Err(JournalError::UnsupportedVersion { found: 9 })
    ));
}

#[test]
fn failing_memory_journal_reports_an_error() {
    let mut journal = MemoryJournal::failing_after(1);
    journal.append(started()).unwrap();
    assert!(matches!(
        journal.append(finished(1, "a", OutcomeCode::Done, 0)),
        Err(JournalError::Io(_))
    ));
}

#[test]
fn record_format_is_stable() {
    let json = serde_json::to_string(&finished(7, "d/x", OutcomeCode::Refused, 0)).unwrap();
    assert_eq!(
        json,
        r#"{"type":"operation_finished","operation":"op-000007","path":"d/x","action":"copy","outcome":"refused","bytes":0,"error_class":null,"detail":null}"#
    );
}
