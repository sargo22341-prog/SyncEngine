//! Commande `preview` : plan d'une sauvegarde ou d'un miroir, simulé.
//!
//! Composition directe (faute d'orchestrateur) : connecteur local en
//! lecture seule → scanner → comparaison → stratégie → plan. Le plan est
//! affiché (texte ou JSON versionné) ; **rien n'est exécuté** ici
//! (`execute` réutilise [`build_plan`] puis l'exécuteur). Les textes
//! français sont produits ici, jamais dans les moteurs.

use std::io::Write;
use std::path::Path;

use sync_backup::BackupStrategy;
use sync_compare::{CompareOptions, compare};
use sync_core::{EntryKind, PlanId, Timestamp};
use sync_decisions::{PlanContext, PlanStrategy, PlanningError, PlanningInput};
use sync_mirror::MirrorStrategy;
use sync_plan::{
    Action, DEFAULT_MAX_DELETE_PERCENT, DeletionGuard, DeletionPolicy, DeletionRefusal,
    ExecutionPlan, Operation, OverwritePolicy, PlanPolicy, PlanSide, Reason, SyncMode,
    UncertainCause,
};
use sync_scanner::Inventory;

use crate::cli::{DiscoveryArgs, PreviewArgs};
use crate::discovery::{
    DiscoveryError, exit, filter_of, kind_marker, run_scan, scan_options, write_issues,
};

/// Plan calculé et inventaires qui l'ont produit.
pub(crate) struct Planned {
    /// Plan (même modèle pour la simulation et l'exécution).
    pub(crate) plan: ExecutionPlan,
    /// Inventaire de la source.
    pub(crate) source: Inventory,
    /// Inventaire de la destination.
    pub(crate) destination: Inventory,
}

/// Calcule le plan : connecteur local en lecture seule → scanner →
/// comparaison → stratégie. Aucune écriture.
pub(crate) fn build_plan(
    source_root: &Path,
    destination_root: &Path,
    args: &DiscoveryArgs,
    preview: &PreviewArgs,
) -> Result<Planned, DiscoveryError> {
    let filter = filter_of(args)?;
    let (strategy, policy) = strategy_and_policy(preview)?;
    ensure_not_nested(source_root, destination_root)?;

    let options = scan_options(args);
    let source = run_scan(source_root, &filter, &options)?;
    let destination = run_scan(destination_root, &filter, &options)?;
    let report = compare(
        &source,
        &destination,
        &CompareOptions::new().with_directory_merge(true),
    )
    .map_err(DiscoveryError::Compare)?;

    let context = plan_context(source_root, destination_root)?;
    let plan = strategy
        .build_plan(&PlanningInput::new(&report, &context), &policy)
        .map_err(|error| match error {
            PlanningError::InvalidPolicy(violation) => DiscoveryError::Policy(violation),
            other => DiscoveryError::Plan(other.to_string()),
        })?;
    Ok(Planned {
        plan,
        source,
        destination,
    })
}

/// `syncengine preview` : renvoie le code de sortie.
pub(crate) fn preview_command(
    out: &mut dyn Write,
    source_root: &Path,
    destination_root: &Path,
    args: &DiscoveryArgs,
    preview: &PreviewArgs,
) -> Result<u8, DiscoveryError> {
    let Planned {
        plan,
        source,
        destination,
    } = build_plan(source_root, destination_root, args, preview)?;

    if preview.json {
        let json = plan
            .to_json()
            .map_err(|error| DiscoveryError::Plan(error.to_string()))?;
        writeln!(out, "{json}")?;
    } else {
        write_plan(out, &plan, preview.show_all)?;
        write_issues(out, "source ", &source)?;
        write_issues(out, "destination ", &destination)?;
        write_summary(out, &plan)?;
    }

    let summary = plan.summary();
    Ok(if plan.needs_attention() {
        exit::UNCERTAIN
    } else if summary.mutations() > 0 {
        exit::DIFFERENCES
    } else {
        exit::OK
    })
}

