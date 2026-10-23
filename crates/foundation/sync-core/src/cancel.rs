//! Jeton d'annulation coopérative.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Jeton d'annulation partagé entre l'appelant et une tâche longue.
///
/// L'annulation est **coopérative** : la tâche consulte
/// [`is_cancelled`](Self::is_cancelled) entre deux étapes bornées (une page
/// de listing, un bloc lu) et s'arrête proprement en conservant son état
/// partiel. Un appel système déjà engagé n'est pas interrompu.
///
/// Les clones partagent le même état : annuler l'un annule tous les autres.
#[derive(Debug, Clone, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    /// Jeton non annulé.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Demande l'annulation (idempotent).
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    /// Vrai si l'annulation a été demandée.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clones_share_state() {
        let token = CancellationToken::new();
        let clone = token.clone();
        assert!(!clone.is_cancelled());
        token.cancel();
        token.cancel();
        assert!(clone.is_cancelled());
    }

    #[test]
    fn visible_across_threads() {
        let token = CancellationToken::new();
        let remote = token.clone();
        std::thread::spawn(move || remote.cancel()).join().unwrap();
        assert!(token.is_cancelled());
    }
}
