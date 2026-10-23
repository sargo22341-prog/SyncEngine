//! Opérations d'un plan : action, motif, préconditions, observations.

use std::fmt;

use serde::{Deserialize, Serialize};
use sync_core::{EntryKind, FileEntry, FileMetadata, OperationId, ResourcePath};

use crate::policy::DeletionRefusal;

/// Action prévue pour un chemin.
///
/// Seules [`Action::Copy`], [`Action::Replace`] et [`Action::Delete`]
/// prévoient une mutation ; les autres sont des constats explicites.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Action {
    /// Créer à la destination une entrée absente (fichier copié ou
    /// répertoire créé).
    Copy,
    /// Remplacer le contenu d'un fichier existant de la destination.
    Replace,
    /// Ne rien faire : la destination est conservée telle quelle.
    Keep,
    /// Supprimer une entrée de la destination (un répertoire n'est supprimé
    /// que vide, jamais récursivement).
    Delete,
    /// Conflit non résolu : rien n'est écrit, une intervention est requise.
    Conflict,
    /// Chemin non traité (incertitude, nature non prise en charge,
    /// suppression refusée…) : rien n'est écrit, le motif l'explique.
    Skip,
}

impl Action {
    /// Toutes les actions, dans l'ordre d'affichage.
    pub const ALL: [Self; 6] = [
        Self::Copy,
        Self::Replace,
        Self::Keep,
        Self::Delete,
        Self::Conflict,
        Self::Skip,
    ];

    /// Vrai si l'action prévoit une mutation de la destination.
    #[must_use]
    pub fn is_mutation(self) -> bool {
        matches!(self, Self::Copy | Self::Replace | Self::Delete)
    }

    /// Risque associé.
    #[must_use]
    pub fn risk(self) -> Risk {
        match self {
            Self::Copy => Risk::Additive,
            Self::Replace => Risk::Overwrite,
            Self::Delete => Risk::Destructive,
            Self::Keep | Self::Conflict | Self::Skip => Risk::None,
        }
    }

    /// Préconditions à réévaluer juste avant la mutation.
    #[must_use]
    pub fn preconditions(self) -> &'static [Precondition] {
        match self {
            Self::Copy => &[
                Precondition::SourceUnchanged,
                Precondition::DestinationAbsent,
            ],
            Self::Replace => &[
                Precondition::SourceUnchanged,
                Precondition::DestinationUnchanged,
            ],
            Self::Delete => &[
                Precondition::SourceAbsent,
                Precondition::DestinationUnchanged,
            ],
            Self::Keep | Self::Conflict | Self::Skip => &[],
        }
    }

    /// Code stable (forme sérialisée).
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::Copy => "copy",
            Self::Replace => "replace",
            Self::Keep => "keep",
            Self::Delete => "delete",
            Self::Conflict => "conflict",
            Self::Skip => "skip",
        }
    }
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

/// Risque d'une opération pour les données de la destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    /// Aucune écriture.
    None,
    /// Création d'une entrée absente.
    Additive,
    /// Écrasement d'un contenu existant.
    Overwrite,
    /// Suppression d'une entrée existante.
    Destructive,
}

/// Condition à revérifier juste avant une mutation (phase 04). Si elle
/// n'est plus vraie, l'opération doit être refusée, jamais forcée.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Precondition {
    /// La source correspond encore à l'observation enregistrée (nature,
    /// taille, date à la précision déclarée près, empreinte si présente).
    SourceUnchanged,
    /// La source est toujours absente (une suppression ne se fonde jamais
    /// sur une absence non revérifiée).
    SourceAbsent,
    /// La destination est toujours absente (pas d'écrasement implicite).
    DestinationAbsent,
    /// La destination correspond encore à l'observation enregistrée.
    DestinationUnchanged,
}

/// Côté d'un plan unidirectionnel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanSide {
    /// Source.
    Source,
    /// Destination.
    Destination,
}