/// Stratégie et politique demandées (défauts propres au mode).
fn strategy_and_policy(
    preview: &PreviewArgs,
) -> Result<(&'static dyn PlanStrategy, PlanPolicy), DiscoveryError> {
    let (strategy, default): (&'static dyn PlanStrategy, PlanPolicy) = match preview.mode {
        SyncMode::Backup => (&BackupStrategy, BackupStrategy::default_policy()),
        SyncMode::Mirror => (&MirrorStrategy, MirrorStrategy::default_policy()),
        _ => return Err(DiscoveryError::Plan("mode non pris en charge".to_owned())),
    };
    let deletion = if preview.allow_delete {
        let percent = preview
            .max_delete_percent
            .unwrap_or(DEFAULT_MAX_DELETE_PERCENT);
        let guard = DeletionGuard::new()
            .with_max_count(preview.max_delete)
            .with_max_percent(Some(percent))
            .map_err(|error| DiscoveryError::Plan(error.to_string()))?
            .with_empty_source_allowed(preview.allow_empty_source);
        DeletionPolicy::enabled(guard)
    } else {
        DeletionPolicy::Disabled
    };
    let overwrite = preview.overwrite.unwrap_or(default.overwrite);
    Ok((strategy, PlanPolicy::new(overwrite, deletion)))
}

/// Refuse une destination située dans la source (ou l'inverse, ou le même
/// dossier) : un miroir s'y recopierait lui-même ou effacerait sa propre
/// source. Les chemins sont résolus (liens et jonctions compris) ; un
/// dossier introuvable est laissé au scan, qui le signale comme
/// indisponible.
pub(crate) fn ensure_not_nested(source: &Path, destination: &Path) -> Result<(), DiscoveryError> {
    let (Ok(source), Ok(destination)) = (
        std::fs::canonicalize(source),
        std::fs::canonicalize(destination),
    ) else {
        return Ok(());
    };
    let message = if source == destination {
        "la source et la destination désignent le même dossier"
    } else if destination.starts_with(&source) {
        "la destination est située dans la source"
    } else if source.starts_with(&destination) {
        "la source est située dans la destination"
    } else {
        return Ok(());
    };
    Err(DiscoveryError::Nested(message.to_owned()))
}

/// Libellé d'emplacement enregistré dans le plan (chemin absolu, sans accès
/// disque) ; sert aussi à vérifier qu'un plan enregistré désigne bien les
/// dossiers fournis.
pub(crate) fn location(path: &Path) -> String {
    std::path::absolute(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .display()
        .to_string()
}

fn plan_context(source: &Path, destination: &Path) -> Result<PlanContext, DiscoveryError> {
    let created_at = Timestamp::now()
        .map_err(|error| DiscoveryError::Plan(format!("horloge système : {error}")))?;
    let plan_id = PlanId::new(format!(
        "preview-{}-{}",
        created_at.secs(),
        std::process::id()
    ))
    .map_err(|error| DiscoveryError::Plan(error.to_string()))?;
    Ok(PlanContext {
        plan_id,
        profile: None,
        created_at,
        source_location: location(source),
        destination_location: location(destination),
    })
}

fn write_plan(out: &mut dyn Write, plan: &ExecutionPlan, show_all: bool) -> std::io::Result<()> {
    writeln!(
        out,
        "SIMULATION — aucun fichier n'est copié, modifié ni supprimé."
    )?;
    writeln!(out, "Plan : {}", mode_text(plan.mode()))?;
    writeln!(out, "  source       : {}", plan.source().location())?;
    writeln!(out, "  destination  : {}", plan.destination().location())?;
    writeln!(
        out,
        "  écrasement   : {}",
        overwrite_text(plan.policy().overwrite)
    )?;
    writeln!(
        out,
        "  suppressions : {}",
        deletion_text(&plan.policy().deletion)
    )?;
    for op in plan.operations() {
        let hidden = !show_all && matches!(op.reason(), Reason::Equivalent { .. });
        if !hidden {
            writeln!(out, "{}", operation_line(op))?;
        }
    }
    Ok(())
}

fn write_summary(out: &mut dyn Write, plan: &ExecutionPlan) -> std::io::Result<()> {
    let s = plan.summary();
    let unknown = if s.unknown_sizes > 0 {
        format!(" + {} fichier(s) de taille inconnue", s.unknown_sizes)
    } else {
        String::new()
    };
    writeln!(
        out,
        "Résumé : {} à copier, {} à remplacer, {} à supprimer, {} conservé(s), {} conflit(s), {} ignoré(s) ; {} o à transférer{unknown}",
        s.copy, s.replace, s.delete, s.keep, s.conflict, s.skip, s.bytes_to_transfer
    )?;
    if s.deletions_refused > 0 {
        writeln!(
            out,
            "SUPPRESSIONS REFUSÉES : {} suppression(s) écartée(s) par un garde-fou ; aucune n'est retenue.",
            s.deletions_refused
        )?;
    }
    if !plan.source().is_complete() || !plan.destination().is_complete() {
        writeln!(
            out,
            "ATTENTION : inventaire incomplet ; aucune suppression n'est retenue et les chemins incertains sont ignorés."
        )?;
    }
    if s.conflict > 0 {
        writeln!(
            out,
            "ATTENTION : {} conflit(s) non résolu(s) : rien n'y sera écrit.",
            s.conflict
        )?;
    }
    writeln!(
        out,
        "SIMULATION : rien n'a été modifié ; « syncengine execute … --confirm » exécuterait ce plan."
    )
}

fn operation_line(op: &Operation) -> String {
    let suffix = if op.kind() == EntryKind::Directory {
        "/"
    } else {
        ""
    };
    format!(
        "{:<10} {}{suffix}  ({})",
        action_label(op.action()),
        op.path(),
        details(op)
    )
}

pub(crate) fn action_label(action: Action) -> &'static str {
    match action {
        Action::Copy => "COPIER",
        Action::Replace => "REMPLACER",
        Action::Keep => "CONSERVER",
        Action::Delete => "SUPPRIMER",
        Action::Conflict => "CONFLIT",
        Action::Skip => "IGNORER",
        _ => "?",
    }
}

