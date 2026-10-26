//! Commandes `execute` (exécution réelle d'un plan) et `journal`
//! (diagnostic d'un journal d'exécution).
//!
//! Composition directe (faute d'orchestrateur) : le plan est calculé comme
//! pour `preview` (ou chargé depuis un fichier `preview --json`), puis
//! exécuté par `sync-executor` avec la source ouverte **en lecture seule**
//! et la destination seule ouverte avec mutations. Les textes français sont
//! produits ici, jamais dans les moteurs.

use std::io::{self, Write};
use std::path::Path;
use std::sync::Arc;

use connector_local::LocalOptions;
use sync_core::{ExecutionId, ExecutionStatus, ResourcePath, Timestamp};
use sync_events::{EventEmitter, EventKind, bounded_channel};
use sync_executor::{
    ExecutionOptions, ExecutionReport, ExecutionRequest, Executor, OutcomeStatus, PlanExecutor,
    StopCause,
};
use sync_journal::{FileJournal, read_journal};
use sync_plan::{Action, ExecutionPlan, SyncMode};
use sync_versioning::VersionStore;

use crate::cli::{DiscoveryArgs, ExecuteArgs, PreviewArgs};
use crate::discovery::{DiscoveryError, connector_for, exit, local_connector};
use crate::preview::{action_label, build_plan, ensure_not_nested, location};

/// `syncengine execute` : renvoie le code de sortie.
pub(crate) fn execute_command(
    out: &mut dyn Write,
    source_root: &Path,
    destination_root: &Path,
    args: &DiscoveryArgs,
    preview: &PreviewArgs,
    execute: &ExecuteArgs,
) -> Result<u8, DiscoveryError> {
    ensure_not_nested(source_root, destination_root)?;
    let lock_root =
        sync_executor::default_lock_root().map_err(|e| DiscoveryError::Execution(e.to_string()))?;
    let _owner = sync_executor::execution_lock(&lock_root)
        .map_err(|e| DiscoveryError::Execution(e.to_string()))?;
    let journal_path = execute
        .journal
        .as_deref()
        .ok_or_else(|| DiscoveryError::Path("« --journal » manquant".to_owned()))?;
    ensure_outside(journal_path, source_root, destination_root, "le journal")?;
    if let Some(versions) = &execute.versions {
        if !versions.is_dir() {
            return Err(DiscoveryError::Path(format!(
                "{} : le dossier des versions doit exister",
                versions.display()
            )));
        }
        ensure_outside(
            versions,
            source_root,
            destination_root,
            "le dossier des versions",
        )?;
    }

    let plan = match &execute.plan {
        Some(plan_path) => load_plan(plan_path, source_root, destination_root)?,
        None => build_plan(source_root, destination_root, args, preview)?.plan,
    };

    let source = connector_for(source_root)?;
    let destination = local_connector(destination_root, LocalOptions::new().with_mutations())?;
    let now = Timestamp::now()
        .map_err(|error| DiscoveryError::Execution(format!("horloge système : {error}")))?;
    let execution_id = ExecutionId::new(format!("exec-{}-{}", now.secs(), std::process::id()))
        .map_err(|error| DiscoveryError::Execution(error.to_string()))?;
    let versions_connector = match &execute.versions {
        Some(dir) => Some(local_connector(dir, LocalOptions::new().with_mutations())?),
        None => None,
    };
    let store = match &versions_connector {
        Some(connector) => {
            let folder = ResourcePath::parse(execution_id.as_str())
                .map_err(|error| DiscoveryError::Execution(error.to_string()))?;
            Some(
                VersionStore::new(connector, folder)
                    .map_err(|error| DiscoveryError::Execution(error.to_string()))?,
            )
        }
        None => None,
    };
    let mut journal = FileJournal::create(journal_path).map_err(|error| {
        DiscoveryError::Execution(format!("journal {} : {error}", journal_path.display()))
    })?;

    let (emitter, printer) = if execute.progress {
        let (sink, receiver) = bounded_channel(64);
        let emitter = EventEmitter::new(execution_id.clone(), None, Arc::new(sink));
        let printer = std::thread::spawn(move || print_progress(&receiver));
        (Some(emitter), Some(printer))
    } else {
        (None, None)
    };

    let mut options = ExecutionOptions::new()
        .with_verify(execute.verify)
        .with_deletions_confirmed(execute.confirm_delete);
    if let Some(workers) = execute.workers {
        options = options.with_copy_workers(workers);
    }
    let cancel = crate::cancellation();
    let mut request = ExecutionRequest::new(
        &plan,
        &source,
        &destination,
        execution_id.clone(),
        &mut journal,
        &cancel,
    )
    .with_options(options)
    .at(now);
    if let Some(store) = &store {
        request = request.with_versions(store);
    }
    if let Some(emitter) = &emitter {
        request = request.with_events(emitter);
    }
    let result = Executor::new().execute(request);
    // Fin des événements : le fil d'affichage se termine.
    drop(emitter);
    if let Some(printer) = printer
        && printer.join().is_err()
    {
        writeln!(
            io::stderr().lock(),
            "syncengine : affichage de la progression interrompu"
        )?;
    }

    let report = match result {
        Ok(report) => report,
        Err(error) => {
            drop(journal);
            // Journal vide créé par cette commande : retiré, pour que le même
            // chemin reste utilisable après correction.
            let removed = std::fs::metadata(journal_path).is_ok_and(|meta| meta.len() == 0)
                && std::fs::remove_file(journal_path).is_ok();
            let note = if removed { "" } else { " (journal conservé)" };
            return Err(DiscoveryError::Execution(format!(
                "exécution refusée, rien n'a été modifié : {error}{note}"
            )));
        }
    };

    if preview.json {
        let json = serde_json::to_string_pretty(&report)
            .map_err(|error| DiscoveryError::Execution(error.to_string()))?;
        writeln!(out, "{json}")?;
    } else {
        write_report(
            out,
            &plan,
            &report,
            journal_path,
            store.as_ref(),
            preview.show_all,
        )?;
    }
    Ok(match report.status() {
        ExecutionStatus::Succeeded => exit::OK,
        ExecutionStatus::SucceededWithWarnings => exit::UNCERTAIN,
        _ => exit::FAILURE,
    })
}

