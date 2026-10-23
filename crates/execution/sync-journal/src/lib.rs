//! Journal durable d'exécution.
//!
//! L'exécuteur consigne chaque étape d'une exécution **avant** de la
//! réaliser (opération commencée, dépôt temporaire créé, version conservée)
//! et son issue **après**. Le journal est la trace qui permet, après un
//! arrêt brutal (coupure, fermeture de session, processus tué), de savoir
//! ce qui a été fait, ce qui était en cours et quels fichiers temporaires
//! ont pu rester ([`Replay::recovery`]).
//!
//! # Format (version [`JOURNAL_FORMAT_VERSION`])
//! JSON Lines : une ligne par enregistrement,
//! `{"format_version":1,"sequence":N,"at":{…},"record":{"type":…}}`.
//! Une ligne finale incomplète (arrêt pendant l'écriture) est tolérée et
//! signalée ; toute autre ligne illisible, une séquence non consécutive ou
//! une version inconnue rendent le journal inexploitable (jamais interprété
//! au hasard).
//!
//! # Durabilité
//! [`FileJournal`] crée un fichier **nouveau** (jamais d'écrasement d'un
//! journal existant) et force chaque enregistrement sur disque
//! (`sync_data`) avant de rendre la main. Le coût est d'un `fsync` par
//! enregistrement (quelques-uns par opération, jamais par octet) ; il n'a
//! pas été mesuré sur de très grands plans.
//!
//! Les journaux ne sont pas une source de vérité sur l'état des données :
//! ils disent ce que l'exécuteur a fait ou tenté, l'état réel se constate
//! par un nouveau scan.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sync_core::{
    ExecutionId, ExecutionStatus, OperationId, PlanId, ProfileId, ResourcePath, Timestamp,
};
use sync_errors::{ClassifiedError, ErrorClass};
use sync_plan::{Action, PlanSide};
use thiserror::Error;

/// Version du format de journal produite par cette version du code.
pub const JOURNAL_FORMAT_VERSION: u32 = 1;

/// Issue d'une opération, telle que consignée.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum OutcomeCode {
    /// Mutation réalisée et vérifiée.
    Done,
    /// Aucune mutation prévue par le plan (conservation, conflit, saut).
    NoAction,
    /// Mutation refusée : une précondition n'est plus vraie ; rien n'a été modifié.
    Refused,
    /// Mutation tentée et en échec ; la cible n'a pas été validée.
    Failed,
    /// Mutation interrompue par une annulation ; la cible n'a pas été validée.
    Cancelled,
    /// Opération non commencée (arrêt de l'exécution avant elle).
    NotStarted,
}

/// Décomptes finaux d'une exécution.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalTotals {
    /// Mutations réalisées.
    pub done: u64,
    /// Opérations sans mutation prévue.
    pub no_action: u64,
    /// Mutations refusées.
    pub refused: u64,
    /// Mutations en échec.
    pub failed: u64,
    /// Mutations annulées.
    pub cancelled: u64,
    /// Opérations non commencées.
    pub not_started: u64,
    /// Octets transférés et validés.
    pub bytes: u64,
}

/// Enregistrement du journal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum JournalRecord {
    /// Physical receiver, recorded before a bidirectional operation starts.
    OperationDirection {
        /// Operation identifier.
        operation: OperationId,
        /// Physical receiving endpoint.
        target: PlanSide,
    },
    /// Début d'exécution (avant toute mutation).
    ExecutionStarted {
        /// Exécution.
        execution: ExecutionId,
        /// Plan exécuté.
        plan: PlanId,
        /// Profil, s'il y en a un.
        profile: Option<ProfileId>,
        /// Nombre d'opérations du plan.
        operations: u64,
        /// Nombre de mutations prévues.
        mutations: u64,
        /// Libellé de la source (sans secret).
        source: String,
        /// Libellé de la destination (sans secret).
        destination: String,
        /// Dossier du magasin de versions, si la conservation est active.
        versions: Option<ResourcePath>,
    },
    /// Une mutation commence (préconditions sur le point d'être revérifiées).
    OperationStarted {
        /// Opération.
        operation: OperationId,
        /// Chemin.
        path: ResourcePath,
        /// Action.
        action: Action,
    },
    /// Un dépôt temporaire a été créé à la destination.
    StagingCreated {
        /// Opération.
        operation: OperationId,
        /// Chemin du dépôt temporaire (à retirer s'il subsiste après un arrêt).
        staging: ResourcePath,
    },
    /// Une version du fichier de destination a été conservée.
    VersionPreserved {
        /// Opération.
        operation: OperationId,
        /// Emplacement dans le magasin de versions.
        location: ResourcePath,
    },
    /// Issue d'une opération.
    OperationFinished {
        /// Opération.
        operation: OperationId,
        /// Chemin.
        path: ResourcePath,
        /// Action.
        action: Action,
        /// Issue.
        outcome: OutcomeCode,
        /// Octets validés.
        bytes: u64,
        /// Classe d'erreur (échec) le cas échéant.
        error_class: Option<ErrorClass>,
        /// Détail lisible (caviardé par l'émetteur), le cas échéant.
        detail: Option<String>,
    },
    /// Fin d'exécution (absente si l'exécution a été interrompue).
    ExecutionFinished {
        /// Statut final.
        status: ExecutionStatus,
        /// Décomptes.
        totals: JournalTotals,
    },
}

