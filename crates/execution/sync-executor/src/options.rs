//! Options d'exécution.

use std::time::Duration;

use sync_copy::DEFAULT_BUFFER_SIZE;
use sync_verify::VerifyMode;

/// Taille à partir de laquelle un transfert est « gros » : son débit, pas
/// la latence d'ouverture, domine (voir [`ExecutionOptions::large_transfers`]).
pub const LARGE_TRANSFER_BYTES: u64 = 8 * 1024 * 1024;

/// Âge maximal par défaut d'un plan à l'exécution (1 heure).
pub const DEFAULT_MAX_PLAN_AGE: Duration = Duration::from_secs(3600);

/// Avance maximale tolérée de la date de création d'un plan sur l'horloge
/// locale (5 minutes).
pub const DEFAULT_MAX_CLOCK_SKEW: Duration = Duration::from_secs(300);

/// Options d'exécution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct ExecutionOptions {
    /// For planned mirror file copies/replacements, refresh the source metadata
    /// immediately before reading. A growing file is copied up to that initial
    /// size; changed metadata requires a second hash of the copied prefix.
    /// Rewritten prefixes and truncations are still refused.
    /// Other modes and all destination/deletion preconditions stay strict.
    pub refresh_mirror_source: bool,
    /// Requested workers (default 4), capped at 16 and by both endpoints.
    /// Independent copies/replacements respect parent dependencies; the version
    /// store further bounds concurrency. Deletions and bidirectional runs are ordered.
    pub copy_workers: std::num::NonZeroUsize,
    /// Gros transferts (au moins [`LARGE_TRANSFER_BYTES`]) menés en même
    /// temps parmi les workers (défaut 1, comme les « opérations parallèles »
    /// de FreeFileSync). Plusieurs gros flux sur un même disque dur
    /// alternent les positions de tête : mesuré sous Windows, trois fichiers
    /// de 1,4 Go d'un disque dur à un autre passent de 62 Mo/s (un à la fois)
    /// à 29 Mo/s (quatre workers). Les petits fichiers, dominés par la
    /// latence d'ouverture, continuent d'utiliser tous les workers.
    pub large_transfers: std::num::NonZeroUsize,
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
    /// Émet aussi la progression de chaque fichier transféré
    /// (`TransferProgress` au début puis par intervalle, `TransferEnded`).
    /// Faux par défaut : réservé aux observateurs en mémoire qui ne gardent
    /// que le dernier état ; une file bornée perdrait ces événements.
    pub transfer_events: bool,
}

impl Default for ExecutionOptions {
    fn default() -> Self {
        Self {
            refresh_mirror_source: false,
            copy_workers: std::num::NonZeroUsize::new(4).unwrap_or(std::num::NonZeroUsize::MIN),
            large_transfers: std::num::NonZeroUsize::MIN,
            verify: VerifyMode::default(),
            confirm_deletions: false,
            max_plan_age: DEFAULT_MAX_PLAN_AGE,
            max_clock_skew: DEFAULT_MAX_CLOCK_SKEW,
            buffer_size: DEFAULT_BUFFER_SIZE,
            progress_interval_bytes: 8 * 1024 * 1024,
            transfer_events: false,
        }
    }
}

impl ExecutionOptions {
    /// Bounds independent file transfers. One forces sequential execution.
    #[must_use]
    pub fn with_copy_workers(mut self, workers: std::num::NonZeroUsize) -> Self {
        self.copy_workers = workers;
        self
    }
    /// Bounds simultaneous large transfers (see [`Self::large_transfers`]).
    #[must_use]
    pub fn with_large_transfers(mut self, transfers: std::num::NonZeroUsize) -> Self {
        self.large_transfers = transfers;
        self
    }
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

    /// Active les événements par fichier (voir [`Self::transfer_events`]).
    #[must_use]
    pub fn with_transfer_events(mut self, enabled: bool) -> Self {
        self.transfer_events = enabled;
        self
    }
}
