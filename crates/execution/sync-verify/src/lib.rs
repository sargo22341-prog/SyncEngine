//! Contrôles d'intégrité.
//!
//! Ce crate **constate** des écarts ; il ne décide de rien et ne modifie
//! rien. L'exécuteur s'en sert à trois moments :
//! 1. juste avant une mutation, pour revérifier qu'une entrée correspond
//!    encore à l'observation enregistrée dans le plan
//!    ([`compare_observation`]) ;
//! 2. après un transfert, pour contrôler la taille et l'empreinte du
//!    contenu lu ([`check_transfer`]) ;
//! 3. avant validation, pour relire le dépôt temporaire et comparer son
//!    empreinte BLAKE3 à celle du contenu lu à la source
//!    ([`verify_content`], mode [`VerifyMode::Content`]), puis après
//!    validation, pour contrôler l'entrée publiée ([`check_written`]).
//!
//! # Sémantique des dates
//! Deux dates sont égales si leur écart est inférieur à la granularité de la
//! plus grossière des deux précisions déclarées
//! ([`sync_core::TimestampPrecision::coarsest`]) ; une précision inconnue
//! exige l'égalité exacte. Une date ne prouve jamais l'identité du contenu :
//! seule l'empreinte le fait.

use std::fmt;

use connector_api::{StorageConnector, StorageError, StorageErrorKind, StorageOperation};
use serde::{Deserialize, Serialize};
use sync_core::{
    CancellationToken, ContentHash, EntryKind, FileEntry, FileMetadata, HashAlgorithm,
    ResourcePath, Timestamp,
};
use sync_errors::{ClassifiedError, ErrorClass, ErrorContext};
use thiserror::Error;

/// Niveau de vérification d'un fichier transféré.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum VerifyMode {
    /// Taille transférée et empreinte du flux (comparée à celle du plan si
    /// elle y figure) ; le dépôt n'est pas relu.
    Size,
    /// En plus : relecture complète du dépôt temporaire avant validation et
    /// comparaison de son empreinte BLAKE3 avec celle du contenu lu à la
    /// source. Double la lecture ; mode par défaut (intégrité d'abord).
    #[default]
    Content,
}

/// Écart entre ce qui était attendu et ce qui est observé.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "field", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum Mismatch {
    /// Nature différente.
    Kind {
        /// Nature attendue.
        expected: EntryKind,
        /// Nature observée.
        found: EntryKind,
    },
    /// Taille différente ou devenue inconnue.
    Size {
        /// Taille attendue.
        expected: u64,
        /// Taille observée.
        found: Option<u64>,
    },
    /// Date de modification différente (à la précision près) ou inconnue.
    Modified {
        /// Date attendue.
        expected: Timestamp,
        /// Date observée.
        found: Option<Timestamp>,
    },
    /// Empreinte de contenu différente.
    Content {
        /// Empreinte attendue.
        expected: ContentHash,
        /// Empreinte observée.
        found: ContentHash,
    },
    /// L'observation enregistrée ne contient ni taille, ni date, ni
    /// empreinte : l'absence de changement ne peut pas être établie.
    Unverifiable,
}

impl fmt::Display for Mismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Kind { expected, found } => {
                write!(f, "nature {found:?} au lieu de {expected:?}")
            }
            Self::Size { expected, found } => match found {
                Some(found) => write!(f, "taille {found} o au lieu de {expected} o"),
                None => write!(f, "taille devenue inconnue (attendu {expected} o)"),
            },
            Self::Modified { expected, found } => match found {
                Some(found) => write!(
                    f,
                    "date de modification {}.{:09} au lieu de {}.{:09}",
                    found.secs(),
                    found.subsec_nanos(),
                    expected.secs(),
                    expected.subsec_nanos()
                ),
                None => f.write_str("date de modification devenue inconnue"),
            },
            Self::Content { .. } => f.write_str("empreinte du contenu différente"),
            Self::Unverifiable => f.write_str("observation sans taille, date ni empreinte"),
        }
    }
}

/// Vrai si deux dates sont égales à la précision commune déclarée.
fn same_instant(expected: &FileMetadata, found: &FileMetadata) -> bool {
    let (Some(a), Some(b)) = (expected.modified(), found.modified()) else {
        return false;
    };
    let precision = expected
        .modified_precision()
        .coarsest(found.modified_precision());
    let Some(granularity) = precision.granularity() else {
        return a == b;
    };
    let nanos =
        |ts: Timestamp| i128::from(ts.secs()) * 1_000_000_000 + i128::from(ts.subsec_nanos());
    (nanos(a) - nanos(b)).unsigned_abs() < granularity.as_nanos()
}

