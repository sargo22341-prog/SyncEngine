//! Exécuteur de plans.
//!
//! [`Executor`] (implémentation de [`PlanExecutor`]) applique un
//! [`sync_plan::ExecutionPlan`] **déjà validé** (même modèle que la
//! prévisualisation) à deux extrémités :
//!
//! 1. **contrôles préalables**, sans aucune mutation ([`ExecutionError`]) :
//!    âge du plan, connecteurs attendus, confirmation explicite des
//!    suppressions, capacités nécessaires, racines disponibles ;
//! 2. **copies, créations et remplacements**, selon les dépendances
//!    (un dossier avant son contenu, travaux indépendants concurrents) ;
//! 3. **suppressions**, en dernier et du plus profond au moins profond,
//!    seulement si l'exécution n'a pas été arrêtée et si la source est
//!    toujours lisible.
//!
//! Avant **chaque** mutation, les préconditions du plan sont revérifiées sur
//! l'état réel : source inchangée (ou toujours absente pour une
//! suppression), destination absente ou inchangée. Une précondition fausse
//! donne un **refus** motivé ([`Refusal`]) : rien n'est modifié, jamais
//! forcé. Un fichier est transféré dans un dépôt temporaire
//! ([`sync_copy`]), contrôlé (taille, empreinte, relecture selon
//! [`sync_verify::VerifyMode`]), sa source revérifiée, la destination
//! revérifiée, l'éventuelle version conservée ([`sync_versioning`]), puis
//! seulement validé.
//!
//! # Arrêts
//! Annulation, disque plein, extrémité déconnectée ou journal impossible à
//! écrire **arrêtent** l'exécution ([`StopCause`]) : l'opération en cours
//! est abandonnée (dépôt temporaire retiré), les suivantes sont marquées
//! non commencées et **aucune suppression** n'a lieu. Les autres échecs
//! (accès refusé sur un fichier…) sont rapportés et l'exécution continue.
//! L'état partiel est toujours rapporté ([`ExecutionReport`]) et consigné
//! ([`sync_journal`]).
//!
//! # Modèle d'exécution
//! Synchrone avec jusqu'à quatre copies indépendantes par défaut, bornées par
//! les deux connecteurs et [`ExecutionOptions::copy_workers`]. Le coordinateur
//! respecte les dépendances des dossiers et la capacité du magasin de versions ;
//! un seul gros fichier est transféré à la fois par défaut
//! ([`ExecutionOptions::large_transfers`]), les petits occupant les autres workers ;
//! les suppressions attendent la jonction de tous les workers.
//! Le bidirectionnel conserve son parcours séquentiel. Le journal est sérialisé
//! sans retirer ses barrières de durabilité. Un appelant async utilise un fil dédié.
//! Un jeton enfant permet d'arrêter les workers sans annuler le jeton appelant ;
//! une erreur fatale prime sur l'annulation de ses workers frères. L'annulation
//! est coopérative ([`sync_core::CancellationToken`]) : consultée avant
//! chaque opération et à chaque bloc transféré.
//!
//! # Limites connues (phase 04)
//! - pas de verrou contre deux exécutions concurrentes sur la même
//!   destination (les préconditions revérifiées limitent les dégâts) ;
//! - une fenêtre subsiste entre la dernière revérification et l'appel
//!   système de mutation ;
//! - pas de reprise automatique : une exécution interrompue se diagnostique
//!   par son journal, puis se relance par un nouveau plan.

mod error;
mod options;
mod ownership;
mod preflight;
mod report;
mod run;

use std::fmt;

use connector_api::StorageConnector;
use sync_core::{CancellationToken, ExecutionId, Timestamp};
use sync_events::EventEmitter;
use sync_journal::JournalSink;
use sync_plan::ExecutionPlan;
use sync_versioning::VersionStore;

pub use error::ExecutionError;
pub use options::{
    DEFAULT_MAX_CLOCK_SKEW, DEFAULT_MAX_PLAN_AGE, ExecutionOptions, LARGE_TRANSFER_BYTES,
};
pub use ownership::{default_lock_root, execution_lock};
pub use report::{ExecutionReport, OperationOutcome, OutcomeStatus, Refusal, StopCause};
pub use sync_journal::JournalTotals as ExecutionTotals;

