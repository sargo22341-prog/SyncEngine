//! Commandes `scan` et `compare` : composition des moteurs et rendu texte.
//!
//! Lecture seule : seuls le connecteur local (sans capacité d'écriture), le
//! scanner, le filtre et la comparaison sont utilisés. L'orchestrateur
//! applicatif n'existe pas encore ; cette composition directe sera reprise
//! par lui.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use connector_local::{LocalConnector, LocalOptions};
use sync_compare::{
    CompareOptions, ComparisonOutcome, ComparisonReport, Difference, EquivalenceBasis,
    PathComparison, Side, Uncertainty, compare,
};
use sync_core::{CancellationToken, EntryKind, FileEntry, HashAlgorithm, Timestamp};
use sync_filter::{Filter, FilterRules};
use sync_scanner::{
    IncompleteReason, Inventory, ScanError, ScanIssueKind, ScanOptions, UnknownReason, scan,
};

use crate::cli::DiscoveryArgs;

/// Codes de sortie (voir l'aide).
pub(crate) mod exit {
    /// Complet, aucune différence.
    pub(crate) const OK: u8 = 0;
    /// Différences constatées.
    pub(crate) const DIFFERENCES: u8 = 1;
    /// Inventaire incomplet ou constats incertains.
    pub(crate) const UNCERTAIN: u8 = 3;
    /// Échec (dossier inaccessible, sortie impossible).
    pub(crate) const FAILURE: u8 = 4;
}

/// Échec d'une commande de découverte ou de prévisualisation, avant tout
/// résultat exploitable.
#[derive(Debug)]
pub(crate) enum DiscoveryError {
    /// Règles de filtre invalides (erreur d'utilisation).
    Filter(sync_filter::FilterError),
    /// Chemin de dossier inutilisable.
    Path(String),
    /// Scan impossible (racine inaccessible…).
    Scan {
        /// Dossier concerné.
        root: PathBuf,
        /// Cause.
        error: ScanError,
    },
    /// Comparaison refusée.
    Compare(sync_compare::CompareError),
    /// Source et destination imbriquées (ou identiques).
    Nested(String),
    /// Politique refusée par la stratégie (erreur d'utilisation).
    Policy(sync_decisions::PolicyViolation),
    /// Planification ou sérialisation du plan impossible.
    Plan(String),
    /// Écriture de la sortie impossible.
    Output(io::Error),
}

impl std::fmt::Display for DiscoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Filter(error) => write!(f, "filtre invalide : {error}"),
            Self::Path(message) => f.write_str(message),
            Self::Scan { root, error } => {
                write!(f, "{} : {error}", root.display())?;
                if matches!(error, ScanError::RootUnavailable(_)) {
                    f.write_str(" — ce n'est PAS un dossier vide : aucun constat produit")?;
                }
                Ok(())
            }
            Self::Compare(error) => write!(f, "comparaison refusée : {error}"),
            Self::Nested(message) => write!(f, "{message} : aucun plan produit"),
            Self::Policy(error) => write!(f, "politique refusée : {error}"),
            Self::Plan(message) => write!(f, "plan impossible : {message}"),
            Self::Output(error) => write!(f, "sortie impossible : {error}"),
        }
    }
}

impl From<io::Error> for DiscoveryError {
    fn from(error: io::Error) -> Self {
        Self::Output(error)
    }
}

impl DiscoveryError {
    /// Vrai pour une erreur d'utilisation (code 2).
    pub(crate) fn is_usage(&self) -> bool {
        matches!(
            self,
            Self::Filter(_) | Self::Path(_) | Self::Nested(_) | Self::Policy(_)
        )
    }
}

pub(crate) fn filter_of(args: &DiscoveryArgs) -> Result<Filter, DiscoveryError> {
    let mut rules = FilterRules::new().with_case_insensitive(args.ignore_case);
    for pattern in &args.include_files {
        rules = rules.include_file(pattern);
    }
    for pattern in &args.exclude_files {
        rules = rules.exclude_file(pattern);
    }
    for pattern in &args.exclude_directories {
        rules = rules.exclude_directory(pattern);
    }
    Filter::new(rules).map_err(DiscoveryError::Filter)
}

pub(crate) fn scan_options(args: &DiscoveryArgs) -> ScanOptions {
    if args.hash {
        ScanOptions::new().with_hash(HashAlgorithm::Blake3)
    } else {
        ScanOptions::new()
    }
}

fn connector_for(root: &Path) -> Result<LocalConnector, DiscoveryError> {
    // `absolute` ne touche pas au disque : un dossier absent sera signalé
    // par le scan comme indisponible, jamais comme vide.
    let absolute = std::path::absolute(root)
        .map_err(|error| DiscoveryError::Path(format!("{} : {error}", root.display())))?;
    LocalConnector::new(absolute, LocalOptions::new())
        .map_err(|error| DiscoveryError::Path(format!("{} : {error}", root.display())))
}

