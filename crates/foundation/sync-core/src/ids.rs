//! Identifiants typés.
//!
//! Chaque identifiant est un newtype distinct : un [`ProfileId`] ne peut pas
//! être passé là où un [`ExecutionId`] est attendu.
//!
//! ```compile_fail
//! use sync_core::{ExecutionId, ProfileId};
//! fn start(_execution: &ExecutionId) {}
//! let profile = ProfileId::new("daily").unwrap();
//! start(&profile); // erreur de type attendue
//! ```

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Longueur maximale (en octets) d'un identifiant.
pub const MAX_ID_LEN: usize = 128;

/// Erreur de construction d'un identifiant.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum IdError {
    /// L'identifiant est vide.
    #[error("identifiant {id_type} vide")]
    Empty {
        /// Type d'identifiant concerné.
        id_type: &'static str,
    },
    /// L'identifiant dépasse [`MAX_ID_LEN`] octets.
    #[error("identifiant {id_type} trop long : {len} octets (maximum {MAX_ID_LEN})")]
    TooLong {
        /// Type d'identifiant concerné.
        id_type: &'static str,
        /// Longueur reçue.
        len: usize,
    },
    /// L'identifiant contient un caractère hors de `[A-Za-z0-9._-]`.
    #[error("identifiant {id_type} : caractère interdit {ch:?} à l'octet {index}")]
    InvalidChar {
        /// Type d'identifiant concerné.
        id_type: &'static str,
        /// Caractère refusé.
        ch: char,
        /// Position (en octets) du caractère refusé.
        index: usize,
    },
}

fn validate(id_type: &'static str, value: &str) -> Result<(), IdError> {
    if value.is_empty() {
        return Err(IdError::Empty { id_type });
    }
    if value.len() > MAX_ID_LEN {
        return Err(IdError::TooLong {
            id_type,
            len: value.len(),
        });
    }
    match value
        .char_indices()
        .find(|&(_, ch)| !(ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.')))
    {
        Some((index, ch)) => Err(IdError::InvalidChar { id_type, ch, index }),
        None => Ok(()),
    }
}

macro_rules! define_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        ///
        /// Invariant : 1 à [`MAX_ID_LEN`] octets dans `[A-Za-z0-9._-]`,
        /// vérifié à la construction et à la désérialisation.
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            /// Construit l'identifiant après validation.
            ///
            /// # Errors
            /// Retourne [`IdError`] si la valeur est vide, trop longue ou
            /// contient un caractère interdit.
            pub fn new(value: impl Into<String>) -> Result<Self, IdError> {
                let value = value.into();
                validate(stringify!($name), &value)?;
                Ok(Self(value))
            }

            /// Valeur textuelle de l'identifiant.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl FromStr for $name {
            type Err = IdError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Self::new(s)
            }
        }

        impl TryFrom<String> for $name {
            type Error = IdError;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl From<$name> for String {
            fn from(id: $name) -> Self {
                id.0
            }
        }
    };
}

define_id!(
    /// Identifiant d'un profil de synchronisation persistant.
    ProfileId
);
define_id!(
    /// Identifiant d'une exécution (une instance de lancement d'un profil).
    ExecutionId
);
define_id!(
    /// Identifiant d'une opération au sein d'un plan d'exécution.
    OperationId
);
define_id!(
    /// Identifiant d'un type de connecteur (par exemple `local`, `sftp`).
    ConnectorId
);
define_id!(
    /// Identifiant d'une extrémité configurée (source ou destination d'un profil).
    EndpointId
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_identifiers() {
        let id = ProfileId::new("profil-01_daily.v2").unwrap();
        assert_eq!(id.as_str(), "profil-01_daily.v2");
        assert_eq!(id.to_string(), "profil-01_daily.v2");
        assert_eq!("local".parse::<ConnectorId>().unwrap().as_str(), "local");
    }

    #[test]
    fn rejects_empty() {
        assert_eq!(
            ExecutionId::new(""),
            Err(IdError::Empty {
                id_type: "ExecutionId"
            })
        );
    }

    #[test]
    fn rejects_too_long() {
        assert!(OperationId::new("a".repeat(MAX_ID_LEN)).is_ok());
        assert_eq!(
            OperationId::new("a".repeat(MAX_ID_LEN + 1)),
            Err(IdError::TooLong {
                id_type: "OperationId",
                len: MAX_ID_LEN + 1
            })
        );
    }

    #[test]
    fn rejects_forbidden_characters() {
        for (value, ch, index) in [
            ("a b", ' ', 1),
            ("../x", '/', 2),
            ("é", 'é', 0),
            ("a\\b", '\\', 1),
            ("x\0", '\0', 1),
        ] {
            assert_eq!(
                EndpointId::new(value),
                Err(IdError::InvalidChar {
                    id_type: "EndpointId",
                    ch,
                    index
                }),
                "valeur {value:?}"
            );
        }
    }

    #[test]
    fn serde_round_trip_and_validation() {
        let id = ProfileId::new("p1").unwrap();
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"p1\"");
        assert_eq!(serde_json::from_str::<ProfileId>(&json).unwrap(), id);
        assert!(serde_json::from_str::<ProfileId>("\"bad id\"").is_err());
        assert!(serde_json::from_str::<ProfileId>("\"\"").is_err());
    }
}
