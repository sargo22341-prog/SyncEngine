//! Exécution sur stockage mémoire : chemins nominaux, préconditions
//! revérifiées, pannes injectées (disque plein, déconnexion, corruption),
//! annulation, suppressions sous garde-fous.
//!
//! Critères Given/When/Then des cas de sûreté : voir le nom et le premier
//! commentaire de chaque test.

mod common;

use common::*;
use connector_api::{StorageErrorKind, StorageOperation};
use connector_memory::{MemoryConnector, ts};
use sync_core::{CancellationToken, ExecutionId, ExecutionStatus};
use sync_errors::ErrorClass;
use sync_executor::{
    ExecutionError, ExecutionOptions, ExecutionRequest, Executor, OutcomeStatus, PlanExecutor,
    Refusal, StopCause,
};
use sync_filter::{Filter, FilterRules};
use sync_journal::{JournalRecord, MemoryJournal, OutcomeCode};
use sync_plan::{Action, PlanSide, SyncMode};
use sync_verify::{Mismatch, VerifyMode};
use sync_versioning::VersionStore;

fn source() -> MemoryConnector {
    MemoryConnector::new("source")
        .file("a.txt", b"alpha", ts(100))
        .file("docs/b.txt", b"bravo bravo", ts(200))
        .dir("vide")
}

fn no_partial(connector: &MemoryConnector) {
    assert!(
        connector.paths().iter().all(|p| !p.contains(".partial")),
        "fichier temporaire restant : {:?}",
        connector.paths()
    );
}

#[test]
fn backup_copies_files_and_directories_with_their_dates() {
    let source = source();
    let destination = MemoryConnector::new("destination").writable();
    let plan = backup_plan(&source, &destination);
    let mut journal = MemoryJournal::new();
    let report = execute(&plan, &source, &destination, confirmed(), &mut journal).unwrap();

    assert_eq!(report.status(), ExecutionStatus::Succeeded);
    assert_eq!(report.stop(), None);
    assert_eq!(report.totals().done, 4);
    assert_eq!(report.totals().bytes, 16);
    assert_eq!(destination.content("a.txt").unwrap(), b"alpha");
    assert_eq!(destination.content("docs/b.txt").unwrap(), b"bravo bravo");
    assert!(destination.entry_at("vide").is_some());
    assert_eq!(
        destination
            .entry_at("docs/b.txt")
            .unwrap()
            .metadata()
            .modified(),
        Some(ts(200))
    );
    no_partial(&destination);
    // La source n'est jamais mutée.
    assert_eq!(source.mutations(), 0);

    let recovery = journal.replay().recovery();
    assert_eq!(recovery.finished, Some(ExecutionStatus::Succeeded));
    assert!(recovery.in_flight.is_empty());
    assert_eq!(recovery.totals.done, 4);

    // Rejouer : dates conservées, donc plus rien à faire.
    let again = backup_plan(&source, &destination);
    assert_eq!(again.summary().mutations(), 0, "{again:?}");
}

#[test]
fn source_modified_after_planning_is_refused() {
    // Given un plan qui copie a.txt ; When la source est modifiée avant
    // l'exécution ; Then la copie est refusée et rien n'est écrit pour a.txt.
    let source = source();
    let destination = MemoryConnector::new("destination").writable();
    let plan = backup_plan(&source, &destination);
    source.put_file("a.txt", b"alpha modifie", ts(150));
    let report = execute(
        &plan,
        &source,
        &destination,
        confirmed(),
        &mut MemoryJournal::new(),
    )
    .unwrap();
    assert!(matches!(
        refusal(&report, "a.txt"),
        Refusal::SourceChanged {
            mismatch: Mismatch::Size { .. }
        }
    ));
    assert_eq!(destination.content("a.txt"), None);
    assert!(is_done(&report, "docs/b.txt"));
    assert_eq!(report.status(), ExecutionStatus::SucceededWithWarnings);
    no_partial(&destination);
}

#[test]
fn source_modified_during_transfer_is_not_published() {
    // Given une copie en cours ; When la date de la source change pendant
    // la lecture ; Then le dépôt est abandonné, rien n'est publié.
    let source = source().touch_on_read("a.txt", ts(999));
    let destination = MemoryConnector::new("destination").writable();
    let plan = backup_plan(&source, &destination);
    let report = execute(
        &plan,
        &source,
        &destination,
        confirmed(),
        &mut MemoryJournal::new(),
    )
    .unwrap();
    assert!(matches!(
        refusal(&report, "a.txt"),
        Refusal::SourceChangedDuringTransfer {
            mismatch: Mismatch::Modified { .. }
        }
    ));
    assert_eq!(destination.content("a.txt"), None);
    no_partial(&destination);
}

