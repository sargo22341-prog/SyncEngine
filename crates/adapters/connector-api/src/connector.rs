//! Trait des connecteurs de stockage.

use sync_core::ConnectorId;

use crate::capabilities::{StorageCapabilities, StorageFeature};
use crate::error::{StorageError, StorageOperation};

/// Connecteur de stockage.
///
/// # Contrat
/// - [`capabilities`](Self::capabilities) décrit honnêtement ce que le
///   connecteur sait faire ; toute opération non déclarée échoue avec
///   [`StorageErrorKind::NotSupported`](crate::StorageErrorKind::NotSupported) ;
/// - le connecteur ne choisit aucune règle de synchronisation ;
/// - les secrets ne figurent ni dans les erreurs, ni dans les capacités.
///
/// Les opérations d'E/S seront ajoutées en phase 02 ; elles s'ajouteront
/// à ce trait comme une évolution d'API soumise à revue.
pub trait StorageConnector: Send + Sync {
    /// Identifiant du type de connecteur (`local`, `sftp`…).
    fn connector_id(&self) -> &ConnectorId;

    /// Capacités effectives de ce connecteur.
    fn capabilities(&self) -> &StorageCapabilities;
}

/// Vérifie qu'un connecteur déclare `feature` avant d'exécuter `operation`.
///
/// # Errors
/// [`StorageErrorKind::NotSupported`](crate::StorageErrorKind::NotSupported)
/// si la fonctionnalité n'est pas déclarée.
pub fn ensure_supported(
    connector: &dyn StorageConnector,
    feature: StorageFeature,
    operation: StorageOperation,
) -> Result<(), StorageError> {
    if connector.capabilities().supports(feature) {
        Ok(())
    } else {
        Err(StorageError::not_supported(operation)
            .with_connector(connector.connector_id().clone())
            .with_message(format!("capacité absente : {feature:?}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StorageErrorKind;

    /// Double de test : connecteur en lecture seule, sans aucune E/S.
    struct ReadOnlyFake {
        id: ConnectorId,
        caps: StorageCapabilities,
    }

    impl StorageConnector for ReadOnlyFake {
        fn connector_id(&self) -> &ConnectorId {
            &self.id
        }

        fn capabilities(&self) -> &StorageCapabilities {
            &self.caps
        }
    }

    fn fake() -> Box<dyn StorageConnector> {
        Box::new(ReadOnlyFake {
            id: ConnectorId::new("fake").unwrap(),
            caps: StorageCapabilities::none()
                .with_features([StorageFeature::List, StorageFeature::Read]),
        })
    }

    #[test]
    fn supported_feature_passes() {
        let connector = fake();
        assert!(
            ensure_supported(
                connector.as_ref(),
                StorageFeature::Read,
                StorageOperation::Read
            )
            .is_ok()
        );
    }

    #[test]
    fn unsupported_feature_is_refused_explicitly() {
        let connector = fake();
        let error = ensure_supported(
            connector.as_ref(),
            StorageFeature::Delete,
            StorageOperation::Delete,
        )
        .unwrap_err();
        assert_eq!(error.kind(), StorageErrorKind::NotSupported);
        assert_eq!(error.operation(), StorageOperation::Delete);
        assert_eq!(error.connector().map(ConnectorId::as_str), Some("fake"));
    }

    #[test]
    fn trait_objects_are_send_and_sync() {
        fn assert_send_sync<T: Send + Sync + ?Sized>() {}
        assert_send_sync::<dyn StorageConnector>();
    }
}