/// Cause d'incertitude reprise du constat de comparaison.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "cause", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum UncertainCause {
    /// Présence inconnue d'un côté (dossier illisible, lien non suivi…).
    PresenceUnknown {
        /// Côté concerné.
        side: PlanSide,
    },
    /// Chemin exclu par le filtre d'un côté.
    Excluded {
        /// Côté concerné.
        side: PlanSide,
    },
    /// Taille inconnue.
    SizeUnknown,
    /// Date inconnue.
    ModifiedUnknown,
    /// Précision des dates inconnue.
    PrecisionUnknown,
    /// Liens ou entrées spéciales, non comparés.
    NotComparable,
    /// Observations contradictoires.
    InconsistentObservations,
    /// Collision de casse sur un stockage insensible à la casse.
    CaseCollision,
    /// Cause non reconnue par cette version (constat plus récent).
    Other,
}

/// Motif d'une opération. Chaque motif n'est admis qu'avec certaines
/// actions ([`Reason::allows`]).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum Reason {
    /// Absent de la destination (absence établie) ⇒ copie.
    MissingAtDestination,
    /// La source est plus récente ⇒ remplacement.
    SourceNewer,
    /// La destination est plus récente : conflit, ou remplacement si la
    /// politique fait gagner la source.
    DestinationNewer,
    /// Contenu différent sans date exploitable, remplacé parce que la
    /// politique fait gagner la source.
    ContentDiffers,
    /// Contenu différent sans date exploitable (dates égales ou inconnues) :
    /// impossible de savoir quel côté a changé ⇒ conflit.
    AmbiguousChange,
    /// Natures différentes (fichier contre répertoire…) ⇒ conflit.
    KindMismatch {
        /// Nature à la source.
        source: EntryKind,
        /// Nature à la destination.
        destination: EntryKind,
    },
    /// Équivalents.
    Equivalent {
        /// Vrai si le contenu a été vérifié par empreinte.
        verified: bool,
    },
    /// Présent seulement à la destination ; la stratégie le conserve
    /// (sauvegarde).
    DestinationOnly,
    /// Différent, mais la politique interdit le remplacement.
    OverwriteDisabled,
    /// Absent de la source, mais la politique n'autorise pas les suppressions.
    DeletionDisabled,
    /// Absent de la source (absence établie) ⇒ suppression.
    AbsentFromSource,
    /// Constat incertain : jamais d'écriture ni de suppression.
    Uncertain {
        /// Causes (non vide).
        causes: Vec<UncertainCause>,
    },
    /// Nature non prise en charge (lien symbolique, entrée spéciale).
    UnsupportedEntryKind {
        /// Nature rencontrée.
        kind: EntryKind,
    },
    /// Le répertoire parent ne sera pas disponible à la destination.
    ParentUnavailable,
    /// Répertoire dont une partie du contenu est conservée : il ne peut pas
    /// être supprimé.
    DirectoryNotEmptied,
    /// Suppression refusée par un garde-fou.
    DeletionRefused {
        /// Garde-fou en cause.
        refusal: DeletionRefusal,
    },
}

impl Reason {
    /// Vrai si ce motif est admis avec `action`.
    #[must_use]
    pub fn allows(&self, action: Action) -> bool {
        match self {
            Self::MissingAtDestination => action == Action::Copy,
            Self::SourceNewer | Self::ContentDiffers => action == Action::Replace,
            Self::DestinationNewer => matches!(action, Action::Replace | Action::Conflict),
            Self::AmbiguousChange | Self::KindMismatch { .. } => action == Action::Conflict,
            Self::Equivalent { .. }
            | Self::DestinationOnly
            | Self::OverwriteDisabled
            | Self::DeletionDisabled => action == Action::Keep,
            Self::AbsentFromSource => action == Action::Delete,
            Self::Uncertain { .. }
            | Self::UnsupportedEntryKind { .. }
            | Self::ParentUnavailable
            | Self::DirectoryNotEmptied
            | Self::DeletionRefused { .. } => action == Action::Skip,
        }
    }