#[test]
fn recorded_hash_detects_same_size_content_change() {
    // Given un plan avec empreintes ; When le contenu change à taille et
    // date égales ; Then le transfert est refusé (empreinte différente).
    let source = source();
    let destination = MemoryConnector::new("destination").writable();
    let plan = plan_with(
        &source,
        &destination,
        SyncMode::Backup,
        &sync_backup::BackupStrategy::default_policy(),
        &Filter::include_all(),
        true,
    );
    source.put_file("a.txt", b"ALPHA", ts(100));
    let report = execute(
        &plan,
        &source,
        &destination,
        confirmed(),
        &mut MemoryJournal::new(),
    )
    .unwrap();
    assert!(matches!(
        refusal(&report, "a.txt"),
        Refusal::SourceChangedDuringTransfer {
            mismatch: Mismatch::Content { .. }
        }
    ));
    assert_eq!(destination.content("a.txt"), None);
}

#[test]
fn destination_appearing_after_planning_is_never_overwritten() {
    let source = source();
    let destination = MemoryConnector::new("destination").writable();
    let plan = backup_plan(&source, &destination);
    destination.put_file("a.txt", b"ecrit par un tiers", ts(500));
    let report = execute(
        &plan,
        &source,
        &destination,
        confirmed(),
        &mut MemoryJournal::new(),
    )
    .unwrap();
    assert_eq!(refusal(&report, "a.txt"), Refusal::DestinationAppeared);
    assert_eq!(destination.content("a.txt").unwrap(), b"ecrit par un tiers");
}

#[test]
fn replace_preserves_the_previous_version() {
    let source = source();
    let destination =
        MemoryConnector::new("destination")
            .writable()
            .file("a.txt", b"ancien", ts(50));
    let versions = MemoryConnector::new("versions").writable();
    let store = VersionStore::new(&versions, p("exec-test")).unwrap();
    let plan = mirror_plan(&source, &destination);
    assert_eq!(plan.get(&p("a.txt")).unwrap().action(), Action::Replace);
    let mut journal = MemoryJournal::new();
    let report = execute_with_versions(&plan, &source, &destination, &store, &mut journal).unwrap();
    assert_eq!(report.status(), ExecutionStatus::Succeeded);
    assert_eq!(destination.content("a.txt").unwrap(), b"alpha");
    assert_eq!(
        status(&report, "a.txt"),
        &OutcomeStatus::Done {
            bytes: 5,
            version: Some(p("exec-test/a.txt"))
        }
    );
    assert_eq!(versions.content("exec-test/a.txt").unwrap(), b"ancien");
    assert!(journal.lines().iter().any(|line| matches!(
        line.record(),
        JournalRecord::VersionPreserved { location, .. } if location.as_str() == "exec-test/a.txt"
    )));
}

#[test]
fn destination_changed_before_replace_is_refused() {
    let source = source();
    let destination =
        MemoryConnector::new("destination")
            .writable()
            .file("a.txt", b"ancien", ts(50));
    let plan = mirror_plan(&source, &destination);
    destination.put_file("a.txt", b"retouche par un tiers", ts(60));
    let report = execute(
        &plan,
        &source,
        &destination,
        confirmed(),
        &mut MemoryJournal::new(),
    )
    .unwrap();
    assert!(matches!(
        refusal(&report, "a.txt"),
        Refusal::DestinationChanged { .. }
    ));
    assert_eq!(
        destination.content("a.txt").unwrap(),
        b"retouche par un tiers"
    );
}