/// Ligne du journal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalLine {
    format_version: u32,
    sequence: u64,
    at: Timestamp,
    record: JournalRecord,
}

impl JournalLine {
    /// Numéro d'ordre (à partir de 0, consécutif).
    #[must_use]
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    /// Instant d'écriture (horloge locale, informatif).
    #[must_use]
    pub fn at(&self) -> Timestamp {
        self.at
    }

    /// Enregistrement.
    #[must_use]
    pub fn record(&self) -> &JournalRecord {
        &self.record
    }
}

/// Erreur de journal.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum JournalError {
    /// Un journal existe déjà à ce chemin : il n'est jamais écrasé.
    #[error("un journal existe déjà à cet emplacement : il n'est jamais écrasé")]
    AlreadyExists,
    /// Écriture, synchronisation ou lecture impossible.
    #[error("journal : {0}")]
    Io(#[source] io::Error),
    /// Sérialisation impossible.
    #[error("journal : sérialisation impossible : {0}")]
    Serialize(#[source] serde_json::Error),
    /// Ligne illisible (hors fin tronquée).
    #[error("journal corrompu à la ligne {line}")]
    Corrupt {
        /// Numéro de ligne (à partir de 1).
        line: usize,
    },
    /// Version de format non prise en charge.
    #[error("version de journal {found} non prise en charge (attendu {JOURNAL_FORMAT_VERSION})")]
    UnsupportedVersion {
        /// Version lue.
        found: u32,
    },
    /// Numéros d'ordre non consécutifs (lignes perdues ou insérées).
    #[error("journal incohérent : séquence {found} au lieu de {expected}")]
    Sequence {
        /// Numéro attendu.
        expected: u64,
        /// Numéro lu.
        found: u64,
    },
}

impl ClassifiedError for JournalError {
    fn class(&self) -> ErrorClass {
        match self {
            Self::AlreadyExists => ErrorClass::Conflict,
            Self::Io(error) if error.kind() == io::ErrorKind::StorageFull => {
                ErrorClass::ResourceExhausted
            }
            Self::Io(_) | Self::Serialize(_) => ErrorClass::Internal,
            Self::Corrupt { .. } | Self::Sequence { .. } => ErrorClass::Integrity,
            Self::UnsupportedVersion { .. } => ErrorClass::NotSupported,
        }
    }
}

/// Destination des enregistrements.
pub trait JournalSink: Send {
    /// Ajoute un enregistrement et ne rend la main qu'une fois celui-ci
    /// durable (pour un journal persistant).
    ///
    /// # Errors
    /// [`JournalError`] : l'appelant doit alors s'arrêter avant toute
    /// nouvelle mutation, faute de pouvoir la tracer.
    fn append(&mut self, record: JournalRecord) -> Result<(), JournalError>;
}

fn line(sequence: u64, record: JournalRecord) -> JournalLine {
    JournalLine {
        format_version: JOURNAL_FORMAT_VERSION,
        sequence,
        at: Timestamp::now().unwrap_or(Timestamp::UNIX_EPOCH),
        record,
    }
}

/// Journal persistant dans un fichier JSON Lines.
#[derive(Debug)]
pub struct FileJournal {
    file: File,
    path: PathBuf,
    next_sequence: u64,
}

impl FileJournal {
    /// Crée un journal **nouveau** à `path` (le dossier parent doit exister).
    ///
    /// # Errors
    /// [`JournalError::AlreadyExists`] si un fichier existe déjà ;
    /// [`JournalError::Io`] sinon.
    pub fn create(path: impl Into<PathBuf>) -> Result<Self, JournalError> {
        let path = path.into();
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| {
                if error.kind() == io::ErrorKind::AlreadyExists {
                    JournalError::AlreadyExists
                } else {
                    JournalError::Io(error)
                }
            })?;
        Ok(Self {
            file,
            path,
            next_sequence: 0,
        })
    }

    /// Emplacement du journal.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl JournalSink for FileJournal {
    fn append(&mut self, record: JournalRecord) -> Result<(), JournalError> {
        let entry = line(self.next_sequence, record);
        let mut text = serde_json::to_string(&entry).map_err(JournalError::Serialize)?;
        text.push('\n');
        self.file
            .write_all(text.as_bytes())
            .and_then(|()| self.file.sync_data())
            .map_err(JournalError::Io)?;
        self.next_sequence += 1;
        Ok(())
    }
}

