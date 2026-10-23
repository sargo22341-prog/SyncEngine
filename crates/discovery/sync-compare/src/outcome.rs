//! Modèle des constats de comparaison.

use std::fmt;

use sync_core::{EntryKind, FileEntry, ResourcePath};
use sync_scanner::UnknownReason;

/// Côté d'une comparaison. « Gauche » et « droite » ne préjugent d'aucune
/// direction : la stratégie (phase 03) décide qui est source ou destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Side {
    /// Premier inventaire.
    Left,
    /// Second inventaire.
    Right,
}

/// Fondement d'une équivalence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum EquivalenceBasis {
    /// Deux répertoires (leur contenu est comparé chemin par chemin).
    Directory,
    /// Empreintes de contenu identiques, même algorithme : contenu vérifié.
    ContentHash,
    /// Même taille et même date à la précision commune près. Le contenu
    /// **n'est pas** vérifié : une modification qui conserve taille et date
    /// passerait inaperçue.
    SizeAndModified,
}

impl EquivalenceBasis {
    /// Vrai si le contenu a été vérifié par empreinte.
    #[must_use]
    pub fn is_content_verified(self) -> bool {
        matches!(self, Self::ContentHash)
    }
}

/// Différence constatée entre deux entrées présentes des deux côtés.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Difference {
    /// Natures différentes (fichier contre répertoire…).
    Kind {
        /// Nature à gauche.
        left: EntryKind,
        /// Nature à droite.
        right: EntryKind,
    },
    /// Tailles différentes.
    Size {
        /// Taille à gauche.
        left: u64,
        /// Taille à droite.
        right: u64,
    },
    /// Dates de modification différentes au-delà de la précision commune.
    /// « Plus récent » ne dit rien de l'intention : c'est un constat d'horloge.
    Modified {
        /// Côté dont la date est la plus récente.
        newer: Side,
    },
    /// Empreintes de contenu différentes (même algorithme).
    ContentHash,
}

/// Raison d'une incertitude.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Uncertainty {
    /// Présence inconnue d'un côté (listing incomplet, ancêtre non suivi).
    PresenceUnknown {
        /// Côté où la présence est inconnue.
        side: Side,
        /// Raison donnée par l'inventaire.
        reason: UnknownReason,
    },
    /// Le chemin est hors du filtre d'un côté (par exemple un répertoire
    /// exclu portant le nom d'un fichier inclus de l'autre côté).
    ExcludedOnSide {
        /// Côté où le chemin est exclu.
        side: Side,
    },
    /// Taille inconnue d'au moins un côté.
    SizeUnknown,
    /// Date inconnue d'au moins un côté.
    ModifiedUnknown,
    /// Précision des dates inconnue : les dates ne permettent aucune conclusion.
    PrecisionUnknown,
    /// Liens symboliques ou entrées spéciales : leur cible ou contenu n'est
    /// pas comparé.
    NotComparable {
        /// Nature commune des deux entrées.
        kind: EntryKind,
    },
    /// Empreintes identiques mais tailles différentes : observations
    /// contradictoires (fichier modifié entre deux lectures ?).
    InconsistentObservations,
    /// D'autres chemins ne diffèrent de celui-ci que par la casse, alors
    /// qu'au moins un côté n'est pas (ou pas sûrement) sensible à la casse.
    CaseCollision {
        /// Chemins en collision (hors celui-ci), triés.
        with: Vec<ResourcePath>,
    },
}

/// Constat pour un chemin.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ComparisonOutcome {
    /// Présent à gauche ; absence **établie** à droite.
    LeftOnly,
    /// Présent à droite ; absence **établie** à gauche.
    RightOnly,
    /// Équivalents selon le fondement indiqué.
    Equivalent(EquivalenceBasis),
    /// Présents des deux côtés et différents (liste non vide, dans l'ordre
    /// contenu, taille, date).
    Different(Vec<Difference>),
    /// Aucune conclusion sûre (liste non vide). Une stratégie ne doit
    /// **jamais** en déduire une suppression ou un écrasement.
    Uncertain(Vec<Uncertainty>),
}

impl ComparisonOutcome {
    /// Catégorie résumée.
    #[must_use]
    pub fn category(&self) -> OutcomeCategory {
        match self {
            Self::LeftOnly => OutcomeCategory::LeftOnly,
            Self::RightOnly => OutcomeCategory::RightOnly,
            Self::Equivalent(_) => OutcomeCategory::Equivalent,
            Self::Different(_) => OutcomeCategory::Different,
            Self::Uncertain(_) => OutcomeCategory::Uncertain,
        }
    }

    /// Vrai si le constat est incertain.
    #[must_use]
    pub fn is_uncertain(&self) -> bool {
        matches!(self, Self::Uncertain(_))
    }
}

/// Catégorie d'un constat, pour les résumés et filtres d'affichage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum OutcomeCategory {
    /// Voir [`ComparisonOutcome::LeftOnly`].
    LeftOnly,
    /// Voir [`ComparisonOutcome::RightOnly`].
    RightOnly,
    /// Voir [`ComparisonOutcome::Equivalent`].
    Equivalent,
    /// Voir [`ComparisonOutcome::Different`].
    Different,
    /// Voir [`ComparisonOutcome::Uncertain`].
    Uncertain,
}

impl fmt::Display for OutcomeCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::LeftOnly => "gauche seulement",
            Self::RightOnly => "droite seulement",
            Self::Equivalent => "équivalent",
            Self::Different => "différent",
            Self::Uncertain => "incertain",
        })
    }
}

/// Constat pour un chemin, avec les entrées observées de chaque côté.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathComparison {
    pub(crate) path: ResourcePath,
    pub(crate) left: Option<FileEntry>,
    pub(crate) right: Option<FileEntry>,
    pub(crate) outcome: ComparisonOutcome,
}

impl PathComparison {
    /// Chemin logique comparé.
    #[must_use]
    pub fn path(&self) -> &ResourcePath {
        &self.path
    }

    /// Entrée observée à gauche, si présente.
    #[must_use]
    pub fn left(&self) -> Option<&FileEntry> {
        self.left.as_ref()
    }

    /// Entrée observée à droite, si présente.
    #[must_use]
    pub fn right(&self) -> Option<&FileEntry> {
        self.right.as_ref()
    }

    /// Constat.
    #[must_use]
    pub fn outcome(&self) -> &ComparisonOutcome {
        &self.outcome
    }
}
