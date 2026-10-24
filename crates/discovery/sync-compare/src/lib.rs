//! Comparaison d'inventaires : des faits, jamais des décisions.
//!
//! [`compare`] confronte deux [`sync_scanner::Inventory`] (« gauche » et
//! « droite », sans notion de source ni de destination) et produit un
//! [`ComparisonReport`] : pour chaque chemin, les entrées observées et un
//! [`ComparisonOutcome`].
//!
//! # Principes
//! - **Aucune E/S** : la comparaison ne lit que les inventaires.
//! - **Aucune politique** : `LeftOnly` ne veut pas dire « copier », ni
//!   « supprimer » ; c'est la stratégie (phase 03) qui décide.
//! - **Incertitude explicite** : absence non établie (listing incomplet,
//!   lien non suivi), taille/date/précision inconnue, liens non comparés,
//!   collisions de casse ⇒ [`ComparisonOutcome::Uncertain`].
//! - Une équivalence par taille et date n'est pas une preuve de contenu :
//!   [`EquivalenceBasis::SizeAndModified`] la distingue de
//!   [`EquivalenceBasis::ContentHash`].
//!
//! # Écart au contrat conceptuel
//! `docs/CONTRATS.md` esquissait un trait `ComparisonEngine` et plaçait
//! `ComparisonOutcome` dans `sync-core`. Une seule implémentation existe :
//! la phase 02 expose des fonctions ([`compare`], [`compare_entries`]) et
//! garde le modèle dans ce crate pur (voir ADR 0002).

mod compare;
mod outcome;

pub use compare::{
    CompareError, CompareOptions, ComparisonReport, ComparisonSummary, compare, compare_entries,
    content_candidates,
};
pub use outcome::{
    ComparisonOutcome, Difference, EquivalenceBasis, OutcomeCategory, PathComparison, Side,
    Uncertainty,
};