    /// Côtés qui doivent avoir été observés pour ce motif.
    pub(crate) fn shape(&self) -> Shape {
        match self {
            Self::MissingAtDestination => Shape::SourceOnly,
            Self::SourceNewer
            | Self::DestinationNewer
            | Self::ContentDiffers
            | Self::AmbiguousChange
            | Self::KindMismatch { .. }
            | Self::Equivalent { .. }
            | Self::OverwriteDisabled => Shape::Both,
            Self::DestinationOnly
            | Self::DeletionDisabled
            | Self::AbsentFromSource
            | Self::DirectoryNotEmptied
            | Self::DeletionRefused { .. } => Shape::DestinationOnly,
            Self::Uncertain { .. }
            | Self::UnsupportedEntryKind { .. }
            | Self::ParentUnavailable => Shape::Any,
        }
    }
}

/// Côtés observés attendus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Shape {
    SourceOnly,
    DestinationOnly,
    Both,
    Any,
}

impl Shape {
    pub(crate) fn accepts(self, source: bool, destination: bool) -> bool {
        match self {
            Self::SourceOnly => source && !destination,
            Self::DestinationOnly => !source && destination,
            Self::Both => source && destination,
            Self::Any => source || destination,
        }
    }
}

/// Observation d'une entrée au moment du scan (base des préconditions).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntrySnapshot {
    kind: EntryKind,
    metadata: FileMetadata,
}

impl EntrySnapshot {
    /// Construit une observation.
    #[must_use]
    pub fn new(kind: EntryKind, metadata: FileMetadata) -> Self {
        Self { kind, metadata }
    }

    /// Nature observée.
    #[must_use]
    pub fn kind(&self) -> EntryKind {
        self.kind
    }

    /// Métadonnées observées (taille, date et précision, empreinte).
    #[must_use]
    pub fn metadata(&self) -> &FileMetadata {
        &self.metadata
    }
}

impl From<&FileEntry> for EntrySnapshot {
    fn from(entry: &FileEntry) -> Self {
        Self::new(entry.kind(), entry.metadata().clone())
    }
}

/// Description d'une opération à planifier, avant numérotation.
///
/// [`crate::ExecutionPlan::new`] dérive le reste (identifiant, nature,
/// risque, préconditions, octets, dépendances) et valide l'ensemble.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationSpec {
    /// Chemin logique (identique des deux côtés).
    pub path: ResourcePath,
    /// Action prévue.
    pub action: Action,
    /// Motif.
    pub reason: Reason,
    /// Observation à la source, si présente.
    pub source: Option<EntrySnapshot>,
    /// Observation à la destination, si présente.
    pub destination: Option<EntrySnapshot>,
}

/// Opération d'un plan.
///
/// Tous les champs autres que le chemin, l'action, le motif et les
/// observations sont **dérivés** et revérifiés au chargement d'un plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Operation {
    pub(crate) id: OperationId,
    pub(crate) path: ResourcePath,
    pub(crate) kind: EntryKind,
    pub(crate) action: Action,
    pub(crate) reason: Reason,
    pub(crate) risk: Risk,
    pub(crate) preconditions: Vec<Precondition>,
    pub(crate) depends_on: Vec<OperationId>,
    pub(crate) bytes: Option<u64>,
    pub(crate) source: Option<EntrySnapshot>,
    pub(crate) destination: Option<EntrySnapshot>,
}

impl Operation {
    /// Identifiant stable dans le plan.
    #[must_use]
    pub fn id(&self) -> &OperationId {
        &self.id
    }

    /// Chemin logique.
    #[must_use]
    pub fn path(&self) -> &ResourcePath {
        &self.path
    }