/// Compare l'entrée actuelle à une observation enregistrée (nature et
/// métadonnées).
///
/// - nature : toujours comparée ;
/// - répertoire : la nature suffit (sa date change avec son contenu) ;
/// - fichier : taille et date comparées si elles avaient été observées ;
///   l'empreinte n'est pas recalculée ici (voir [`check_transfer`]).
///
/// # Errors
/// Premier [`Mismatch`] constaté ; [`Mismatch::Unverifiable`] si
/// l'observation d'un fichier ne contient ni taille, ni date, ni empreinte.
pub fn compare_observation(
    expected_kind: EntryKind,
    expected: &FileMetadata,
    current: &FileEntry,
) -> Result<(), Mismatch> {
    if current.kind() != expected_kind {
        return Err(Mismatch::Kind {
            expected: expected_kind,
            found: current.kind(),
        });
    }
    if expected_kind != EntryKind::File {
        return Ok(());
    }
    let found = current.metadata();
    if expected.size().is_none() && expected.modified().is_none() && expected.hash().is_none() {
        return Err(Mismatch::Unverifiable);
    }
    if let Some(size) = expected.size()
        && found.size() != Some(size)
    {
        return Err(Mismatch::Size {
            expected: size,
            found: found.size(),
        });
    }
    if let Some(modified) = expected.modified()
        && !same_instant(expected, found)
    {
        return Err(Mismatch::Modified {
            expected: modified,
            found: found.modified(),
        });
    }
    Ok(())
}

/// Contrôle un transfert : `bytes` octets d'empreinte `hash` ont été lus à
/// la source ; ils doivent correspondre à l'observation enregistrée (taille,
/// et empreinte si elle a été calculée avec le même algorithme).
///
/// # Errors
/// [`Mismatch::Size`] ou [`Mismatch::Content`] : la source a changé depuis
/// l'observation (ou pendant la lecture).
pub fn check_transfer(
    expected: &FileMetadata,
    bytes: u64,
    hash: &ContentHash,
) -> Result<(), Mismatch> {
    if let Some(size) = expected.size()
        && size != bytes
    {
        return Err(Mismatch::Size {
            expected: size,
            found: Some(bytes),
        });
    }
    if let Some(recorded) = expected.hash()
        && recorded.algorithm() == hash.algorithm()
        && recorded != hash
    {
        return Err(Mismatch::Content {
            expected: recorded.clone(),
            found: hash.clone(),
        });
    }
    Ok(())
}

/// Contrôle l'entrée publiée après validation : un fichier ordinaire de
/// `bytes` octets (si sa taille est rapportée).
///
/// # Errors
/// [`Mismatch::Kind`] ou [`Mismatch::Size`].
pub fn check_written(entry: &FileEntry, bytes: u64) -> Result<(), Mismatch> {
    if entry.kind() != EntryKind::File {
        return Err(Mismatch::Kind {
            expected: EntryKind::File,
            found: entry.kind(),
        });
    }
    match entry.metadata().size() {
        Some(size) if size != bytes => Err(Mismatch::Size {
            expected: bytes,
            found: Some(size),
        }),
        _ => Ok(()),
    }
}

/// Échec d'une vérification de contenu.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum VerifyError {
    /// La relecture a échoué.
    #[error("relecture impossible : {0}")]
    Storage(#[source] StorageError),
    /// Le contenu relu ne correspond pas.
    #[error("contenu relu non conforme : {0}")]
    Mismatch(Mismatch),
}

impl ClassifiedError for VerifyError {
    fn class(&self) -> ErrorClass {
        match self {
            Self::Storage(error) => error.kind().class(),
            Self::Mismatch(_) => ErrorClass::Integrity,
        }
    }

    fn context(&self) -> Option<&ErrorContext> {
        match self {
            Self::Storage(error) => error.context(),
            Self::Mismatch(_) => None,
        }
    }
}

/// Compare un contenu relu (octets, empreinte BLAKE3) à celui transféré.
///
/// # Errors
/// [`Mismatch::Size`] puis [`Mismatch::Content`], dans cet ordre.
pub fn check_reread(
    expected: &ContentHash,
    expected_bytes: u64,
    found: &ContentHash,
    found_bytes: u64,
) -> Result<(), Mismatch> {
    if found_bytes != expected_bytes {
        return Err(Mismatch::Size {
            expected: expected_bytes,
            found: Some(found_bytes),
        });
    }
    if found != expected {
        return Err(Mismatch::Content {
            expected: expected.clone(),
            found: found.clone(),
        });
    }
    Ok(())
}