fn details(op: &Operation) -> String {
    let mut text = reason_text(op);
    if matches!(op.action(), Action::Copy | Action::Replace) {
        let size = match (op.kind(), op.bytes()) {
            (EntryKind::Directory, _) => "dossier".to_owned(),
            (_, Some(bytes)) => format!("{bytes} o"),
            (_, None) => "taille inconnue".to_owned(),
        };
        text = format!("{text}, {size}");
    }
    if op.action() == Action::Conflict {
        text.push_str(" ; rien n'est écrit");
    }
    text
}

fn reason_text(op: &Operation) -> String {
    match op.reason() {
        Reason::MissingAtDestination => "absent de la destination".to_owned(),
        Reason::SourceNewer => "source plus récente".to_owned(),
        Reason::DestinationNewer if op.action() == Action::Replace => {
            "destination plus récente, ÉCRASÉE par la source".to_owned()
        }
        Reason::DestinationNewer => "destination plus récente".to_owned(),
        Reason::ContentDiffers => {
            "contenu différent, dates égales ou inconnues : la source l'emporte".to_owned()
        }
        Reason::AmbiguousChange => {
            "contenu différent sans date exploitable : impossible de savoir quel côté a changé"
                .to_owned()
        }
        Reason::KindMismatch {
            source,
            destination,
        } => format!(
            "natures différentes : {} à la source, {} à la destination",
            kind_marker(*source),
            kind_marker(*destination)
        ),
        Reason::Equivalent { .. } if op.kind() == EntryKind::Directory => {
            "dossier présent des deux côtés".to_owned()
        }
        Reason::Equivalent { verified: true } => "contenu identique (empreinte)".to_owned(),
        Reason::Equivalent { verified: false } => {
            "taille et date identiques, contenu non vérifié".to_owned()
        }
        Reason::DestinationOnly => {
            "présent seulement à la destination ; une sauvegarde ne supprime jamais".to_owned()
        }
        Reason::OverwriteDisabled => {
            "différent, remplacement désactivé (--overwrite never)".to_owned()
        }
        Reason::DeletionDisabled => {
            "absent de la source ; suppressions non autorisées (--allow-delete)".to_owned()
        }
        Reason::AbsentFromSource => "absent de la source".to_owned(),
        Reason::Uncertain { causes } => {
            let causes: Vec<String> = causes.iter().map(cause_text).collect();
            format!("incertain : {}", causes.join(" ; "))
        }
        Reason::UnsupportedEntryKind { .. } => {
            "lien ou entrée spéciale : jamais copié ni supprimé".to_owned()
        }
        Reason::ParentUnavailable => "dossier parent indisponible à la destination".to_owned(),
        Reason::DirectoryNotEmptied => {
            "dossier dont une partie du contenu est conservée : non supprimé".to_owned()
        }
        Reason::DeletionRefused { refusal } => {
            format!("suppression refusée : {}", refusal_text(*refusal))
        }
        _ => "motif non reconnu".to_owned(),
    }
}

fn side_text(side: PlanSide) -> &'static str {
    match side {
        PlanSide::Source => "à la source",
        PlanSide::Destination => "à la destination",
    }
}

