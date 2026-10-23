//! Description d'une entrée inventoriée.

use serde::{Deserialize, Serialize};

use crate::hash::ContentHash;
use crate::path::ResourcePath;
use crate::time::{Timestamp, TimestampPrecision};

/// Nature d'une entrée.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum EntryKind {
    /// Fichier ordinaire.
    File,
    /// Répertoire (ou préfixe pour un stockage objet).
    Directory,
    /// Lien symbolique, rapporté sans être suivi ; la politique de suivi est
    /// explicite et définie hors de ce type.
    Symlink,
    /// Autre nature (périphérique, socket, point de jonction…) : jamais
    /// traitée comme un fichier ordinaire.
    Other,
}

/// Métadonnées observées d'une entrée.
///
/// Chaque champ est optionnel : « absent » signifie **inconnu**, jamais zéro
/// ni « pas de date ». Ces métadonnées sont des observations, pas une preuve
/// d'identité de contenu.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileMetadata {
    size: Option<u64>,
    modified: Option<Timestamp>,
    modified_precision: Option<TimestampPrecision>,
    /// Ajout phase 02 : absent des formes sérialisées antérieures.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hash: Option<ContentHash>,
}

impl FileMetadata {
    /// Métadonnées entièrement inconnues.
    #[must_use]
    pub fn unknown() -> Self {
        Self::default()
    }

    /// Ajoute la taille observée, en octets.
    #[must_use]
    pub fn with_size(mut self, size: u64) -> Self {
        self.size = Some(size);
        self
    }

    /// Ajoute la date de modification et la précision garantie par le stockage.
    #[must_use]
    pub fn with_modified(mut self, modified: Timestamp, precision: TimestampPrecision) -> Self {
        self.modified = Some(modified);
        self.modified_precision = Some(precision);
        self
    }

    /// Ajoute une empreinte de contenu observée (fournie par le stockage ou
    /// calculée par lecture complète).
    #[must_use]
    pub fn with_hash(mut self, hash: ContentHash) -> Self {
        self.hash = Some(hash);
        self
    }

    /// Retire l'empreinte (par exemple si le fichier a changé pendant sa lecture).
    #[must_use]
    pub fn without_hash(mut self) -> Self {
        self.hash = None;
        self
    }

    /// Taille en octets, `None` si inconnue.
    #[must_use]
    pub fn size(&self) -> Option<u64> {
        self.size
    }

    /// Date de modification UTC, `None` si inconnue.
    #[must_use]
    pub fn modified(&self) -> Option<Timestamp> {
        self.modified
    }

    /// Précision de la date de modification ; [`TimestampPrecision::Unknown`]
    /// si la date est absente ou sans précision déclarée.
    #[must_use]
    pub fn modified_precision(&self) -> TimestampPrecision {
        self.modified_precision
            .unwrap_or(TimestampPrecision::Unknown)
    }

    /// Empreinte de contenu, `None` si non calculée ou indisponible.
    #[must_use]
    pub fn hash(&self) -> Option<&ContentHash> {
        self.hash.as_ref()
    }
}

/// Entrée observée dans une extrémité.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    path: ResourcePath,
    kind: EntryKind,
    metadata: FileMetadata,
}

impl FileEntry {
    /// Construit une entrée.
    #[must_use]
    pub fn new(path: ResourcePath, kind: EntryKind, metadata: FileMetadata) -> Self {
        Self {
            path,
            kind,
            metadata,
        }
    }

    /// Chemin logique relatif à la racine de l'extrémité.
    #[must_use]
    pub fn path(&self) -> &ResourcePath {
        &self.path
    }

    /// Nature de l'entrée.
    #[must_use]
    pub fn kind(&self) -> EntryKind {
        self.kind
    }

    /// Métadonnées observées.
    #[must_use]
    pub fn metadata(&self) -> &FileMetadata {
        &self.metadata
    }

    /// Remplace les métadonnées (par exemple pour y ajouter une empreinte).
    #[must_use]
    pub fn with_metadata(mut self, metadata: FileMetadata) -> Self {
        self.metadata = metadata;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_metadata_is_explicit() {
        let meta = FileMetadata::unknown();
        assert_eq!(meta.size(), None);
        assert_eq!(meta.modified(), None);
        assert_eq!(meta.modified_precision(), TimestampPrecision::Unknown);
    }

    #[test]
    fn builder_records_observations() {
        let ts = Timestamp::new(1_700_000_000, 0).unwrap();
        let meta = FileMetadata::unknown()
            .with_size(0)
            .with_modified(ts, TimestampPrecision::TwoSeconds);
        // Une taille nulle observée est distincte d'une taille inconnue.
        assert_eq!(meta.size(), Some(0));
        assert_eq!(meta.modified(), Some(ts));
        assert_eq!(meta.modified_precision(), TimestampPrecision::TwoSeconds);
    }

    #[test]
    fn entry_serde_round_trip() {
        let entry = FileEntry::new(
            ResourcePath::parse("a/b.txt").unwrap(),
            EntryKind::File,
            FileMetadata::unknown().with_size(42),
        );
        let json = serde_json::to_string(&entry).unwrap();
        assert_eq!(serde_json::from_str::<FileEntry>(&json).unwrap(), entry);
    }

    #[test]
    fn hash_is_optional_and_backward_compatible() {
        use crate::HashAlgorithm;
        // Forme phase 01, sans champ `hash`.
        let old = r#"{"size":3,"modified":null,"modified_precision":null}"#;
        let meta: FileMetadata = serde_json::from_str(old).unwrap();
        assert_eq!(meta.hash(), None);
        assert_eq!(serde_json::to_string(&meta).unwrap(), old);

        let hash = ContentHash::new(HashAlgorithm::Md5, vec![7; 16]).unwrap();
        let meta = meta.with_hash(hash.clone());
        assert_eq!(meta.hash(), Some(&hash));
        let json = serde_json::to_string(&meta).unwrap();
        assert_eq!(serde_json::from_str::<FileMetadata>(&json).unwrap(), meta);
        assert_eq!(meta.without_hash().hash(), None);
    }

    #[test]
    fn entry_deserialization_rejects_traversal() {
        let json = r#"{"path":"../x","kind":"file","metadata":{"size":null,"modified":null,"modified_precision":null}}"#;
        assert!(serde_json::from_str::<FileEntry>(json).is_err());
    }
}
