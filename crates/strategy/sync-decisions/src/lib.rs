//! Décisions de synchronisation : des constats aux opérations d'un plan.
//!
//! Ce crate relie la comparaison (`sync-compare`, des faits) au plan
//! (`sync-plan`, un contrat) :
//! - [`PlanStrategy`] : contrat commun des stratégies (`sync-backup`,
//!   `sync-mirror`, plus tard bidirectionnelle) ;
//! - [`plan_one_way`] : règle commune des modes unidirectionnels et
//!   garde-fous ; chaque mode ne fournit que ses règles propres
//!   ([`OneWayMode`]) ;
//! - [`decide_overwrite`] : table de décision des fichiers différents.
//!
//! # Règles de sûreté
//! - un constat incertain ne produit **jamais** d'écriture ni de suppression ;
//! - une suppression exige une absence **établie** à la source, une
//!   politique explicite, deux inventaires complets, une source non vide
//!   (sauf autorisation) et le respect des plafonds ; sinon **toutes** les
//!   suppressions deviennent des sauts motivés (journalisés dans le plan) ;
//! - un conflit n'est jamais résolu au hasard : il reste un conflit, rien
//!   n'est écrit ;
//! - aucune E/S : la planification est pure et reproductible.

mod input;
mod one_way;
mod rules;

pub use input::{PlanContext, PlanStrategy, PlanningError, PlanningInput, PolicyViolation};
pub use one_way::{OneWayMode, plan_one_way};
pub use rules::{Decision, decide_overwrite};
