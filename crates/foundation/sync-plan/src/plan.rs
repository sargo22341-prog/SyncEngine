//! Plan d'exécution : en-tête, opérations, résumé.

use serde::{Deserialize, Serialize};
use sync_core::{ConnectorId, OperationId, PlanId, ProfileId, ResourcePath, Timestamp};
use sync_filter::FilterRules;

use crate::Baseline;
use crate::operation::{Action, Operation, OperationSpec, PlanSide, Reason};
use crate::policy::PlanPolicy;
use crate::validate::{PlanError, derive_operations, validate};

/// Version du schéma de plan produite par cette version du code.
///
/// À incrémenter pour toute évolution non rétrocompatible du format ; un
/// plan d'une autre version est refusé au chargement, jamais interprété.
pub const PLAN_SCHEMA_VERSION: u32 = 1;

/// Mode de synchronisation ayant produit le plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SyncMode {
    /// Bidirectional synchronization with reliable previous state.
    Bidirectional,
    /// Sauvegarde : source → destination, jamais de suppression.
    Backup,
    /// Miroir : la destination est rendue conforme à la source, suppressions
    /// comprises si la politique les autorise explicitement.
    Mirror,
}

/// Instantané d'une extrémité au moment de la planification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndpointSnapshot {
    connector: ConnectorId,
    location: String,
    complete: bool,
    entries: u64,
}

impl EndpointSnapshot {
    /// Décrit une extrémité. `location` est un libellé d'affichage (chemin
    /// local, URL sans identifiants) : il ne doit **jamais** contenir de
    /// secret. Le nombre d'entrées est calculé par le plan.
    #[must_use]
    pub fn new(connector: ConnectorId, location: impl Into<String>, complete: bool) -> Self {
        Self {
            connector,
            location: location.into(),
            complete,
            entries: 0,
        }
    }

    /// Connecteur.
    #[must_use]
    pub fn connector(&self) -> &ConnectorId {
        &self.connector
    }

    /// Libellé d'affichage.
    #[must_use]
    pub fn location(&self) -> &str {
        &self.location
    }

    /// Vrai si l'inventaire était complet.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.complete
    }

    /// Entrées observées (opérations dont ce côté est observé).
    #[must_use]
    pub fn entries(&self) -> u64 {
        self.entries
    }
}

/// En-tête fourni pour construire un plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanHeader {
    /// Identifiant du plan.
    pub id: PlanId,
    /// Profil d'origine, s'il y en a un.
    pub profile: Option<ProfileId>,
    /// Mode.
    pub mode: SyncMode,
    /// Instant de création (UTC).
    pub created_at: Timestamp,
    /// Source.
    pub source: EndpointSnapshot,
    /// Destination.
    pub destination: EndpointSnapshot,
    /// Règles de filtre appliquées aux deux inventaires : l'exécution doit
    /// utiliser exactement les mêmes.
    pub filter: FilterRules,
    /// Politique appliquée.
    pub policy: PlanPolicy,
}

/// Résumé d'un plan.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PlanSummary {
    /// Copies (créations).
    pub copy: u64,
    /// Remplacements.
    pub replace: u64,
    /// Conservations.
    pub keep: u64,
    /// Suppressions.
    pub delete: u64,
    /// Conflits non résolus.
    pub conflict: u64,
    /// Chemins sautés.
    pub skip: u64,
    /// Octets à transférer (tailles connues).
    pub bytes_to_transfer: u64,
    /// Copies ou remplacements de taille inconnue.
    pub unknown_sizes: u64,
    /// Suppressions refusées par un garde-fou.
    pub deletions_refused: u64,
    /// Chemins sautés pour incertitude.
    pub uncertain: u64,
}

impl PlanSummary {
    /// Nombre d'opérations portant `action`.
    #[must_use]
    pub fn count(&self, action: Action) -> u64 {
        match action {
            Action::Copy => self.copy,
            Action::Replace => self.replace,
            Action::Keep => self.keep,
            Action::Delete => self.delete,
            Action::Conflict => self.conflict,
            Action::Skip => self.skip,
        }
    }

    /// Nombre de mutations prévues.
    #[must_use]
    pub fn mutations(&self) -> u64 {
        self.copy + self.replace + self.delete
    }
}

