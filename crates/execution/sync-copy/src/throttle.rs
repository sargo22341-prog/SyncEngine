//! Limitation de débit partagée par les transferts d'une exécution.

use std::num::NonZeroU64;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sync_core::CancellationToken;

/// Plus longue attente avant de revérifier l'annulation ou la pause.
const SLICE: Duration = Duration::from_millis(50);

/// Débit maximal, en octets par seconde, partagé par tous les workers qui
/// reçoivent un clone. Chaque transfert réserve la durée que ses octets
/// « coûtent » à ce débit puis attend son tour : la somme des débits ne
/// dépasse donc pas la limite, quel que soit le nombre de workers. Ne porte
/// que sur le contenu transféré (pas sur les relectures de vérification).
#[derive(Debug, Clone)]
pub struct Throttle {
    inner: Arc<State>,
}

#[derive(Debug)]
struct State {
    bytes_per_second: NonZeroU64,
    /// Instant à partir duquel le prochain octet peut partir.
    next: Mutex<Instant>,
}

impl Throttle {
    /// Limite à `bytes_per_second` octets par seconde.
    #[must_use]
    pub fn new(bytes_per_second: NonZeroU64) -> Self {
        Self {
            inner: Arc::new(State {
                bytes_per_second,
                next: Mutex::new(Instant::now()),
            }),
        }
    }

    /// Débit limite, en octets par seconde.
    #[must_use]
    pub fn bytes_per_second(&self) -> u64 {
        self.inner.bytes_per_second.get()
    }

    /// Réserve `bytes` octets puis attend l'instant prévu. Rend `false` si
    /// l'annulation a été demandée pendant l'attente (la réservation reste
    /// prise : l'exécution s'arrête de toute façon). La pause, coopérative,
    /// est honorée par [`CancellationToken::checkpoint`].
    #[must_use]
    pub fn pace(&self, bytes: u64, cancel: &CancellationToken) -> bool {
        let cost = Duration::from_secs_f64(bytes as f64 / self.inner.bytes_per_second.get() as f64);
        let due = {
            let mut next = self
                .inner
                .next
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let start = (*next).max(Instant::now());
            *next = start + cost;
            start
        };
        loop {
            let now = Instant::now();
            if now >= due {
                return true;
            }
            if cancel.checkpoint() {
                return false;
            }
            std::thread::sleep((due - now).min(SLICE));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transfers_are_spread_over_time_at_the_requested_rate() {
        // 100 kB/s: three 10 kB chunks cannot all leave before ~0.2 s.
        let throttle = Throttle::new(NonZeroU64::new(100_000).unwrap_or(NonZeroU64::MIN));
        let cancel = CancellationToken::new();
        let started = Instant::now();
        for _ in 0..3 {
            assert!(throttle.pace(10_000, &cancel));
        }
        // The third chunk starts after two paid ones: at least 0.2 s.
        assert!(started.elapsed() >= Duration::from_millis(190));
        assert_eq!(throttle.bytes_per_second(), 100_000);
    }

    #[test]
    fn the_limit_is_shared_between_clones() {
        let throttle = Throttle::new(NonZeroU64::new(100_000).unwrap_or(NonZeroU64::MIN));
        let other = throttle.clone();
        let cancel = CancellationToken::new();
        let started = Instant::now();
        assert!(throttle.pace(10_000, &cancel));
        assert!(other.pace(10_000, &cancel));
        assert!(throttle.pace(10_000, &cancel));
        assert!(started.elapsed() >= Duration::from_millis(190));
    }

    #[test]
    fn a_cancelled_wait_returns_promptly_instead_of_sleeping_the_whole_delay() {
        let throttle = Throttle::new(NonZeroU64::new(1_000).unwrap_or(NonZeroU64::MIN));
        let cancel = CancellationToken::new();
        assert!(throttle.pace(1_000, &cancel));
        cancel.cancel();
        let started = Instant::now();
        // The next chunk would wait one second.
        assert!(!throttle.pace(1_000, &cancel));
        assert!(started.elapsed() < Duration::from_millis(500));
    }
}