#[test]
fn disk_full_stops_the_run_without_deleting_anything() {
    // Given un miroir avec copies et une suppression confirmée ; When le
    // disque se remplit pendant une copie ; Then l'exécution s'arrête
    // (insufficient_space), aucun dépôt ne reste, les opérations suivantes
    // ne commencent pas et aucune suppression n'a lieu.
    let source = MemoryConnector::new("source")
        .file("1.bin", &[1u8; 400], ts(10))
        .file("2.bin", &[2u8; 400], ts(10))
        .file("3.bin", &[3u8; 400], ts(10));
    let destination = MemoryConnector::new("destination")
        .writable()
        .file("obsolete.txt", b"x", ts(5))
        .with_space_limit(700);
    let plan = mirror_plan(&source, &destination);
    assert_eq!(plan.summary().delete, 1);
    let mut journal = MemoryJournal::new();
    let report = execute(&plan, &source, &destination, confirmed(), &mut journal).unwrap();

    assert_eq!(report.status(), ExecutionStatus::Failed);
    assert_eq!(report.stop(), Some(StopCause::InsufficientSpace));
    assert!(is_done(&report, "1.bin"));
    match status(&report, "2.bin") {
        OutcomeStatus::Failed { error } => assert_eq!(error.class(), ErrorClass::ResourceExhausted),
        other => panic!("{other:?}"),
    }
    assert_eq!(status(&report, "3.bin"), &OutcomeStatus::NotStarted);
    assert_eq!(status(&report, "obsolete.txt"), &OutcomeStatus::NotStarted);
    assert_eq!(destination.content("obsolete.txt").unwrap(), b"x");
    assert_eq!(destination.content("2.bin"), None);
    no_partial(&destination);
    assert_eq!(
        journal.replay().recovery().finished,
        Some(ExecutionStatus::Failed)
    );
}

#[test]
fn disk_full_during_replace_keeps_the_original() {
    let source = MemoryConnector::new("source").file("a.bin", &[7u8; 600], ts(10));
    let destination = MemoryConnector::new("destination")
        .writable()
        .file("a.bin", &[1u8; 300], ts(5))
        .with_space_limit(800);
    let plan = mirror_plan(&source, &destination);
    let report = execute(
        &plan,
        &source,
        &destination,
        confirmed(),
        &mut MemoryJournal::new(),
    )
    .unwrap();
    assert_eq!(report.stop(), Some(StopCause::InsufficientSpace));
    assert_eq!(destination.content("a.bin").unwrap(), vec![1u8; 300]);
    no_partial(&destination);
}

#[test]
fn source_disconnection_mid_transfer_stops_and_blocks_deletions() {
    let source = MemoryConnector::new("source")
        .file("a.bin", &[1u8; 5000], ts(10))
        .file("b.bin", b"b", ts(10))
        .fail_read_after("a.bin", 2048, StorageErrorKind::Disconnected);
    let destination =
        MemoryConnector::new("destination")
            .writable()
            .file("obsolete.txt", b"x", ts(5));
    let plan = mirror_plan(&source, &destination);
    let report = execute(
        &plan,
        &source,
        &destination,
        confirmed(),
        &mut MemoryJournal::new(),
    )
    .unwrap();
    assert_eq!(report.stop(), Some(StopCause::SourceUnavailable));
    assert_eq!(report.status(), ExecutionStatus::Failed);
    assert_eq!(status(&report, "b.bin"), &OutcomeStatus::NotStarted);
    assert_eq!(destination.content("obsolete.txt").unwrap(), b"x");
    assert_eq!(destination.content("a.bin"), None);
    no_partial(&destination);
}

#[test]
fn write_failure_on_one_file_does_not_stop_the_run() {
    let source = source();
    let destination = MemoryConnector::new("destination").writable().fail(
        "a.txt",
        StorageOperation::Write,
        StorageErrorKind::PermissionDenied,
    );
    let plan = backup_plan(&source, &destination);
    let report = execute(
        &plan,
        &source,
        &destination,
        confirmed(),
        &mut MemoryJournal::new(),
    )
    .unwrap();
    assert!(matches!(
        status(&report, "a.txt"),
        OutcomeStatus::Failed { .. }
    ));
    assert!(is_done(&report, "docs/b.txt"));
    assert_eq!(report.stop(), None);
    assert_eq!(report.status(), ExecutionStatus::Failed);
}