/// Relit `path` en flux sur `connector` et vérifie qu'il contient exactement
/// `expected_bytes` octets d'empreinte `expected` (BLAKE3).
///
/// # Errors
/// [`VerifyError::Storage`] (lecture, annulation, algorithme non BLAKE3 :
/// `NotSupported`) ou [`VerifyError::Mismatch`].
pub fn verify_content(
    connector: &dyn StorageConnector,
    path: &ResourcePath,
    expected: &ContentHash,
    expected_bytes: u64,
    buffer_size: usize,
    cancel: &CancellationToken,
) -> Result<(), VerifyError> {
    verify_read(
        connector,
        path,
        expected,
        expected_bytes,
        buffer_size,
        cancel,
        false,
    )
}

/// Verify exactly the first `expected_bytes` bytes, ignoring a later tail.
/// EOF before the boundary and changed prefix bytes are integrity failures.
/// This does not claim an application-consistent database snapshot.
///
/// # Errors
/// The same storage, cancellation and integrity errors as [`verify_content`].
pub fn verify_prefix(
    connector: &dyn StorageConnector,
    path: &ResourcePath,
    expected: &ContentHash,
    expected_bytes: u64,
    buffer_size: usize,
    cancel: &CancellationToken,
) -> Result<(), VerifyError> {
    verify_read(
        connector,
        path,
        expected,
        expected_bytes,
        buffer_size,
        cancel,
        true,
    )
}

fn verify_read(
    connector: &dyn StorageConnector,
    path: &ResourcePath,
    expected: &ContentHash,
    expected_bytes: u64,
    buffer_size: usize,
    cancel: &CancellationToken,
    bounded: bool,
) -> Result<(), VerifyError> {
    if expected.algorithm() != HashAlgorithm::Blake3 {
        return Err(VerifyError::Storage(
            StorageError::new(StorageErrorKind::NotSupported, StorageOperation::Read)
                .with_path(path.clone())
                .with_message("vérification de contenu : seul BLAKE3 est calculé"),
        ));
    }
    let mut reader = connector
        .open_read(path, cancel)
        .map_err(VerifyError::Storage)?;
    let mut buffer = vec![0u8; buffer_size.max(1)];
    let mut hasher = blake3::Hasher::new();
    let mut bytes: u64 = 0;
    loop {
        if cancel.checkpoint() {
            return Err(VerifyError::Storage(
                StorageError::new(StorageErrorKind::Cancelled, StorageOperation::Read)
                    .with_path(path.clone()),
            ));
        }
        let available = if bounded {
            expected_bytes
                .saturating_sub(bytes)
                .min(buffer.len() as u64) as usize
        } else {
            buffer.len()
        };
        if available == 0 {
            break;
        }
        let read = reader
            .read(&mut buffer[..available])
            .map_err(VerifyError::Storage)?;
        if read == 0 {
            break;
        }
        hasher.update(buffer.get(..read).unwrap_or_default());
        bytes = bytes.saturating_add(read as u64);
    }
    if bytes != expected_bytes {
        return Err(VerifyError::Mismatch(Mismatch::Size {
            expected: expected_bytes,
            found: Some(bytes),
        }));
    }
    let found = ContentHash::new(HashAlgorithm::Blake3, hasher.finalize().as_bytes().to_vec())
        .map_err(|_| {
            VerifyError::Storage(
                StorageError::new(StorageErrorKind::Permanent, StorageOperation::Read)
                    .with_path(path.clone()),
            )
        })?;
    check_reread(expected, expected_bytes, &found, bytes).map_err(VerifyError::Mismatch)
}

#[cfg(test)]
mod tests {
    use sync_core::TimestampPrecision;

    use super::*;

    fn ts(secs: i64, nanos: u32) -> Timestamp {
        Timestamp::new(secs, nanos).unwrap()
    }

    fn file(size: u64, modified: Timestamp, precision: TimestampPrecision) -> FileMetadata {
        FileMetadata::unknown()
            .with_size(size)
            .with_modified(modified, precision)
    }

    fn entry(kind: EntryKind, metadata: FileMetadata) -> FileEntry {
        FileEntry::new(ResourcePath::parse("a").unwrap(), kind, metadata)
    }