/// Journal en mémoire (tests, exécutions sans trace persistante voulue).
///
/// Il n'est **pas** durable : un arrêt brutal le perd.
#[derive(Debug, Default)]
pub struct MemoryJournal {
    lines: Vec<JournalLine>,
    fail_after: Option<usize>,
}

impl MemoryJournal {
    /// Journal vide.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Journal qui échoue (`Io`) dès qu'il contient `count` enregistrements
    /// (simulation de disque plein ou de support retiré).
    #[must_use]
    pub fn failing_after(count: usize) -> Self {
        Self {
            lines: Vec::new(),
            fail_after: Some(count),
        }
    }

    /// Lignes enregistrées.
    #[must_use]
    pub fn lines(&self) -> &[JournalLine] {
        &self.lines
    }

    /// Relecture (même analyse que pour un fichier).
    #[must_use]
    pub fn replay(&self) -> Replay {
        Replay {
            lines: self.lines.clone(),
            truncated_tail: false,
        }
    }
}

impl JournalSink for MemoryJournal {
    fn append(&mut self, record: JournalRecord) -> Result<(), JournalError> {
        if self
            .fail_after
            .is_some_and(|limit| self.lines.len() >= limit)
        {
            return Err(JournalError::Io(io::Error::from(
                io::ErrorKind::StorageFull,
            )));
        }
        let sequence = self.lines.len() as u64;
        self.lines.push(line(sequence, record));
        Ok(())
    }
}

/// Journal relu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replay {
    lines: Vec<JournalLine>,
    truncated_tail: bool,
}

/// Opération commencée mais sans issue consignée (exécution interrompue).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InFlight {
    /// Physical receiving endpoint (destination for legacy journals).
    pub target: PlanSide,
    /// Opération.
    pub operation: OperationId,
    /// Chemin.
    pub path: ResourcePath,
    /// Action.
    pub action: Action,
    /// Dépôt temporaire créé, susceptible de subsister à la destination.
    pub staging: Option<ResourcePath>,
    /// Version conservée avant l'interruption.
    pub version: Option<ResourcePath>,
}

/// Diagnostic de reprise (lecture seule : rien n'est corrigé).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RecoveryReport {
    /// Exécution.
    pub execution: Option<ExecutionId>,
    /// Plan.
    pub plan: Option<PlanId>,
    /// Statut final consigné ; `None` : exécution interrompue.
    pub finished: Option<ExecutionStatus>,
    /// Décomptes recalculés depuis les issues consignées.
    pub totals: JournalTotals,
    /// Opérations commencées sans issue.
    pub in_flight: Vec<InFlight>,
    /// Mutations prévues par le plan (consignées au début de l'exécution).
    pub mutations_planned: u64,
    /// Vrai si la dernière ligne était incomplète.
    pub truncated_tail: bool,
}

impl RecoveryReport {
    /// Vrai si l'exécution ne s'est pas terminée proprement.
    #[must_use]
    pub fn is_interrupted(&self) -> bool {
        self.finished.is_none()
    }
}

impl Replay {
    /// Lignes lues.
    #[must_use]
    pub fn lines(&self) -> &[JournalLine] {
        &self.lines
    }

    /// Vrai si une dernière ligne incomplète a été ignorée.
    #[must_use]
    pub fn truncated_tail(&self) -> bool {
        self.truncated_tail
    }

