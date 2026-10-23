//! Algorithmes de hachage nommés.

use serde::{Deserialize, Serialize};

/// Algorithme de hachage de contenu.
///
/// Sert à déclarer ce qu'un connecteur sait fournir et à étiqueter une
/// empreinte. Le calcul effectif (BLAKE3 retenu pour les fichiers locaux)
/// relève des phases 02 et 04.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum HashAlgorithm {
    /// BLAKE3, 256 bits.
    Blake3,
    /// SHA-256.
    Sha256,
    /// SHA-1 (fourni par certains serveurs ; non résistant aux collisions).
    Sha1,
    /// MD5 (ETag S3 simple, certains serveurs ; non résistant aux collisions).
    Md5,
}

impl HashAlgorithm {
    /// Taille de l'empreinte en octets.
    #[must_use]
    pub fn digest_len(self) -> usize {
        match self {
            Self::Blake3 | Self::Sha256 => 32,
            Self::Sha1 => 20,
            Self::Md5 => 16,
        }
    }

    /// Vrai si l'algorithme est considéré résistant aux collisions.
    #[must_use]
    pub fn is_collision_resistant(self) -> bool {
        matches!(self, Self::Blake3 | Self::Sha256)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_lengths() {
        assert_eq!(HashAlgorithm::Blake3.digest_len(), 32);
        assert_eq!(HashAlgorithm::Sha256.digest_len(), 32);
        assert_eq!(HashAlgorithm::Sha1.digest_len(), 20);
        assert_eq!(HashAlgorithm::Md5.digest_len(), 16);
        assert!(!HashAlgorithm::Md5.is_collision_resistant());
        assert!(HashAlgorithm::Blake3.is_collision_resistant());
    }

    #[test]
    fn serde_names_are_stable() {
        assert_eq!(
            serde_json::to_string(&HashAlgorithm::Blake3).unwrap(),
            "\"blake3\""
        );
        assert_eq!(
            serde_json::from_str::<HashAlgorithm>("\"sha256\"").unwrap(),
            HashAlgorithm::Sha256
        );
    }
}