    #[test]
    fn dates_are_compared_at_the_coarsest_precision() {
        let expected = file(3, ts(100, 0), TimestampPrecision::TwoSeconds);
        let current = entry(
            EntryKind::File,
            file(3, ts(101, 500_000_000), TimestampPrecision::Nanosecond),
        );
        assert_eq!(
            compare_observation(EntryKind::File, &expected, &current),
            Ok(())
        );
        let current = entry(
            EntryKind::File,
            file(3, ts(102, 0), TimestampPrecision::Nanosecond),
        );
        assert!(matches!(
            compare_observation(EntryKind::File, &expected, &current),
            Err(Mismatch::Modified { .. })
        ));
        // Précision inconnue : égalité exacte exigée.
        let expected = file(3, ts(100, 0), TimestampPrecision::Unknown);
        let current = entry(
            EntryKind::File,
            file(3, ts(100, 1), TimestampPrecision::Nanosecond),
        );
        assert!(compare_observation(EntryKind::File, &expected, &current).is_err());
    }

    #[test]
    fn size_kind_and_unknown_observations() {
        let expected = file(3, ts(100, 0), TimestampPrecision::Nanosecond);
        let bigger = entry(
            EntryKind::File,
            file(4, ts(100, 0), TimestampPrecision::Nanosecond),
        );
        assert_eq!(
            compare_observation(EntryKind::File, &expected, &bigger),
            Err(Mismatch::Size {
                expected: 3,
                found: Some(4)
            })
        );
        let dir = entry(EntryKind::Directory, FileMetadata::unknown());
        assert_eq!(
            compare_observation(EntryKind::File, &expected, &dir),
            Err(Mismatch::Kind {
                expected: EntryKind::File,
                found: EntryKind::Directory
            })
        );
        // Un répertoire n'est comparé que par sa nature.
        assert_eq!(
            compare_observation(EntryKind::Directory, &FileMetadata::unknown(), &dir),
            Ok(())
        );
        let unknown = entry(EntryKind::File, FileMetadata::unknown());
        assert_eq!(
            compare_observation(EntryKind::File, &FileMetadata::unknown(), &unknown),
            Err(Mismatch::Unverifiable)
        );
        // Une date observée puis devenue inconnue est un écart.
        let no_date = entry(EntryKind::File, FileMetadata::unknown().with_size(3));
        assert!(matches!(
            compare_observation(EntryKind::File, &expected, &no_date),
            Err(Mismatch::Modified { found: None, .. })
        ));
    }

    #[test]
    fn transfer_must_match_size_and_recorded_hash() {
        let content = b"abc";
        let hash = ContentHash::new(
            HashAlgorithm::Blake3,
            blake3::hash(content).as_bytes().to_vec(),
        )
        .unwrap();
        let other = ContentHash::new(
            HashAlgorithm::Blake3,
            blake3::hash(b"abd").as_bytes().to_vec(),
        )
        .unwrap();
        let expected = FileMetadata::unknown().with_size(3).with_hash(hash.clone());
        assert_eq!(check_transfer(&expected, 3, &hash), Ok(()));
        assert!(matches!(
            check_transfer(&expected, 3, &other),
            Err(Mismatch::Content { .. })
        ));
        assert!(matches!(
            check_transfer(&expected, 4, &hash),
            Err(Mismatch::Size { .. })
        ));
        // Empreinte d'un autre algorithme : non comparable, seule la taille compte.
        let md5 = ContentHash::new(HashAlgorithm::Md5, vec![0; 16]).unwrap();
        let expected = FileMetadata::unknown().with_size(3).with_hash(md5);
        assert_eq!(check_transfer(&expected, 3, &other), Ok(()));
    }

    #[test]
    fn written_entry_is_checked() {
        let ok = entry(EntryKind::File, FileMetadata::unknown().with_size(5));
        assert_eq!(check_written(&ok, 5), Ok(()));
        assert!(check_written(&ok, 6).is_err());
        let dir = entry(EntryKind::Directory, FileMetadata::unknown());
        assert!(check_written(&dir, 0).is_err());
    }

    #[test]
    fn mismatch_serialization_is_tagged() {
        let json = serde_json::to_string(&Mismatch::Size {
            expected: 3,
            found: None,
        })
        .unwrap();
        assert_eq!(json, r#"{"field":"size","expected":3,"found":null}"#);
        assert_eq!(
            serde_json::to_string(&VerifyMode::Content).unwrap(),
            "\"content\""
        );
    }
}
