//! Capacités déclarées par un connecteur.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use sync_core::{HashAlgorithm, TimestampPrecision};

/// Fonctionnalité binaire qu'un connecteur peut prendre en charge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum StorageFeature {
    /// Conservation explicite de la date fournie dans WriteOptions.modified.
    SetModifiedTime,
    /// Lister le contenu d'un répertoire ou préfixe.
    List,
    /// Lire les métadonnées d'une entrée.
    Stat,
    /// Lire le contenu d'un fichier.
    Read,
    /// Écrire le contenu d'un fichier.
    Write,
    /// Écriture conditionnelle (échoue si la cible a changé : ETag, date…).
    ConditionalWrite,
    /// Créer un répertoire.
    CreateDirectory,
    /// Renommer ou déplacer une entrée sans copie de contenu.
    Move,
    /// Supprimer une entrée.
    Delete,
    /// Copie effectuée par le serveur, sans transit local.
    ServerSideCopy,
    /// Lecture en flux, sans charger le fichier en mémoire.
    StreamingRead,
    /// Écriture en flux.
    StreamingWrite,
    /// Reprise d'un transfert interrompu.
    ResumableTransfer,
    /// Versionnement natif côté stockage.
    NativeVersioning,
    /// Consultation de l'espace libre ou du quota.
    QuotaReporting,
    /// Annulation d'une opération en cours.
    Cancellation,
}

/// Règle de casse des noms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CaseSensitivity {
    /// `a` et `A` désignent deux entrées distinctes.
    Sensitive,
    /// Insensible à la casse, casse d'origine conservée (NTFS par défaut).
    InsensitivePreserving,
    /// Insensible à la casse, casse non conservée.
    Insensitive,
    /// Inconnue : toute collision de casse doit être traitée comme incertaine.
    Unknown,
}

/// Origine des horodatages rapportés.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ClockSource {
    /// Horloge du système local.
    LocalSystem,
    /// Horloge du serveur distant (peut dériver de l'horloge locale).
    Server,
    /// Origine inconnue.
    Unknown,
}

/// Cohérence d'un listing après écriture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ListingConsistency {
    /// Une écriture terminée est immédiatement visible dans le listing.
    /// Ne promet pas un snapshot pendant des modifications concurrentes.
    /// Une absence exige toutes les pages sans erreur ; l'exécuteur revalide
    /// encore les préconditions juste avant une mutation.
    Strong,
    /// Visibilité différée possible : une absence n'est pas une preuve.
    Eventual,
    /// Inconnue : traiter comme [`ListingConsistency::Eventual`].
    Unknown,
}

/// Pagination du listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Pagination {
    /// Listing rendu en un bloc par le protocole.
    Unpaginated,
    /// Listing en pages successives.
    Paginated {
        /// Taille maximale d'une page imposée par le service, si connue.
        max_page_size: Option<u32>,
    },
}

/// Prise en charge des liens symboliques.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SymlinkSupport {
    /// Le stockage n'a pas de notion de lien symbolique.
    None,
    /// Les liens sont rapportés comme tels, sans être suivis.
    Reported,
}

/// Capacités détaillées d'un connecteur (ou d'une extrémité configurée).
///
/// La valeur par défaut, [`StorageCapabilities::none`], ne déclare **rien** :
/// un connecteur doit annoncer explicitement chaque capacité. Une opération
/// dont la capacité est absente doit échouer avec
/// [`StorageErrorKind::NotSupported`](crate::StorageErrorKind::NotSupported).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct StorageCapabilities {
    /// Fonctionnalités prises en charge.
    pub features: BTreeSet<StorageFeature>,
    /// Algorithmes d'empreinte que le stockage sait fournir sans tout relire.
    pub native_hashes: BTreeSet<HashAlgorithm>,
    /// Règle de casse des noms.
    pub case_sensitivity: CaseSensitivity,
    /// Précision des dates de modification conservées.
    pub timestamp_precision: TimestampPrecision,
    /// Origine des dates rapportées.
    pub clock: ClockSource,
    /// Cohérence du listing.
    pub listing_consistency: ListingConsistency,
    /// Mode de pagination du listing.
    pub pagination: Pagination,
    /// Traitement des liens symboliques.
    pub symlinks: SymlinkSupport,
    /// Taille maximale d'un fichier en octets, si limitée et connue.
    pub max_file_size: Option<u64>,
    /// Vrai si le service impose une limitation de débit.
    pub rate_limited: bool,
}

impl Default for StorageCapabilities {
    fn default() -> Self {
        Self::none()
    }
}

impl StorageCapabilities {
    /// Capacités vides et hypothèses les plus prudentes.
    #[must_use]
    pub fn none() -> Self {
        Self {
            features: BTreeSet::new(),
            native_hashes: BTreeSet::new(),
            case_sensitivity: CaseSensitivity::Unknown,
            timestamp_precision: TimestampPrecision::Unknown,
            clock: ClockSource::Unknown,
            listing_consistency: ListingConsistency::Unknown,
            pagination: Pagination::Unpaginated,
            symlinks: SymlinkSupport::None,
            max_file_size: None,
            rate_limited: false,
        }
    }

    /// Ajoute des fonctionnalités.
    #[must_use]
    pub fn with_features(mut self, features: impl IntoIterator<Item = StorageFeature>) -> Self {
        self.features.extend(features);
        self
    }

    /// Vrai si la fonctionnalité est déclarée.
    #[must_use]
    pub fn supports(&self, feature: StorageFeature) -> bool {
        self.features.contains(&feature)
    }

    /// Vrai si le stockage ne déclare que des opérations sans mutation.
    #[must_use]
    pub fn is_read_only(&self) -> bool {
        !self.features.iter().any(|feature| {
            matches!(
                feature,
                StorageFeature::Write
                    | StorageFeature::ConditionalWrite
                    | StorageFeature::CreateDirectory
                    | StorageFeature::Move
                    | StorageFeature::Delete
                    | StorageFeature::ServerSideCopy
                    | StorageFeature::StreamingWrite
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_is_prudent() {
        let caps = StorageCapabilities::none();
        assert!(caps.features.is_empty());
        assert!(caps.is_read_only());
        assert!(!caps.supports(StorageFeature::Delete));
        assert_eq!(caps.case_sensitivity, CaseSensitivity::Unknown);
        assert_eq!(caps.timestamp_precision, TimestampPrecision::Unknown);
        assert_eq!(caps.listing_consistency, ListingConsistency::Unknown);
        assert_eq!(StorageCapabilities::default(), caps);
    }

    #[test]
    fn read_only_detection() {
        let caps = StorageCapabilities::none().with_features([
            StorageFeature::List,
            StorageFeature::Stat,
            StorageFeature::Read,
            StorageFeature::StreamingRead,
        ]);
        assert!(caps.is_read_only());
        assert!(caps.supports(StorageFeature::Read));
        let caps = caps.with_features([StorageFeature::Delete]);
        assert!(!caps.is_read_only());
    }

    #[test]
    fn serde_round_trip() {
        let mut caps = StorageCapabilities::none().with_features([StorageFeature::List]);
        caps.native_hashes.insert(HashAlgorithm::Sha256);
        caps.pagination = Pagination::Paginated {
            max_page_size: Some(1000),
        };
        caps.max_file_size = Some(250 * 1024 * 1024 * 1024);
        let json = serde_json::to_string(&caps).unwrap();
        assert_eq!(
            serde_json::from_str::<StorageCapabilities>(&json).unwrap(),
            caps
        );
    }
}