/// Demande d'exécution d'un plan.
pub struct ExecutionRequest<'a> {
    plan: &'a ExecutionPlan,
    source: &'a dyn StorageConnector,
    destination: &'a dyn StorageConnector,
    execution_id: ExecutionId,
    journal: &'a mut dyn JournalSink,
    cancel: &'a CancellationToken,
    options: ExecutionOptions,
    versions: Option<&'a VersionStore<'a>>,
    events: Option<&'a EventEmitter>,
    now: Timestamp,
}

impl fmt::Debug for ExecutionRequest<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExecutionRequest")
            .field("plan", self.plan.id())
            .field("source", self.source.connector_id())
            .field("destination", self.destination.connector_id())
            .field("execution_id", &self.execution_id)
            .field("options", &self.options)
            .field("versions", &self.versions)
            .field("now", &self.now)
            .finish_non_exhaustive()
    }
}

impl<'a> ExecutionRequest<'a> {
    /// Exécution de `plan` de `source` (lue seulement) vers `destination`.
    ///
    /// Options par défaut ([`ExecutionOptions::default`] : suppressions
    /// **non confirmées**), sans conservation de versions ni événements,
    /// âge du plan évalué à l'instant courant.
    ///
    /// La source peut (et devrait) être ouverte en lecture seule :
    /// l'exécuteur ne lui demande jamais de mutation.
    #[must_use]
    pub fn new(
        plan: &'a ExecutionPlan,
        source: &'a dyn StorageConnector,
        destination: &'a dyn StorageConnector,
        execution_id: ExecutionId,
        journal: &'a mut dyn JournalSink,
        cancel: &'a CancellationToken,
    ) -> Self {
        Self {
            plan,
            source,
            destination,
            execution_id,
            journal,
            cancel,
            options: ExecutionOptions::default(),
            versions: None,
            events: None,
            // Horloge inutilisable : l'époque Unix fait paraître le plan
            // « venu du futur », donc refusé (direction sûre).
            now: Timestamp::now().unwrap_or(Timestamp::UNIX_EPOCH),
        }
    }

    /// Options d'exécution.
    #[must_use]
    pub fn with_options(mut self, options: ExecutionOptions) -> Self {
        self.options = options;
        self
    }

    /// Conserve une version de chaque fichier remplacé ou supprimé.
    #[must_use]
    pub fn with_versions(mut self, versions: &'a VersionStore<'a>) -> Self {
        self.versions = Some(versions);
        self
    }

    /// Diffuse statut, progression et diagnostics (informatifs).
    #[must_use]
    pub fn with_events(mut self, events: &'a EventEmitter) -> Self {
        self.events = Some(events);
        self
    }

    /// Instant de référence pour l'âge du plan (tests, rejouabilité).
    #[must_use]
    pub fn at(mut self, now: Timestamp) -> Self {
        self.now = now;
        self
    }
}

/// Exécuteur de plans.
pub trait PlanExecutor: Send + Sync {
    /// Exécute la demande.
    ///
    /// # Préconditions
    /// Le plan est validé par construction ; les connecteurs sont ceux que
    /// le plan désigne ; la destination n'est imbriquée ni dans la source ni
    /// dans le magasin de versions (vérifié par l'appelant, qui connaît les
    /// chemins concrets).
    ///
    /// # Errors
    /// [`ExecutionError`] si un contrôle préalable échoue : **aucune**
    /// mutation n'a eu lieu. Une fois l'exécution commencée, les échecs sont
    /// rapportés dans l'[`ExecutionReport`] (statut `failed`, `cancelled`…),
    /// jamais masqués.
    fn execute(&self, request: ExecutionRequest<'_>) -> Result<ExecutionReport, ExecutionError>;
}

/// Exécuteur à parallélisme borné (voir la documentation du crate).
#[derive(Debug, Clone, Copy, Default)]
pub struct Executor;

impl Executor {
    /// Exécuteur.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl PlanExecutor for Executor {
    fn execute(&self, request: ExecutionRequest<'_>) -> Result<ExecutionReport, ExecutionError> {
        preflight::check(&request)?;
        run::start(request)
    }
}