    /// Nature de l'entrée concernée (celle de la source pour une copie ou
    /// un remplacement, celle de la destination pour une suppression).
    #[must_use]
    pub fn kind(&self) -> EntryKind {
        self.kind
    }

    /// Action prévue.
    #[must_use]
    pub fn action(&self) -> Action {
        self.action
    }

    /// Motif.
    #[must_use]
    pub fn reason(&self) -> &Reason {
        &self.reason
    }

    /// Risque.
    #[must_use]
    pub fn risk(&self) -> Risk {
        self.risk
    }

    /// Préconditions à revérifier juste avant la mutation.
    #[must_use]
    pub fn preconditions(&self) -> &[Precondition] {
        &self.preconditions
    }

    /// Opérations à achever avant celle-ci (création du répertoire parent ;
    /// pour la suppression d'un répertoire, suppression de son contenu).
    #[must_use]
    pub fn depends_on(&self) -> &[OperationId] {
        &self.depends_on
    }

    /// Octets à transférer : taille du fichier source pour une copie ou un
    /// remplacement (`None` si inconnue), 0 sinon.
    #[must_use]
    pub fn bytes(&self) -> Option<u64> {
        self.bytes
    }

    /// Observation à la source.
    #[must_use]
    pub fn source(&self) -> Option<&EntrySnapshot> {
        self.source.as_ref()
    }

    /// Observation à la destination.
    #[must_use]
    pub fn destination(&self) -> Option<&EntrySnapshot> {
        self.destination.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutations_and_risks() {
        let mutations: Vec<Action> = Action::ALL
            .into_iter()
            .filter(|a| a.is_mutation())
            .collect();
        assert_eq!(mutations, [Action::Copy, Action::Replace, Action::Delete]);
        assert_eq!(Action::Delete.risk(), Risk::Destructive);
        assert_eq!(Action::Conflict.risk(), Risk::None);
        assert!(Action::Skip.preconditions().is_empty());
        assert!(
            Action::Delete
                .preconditions()
                .contains(&Precondition::SourceAbsent)
        );
        for action in Action::ALL {
            assert_eq!(
                serde_json::to_string(&action).unwrap(),
                format!("\"{action}\"")
            );
        }
    }

    #[test]
    fn reasons_are_bound_to_actions() {
        assert!(Reason::AbsentFromSource.allows(Action::Delete));
        assert!(!Reason::AbsentFromSource.allows(Action::Skip));
        assert!(Reason::DestinationNewer.allows(Action::Conflict));
        assert!(Reason::DestinationNewer.allows(Action::Replace));
        let refused = Reason::DeletionRefused {
            refusal: DeletionRefusal::SourceIncomplete,
        };
        assert!(refused.allows(Action::Skip));
        assert!(!refused.allows(Action::Delete));
        for action in Action::ALL {
            let allowed = Reason::Uncertain { causes: vec![] }.allows(action);
            assert_eq!(allowed, action == Action::Skip, "{action}");
        }
    }

    #[test]
    fn reason_serialization_is_tagged() {
        let reason = Reason::Uncertain {
            causes: vec![UncertainCause::PresenceUnknown {
                side: PlanSide::Source,
            }],
        };
        let json = serde_json::to_string(&reason).unwrap();
        assert_eq!(
            json,
            r#"{"code":"uncertain","causes":[{"cause":"presence_unknown","side":"source"}]}"#
        );
        assert_eq!(serde_json::from_str::<Reason>(&json).unwrap(), reason);
        let refused = Reason::DeletionRefused {
            refusal: DeletionRefusal::TooMany { count: 3, limit: 2 },
        };
        let json = serde_json::to_string(&refused).unwrap();
        assert_eq!(
            json,
            r#"{"code":"deletion_refused","refusal":{"cause":"too_many","count":3,"limit":2}}"#
        );
        assert!(serde_json::from_str::<Reason>(r#"{"code":"inconnu"}"#).is_err());
    }
}