/// Refuse un chemin (journal, versions) situé dans la source ou la
/// destination : il serait inventorié, voire supprimé par un miroir.
fn ensure_outside(
    path: &Path,
    source: &Path,
    destination: &Path,
    what: &str,
) -> Result<(), DiscoveryError> {
    let absolute = std::path::absolute(path)
        .map_err(|error| DiscoveryError::Path(format!("{} : {error}", path.display())))?;
    // Le journal n'existe pas encore : son dossier parent est résolu.
    let resolved = std::fs::canonicalize(&absolute)
        .or_else(|_| {
            absolute
                .parent()
                .map(std::fs::canonicalize)
                .unwrap_or_else(|| Ok(absolute.clone()))
        })
        .unwrap_or(absolute);
    for (root, name) in [(source, "la source"), (destination, "la destination")] {
        if let Ok(root) = std::fs::canonicalize(root)
            && resolved.starts_with(&root)
        {
            return Err(DiscoveryError::Nested(format!(
                "{what} ne peut pas être situé dans {name}"
            )));
        }
    }
    Ok(())
}

/// Charge un plan enregistré et vérifie qu'il désigne les dossiers fournis.
fn load_plan(
    path: &Path,
    source: &Path,
    destination: &Path,
) -> Result<ExecutionPlan, DiscoveryError> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| DiscoveryError::Path(format!("{} : {error}", path.display())))?;
    let plan = ExecutionPlan::from_json(&text).map_err(|error| {
        DiscoveryError::Execution(format!("plan {} refusé : {error}", path.display()))
    })?;
    for (recorded, given, side) in [
        (plan.source().location(), location(source), "source"),
        (
            plan.destination().location(),
            location(destination),
            "destination",
        ),
    ] {
        if recorded != given {
            return Err(DiscoveryError::Execution(format!(
                "le plan désigne la {side} « {recorded} », pas « {given} » : refus"
            )));
        }
    }
    Ok(plan)
}