/// Résultat d'un scan : complet, ou partiel après annulation.
pub(crate) fn run_scan(
    root: &Path,
    filter: &Filter,
    options: &ScanOptions,
) -> Result<Inventory, DiscoveryError> {
    let connector = connector_for(root)?;
    // Pas de gestion de Ctrl+C en phase 02 : l'interruption tue le
    // processus, ce qui est sans risque puisque rien n'est modifié.
    scan(&connector, filter, options, &CancellationToken::new()).map_err(|error| {
        DiscoveryError::Scan {
            root: root.to_path_buf(),
            error,
        }
    })
}

/// `syncengine scan` : renvoie le code de sortie.
pub(crate) fn scan_command(
    out: &mut dyn Write,
    root: &Path,
    args: &DiscoveryArgs,
) -> Result<u8, DiscoveryError> {
    let filter = filter_of(args)?;
    let inventory = run_scan(root, &filter, &scan_options(args))?;
    writeln!(out, "Inventaire (lecture seule) : {}", root.display())?;
    for entry in inventory.entries() {
        writeln!(out, "{}", entry_line(entry))?;
    }
    write_issues(out, "", &inventory)?;
    let stats = inventory.stats();
    writeln!(
        out,
        "Résumé : {} fichier(s), {} dossier(s), {} lien(s)/spécial(aux), {} octet(s) connus, {} exclu(s), {} empreinte(s)",
        stats.files,
        stats.directories,
        stats.others,
        stats.known_bytes,
        stats.excluded,
        stats.hashed
    )?;
    if inventory.is_complete() {
        writeln!(out, "Inventaire COMPLET.")?;
        Ok(exit::OK)
    } else {
        let count = inventory.incomplete_directories().count();
        writeln!(
            out,
            "Inventaire INCOMPLET : {count} dossier(s) non listé(s) entièrement ; une absence n'y prouve rien."
        )?;
        Ok(exit::UNCERTAIN)
    }
}

/// `syncengine compare` : renvoie le code de sortie.
pub(crate) fn compare_command(
    out: &mut dyn Write,
    left_root: &Path,
    right_root: &Path,
    args: &DiscoveryArgs,
    show_all: bool,
) -> Result<u8, DiscoveryError> {
    let filter = filter_of(args)?;
    let options = scan_options(args);
    let left = run_scan(left_root, &filter, &options)?;
    let right = run_scan(right_root, &filter, &options)?;
    let report = compare(&left, &right, &CompareOptions::new()).map_err(DiscoveryError::Compare)?;

    writeln!(out, "Comparaison (simulation, aucune modification)")?;
    writeln!(out, "  gauche : {}", left_root.display())?;
    writeln!(out, "  droite : {}", right_root.display())?;
    for row in report.rows() {
        if !show_all && matches!(row.outcome(), ComparisonOutcome::Equivalent(_)) {
            continue;
        }
        writeln!(out, "{}", row_line(row))?;
    }
    write_issues(out, "gauche ", &left)?;
    write_issues(out, "droite ", &right)?;
    write_summary(out, &report)?;

    let summary = report.summary();
    if summary.uncertain > 0 || !report.left_complete() || !report.right_complete() {
        Ok(exit::UNCERTAIN)
    } else if summary.left_only + summary.right_only + summary.different > 0 {
        Ok(exit::DIFFERENCES)
    } else {
        Ok(exit::OK)
    }
}

pub(crate) fn write_issues(
    out: &mut dyn Write,
    side: &str,
    inventory: &Inventory,
) -> io::Result<()> {
    for (path, reason) in inventory.incomplete_directories() {
        let path = if path.is_root() {
            "<racine>"
        } else {
            path.as_str()
        };
        writeln!(
            out,
            "!! {side}dossier incomplet : {path} ({})",
            incomplete_text(reason)
        )?;
    }
    // Les échecs de listing sont déjà rapportés comme dossiers incomplets.
    for issue in inventory
        .issues()
        .iter()
        .filter(|issue| !matches!(issue.kind(), ScanIssueKind::ListFailed(_)))
    {
        writeln!(out, "!! {side}{issue}")?;
    }
    Ok(())
}

fn write_summary(out: &mut dyn Write, report: &ComparisonReport) -> io::Result<()> {
    let s = report.summary();
    writeln!(
        out,
        "Résumé : {} gauche seulement, {} droite seulement, {} différent(s), {} incertain(s), {} équivalent(s) dont {} vérifié(s) par empreinte",
        s.left_only, s.right_only, s.different, s.uncertain, s.equivalent, s.content_verified
    )?;
    if s.uncertain > 0 || !report.left_complete() || !report.right_complete() {
        writeln!(
            out,
            "ATTENTION : constats incomplets ou incertains ; aucune suppression ne doit en être déduite."
        )?;
    }
    Ok(())
}

pub(crate) fn kind_marker(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::File => "F",
        EntryKind::Directory => "D",
        EntryKind::Symlink => "L",
        _ => "?",
    }
}