#[test]
fn silent_corruption_is_caught_by_content_verification() {
    let source = source();
    let destination = MemoryConnector::new("destination")
        .writable()
        .corrupt_writes_to("a.txt");
    let plan = backup_plan(&source, &destination);
    let report = execute(
        &plan,
        &source,
        &destination,
        confirmed(),
        &mut MemoryJournal::new(),
    )
    .unwrap();
    match status(&report, "a.txt") {
        OutcomeStatus::Failed { error } => assert_eq!(error.class(), ErrorClass::Integrity),
        other => panic!("{other:?}"),
    }
    assert_eq!(destination.content("a.txt"), None);
    no_partial(&destination);

    // En mode `size`, la corruption n'est pas détectée : limite documentée.
    let destination = MemoryConnector::new("destination")
        .writable()
        .corrupt_writes_to("a.txt");
    let report = execute(
        &plan,
        &source,
        &destination,
        confirmed().with_verify(VerifyMode::Size),
        &mut MemoryJournal::new(),
    )
    .unwrap();
    assert!(is_done(&report, "a.txt"));
    assert_ne!(destination.content("a.txt").unwrap(), b"alpha");
}

#[test]
fn content_verification_rereads_through_the_writer_when_supported() {
    let source = source();
    // The writer rereads its own staging: no reopening by path is needed.
    let destination = MemoryConnector::new("destination")
        .writable()
        .with_read_back();
    let plan = backup_plan(&source, &destination);
    let report = execute(
        &plan,
        &source,
        &destination,
        confirmed(),
        &mut MemoryJournal::new(),
    )
    .unwrap();
    assert!(is_done(&report, "a.txt"));
    assert_eq!(destination.content("a.txt").unwrap(), b"alpha");
    assert_eq!(destination.read_opens(), 0);

    // Same guarantee as the path-based reread: corruption is refused.
    let destination = MemoryConnector::new("destination")
        .writable()
        .with_read_back()
        .corrupt_writes_to("a.txt");
    let report = execute(
        &plan,
        &source,
        &destination,
        confirmed(),
        &mut MemoryJournal::new(),
    )
    .unwrap();
    match status(&report, "a.txt") {
        OutcomeStatus::Failed { error } => assert_eq!(error.class(), ErrorClass::Integrity),
        other => panic!("{other:?}"),
    }
    assert_eq!(destination.content("a.txt"), None);
    no_partial(&destination);
    assert_eq!(destination.read_opens(), 0);
}

#[test]
fn cancellation_stops_between_operations_and_reports_partial_state() {
    let source = MemoryConnector::new("source")
        .file("1.txt", b"1", ts(10))
        .file("2.txt", b"2", ts(10))
        .file("3.txt", b"3", ts(10));
    let destination =
        MemoryConnector::new("destination")
            .writable()
            .file("obsolete.txt", b"x", ts(5));
    let plan = mirror_plan(&source, &destination);
    let cancel = CancellationToken::new();
    let recorder = Recorder::new(Some((1, cancel.clone())));
    let emitter = recorder.emitter();
    let mut journal = MemoryJournal::new();
    let report = Executor::new()
        .execute(
            ExecutionRequest::new(
                &plan,
                &source,
                &destination,
                ExecutionId::new("exec-test").unwrap(),
                &mut journal,
                &cancel,
            )
            .with_options(confirmed())
            .with_events(&emitter)
            .at(now()),
        )
        .unwrap();
    assert_eq!(report.status(), ExecutionStatus::Cancelled);
    assert_eq!(report.stop(), Some(StopCause::Cancelled));
    assert!(is_done(&report, "1.txt"));
    assert_eq!(status(&report, "2.txt"), &OutcomeStatus::NotStarted);
    assert_eq!(status(&report, "obsolete.txt"), &OutcomeStatus::NotStarted);
    assert_eq!(destination.content("obsolete.txt").unwrap(), b"x");
    assert_eq!(destination.content("2.txt"), None);
    let recovery = journal.replay().recovery();
    assert_eq!(recovery.finished, Some(ExecutionStatus::Cancelled));
    assert_eq!(recovery.totals.done, 1);

    // Annulation déjà demandée : refus avant toute mutation.
    let error = Executor::new()
        .execute(
            ExecutionRequest::new(
                &plan,
                &source,
                &destination,
                ExecutionId::new("exec-2").unwrap(),
                &mut MemoryJournal::new(),
                &cancel,
            )
            .at(now()),
        )
        .unwrap_err();
    assert!(matches!(error, ExecutionError::Cancelled));
}