/// Plan d'exécution immuable, sérialisable et versionné.
///
/// Le même plan sert à la prévisualisation et à l'exécution (phase 04),
/// qui doit revérifier les [`crate::Precondition`] de chaque opération juste
/// avant la mutation. Un plan ne peut être obtenu que par
/// [`ExecutionPlan::new`] ou par chargement ([`ExecutionPlan::from_json`]) :
/// dans les deux cas ses invariants sont vérifiés (voir [`PlanError`]),
/// notamment :
/// - une opération par chemin, triées par chemin, identifiants uniques ;
/// - action, motif, observations et champs dérivés cohérents ;
/// - aucune suppression en sauvegarde, sans politique explicite, avec un
///   inventaire incomplet ou au-delà des garde-fous ;
/// - copie seulement sous un répertoire disponible à la destination ;
///   suppression d'un répertoire seulement si tout son contenu l'est aussi.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawPlan")]
pub struct ExecutionPlan {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) baseline: Option<Baseline>,
    pub(crate) schema_version: u32,
    pub(crate) id: PlanId,
    pub(crate) profile: Option<ProfileId>,
    pub(crate) mode: SyncMode,
    pub(crate) created_at: Timestamp,
    pub(crate) source: EndpointSnapshot,
    pub(crate) destination: EndpointSnapshot,
    pub(crate) filter: FilterRules,
    pub(crate) policy: PlanPolicy,
    pub(crate) operations: Vec<Operation>,
}

/// Forme sérialisée brute, validée avant de devenir un [`ExecutionPlan`].
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawPlan {
    #[serde(default)]
    baseline: Option<Baseline>,
    schema_version: u32,
    id: PlanId,
    profile: Option<ProfileId>,
    mode: SyncMode,
    created_at: Timestamp,
    source: EndpointSnapshot,
    destination: EndpointSnapshot,
    filter: FilterRules,
    policy: PlanPolicy,
    operations: Vec<Operation>,
}

impl TryFrom<RawPlan> for ExecutionPlan {
    type Error = PlanError;

    fn try_from(raw: RawPlan) -> Result<Self, Self::Error> {
        let plan = Self {
            baseline: raw.baseline,
            schema_version: raw.schema_version,
            id: raw.id,
            profile: raw.profile,
            mode: raw.mode,
            created_at: raw.created_at,
            source: raw.source,
            destination: raw.destination,
            filter: raw.filter,
            policy: raw.policy,
            operations: raw.operations,
        };
        validate(&plan)?;
        Ok(plan)
    }
}

impl ExecutionPlan {
    /// Construit et valide un plan.
    ///
    /// Les opérations sont triées par chemin puis numérotées (`op-000001`…) ;
    /// nature, risque, préconditions, octets, dépendances et décomptes
    /// d'entrées sont dérivés.
    ///
    /// # Errors
    /// [`PlanError`] si un invariant n'est pas respecté (chemin en double,
    /// motif incompatible, suppression non autorisée…). Une stratégie
    /// correcte ne produit jamais d'erreur : elle transforme d'elle-même les
    /// opérations non admissibles en sauts motivés.
    pub fn new(header: PlanHeader, specs: Vec<OperationSpec>) -> Result<Self, PlanError> {
        Self::construct(
            header,
            specs
                .into_iter()
                .map(|s| (PlanSide::Destination, s))
                .collect(),
            None,
        )
    }

    /// Bidirectional v2 plan; the side associated with each description is
    /// the physical receiver. Observations and preconditions are relative
    /// to this direction. One-way modes reject reversed operations.
    ///
    /// # Errors
    /// `PlanError` si la direction, le mode ou un invariant est invalide.
    pub fn new_directed(
        header: PlanHeader,
        specs: Vec<(PlanSide, OperationSpec)>,
    ) -> Result<Self, PlanError> {
        Self::construct(header, specs, None)
    }

    /// Builds a v2 plan carrying its previous successful reference state.
    ///
    /// # Errors
    /// Rejects inconsistent reference identity or mutations without evidence.
    pub fn new_bidirectional(
        header: PlanHeader,
        specs: Vec<(PlanSide, OperationSpec)>,
        baseline: Option<Baseline>,
    ) -> Result<Self, PlanError> {
        Self::construct(header, specs, baseline)
    }

