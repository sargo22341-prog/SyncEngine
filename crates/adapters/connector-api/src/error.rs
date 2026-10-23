//! Erreurs catégorisées des connecteurs.

use std::error::Error;
use std::fmt;
use std::time::Duration;

use sync_core::{ConnectorId, ResourcePath};
use sync_errors::{ClassifiedError, ErrorClass, ErrorContext, redact_secrets};

/// Opération de stockage concernée par une erreur.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum StorageOperation {
    /// Connexion ou ouverture de l'extrémité.
    Connect,
    /// Listing.
    List,
    /// Lecture de métadonnées.
    Stat,
    /// Lecture de contenu.
    Read,
    /// Écriture de contenu.
    Write,
    /// Création de répertoire.
    CreateDirectory,
    /// Déplacement ou renommage.
    Move,
    /// Suppression.
    Delete,
    /// Copie côté serveur.
    ServerSideCopy,
}

impl StorageOperation {
    fn label(self) -> &'static str {
        match self {
            Self::Connect => "connect",
            Self::List => "list",
            Self::Stat => "stat",
            Self::Read => "read",
            Self::Write => "write",
            Self::CreateDirectory => "create_directory",
            Self::Move => "move",
            Self::Delete => "delete",
            Self::ServerSideCopy => "server_side_copy",
        }
    }
}

impl fmt::Display for StorageOperation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Catégorie d'erreur de stockage.
///
/// Règle de sûreté : **aucune** catégorie, pas même [`Self::NotFound`]
/// renvoyée par un listing partiel, ne doit être interprétée par un appelant
/// comme la preuve qu'une source est vide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum StorageErrorKind {
    /// L'entrée demandée n'existe pas.
    NotFound,
    /// Accès refusé par le stockage ou l'OS.
    PermissionDenied,
    /// Précondition non satisfaite (la cible a changé, existe déjà…).
    Conflict,
    /// Limitation de débit ; délai conseillé s'il est fourni par le service.
    Throttled {
        /// Délai conseillé avant nouvelle tentative.
        retry_after: Option<Duration>,
    },
    /// Opération ou option non prise en charge par ce connecteur.
    NotSupported,
    /// Identifiants absents, expirés ou refusés.
    Unauthenticated,
    /// Connexion perdue ou extrémité injoignable (montage absent, réseau).
    Disconnected,
    /// Échec temporaire, nouvelle tentative possible.
    Transient,
    /// Échec définitif.
    Permanent,
    /// Espace disque ou quota insuffisant.
    InsufficientSpace,
    /// Opération annulée à la demande.
    Cancelled,
}

impl StorageErrorKind {
    /// Vrai si une nouvelle tentative différée peut réussir.
    #[must_use]
    pub fn is_transient(self) -> bool {
        matches!(
            self,
            Self::Throttled { .. } | Self::Disconnected | Self::Transient
        )
    }

    /// Classe applicative correspondante.
    #[must_use]
    pub fn class(self) -> ErrorClass {
        match self {
            Self::NotFound => ErrorClass::NotFound,
            Self::PermissionDenied => ErrorClass::PermissionDenied,
            Self::Conflict => ErrorClass::Conflict,
            Self::Throttled { .. } => ErrorClass::Throttled,
            Self::NotSupported => ErrorClass::NotSupported,
            Self::Unauthenticated => ErrorClass::Unauthenticated,
            Self::Disconnected | Self::Transient => ErrorClass::Unavailable,
            Self::Permanent => ErrorClass::Internal,
            Self::InsufficientSpace => ErrorClass::ResourceExhausted,
            Self::Cancelled => ErrorClass::Cancelled,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::NotFound => "introuvable",
            Self::PermissionDenied => "accès refusé",
            Self::Conflict => "conflit de précondition",
            Self::Throttled { .. } => "débit limité par le service",
            Self::NotSupported => "non pris en charge",
            Self::Unauthenticated => "authentification requise ou refusée",
            Self::Disconnected => "connexion perdue ou extrémité injoignable",
            Self::Transient => "échec temporaire",
            Self::Permanent => "échec définitif",
            Self::InsufficientSpace => "espace insuffisant",
            Self::Cancelled => "annulé",
        }
    }
}

/// Erreur d'un connecteur de stockage.
///
/// Le message libre est caviardé à la construction ([`redact_secrets`]) ;
/// il ne doit de toute façon jamais contenir de secret.
#[derive(Debug)]
pub struct StorageError(Box<Inner>);

// Contenu en boîte : `Result<_, StorageError>` reste de la taille d'un pointeur.
#[derive(Debug)]
struct Inner {
    kind: StorageErrorKind,
    operation: StorageOperation,
    connector: Option<ConnectorId>,
    context: ErrorContext,
    message: Option<String>,
    source: Option<Box<dyn Error + Send + Sync + 'static>>,
}

impl StorageError {
    /// Crée une erreur pour `operation`.
    #[must_use]
    pub fn new(kind: StorageErrorKind, operation: StorageOperation) -> Self {
        Self(Box::new(Inner {
            kind,
            operation,
            connector: None,
            context: ErrorContext::new(),
            message: None,
            source: None,
        }))
    }