#[test]
fn cancellation_during_a_transfer_leaves_no_staging_file() {
    let source = MemoryConnector::new("source").file("gros.bin", &[9u8; 100_000], ts(10));
    let destination = MemoryConnector::new("destination").writable();
    let plan = backup_plan(&source, &destination);
    let cancel = CancellationToken::new();
    // Annule au premier événement de progression d'octets.
    struct CancelOnBytes(CancellationToken);
    impl sync_events::EventSink for CancelOnBytes {
        fn emit(&self, event: sync_events::Event) -> sync_events::EmitOutcome {
            if let sync_events::EventKind::Progress(progress) = event.kind()
                && progress.bytes_done > 0
            {
                self.0.cancel();
            }
            sync_events::EmitOutcome::Delivered
        }
    }
    let emitter = sync_events::EventEmitter::new(
        ExecutionId::new("exec-test").unwrap(),
        None,
        std::sync::Arc::new(CancelOnBytes(cancel.clone())),
    );
    let report = Executor::new()
        .execute(
            ExecutionRequest::new(
                &plan,
                &source,
                &destination,
                ExecutionId::new("exec-test").unwrap(),
                &mut MemoryJournal::new(),
                &cancel,
            )
            .with_options(confirmed().with_progress_interval(4096))
            .with_events(&emitter)
            .at(now()),
        )
        .unwrap();
    assert_eq!(report.status(), ExecutionStatus::Cancelled);
    assert_eq!(status(&report, "gros.bin"), &OutcomeStatus::Cancelled);
    assert!(destination.paths().is_empty(), "{:?}", destination.paths());
}

#[test]
fn deletions_require_explicit_confirmation() {
    // Given un plan miroir avec suppression ; When l'exécution n'est pas
    // confirmée ; Then refus avant toute mutation (aucune copie non plus).
    let source = source();
    let destination =
        MemoryConnector::new("destination")
            .writable()
            .file("obsolete.txt", b"x", ts(5));
    let plan = mirror_plan(&source, &destination);
    let error = execute(
        &plan,
        &source,
        &destination,
        ExecutionOptions::new(),
        &mut MemoryJournal::new(),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        ExecutionError::DeletionsNotConfirmed { count: 1 }
    ));
    assert_eq!(destination.mutations(), 0);
}

#[test]
fn mirror_deletes_deepest_first_and_keeps_versions() {
    let source = MemoryConnector::new("source").file("garde.txt", b"g", ts(10));
    let destination = MemoryConnector::new("destination")
        .writable()
        .file("garde.txt", b"g", ts(10))
        .file("vieux/sous/x.txt", b"xx", ts(5))
        .file("vieux/y.txt", b"y", ts(5));
    let versions = MemoryConnector::new("versions").writable();
    let store = VersionStore::new(&versions, p("exec-test")).unwrap();
    let plan = mirror_plan(&source, &destination);
    assert_eq!(plan.summary().delete, 4);
    let mut journal = MemoryJournal::new();
    let report = execute_with_versions(&plan, &source, &destination, &store, &mut journal).unwrap();
    assert_eq!(report.status(), ExecutionStatus::Succeeded);
    assert_eq!(destination.paths(), ["garde.txt"]);
    assert_eq!(
        versions.content("exec-test/vieux/sous/x.txt").unwrap(),
        b"xx"
    );
    assert_eq!(versions.content("exec-test/vieux/y.txt").unwrap(), b"y");
    // Ordre : contenu avant contenant.
    let deleted: Vec<String> = journal
        .lines()
        .iter()
        .filter_map(|line| match line.record() {
            JournalRecord::OperationFinished {
                path,
                outcome: OutcomeCode::Done,
                action: Action::Delete,
                ..
            } => Some(path.as_str().to_owned()),
            _ => None,
        })
        .collect();
    assert_eq!(
        deleted,
        ["vieux/y.txt", "vieux/sous/x.txt", "vieux/sous", "vieux"]
    );
}

#[test]
fn deletion_is_refused_when_the_entry_reappears_at_the_source() {
    let source = MemoryConnector::new("source").file("garde.txt", b"g", ts(10));
    let destination = MemoryConnector::new("destination")
        .writable()
        .file("garde.txt", b"g", ts(10))
        .file("x.txt", b"x", ts(5));
    let plan = mirror_plan(&source, &destination);
    source.put_file("x.txt", b"revenu", ts(20));
    let report = execute(
        &plan,
        &source,
        &destination,
        confirmed(),
        &mut MemoryJournal::new(),
    )
    .unwrap();
    assert_eq!(refusal(&report, "x.txt"), Refusal::SourceReappeared);
    assert_eq!(destination.content("x.txt").unwrap(), b"x");
}