fn cause_text(cause: &UncertainCause) -> String {
    match cause {
        UncertainCause::PresenceUnknown { side } => {
            format!("présence inconnue {}", side_text(*side))
        }
        UncertainCause::Excluded { side } => format!("exclu par le filtre {}", side_text(*side)),
        UncertainCause::SizeUnknown => "taille inconnue".to_owned(),
        UncertainCause::ModifiedUnknown => "date inconnue".to_owned(),
        UncertainCause::PrecisionUnknown => "précision des dates inconnue".to_owned(),
        UncertainCause::NotComparable => "liens non comparés".to_owned(),
        UncertainCause::InconsistentObservations => "observations contradictoires".to_owned(),
        UncertainCause::CaseCollision => "collision de casse".to_owned(),
        _ => "cause non précisée".to_owned(),
    }
}

fn refusal_text(refusal: DeletionRefusal) -> String {
    match refusal {
        DeletionRefusal::SourceIncomplete => "inventaire de la source incomplet".to_owned(),
        DeletionRefusal::DestinationIncomplete => {
            "inventaire de la destination incomplet".to_owned()
        }
        DeletionRefusal::SourceEmpty => {
            "la source est vide (montage absent ?) ; --allow-empty-source pour l'admettre"
                .to_owned()
        }
        DeletionRefusal::TooMany { count, limit } => {
            format!("{count} suppressions dépassent le plafond de {limit} (--max-delete)")
        }
        DeletionRefusal::TooLarge {
            count,
            total,
            max_percent,
        } => format!(
            "{count} suppressions sur {total} entrées dépassent {max_percent} % (--max-delete-percent)"
        ),
        _ => "garde-fou".to_owned(),
    }
}

fn mode_text(mode: SyncMode) -> &'static str {
    match mode {
        SyncMode::Backup => "sauvegarde (source → destination, jamais de suppression)",
        SyncMode::Mirror => "miroir (destination rendue conforme à la source)",
        _ => "mode non reconnu",
    }
}

fn overwrite_text(policy: OverwritePolicy) -> &'static str {
    match policy {
        OverwritePolicy::SourceWins => "la source l'emporte toujours",
        OverwritePolicy::NewerSourceOnly => {
            "seulement si la source est plus récente, sinon conflit"
        }
        OverwritePolicy::KeepDestination => "jamais (destination conservée)",
        _ => "politique non reconnue",
    }
}

fn deletion_text(policy: &DeletionPolicy) -> String {
    let Some(guard) = policy.guard() else {
        return "non autorisées".to_owned();
    };
    let mut limits = Vec::new();
    if let Some(count) = guard.max_count() {
        limits.push(format!("au plus {count}"));
    }
    if let Some(percent) = guard.max_percent() {
        limits.push(format!("au plus {percent} % des entrées de la destination"));
    }
    if !guard.allows_empty_source() {
        limits.push("aucune si la source est vide".to_owned());
    }
    limits.push("aucune si un inventaire est incomplet".to_owned());
    format!("autorisées ({})", limits.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policies_follow_the_mode() {
        let backup = strategy_and_policy(&PreviewArgs::default()).unwrap();
        assert_eq!(backup.0.mode(), SyncMode::Backup);
        assert_eq!(backup.1, BackupStrategy::default_policy());

        let mirror_args = PreviewArgs {
            mode: SyncMode::Mirror,
            allow_delete: true,
            max_delete: Some(10),
            ..PreviewArgs::default()
        };
        let (strategy, policy) = strategy_and_policy(&mirror_args).unwrap();
        assert_eq!(strategy.mode(), SyncMode::Mirror);
        assert_eq!(policy.overwrite, OverwritePolicy::SourceWins);
        let guard = policy.deletion.guard().unwrap();
        assert_eq!(guard.max_count(), Some(10));
        assert_eq!(guard.max_percent(), Some(DEFAULT_MAX_DELETE_PERCENT));
        assert!(!guard.allows_empty_source());
    }

    #[test]
    fn nested_folders_are_refused() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path();
        let inner = root.join("destination");
        let sibling = root.join("voisin");
        std::fs::create_dir(&inner).unwrap();
        std::fs::create_dir(&sibling).unwrap();
        assert!(matches!(
            ensure_not_nested(root, &inner),
            Err(DiscoveryError::Nested(_))
        ));
        assert!(matches!(
            ensure_not_nested(&inner, root),
            Err(DiscoveryError::Nested(_))
        ));
        assert!(matches!(
            ensure_not_nested(&inner, &inner),
            Err(DiscoveryError::Nested(_))
        ));
        assert!(ensure_not_nested(&inner, &sibling).is_ok());
        assert!(ensure_not_nested(&root.join("absent-a"), &root.join("absent-b")).is_ok());
    }
}
