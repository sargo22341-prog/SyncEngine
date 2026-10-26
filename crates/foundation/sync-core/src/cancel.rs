//! Jeton de contrôle coopératif : annulation et pause réversible.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};

/// Jeton d'annulation partagé entre l'appelant et une tâche longue.
///
/// L'annulation est **coopérative** : la tâche consulte
/// [`is_cancelled`](Self::is_cancelled) entre deux étapes bornées (une page
/// de listing, un bloc lu) et s'arrête proprement en conservant son état
/// partiel. Un appel système déjà engagé n'est pas interrompu.
///
/// Les clones partagent l'annulation et la pause. Les workers utilisent
/// [`checkpoint`](Self::checkpoint) pour attendre sans scrutation ; les
/// observateurs peuvent lire `is_cancelled` sans attendre. L'annulation
/// réveille tous les workers, y compris pendant une pause.
#[derive(Debug, Clone, Default)]
pub struct CancellationToken {
    control: Arc<Control>,
    ancestors: Arc<Vec<Arc<Control>>>,
    gate: Arc<Gate>,
}

#[derive(Debug, Default)]
struct Control {
    cancelled: AtomicBool,
    paused: AtomicBool,
}

#[derive(Debug, Default)]
struct Gate {
    gate: Mutex<()>,
    wake: Condvar,
}

impl CancellationToken {
    /// Creates an independently cancellable child which inherits cancellation
    /// and pause from this token. Cancelling or pausing the child never changes
    /// its parent. A shared condition variable wakes paused children on either
    /// parent or child cancellation, without polling or a watcher thread.
    #[must_use]
    pub fn child_token(&self) -> Self {
        let mut ancestors = self.ancestors.as_ref().clone();
        ancestors.push(Arc::clone(&self.control));
        Self {
            control: Arc::default(),
            ancestors: Arc::new(ancestors),
            gate: Arc::clone(&self.gate),
        }
    }
    /// Jeton non annulé.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Demande l'annulation (idempotent).
    pub fn cancel(&self) {
        let _guard = self.gate.gate.lock().unwrap_or_else(|e| e.into_inner());
        self.control.cancelled.store(true, Ordering::Release);
        self.gate.wake.notify_all();
    }

    /// Vrai si l'annulation a été demandée.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.control.cancelled.load(Ordering::Acquire)
            || self
                .ancestors
                .iter()
                .any(|state| state.cancelled.load(Ordering::Acquire))
    }
    /// Requests or releases a cooperative pause. Cancellation always wins.
    /// No system call already in progress is interrupted.
    pub fn set_paused(&self, paused: bool) {
        let _guard = self.gate.gate.lock().unwrap_or_else(|e| e.into_inner());
        self.control.paused.store(paused, Ordering::Release);
        self.gate.wake.notify_all();
    }

    /// Whether a pause has been requested (not an acknowledgement from workers).
    #[must_use]
    pub fn is_paused(&self) -> bool {
        !self.is_cancelled()
            && (self.control.paused.load(Ordering::Acquire)
                || self
                    .ancestors
                    .iter()
                    .any(|state| state.paused.load(Ordering::Acquire)))
    }

    /// Waits at a safe worker boundary while paused, then returns cancellation.
    /// Call only on worker threads, never while holding application state locks.
    /// The cancellation query remains nonblocking via is_cancelled.
    #[must_use]
    pub fn checkpoint(&self) -> bool {
        if self.is_paused() {
            let mut guard = self.gate.gate.lock().unwrap_or_else(|e| e.into_inner());
            while self.is_paused() {
                guard = self
                    .gate
                    .wake
                    .wait(guard)
                    .unwrap_or_else(|e| e.into_inner());
            }
        }
        self.is_cancelled()
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

#[cfg(test)]
mod pause_tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn child_abort_wakes_paused_workers_without_cancelling_the_parent() {
        let parent = CancellationToken::new();
        let child = parent.child_token();
        let sibling = parent.child_token();
        parent.set_paused(true);
        let (tx, rx) = mpsc::channel();
        let worker_token = child.clone();
        let worker = std::thread::spawn(move || tx.send(worker_token.checkpoint()).unwrap());
        assert!(rx.recv_timeout(Duration::from_millis(40)).is_err());
        child.cancel();
        assert!(rx.recv_timeout(Duration::from_secs(5)).unwrap());
        worker.join().unwrap();
        assert!(!parent.is_cancelled());
        assert!(parent.is_paused());
        assert!(!sibling.is_cancelled());
        parent.set_paused(false);
        let grandchild = sibling.child_token();
        parent.cancel();
        assert!(sibling.is_cancelled() && grandchild.is_cancelled());
    }

    #[test]
    fn pause_is_reversible_and_cancellation_wakes_every_waiter() {
        for cancel in [false, true] {
            let token = CancellationToken::new();
            token.set_paused(true);
            assert!(!token.is_cancelled());
            assert!(token.is_paused());
            let (tx, rx) = mpsc::channel();
            let workers: Vec<_> = (0..3)
                .map(|_| {
                    let token = token.clone();
                    let tx = tx.clone();
                    std::thread::spawn(move || {
                        tx.send(token.checkpoint()).unwrap();
                    })
                })
                .collect();
            let blocked = rx.recv_timeout(Duration::from_millis(40)).is_err();
            if cancel {
                token.cancel();
            } else {
                token.set_paused(false);
            }
            assert!(blocked);
            for _ in 0..3 {
                assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), cancel);
            }
            for worker in workers {
                worker.join().unwrap();
            }
            assert!(!token.is_paused());
            token.set_paused(true);
            if cancel {
                assert!(token.checkpoint());
            }
            token.set_paused(false);
        }
    }
}