fn entry_line(entry: &FileEntry) -> String {
    let meta = entry.metadata();
    let size = meta
        .size()
        .map_or_else(|| "-".to_owned(), |size| size.to_string());
    let modified = meta.modified().map_or_else(|| "-".to_owned(), format_utc);
    let mut line = format!(
        "{} {:>12}  {:<20}  {}",
        kind_marker(entry.kind()),
        size,
        modified,
        entry.path()
    );
    if entry.kind() == EntryKind::Symlink {
        line.push_str("  (lien non suivi)");
    }
    if let Some(hash) = meta.hash() {
        line.push_str("  ");
        line.push_str(&hash.to_hex());
    }
    line
}

fn side_text(side: Side) -> &'static str {
    match side {
        Side::Left => "gauche",
        Side::Right => "droite",
    }
}

fn row_line(row: &PathComparison) -> String {
    let path = row.path();
    match row.outcome() {
        ComparisonOutcome::LeftOnly => format!("<   {path}  (gauche seulement)"),
        ComparisonOutcome::RightOnly => format!(">   {path}  (droite seulement)"),
        ComparisonOutcome::Equivalent(basis) => {
            let basis = match basis {
                EquivalenceBasis::Directory => "dossiers",
                EquivalenceBasis::ContentHash => "contenu identique",
                EquivalenceBasis::SizeAndModified => {
                    "taille et date identiques, contenu non vérifié"
                }
                _ => "équivalent",
            };
            format!("=   {path}  ({basis})")
        }
        ComparisonOutcome::Different(differences) => {
            let details: Vec<String> = differences.iter().map(difference_text).collect();
            format!("!=  {path}  ({})", details.join(" ; "))
        }
        ComparisonOutcome::Uncertain(reasons) => {
            let details: Vec<String> = reasons.iter().map(uncertainty_text).collect();
            format!("??  {path}  (incertain : {})", details.join(" ; "))
        }
        _ => format!("??  {path}"),
    }
}

fn difference_text(difference: &Difference) -> String {
    match difference {
        Difference::Kind { left, right } => {
            format!("nature {} ≠ {}", kind_marker(*left), kind_marker(*right))
        }
        Difference::Size { left, right } => format!("taille {left} ≠ {right}"),
        Difference::Modified { newer } => format!("date plus récente à {}", side_text(*newer)),
        Difference::ContentHash => "contenu différent".to_owned(),
        _ => "différence".to_owned(),
    }
}

pub(crate) fn incomplete_text(reason: IncompleteReason) -> String {
    match reason {
        IncompleteReason::ListFailed(kind) => format!("listing impossible : {kind:?}"),
        IncompleteReason::OmittedEntries => "entrées non représentables".to_owned(),
        IncompleteReason::InconsistentListing => "listing incohérent".to_owned(),
        IncompleteReason::NotScanned => "non parcouru".to_owned(),
        _ => "incomplet".to_owned(),
    }
}

fn uncertainty_text(uncertainty: &Uncertainty) -> String {
    match uncertainty {
        Uncertainty::PresenceUnknown { side, reason } => {
            let why = match reason {
                UnknownReason::IncompleteDirectory(reason) => incomplete_text(*reason),
                UnknownReason::BehindUnfollowedEntry => "derrière un lien non suivi".to_owned(),
                _ => "inconnue".to_owned(),
            };
            format!("présence inconnue à {} ({why})", side_text(*side))
        }
        Uncertainty::ExcludedOnSide { side } => {
            format!("exclu par le filtre à {}", side_text(*side))
        }
        Uncertainty::SizeUnknown => "taille inconnue".to_owned(),
        Uncertainty::ModifiedUnknown => "date inconnue".to_owned(),
        Uncertainty::PrecisionUnknown => "précision des dates inconnue".to_owned(),
        Uncertainty::NotComparable { .. } => "liens non comparés".to_owned(),
        Uncertainty::InconsistentObservations => "observations contradictoires".to_owned(),
        Uncertainty::CaseCollision { with } => {
            let others: Vec<&str> = with.iter().map(|p| p.as_str()).collect();
            format!("collision de casse avec {}", others.join(", "))
        }
        _ => "incertain".to_owned(),
    }
}

/// Date UTC lisible `AAAA-MM-JJ hh:mm:ss`, sans dépendance externe.
fn format_utc(ts: Timestamp) -> String {
    let secs = ts.secs();
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Conversion jours depuis 1970-01-01 → date grégorienne (algorithme de
/// H. Hinnant, domaine public).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_utc_dates() {
        assert_eq!(format_utc(Timestamp::UNIX_EPOCH), "1970-01-01 00:00:00");
        assert_eq!(
            format_utc(Timestamp::new(1_761_224_400, 0).unwrap()),
            "2025-10-23 13:00:00"
        );
        assert_eq!(
            format_utc(Timestamp::new(951_782_400, 0).unwrap()),
            "2000-02-29 00:00:00"
        );
        assert_eq!(
            format_utc(Timestamp::new(-1, 0).unwrap()),
            "1969-12-31 23:59:59"
        );
    }
}