fn print_progress(receiver: &std::sync::mpsc::Receiver<sync_events::Event>) {
    for event in receiver {
        let line = match event.kind() {
            EventKind::Progress(progress) => {
                let bytes = match progress.bytes_total {
                    Some(total) => format!("{}/{total} o", progress.bytes_done),
                    None => format!("{} o", progress.bytes_done),
                };
                format!(
                    "progression : {}/{} opération(s), {bytes}",
                    progress.entries_done,
                    progress.entries_total.unwrap_or(0)
                )
            }
            EventKind::EventsDropped { count } => {
                format!("progression : {count} événement(s) non affiché(s)")
            }
            _ => continue,
        };
        if writeln!(io::stderr().lock(), "{line}").is_err() {
            break;
        }
    }
}

fn mode_text(mode: SyncMode) -> &'static str {
    match mode {
        SyncMode::Backup => "sauvegarde",
        SyncMode::Mirror => "miroir",
        _ => "mode non reconnu",
    }
}

fn done_label(action: Action, directory: bool) -> &'static str {
    match (action, directory) {
        (Action::Copy, true) => "CRÉÉ",
        (Action::Copy, false) => "COPIÉ",
        (Action::Replace, _) => "REMPLACÉ",
        (Action::Delete, _) => "SUPPRIMÉ",
        _ => "FAIT",
    }
}

fn stop_text(stop: StopCause) -> &'static str {
    match stop {
        StopCause::Cancelled => "annulation demandée",
        StopCause::InsufficientSpace => "espace disque insuffisant à la destination",
        StopCause::SourceUnavailable => "source déconnectée ou illisible",
        StopCause::DestinationUnavailable => "destination déconnectée",
        StopCause::JournalFailure => "journal impossible à écrire",
        _ => "cause non reconnue",
    }
}

fn status_text(status: ExecutionStatus) -> &'static str {
    match status {
        ExecutionStatus::Succeeded => "réussie",
        ExecutionStatus::SucceededWithWarnings => "réussie avec réserves (voir REFUSÉ / plan)",
        ExecutionStatus::Failed => "ÉCHEC (état partiel : voir ci-dessus et le journal)",
        ExecutionStatus::Cancelled => "ANNULÉE (état partiel : voir ci-dessus et le journal)",
        _ => "inachevée",
    }
}

fn write_report(
    out: &mut dyn Write,
    plan: &ExecutionPlan,
    report: &ExecutionReport,
    journal: &Path,
    versions: Option<&VersionStore<'_>>,
    show_all: bool,
) -> io::Result<()> {
    writeln!(out, "EXÉCUTION RÉELLE — la destination est modifiée.")?;
    writeln!(
        out,
        "Plan : {} ({}, exécution {})",
        mode_text(plan.mode()),
        plan.id(),
        report.execution()
    )?;
    writeln!(out, "  source       : {}", plan.source().location())?;
    writeln!(out, "  destination  : {}", plan.destination().location())?;
    writeln!(out, "  journal      : {}", journal.display())?;
    match versions {
        Some(store) => writeln!(out, "  versions     : sous-dossier {}", store.folder())?,
        None => writeln!(out, "  versions     : aucune conservation")?,
    }
    for outcome in report.outcomes() {
        let directory = plan
            .get(outcome.path())
            .is_some_and(|op| op.kind() == sync_core::EntryKind::Directory);
        let suffix = if directory { "/" } else { "" };
        let path = outcome.path();
        let line = match outcome.status() {
            OutcomeStatus::Done { bytes, version } => {
                let mut detail = if directory || outcome.action() == Action::Delete {
                    String::new()
                } else {
                    format!("  ({bytes} o)")
                };
                if let Some(version) = version {
                    detail.push_str(&format!("  [version conservée : {version}]"));
                }
                format!(
                    "{:<10} {path}{suffix}{detail}",
                    done_label(outcome.action(), directory)
                )
            }
            OutcomeStatus::Refused { refusal } => {
                format!(
                    "{:<10} {path}{suffix}  (refus : {refusal} ; rien n'est modifié)",
                    "REFUSÉ"
                )
            }
            OutcomeStatus::Failed { error } => {
                format!("{:<10} {path}{suffix}  ({})", "ÉCHEC", error.message())
            }
            OutcomeStatus::Cancelled => format!("{:<10} {path}{suffix}", "ANNULÉ"),
            OutcomeStatus::NotStarted => format!("{:<10} {path}{suffix}", "NON FAIT"),
            OutcomeStatus::NoAction if show_all => format!(
                "{:<10} {path}{suffix}  (prévu : {})",
                "INCHANGÉ",
                action_label(outcome.action()).to_lowercase()
            ),
            _ => continue,
        };
        writeln!(out, "{line}")?;
    }
    let totals = report.totals();
    writeln!(
        out,
        "Résumé : {} réalisée(s), {} refusée(s), {} en échec, {} annulée(s), {} non commencée(s), {} sans action ; {} o transférés",
        totals.done,
        totals.refused,
        totals.failed,
        totals.cancelled,
        totals.not_started,
        totals.no_action,
        totals.bytes
    )?;
    if let Some(stop) = report.stop() {
        writeln!(
            out,
            "ARRÊT ANTICIPÉ : {} ; les opérations suivantes et toutes les suppressions restantes n'ont pas été faites.",
            stop_text(stop)
        )?;
    }
    for leftover in report.leftovers() {
        writeln!(
            out,
            "ATTENTION : fichier temporaire non retiré à la destination : {leftover}"
        )?;
    }
    let summary = plan.summary();
    if summary.conflict + summary.skip > 0 {
        writeln!(
            out,
            "Rappel du plan : {} conflit(s) et {} chemin(s) ignoré(s) n'ont reçu aucune écriture.",
            summary.conflict, summary.skip
        )?;
    }
    writeln!(out, "Statut : {}", status_text(report.status()))
}

