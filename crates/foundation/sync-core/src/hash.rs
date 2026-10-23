//! Algorithmes de hachage nommés et empreintes de contenu.

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

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

/// Erreur de construction d'une [`ContentHash`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum ContentHashError {
    /// La longueur de l'empreinte ne correspond pas à l'algorithme.
    #[error("empreinte {algorithm:?} de {len} octets (attendu {expected})")]
    WrongLength {
        /// Algorithme annoncé.
        algorithm: HashAlgorithm,
        /// Longueur reçue.
        len: usize,
        /// Longueur attendue.
        expected: usize,
    },
    /// La forme hexadécimale est invalide.
    #[error("empreinte hexadécimale invalide")]
    InvalidHex,
}

/// Empreinte de contenu observée, étiquetée par son algorithme.
///
/// Invariant : la longueur de l'empreinte est celle de l'algorithme
/// ([`HashAlgorithm::digest_len`]). Deux empreintes ne sont comparables que
/// si elles utilisent le même algorithme.
///
/// Sérialisation : `{"algorithm":"blake3","hex":"…"}` (hexadécimal minuscule).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "RawContentHash", into = "RawContentHash")]
pub struct ContentHash {
    algorithm: HashAlgorithm,
    digest: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
struct RawContentHash {
    algorithm: HashAlgorithm,
    hex: String,
}

impl TryFrom<RawContentHash> for ContentHash {
    type Error = ContentHashError;

    fn try_from(raw: RawContentHash) -> Result<Self, Self::Error> {
        Self::from_hex(raw.algorithm, &raw.hex)
    }
}

impl From<ContentHash> for RawContentHash {
    fn from(hash: ContentHash) -> Self {
        Self {
            hex: hash.to_hex(),
            algorithm: hash.algorithm,
        }
    }
}

impl ContentHash {
    /// Construit une empreinte à partir de ses octets.
    ///
    /// # Errors
    /// [`ContentHashError::WrongLength`] si la longueur ne correspond pas.
    pub fn new(
        algorithm: HashAlgorithm,
        digest: impl Into<Vec<u8>>,
    ) -> Result<Self, ContentHashError> {
        let digest = digest.into();
        let expected = algorithm.digest_len();
        if digest.len() != expected {
            return Err(ContentHashError::WrongLength {
                algorithm,
                len: digest.len(),
                expected,
            });
        }
        Ok(Self { algorithm, digest })
    }

    /// Construit une empreinte depuis sa forme hexadécimale (casse indifférente).
    ///
    /// # Errors
    /// [`ContentHashError::InvalidHex`] ou [`ContentHashError::WrongLength`].
    pub fn from_hex(algorithm: HashAlgorithm, hex: &str) -> Result<Self, ContentHashError> {
        let (pairs, rest) = hex.as_bytes().as_chunks::<2>();
        if !rest.is_empty() {
            return Err(ContentHashError::InvalidHex);
        }
        let digest = pairs
            .iter()
            .map(|&[high, low]| Ok(hex_value(high)? << 4 | hex_value(low)?))
            .collect::<Result<Vec<u8>, ContentHashError>>()?;
        Self::new(algorithm, digest)
    }

    /// Algorithme utilisé.
    #[must_use]
    pub fn algorithm(&self) -> HashAlgorithm {
        self.algorithm
    }

    /// Octets de l'empreinte.
    #[must_use]
    pub fn digest(&self) -> &[u8] {
        &self.digest
    }

    /// Forme hexadécimale minuscule.
    #[must_use]
    pub fn to_hex(&self) -> String {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        let mut out = String::with_capacity(self.digest.len() * 2);
        for byte in &self.digest {
            out.push(char::from(DIGITS[usize::from(byte >> 4)]));
            out.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
        }
        out
    }
}

fn hex_value(digit: u8) -> Result<u8, ContentHashError> {
    match digit {
        b'0'..=b'9' => Ok(digit - b'0'),
        b'a'..=b'f' => Ok(digit - b'a' + 10),
        b'A'..=b'F' => Ok(digit - b'A' + 10),
        _ => Err(ContentHashError::InvalidHex),
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}:{}", self.algorithm, self.to_hex())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_hash_validates_length() {
        assert!(ContentHash::new(HashAlgorithm::Md5, vec![0; 16]).is_ok());
        assert_eq!(
            ContentHash::new(HashAlgorithm::Blake3, vec![0; 16]),
            Err(ContentHashError::WrongLength {
                algorithm: HashAlgorithm::Blake3,
                len: 16,
                expected: 32
            })
        );
    }

    #[test]
    fn content_hash_hex_round_trip() {
        let hash = ContentHash::new(HashAlgorithm::Sha1, (0u8..20).collect::<Vec<_>>()).unwrap();
        let hex = hash.to_hex();
        assert_eq!(hex, "000102030405060708090a0b0c0d0e0f10111213");
        assert_eq!(
            ContentHash::from_hex(HashAlgorithm::Sha1, &hex.to_uppercase()).unwrap(),
            hash
        );
        assert_eq!(
            ContentHash::from_hex(HashAlgorithm::Sha1, "0g"),
            Err(ContentHashError::InvalidHex)
        );
        assert_eq!(
            ContentHash::from_hex(HashAlgorithm::Sha1, "abc"),
            Err(ContentHashError::InvalidHex)
        );
        let json = serde_json::to_string(&hash).unwrap();
        assert_eq!(json, format!(r#"{{"algorithm":"sha1","hex":"{hex}"}}"#));
        assert_eq!(serde_json::from_str::<ContentHash>(&json).unwrap(), hash);
        assert!(serde_json::from_str::<ContentHash>(r#"{"algorithm":"md5","hex":"00"}"#).is_err());
    }

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