    /// Raccourci : opération refusée car la capacité n'est pas déclarée.
    #[must_use]
    pub fn not_supported(operation: StorageOperation) -> Self {
        Self::new(StorageErrorKind::NotSupported, operation)
    }

    /// Indique le connecteur à l'origine de l'erreur.
    #[must_use]
    pub fn with_connector(mut self, connector: ConnectorId) -> Self {
        self.0.connector = Some(connector);
        self
    }

    /// Indique le chemin logique concerné.
    #[must_use]
    pub fn with_path(mut self, path: ResourcePath) -> Self {
        self.0.context = self.0.context.with_path(path);
        self
    }

    /// Ajoute un détail lisible (caviardé).
    #[must_use]
    pub fn with_message(mut self, message: impl AsRef<str>) -> Self {
        self.0.message = Some(redact_secrets(message.as_ref()));
        self
    }

    /// Conserve l'erreur d'origine dans la chaîne de causes.
    #[must_use]
    pub fn with_source(mut self, source: impl Error + Send + Sync + 'static) -> Self {
        self.0.source = Some(Box::new(source));
        self
    }

    /// Catégorie.
    #[must_use]
    pub fn kind(&self) -> StorageErrorKind {
        self.0.kind
    }

    /// Opération concernée.
    #[must_use]
    pub fn operation(&self) -> StorageOperation {
        self.0.operation
    }

    /// Connecteur à l'origine, s'il est indiqué.
    #[must_use]
    pub fn connector(&self) -> Option<&ConnectorId> {
        self.0.connector.as_ref()
    }

    /// Chemin logique concerné, s'il est indiqué.
    #[must_use]
    pub fn path(&self) -> Option<&ResourcePath> {
        self.0.context.path()
    }

    /// Vrai si une nouvelle tentative différée peut réussir.
    #[must_use]
    pub fn is_transient(&self) -> bool {
        self.0.kind.is_transient()
    }
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(connector) = &self.0.connector {
            write!(f, "[{connector}] ")?;
        }
        write!(f, "{} : {}", self.0.operation, self.0.kind.label())?;
        if let Some(path) = self.0.context.path() {
            write!(f, " ({path})")?;
        }
        if let Some(message) = &self.0.message {
            write!(f, " — {message}")?;
        }
        Ok(())
    }
}

impl Error for StorageError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.0
            .source
            .as_deref()
            .map(|source| source as &(dyn Error + 'static))
    }
}

impl ClassifiedError for StorageError {
    fn class(&self) -> ErrorClass {
        self.0.kind.class()
    }

    fn context(&self) -> Option<&ErrorContext> {
        Some(&self.0.context)
    }
}

#[cfg(test)]
mod tests {
    use sync_errors::{ErrorReport, Severity};

    use super::*;

    #[test]
    fn transient_kinds() {
        assert!(StorageErrorKind::Disconnected.is_transient());
        assert!(StorageErrorKind::Transient.is_transient());
        assert!(StorageErrorKind::Throttled { retry_after: None }.is_transient());
        for kind in [
            StorageErrorKind::NotFound,
            StorageErrorKind::PermissionDenied,
            StorageErrorKind::Conflict,
            StorageErrorKind::NotSupported,
            StorageErrorKind::Unauthenticated,
            StorageErrorKind::Permanent,
            StorageErrorKind::InsufficientSpace,
            StorageErrorKind::Cancelled,
        ] {
            assert!(!kind.is_transient(), "{kind:?}");
        }
    }

    #[test]
    fn class_mapping() {
        assert_eq!(
            StorageErrorKind::Disconnected.class(),
            ErrorClass::Unavailable
        );
        assert_eq!(
            StorageErrorKind::InsufficientSpace.class(),
            ErrorClass::ResourceExhausted
        );
        assert_eq!(
            StorageErrorKind::NotSupported.class(),
            ErrorClass::NotSupported
        );
    }

    #[test]
    fn display_contains_context_and_redacts_message() {
        let error = StorageError::new(StorageErrorKind::PermissionDenied, StorageOperation::Read)
            .with_connector(ConnectorId::new("local").unwrap())
            .with_path(ResourcePath::parse("docs/a.txt").unwrap())
            .with_message("token=abc123 refusé");
        assert_eq!(
            error.to_string(),
            "[local] read : accès refusé (docs/a.txt) — token=*** refusé"
        );
    }

    #[test]
    fn keeps_source_chain_in_report() {
        let io = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "os error 5");
        let error = StorageError::new(StorageErrorKind::PermissionDenied, StorageOperation::List)
            .with_source(io);
        assert!(error.source().is_some());
        let report = ErrorReport::capture(&error, Severity::Error, &ErrorContext::new());
        assert_eq!(report.class(), ErrorClass::PermissionDenied);
        assert_eq!(report.causes(), ["os error 5"]);
    }
}