#[test]
fn unreadable_source_blocks_every_deletion() {
    // Given deux suppressions prévues ; When l'absence de la première ne
    // peut pas être revérifiée (accès refusé à la source) ; Then aucune
    // suppression n'a lieu.
    let source = MemoryConnector::new("source")
        .file("garde.txt", b"g", ts(10))
        .fail(
            "z.txt",
            StorageOperation::Stat,
            StorageErrorKind::PermissionDenied,
        );
    let destination = MemoryConnector::new("destination")
        .writable()
        .file("garde.txt", b"g", ts(10))
        .file("y.txt", b"y", ts(5))
        .file("z.txt", b"z", ts(5));
    let plan = mirror_plan(&source, &destination);
    assert_eq!(plan.summary().delete, 2);
    let report = execute(
        &plan,
        &source,
        &destination,
        confirmed(),
        &mut MemoryJournal::new(),
    )
    .unwrap();
    assert_eq!(
        refusal(&report, "z.txt"),
        Refusal::SourceUnverifiable {
            class: ErrorClass::PermissionDenied
        }
    );
    assert_eq!(refusal(&report, "y.txt"), Refusal::DeletionsBlocked);
    assert_eq!(destination.paths(), ["garde.txt", "y.txt", "z.txt"]);
}

#[test]
fn incomplete_source_scan_executes_zero_deletions() {
    // Given un dossier source illisible au scan ; When le miroir autorisant
    // les suppressions est exécuté ; Then aucune suppression n'a lieu.
    let source = MemoryConnector::new("source")
        .file("a.txt", b"a", ts(10))
        .file("docs/b.txt", b"b", ts(10))
        .fail(
            "docs",
            StorageOperation::List,
            StorageErrorKind::PermissionDenied,
        );
    let destination = MemoryConnector::new("destination")
        .writable()
        .file("docs/b.txt", b"b", ts(10))
        .file("docs/z.txt", b"z", ts(10))
        .file("obsolete.txt", b"o", ts(10));
    let plan = mirror_plan(&source, &destination);
    assert_eq!(plan.summary().delete, 0);
    assert!(plan.summary().deletions_refused > 0);
    let report = execute(
        &plan,
        &source,
        &destination,
        confirmed(),
        &mut MemoryJournal::new(),
    )
    .unwrap();
    assert!(is_done(&report, "a.txt"));
    assert_eq!(report.status(), ExecutionStatus::SucceededWithWarnings);
    assert!(
        report
            .outcomes()
            .iter()
            .all(|outcome| outcome.action() != Action::Delete)
    );
    assert_eq!(
        destination.paths(),
        ["a.txt", "docs", "docs/b.txt", "docs/z.txt", "obsolete.txt"]
    );
}

#[test]
fn directory_keeping_unplanned_content_is_never_removed() {
    // Given un dossier à supprimer contenant un fichier exclu par le filtre ;
    // Then le dossier est refusé (non vide) et le fichier reste.
    let source = MemoryConnector::new("source").file("garde.txt", b"g", ts(10));
    let destination = MemoryConnector::new("destination")
        .writable()
        .file("garde.txt", b"g", ts(10))
        .file("vieux/a.txt", b"a", ts(5))
        .file("vieux/notes.keep", b"k", ts(5));
    let filter = Filter::new(FilterRules::new().exclude_file("*.keep")).unwrap();
    let plan = plan_with(
        &source,
        &destination,
        SyncMode::Mirror,
        &mirror_policy(),
        &filter,
        false,
    );
    let report = execute(
        &plan,
        &source,
        &destination,
        confirmed(),
        &mut MemoryJournal::new(),
    )
    .unwrap();
    assert!(is_done(&report, "vieux/a.txt"));
    assert_eq!(refusal(&report, "vieux"), Refusal::DirectoryNotEmpty);
    assert_eq!(destination.content("vieux/notes.keep").unwrap(), b"k");
}

#[test]
fn failed_parent_creation_blocks_its_content() {
    let source = MemoryConnector::new("source").file("d/f.txt", b"f", ts(10));
    let destination = MemoryConnector::new("destination").writable();
    let plan = backup_plan(&source, &destination);
    // Un fichier occupe le nom du dossier à créer.
    destination.put_file("d", b"intrus", ts(1));
    let report = execute(
        &plan,
        &source,
        &destination,
        confirmed(),
        &mut MemoryJournal::new(),
    )
    .unwrap();
    assert_eq!(refusal(&report, "d"), Refusal::DestinationAppeared);
    assert!(matches!(
        refusal(&report, "d/f.txt"),
        Refusal::DependencyNotDone { .. }
    ));
}

