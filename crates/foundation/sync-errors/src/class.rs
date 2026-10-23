//! Classes d'erreur et gravité.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Classe d'erreur stable, exploitable par l'interface et les journaux.
///
/// La forme sérialisée et [`ErrorClass::code`] sont des contrats : ne pas les
/// renommer sans migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ErrorClass {
    /// Entrée ou configuration invalide.
    InvalidInput,
    /// Ressource introuvable.
    NotFound,
    /// Accès refusé.
    PermissionDenied,
    /// Conflit d'état (précondition non satisfaite, modification concurrente).
    Conflict,
    /// Identification absente, expirée ou refusée.
    Unauthenticated,
    /// Limitation de débit imposée par le service distant.
    Throttled,
    /// Service ou support temporairement indisponible (déconnexion, délai).
    Unavailable,
    /// Opération non prise en charge par le composant ou le stockage.
    NotSupported,
    /// Ressource épuisée (disque plein, quota atteint).
    ResourceExhausted,
    /// Données incohérentes ou contrôle d'intégrité en échec.
    Integrity,
    /// Opération annulée à la demande.
    Cancelled,
    /// Défaut interne inattendu.
    Internal,
}

impl ErrorClass {
    /// Code textuel stable (identique à la forme sérialisée).
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidInput => "invalid_input",
            Self::NotFound => "not_found",
            Self::PermissionDenied => "permission_denied",
            Self::Conflict => "conflict",
            Self::Unauthenticated => "unauthenticated",
            Self::Throttled => "throttled",
            Self::Unavailable => "unavailable",
            Self::NotSupported => "not_supported",
            Self::ResourceExhausted => "resource_exhausted",
            Self::Integrity => "integrity",
            Self::Cancelled => "cancelled",
            Self::Internal => "internal",
        }
    }

    /// Vrai si une nouvelle tentative différée peut réussir sans intervention.
    ///
    /// Une erreur transitoire ne prouve **rien** sur l'état de la ressource :
    /// en particulier, elle ne permet jamais de conclure à son absence.
    #[must_use]
    pub fn is_transient(self) -> bool {
        matches!(self, Self::Throttled | Self::Unavailable)
    }
}

impl fmt::Display for ErrorClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

/// Gravité d'un diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Non fatal : l'opération concernée a abouti ou a été sautée volontairement.
    Warning,
    /// L'opération concernée a échoué ; l'exécution peut continuer.
    Error,
    /// L'exécution entière doit s'arrêter.
    Fatal,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_matches_serialized_form() {
        for class in [
            ErrorClass::InvalidInput,
            ErrorClass::NotFound,
            ErrorClass::PermissionDenied,
            ErrorClass::Conflict,
            ErrorClass::Unauthenticated,
            ErrorClass::Throttled,
            ErrorClass::Unavailable,
            ErrorClass::NotSupported,
            ErrorClass::ResourceExhausted,
            ErrorClass::Integrity,
            ErrorClass::Cancelled,
            ErrorClass::Internal,
        ] {
            let json = serde_json::to_string(&class).unwrap();
            assert_eq!(json, format!("\"{}\"", class.code()));
            assert_eq!(class.to_string(), class.code());
        }
    }

    #[test]
    fn only_throttled_and_unavailable_are_transient() {
        assert!(ErrorClass::Throttled.is_transient());
        assert!(ErrorClass::Unavailable.is_transient());
        assert!(!ErrorClass::NotFound.is_transient());
        assert!(!ErrorClass::PermissionDenied.is_transient());
        assert!(!ErrorClass::Unauthenticated.is_transient());
    }

    #[test]
    fn severity_is_ordered() {
        assert!(Severity::Warning < Severity::Error);
        assert!(Severity::Error < Severity::Fatal);
    }
}