    /// Analyse de reprise.
    #[must_use]
    pub fn recovery(&self) -> RecoveryReport {
        let mut report = RecoveryReport {
            truncated_tail: self.truncated_tail,
            ..RecoveryReport::default()
        };
        let mut open: Vec<InFlight> = Vec::new();
        let mut directions = std::collections::BTreeMap::new();
        for entry in &self.lines {
            match &entry.record {
                JournalRecord::OperationDirection { operation, target } => {
                    directions.insert(operation.clone(), *target);
                }
                JournalRecord::ExecutionStarted {
                    execution,
                    plan,
                    mutations,
                    ..
                } => {
                    report.execution = Some(execution.clone());
                    report.plan = Some(plan.clone());
                    report.mutations_planned = *mutations;
                }
                JournalRecord::OperationStarted {
                    operation,
                    path,
                    action,
                } => open.push(InFlight {
                    target: directions
                        .get(operation)
                        .copied()
                        .unwrap_or(PlanSide::Destination),
                    operation: operation.clone(),
                    path: path.clone(),
                    action: *action,
                    staging: None,
                    version: None,
                }),
                JournalRecord::StagingCreated { operation, staging } => {
                    if let Some(op) = open.iter_mut().find(|op| &op.operation == operation) {
                        op.staging = Some(staging.clone());
                    }
                }
                JournalRecord::VersionPreserved {
                    operation,
                    location,
                } => {
                    if let Some(op) = open.iter_mut().find(|op| &op.operation == operation) {
                        op.version = Some(location.clone());
                    }
                }
                JournalRecord::OperationFinished {
                    operation,
                    outcome,
                    bytes,
                    ..
                } => {
                    open.retain(|op| &op.operation != operation);
                    let totals = &mut report.totals;
                    match outcome {
                        OutcomeCode::Done => totals.done += 1,
                        OutcomeCode::NoAction => totals.no_action += 1,
                        OutcomeCode::Refused => totals.refused += 1,
                        OutcomeCode::Failed => totals.failed += 1,
                        OutcomeCode::Cancelled => totals.cancelled += 1,
                        OutcomeCode::NotStarted => totals.not_started += 1,
                    }
                    totals.bytes = totals.bytes.saturating_add(*bytes);
                }
                JournalRecord::ExecutionFinished { status, .. } => {
                    report.finished = Some(*status);
                }
            }
        }
        report.in_flight = open;
        report
    }
}

/// Relit un journal depuis un texte JSON Lines.
///
/// # Errors
/// [`JournalError::Corrupt`], [`JournalError::UnsupportedVersion`] ou
/// [`JournalError::Sequence`].
pub fn parse_journal(text: &str) -> Result<Replay, JournalError> {
    let mut lines = Vec::new();
    let mut truncated_tail = false;
    let ends_with_newline = text.ends_with('\n');
    let raw: Vec<&str> = text.lines().collect();
    for (index, raw_line) in raw.iter().enumerate() {
        if raw_line.trim().is_empty() {
            continue;
        }
        let is_last = index + 1 == raw.len();
        let value: serde_json::Value = match serde_json::from_str(raw_line) {
            Ok(value) => value,
            Err(_) if is_last && !ends_with_newline => {
                truncated_tail = true;
                break;
            }
            Err(_) => return Err(JournalError::Corrupt { line: index + 1 }),
        };
        let version = value
            .get("format_version")
            .and_then(serde_json::Value::as_u64)
            .ok_or(JournalError::Corrupt { line: index + 1 })?;
        if version != u64::from(JOURNAL_FORMAT_VERSION) {
            return Err(JournalError::UnsupportedVersion {
                found: u32::try_from(version).unwrap_or(u32::MAX),
            });
        }
        let entry: JournalLine =
            serde_json::from_value(value).map_err(|_| JournalError::Corrupt { line: index + 1 })?;
        let expected = lines.len() as u64;
        if entry.sequence != expected {
            return Err(JournalError::Sequence {
                expected,
                found: entry.sequence,
            });
        }
        lines.push(entry);
    }
    Ok(Replay {
        lines,
        truncated_tail,
    })
}

/// Relit un journal depuis un fichier.
///
/// # Errors
/// [`JournalError::Io`] si le fichier est illisible (ou non UTF-8), puis
/// les erreurs de [`parse_journal`].
pub fn read_journal(path: &Path) -> Result<Replay, JournalError> {
    let text = std::fs::read_to_string(path).map_err(JournalError::Io)?;
    parse_journal(&text)
}
