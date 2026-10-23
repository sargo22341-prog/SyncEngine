//! Contexte non secret attaché aux erreurs.

use serde::{Deserialize, Serialize};
use sync_core::{ExecutionId, OperationId, ProfileId, ResourcePath};

/// Contexte applicatif d'une erreur.
///
/// Ne contient que des identifiants typés et un chemin logique : aucun champ
/// libre où un secret pourrait être glissé.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorContext {
    profile_id: Option<ProfileId>,
    execution_id: Option<ExecutionId>,
    operation_id: Option<OperationId>,
    path: Option<ResourcePath>,
}

impl ErrorContext {
    /// Contexte vide.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Rattache le profil concerné.
    #[must_use]
    pub fn with_profile(mut self, id: ProfileId) -> Self {
        self.profile_id = Some(id);
        self
    }

    /// Rattache l'exécution concernée.
    #[must_use]
    pub fn with_execution(mut self, id: ExecutionId) -> Self {
        self.execution_id = Some(id);
        self
    }

    /// Rattache l'opération de plan concernée.
    #[must_use]
    pub fn with_operation(mut self, id: OperationId) -> Self {
        self.operation_id = Some(id);
        self
    }

    /// Rattache le chemin logique concerné.
    #[must_use]
    pub fn with_path(mut self, path: ResourcePath) -> Self {
        self.path = Some(path);
        self
    }

    /// Profil concerné.
    #[must_use]
    pub fn profile_id(&self) -> Option<&ProfileId> {
        self.profile_id.as_ref()
    }

    /// Exécution concernée.
    #[must_use]
    pub fn execution_id(&self) -> Option<&ExecutionId> {
        self.execution_id.as_ref()
    }

    /// Opération concernée.
    #[must_use]
    pub fn operation_id(&self) -> Option<&OperationId> {
        self.operation_id.as_ref()
    }

    /// Chemin logique concerné.
    #[must_use]
    pub fn path(&self) -> Option<&ResourcePath> {
        self.path.as_ref()
    }

    /// Complète les champs absents de `self` avec ceux de `outer`.
    ///
    /// Les valeurs déjà présentes (les plus proches de l'erreur) sont gardées.
    #[must_use]
    pub fn or(mut self, outer: &Self) -> Self {
        self.profile_id = self.profile_id.or_else(|| outer.profile_id.clone());
        self.execution_id = self.execution_id.or_else(|| outer.execution_id.clone());
        self.operation_id = self.operation_id.or_else(|| outer.operation_id.clone());
        self.path = self.path.or_else(|| outer.path.clone());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inner_values_take_precedence() {
        let inner = ErrorContext::new().with_path(ResourcePath::parse("a/b").unwrap());
        let outer = ErrorContext::new()
            .with_profile(ProfileId::new("p").unwrap())
            .with_path(ResourcePath::parse("a").unwrap());
        let merged = inner.or(&outer);
        assert_eq!(merged.profile_id().map(ProfileId::as_str), Some("p"));
        assert_eq!(merged.path().map(ResourcePath::as_str), Some("a/b"));
        assert_eq!(merged.execution_id(), None);
    }
}