    /// Previous state used to justify bidirectional mutations, if available.
    #[must_use]
    pub fn baseline(&self) -> Option<&Baseline> {
        self.baseline.as_ref()
    }

    fn construct(
        header: PlanHeader,
        specs: Vec<(PlanSide, OperationSpec)>,
        baseline: Option<Baseline>,
    ) -> Result<Self, PlanError> {
        let operations = derive_operations(specs)?;
        let mut source = header.source;
        let mut destination = header.destination;
        source.entries = count(&operations, |op| op.observation(PlanSide::Source).is_some());
        destination.entries = count(&operations, |op| {
            op.observation(PlanSide::Destination).is_some()
        });
        let plan = Self {
            baseline,
            schema_version: if header.mode == SyncMode::Bidirectional {
                2
            } else {
                PLAN_SCHEMA_VERSION
            },
            id: header.id,
            profile: header.profile,
            mode: header.mode,
            created_at: header.created_at,
            source,
            destination,
            filter: header.filter,
            policy: header.policy,
            operations,
        };
        validate(&plan)?;
        Ok(plan)
    }

    /// Version du schéma.
    #[must_use]
    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Identifiant.
    #[must_use]
    pub fn id(&self) -> &PlanId {
        &self.id
    }

    /// Profil d'origine.
    #[must_use]
    pub fn profile(&self) -> Option<&ProfileId> {
        self.profile.as_ref()
    }

    /// Mode.
    #[must_use]
    pub fn mode(&self) -> SyncMode {
        self.mode
    }

    /// Instant de création.
    #[must_use]
    pub fn created_at(&self) -> Timestamp {
        self.created_at
    }

    /// Source.
    #[must_use]
    pub fn source(&self) -> &EndpointSnapshot {
        &self.source
    }

    /// Destination.
    #[must_use]
    pub fn destination(&self) -> &EndpointSnapshot {
        &self.destination
    }

    /// Règles de filtre appliquées.
    #[must_use]
    pub fn filter_rules(&self) -> &FilterRules {
        &self.filter
    }

    /// Politique appliquée.
    #[must_use]
    pub fn policy(&self) -> &PlanPolicy {
        &self.policy
    }

    /// Opérations, triées par chemin.
    #[must_use]
    pub fn operations(&self) -> &[Operation] {
        &self.operations
    }

    /// Opération portant sur `path`.
    #[must_use]
    pub fn get(&self, path: &ResourcePath) -> Option<&Operation> {
        self.operations
            .binary_search_by(|op| op.path.cmp(path))
            .ok()
            .and_then(|index| self.operations.get(index))
    }

    /// Opération d'identifiant `id`.
    #[must_use]
    pub fn operation(&self, id: &OperationId) -> Option<&Operation> {
        self.operations.iter().find(|op| &op.id == id)
    }

    /// Résumé.
    #[must_use]
    pub fn summary(&self) -> PlanSummary {
        let mut summary = PlanSummary::default();
        for op in &self.operations {
            match op.action {
                Action::Copy => summary.copy += 1,
                Action::Replace => summary.replace += 1,
                Action::Keep => summary.keep += 1,
                Action::Delete => summary.delete += 1,
                Action::Conflict => summary.conflict += 1,
                Action::Skip => summary.skip += 1,
            }
            if matches!(op.action, Action::Copy | Action::Replace) {
                match op.bytes {
                    Some(bytes) => summary.bytes_to_transfer += bytes,
                    None => summary.unknown_sizes += 1,
                }
            }
            match op.reason {
                Reason::DeletionRefused { .. } | Reason::DeletionDisabled => {
                    summary.deletions_refused += 1
                }
                Reason::Uncertain { .. } => summary.uncertain += 1,
                _ => {}
            }
        }
        summary
    }

    /// Vrai si le plan demande l'attention de l'utilisateur : conflits,
    /// chemins sautés (incertitudes, refus) ou inventaire incomplet.
    #[must_use]
    pub fn needs_attention(&self) -> bool {
        let summary = self.summary();
        summary.conflict + summary.skip > 0 || !self.source.complete || !self.destination.complete
    }
}

fn count(operations: &[Operation], predicate: impl Fn(&Operation) -> bool) -> u64 {
    operations.iter().filter(|op| predicate(op)).count() as u64
}
