//! Options d'exécution.

use std::time::Duration;

use sync_copy::DEFAULT_BUFFER_SIZE;
use sync_verify::VerifyMode;

/// Âge maximal par défaut d'un plan à l'exécution (1 heure).
pub const DEFAULT_MAX_PLAN_AGE: Duration = Duration::from_secs(3600);

/// Avance maximale tolérée de la date de création d'un plan sur l'horloge
/// locale (5 minutes).
pub const DEFAULT_MAX_CLOCK_SKEW: Duration = Duration::from_secs(300);

/// Options d'exécution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct ExecutionOptions {
    /// Niveau de vérification des fichiers transférés (défaut :
    /// [`VerifyMode::Content`]).
    pub verify: VerifyMode,
    /// Confirmation **explicite** des suppressions prévues par le plan.
    /// Faux par défaut : un plan qui contient des suppressions est alors
    /// refusé avant toute mutation.
    pub confirm_deletions: bool,
    /// Âge maximal du plan ; au-delà, il doit être reconstruit.
    pub max_plan_age: Duration,
    /// Avance maximale tolérée de la date de création du plan.
    pub max_clock_skew: Duration,
    /// Taille du tampon de transfert.
    pub buffer_size: usize,
    /// Octets transférés entre deux événements de progression (en plus d'un
    /// événement par opération).
    pub progress_interval_bytes: u64,
}

impl Default for ExecutionOptions {
    fn default() -> Self {
        Self {
            verify: VerifyMode::default(),
            confirm_deletions: false,
            max_plan_age: DEFAULT_MAX_PLAN_AGE,
            max_clock_skew: DEFAULT_MAX_CLOCK_SKEW,
            buffer_size: DEFAULT_BUFFER_SIZE,
            progress_interval_bytes: 8 * 1024 * 1024,
        }
    }
}

impl ExecutionOptions {
    /// Options par défaut.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Niveau de vérification.
    #[must_use]
    pub fn with_verify(mut self, verify: VerifyMode) -> Self {
        self.verify = verify;
        self
    }

    /// Confirme (ou non) les suppressions prévues.
    #[must_use]
    pub fn with_deletions_confirmed(mut self, confirmed: bool) -> Self {
        self.confirm_deletions = confirmed;
        self
    }

    /// Âge maximal du plan.
    #[must_use]
    pub fn with_max_plan_age(mut self, age: Duration) -> Self {
        self.max_plan_age = age;
        self
    }

    /// Taille du tampon de transfert (au moins 1 octet).
    #[must_use]
    pub fn with_buffer_size(mut self, size: usize) -> Self {
        self.buffer_size = size.max(1);
        self
    }

    /// Intervalle des événements de progression, en octets (au moins 1).
    #[must_use]
    pub fn with_progress_interval(mut self, bytes: u64) -> Self {
        self.progress_interval_bytes = bytes.max(1);
        self
    }
}