/// `syncengine journal` : renvoie le code de sortie.
pub(crate) fn journal_command(out: &mut dyn Write, path: &Path) -> Result<u8, DiscoveryError> {
    let replay = read_journal(path).map_err(|error| {
        DiscoveryError::Execution(format!("journal {} illisible : {error}", path.display()))
    })?;
    let recovery = replay.recovery();
    writeln!(out, "Journal : {}", path.display())?;
    if let (Some(execution), Some(plan)) = (&recovery.execution, &recovery.plan) {
        writeln!(out, "  exécution    : {execution} (plan {plan})")?;
    }
    match recovery.finished {
        Some(status) => writeln!(out, "  état         : terminée — {}", status_text(status))?,
        None => writeln!(
            out,
            "  état         : INTERROMPUE (aucune fin consignée : arrêt brutal, coupure…)"
        )?,
    }
    let totals = recovery.totals;
    writeln!(
        out,
        "  opérations   : {} mutation(s) prévue(s) ; {} réalisée(s), {} refusée(s), {} en échec, {} annulée(s), {} non commencée(s) ; {} o transférés",
        recovery.mutations_planned,
        totals.done,
        totals.refused,
        totals.failed,
        totals.cancelled,
        totals.not_started,
        totals.bytes
    )?;
    if recovery.truncated_tail {
        writeln!(
            out,
            "  note         : dernière ligne incomplète ignorée (arrêt pendant l'écriture)"
        )?;
    }
    for in_flight in &recovery.in_flight {
        let mut line = format!(
            "EN COURS     {} {}",
            action_label(in_flight.action),
            in_flight.path
        );
        if let Some(staging) = &in_flight.staging {
            line.push_str(&format!(
                "  [fichier temporaire possible à la destination : {staging}]"
            ));
        }
        if let Some(version) = &in_flight.version {
            line.push_str(&format!("  [version conservée : {version}]"));
        }
        writeln!(out, "{line}")?;
    }
    if recovery.is_interrupted() {
        writeln!(
            out,
            "Reprise : relancer « preview » puis « execute » ; un fichier « .syncengine-….partial » ne contient jamais une version validée."
        )?;
    }
    Ok(match recovery.finished {
        Some(ExecutionStatus::Succeeded) => exit::OK,
        Some(ExecutionStatus::SucceededWithWarnings) | None => exit::UNCERTAIN,
        Some(_) => exit::FAILURE,
    })
}