#[test]
fn preflight_refuses_before_any_mutation() {
    let source = source();
    let destination = MemoryConnector::new("destination").writable();
    let plan = backup_plan(&source, &destination);
    let run = |at, source: &MemoryConnector, destination: &MemoryConnector| {
        Executor::new().execute(
            ExecutionRequest::new(
                &plan,
                source,
                destination,
                ExecutionId::new("e").unwrap(),
                &mut MemoryJournal::new(),
                &CancellationToken::new(),
            )
            .with_options(confirmed())
            .at(at),
        )
    };
    assert!(matches!(
        run(ts(NOW + 7200), &source, &destination),
        Err(ExecutionError::PlanExpired { .. })
    ));
    assert!(matches!(
        run(ts(NOW - 3600), &source, &destination),
        Err(ExecutionError::PlanFromFuture)
    ));
    let other = MemoryConnector::new("autre").writable();
    assert!(matches!(
        run(now(), &source, &other),
        Err(ExecutionError::ConnectorMismatch {
            side: PlanSide::Destination,
            ..
        })
    ));
    let read_only = MemoryConnector::new("destination");
    assert!(matches!(
        run(now(), &source, &read_only),
        Err(ExecutionError::CapabilityMissing {
            side: PlanSide::Destination,
            ..
        })
    ));
    source.set_disconnected(true);
    assert!(matches!(
        run(now(), &source, &destination),
        Err(ExecutionError::EndpointUnavailable {
            side: PlanSide::Source,
            ..
        })
    ));
    assert_eq!(destination.mutations(), 0);
    assert!(destination.paths().is_empty());
}

#[test]
fn journal_failure_stops_before_untraced_mutations() {
    let source = source();
    let destination = MemoryConnector::new("destination").writable();
    let plan = backup_plan(&source, &destination);
    // Début consigné, puis le journal devient inutilisable.
    let mut journal = MemoryJournal::failing_after(1);
    let report = execute(&plan, &source, &destination, confirmed(), &mut journal).unwrap();
    assert_eq!(report.stop(), Some(StopCause::JournalFailure));
    assert_eq!(report.status(), ExecutionStatus::Failed);
    assert_eq!(destination.mutations(), 0);

    let mut journal = MemoryJournal::failing_after(0);
    assert!(matches!(
        execute(&plan, &source, &destination, confirmed(), &mut journal),
        Err(ExecutionError::Journal(_))
    ));
}

#[test]
fn progress_events_are_aggregated() {
    let source = MemoryConnector::new("source").file("gros.bin", &[1u8; 50_000], ts(10));
    let destination = MemoryConnector::new("destination").writable();
    let plan = backup_plan(&source, &destination);
    let recorder = Recorder::new(None);
    let emitter = recorder.emitter();
    Executor::new()
        .execute(
            ExecutionRequest::new(
                &plan,
                &source,
                &destination,
                ExecutionId::new("exec-test").unwrap(),
                &mut MemoryJournal::new(),
                &CancellationToken::new(),
            )
            .with_options(confirmed().with_progress_interval(16_384))
            .with_events(&emitter)
            .at(now()),
        )
        .unwrap();
    let events = recorder.events();
    let progress: Vec<_> = events
        .iter()
        .filter_map(|event| match event.kind() {
            sync_events::EventKind::Progress(progress) => Some(*progress),
            _ => None,
        })
        .collect();
    // 50 000 octets par blocs de 1 Kio : 3 événements d'octets, pas 49.
    assert!(progress.len() <= 6, "{progress:?}");
    let last = progress.last().unwrap();
    assert_eq!(last.bytes_done, 50_000);
    assert_eq!(last.bytes_total, Some(50_000));
    assert_eq!((last.entries_done, last.entries_total), (1, Some(1)));
    let statuses: Vec<ExecutionStatus> = events
        .iter()
        .filter_map(|event| match event.kind() {
            sync_events::EventKind::StatusChanged { status } => Some(*status),
            _ => None,
        })
        .collect();
    assert_eq!(
        statuses,
        [ExecutionStatus::Running, ExecutionStatus::Succeeded]
    );
}
