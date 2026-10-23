//! Rattachement des erreurs concrètes à une classe et instantané sérialisable.

use std::error::Error;

use serde::{Deserialize, Serialize};

use crate::class::{ErrorClass, Severity};
use crate::context::ErrorContext;
use crate::redact::redact_secrets;

/// Nombre maximal de causes recopiées dans un [`ErrorReport`].
pub const MAX_REPORTED_CAUSES: usize = 16;

/// Erreur concrète rattachée à une [`ErrorClass`] stable.
///
/// À implémenter par les types d'erreur publics de chaque crate.
pub trait ClassifiedError: Error {
    /// Classe stable de l'erreur.
    fn class(&self) -> ErrorClass;

    /// Contexte non secret, s'il est connu à ce niveau.
    fn context(&self) -> Option<&ErrorContext> {
        None
    }
}

/// Instantané sérialisable et caviardé d'une erreur.
///
/// Destiné à l'interface, aux journaux et aux événements. Les textes
/// (`message`, `causes`) passent par [`redact_secrets`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorReport {
    class: ErrorClass,
    severity: Severity,
    message: String,
    context: ErrorContext,
    causes: Vec<String>,
}

impl ErrorReport {
    /// Capture `error`, sa chaîne de causes (au plus [`MAX_REPORTED_CAUSES`])
    /// et son contexte, complété par `outer` pour les champs absents.
    #[must_use]
    pub fn capture(error: &dyn ClassifiedError, severity: Severity, outer: &ErrorContext) -> Self {
        let context = error.context().cloned().unwrap_or_default().or(outer);
        let mut causes = Vec::new();
        let mut source = error.source();
        while let Some(cause) = source {
            if causes.len() == MAX_REPORTED_CAUSES {
                break;
            }
            causes.push(redact_secrets(&cause.to_string()));
            source = cause.source();
        }
        Self {
            class: error.class(),
            severity,
            message: redact_secrets(&error.to_string()),
            context,
            causes,
        }
    }

    /// Classe de l'erreur.
    #[must_use]
    pub fn class(&self) -> ErrorClass {
        self.class
    }

    /// Gravité attribuée par l'appelant.
    #[must_use]
    pub fn severity(&self) -> Severity {
        self.severity
    }

    /// Message principal caviardé.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Contexte non secret.
    #[must_use]
    pub fn context(&self) -> &ErrorContext {
        &self.context
    }

    /// Causes successives caviardées, de la plus proche à la plus profonde.
    #[must_use]
    pub fn causes(&self) -> &[String] {
        &self.causes
    }
}

#[cfg(test)]
mod tests {
    use sync_core::{ExecutionId, ProfileId, ResourcePath};

    use super::*;

    #[derive(Debug, thiserror::Error)]
    #[error("connexion perdue vers sftp://bob:pw@host/x")]
    struct Network;

    #[derive(Debug, thiserror::Error)]
    #[error("lecture impossible")]
    struct ReadFailed {
        #[source]
        source: Network,
        context: ErrorContext,
    }

    impl ClassifiedError for ReadFailed {
        fn class(&self) -> ErrorClass {
            ErrorClass::Unavailable
        }

        fn context(&self) -> Option<&ErrorContext> {
            Some(&self.context)
        }
    }

    #[derive(Debug, thiserror::Error)]
    #[error("niveau {0}")]
    struct Deep(u32, #[source] Option<Box<Deep>>);

    impl ClassifiedError for Deep {
        fn class(&self) -> ErrorClass {
            ErrorClass::Internal
        }
    }

    #[test]
    fn captures_class_causes_and_context() {
        let error = ReadFailed {
            source: Network,
            context: ErrorContext::new().with_path(ResourcePath::parse("a/b").unwrap()),
        };
        let outer = ErrorContext::new()
            .with_profile(ProfileId::new("p").unwrap())
            .with_execution(ExecutionId::new("e1").unwrap());
        let report = ErrorReport::capture(&error, Severity::Error, &outer);

        assert_eq!(report.class(), ErrorClass::Unavailable);
        assert_eq!(report.severity(), Severity::Error);
        assert_eq!(report.message(), "lecture impossible");
        assert_eq!(report.causes(), ["connexion perdue vers sftp://***@host/x"]);
        assert_eq!(
            report.context().path().map(ResourcePath::as_str),
            Some("a/b")
        );
        assert_eq!(
            report.context().execution_id().map(ExecutionId::as_str),
            Some("e1")
        );

        let json = serde_json::to_string(&report).unwrap();
        assert!(!json.contains("pw@"));
        assert_eq!(serde_json::from_str::<ErrorReport>(&json).unwrap(), report);
    }

    #[test]
    fn cause_chain_is_bounded() {
        let mut error = Deep(0, None);
        for level in 1..=40 {
            error = Deep(level, Some(Box::new(error)));
        }
        let report = ErrorReport::capture(&error, Severity::Fatal, &ErrorContext::new());
        assert_eq!(report.causes().len(), MAX_REPORTED_CAUSES);
        assert_eq!(report.causes()[0], "niveau 39");
    }
}
