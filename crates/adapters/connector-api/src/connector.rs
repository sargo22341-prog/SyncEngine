//! Trait des connecteurs de stockage.

use sync_core::{CancellationToken, ConnectorId, EntryKind, FileEntry, ResourcePath};

use crate::capabilities::{StorageCapabilities, StorageFeature};
use crate::error::{StorageError, StorageOperation};
use crate::listing::{ListPage, PageToken};
use crate::write::{ContentWriter, WriteOptions};

/// Flux de lecture du contenu d'un fichier.
///
/// Le contenu est lu par blocs : jamais chargé entièrement en mémoire.
/// L'implémentation consulte le jeton d'annulation reçu à l'ouverture avant
/// chaque bloc et renvoie alors
/// [`StorageErrorKind::Cancelled`](crate::StorageErrorKind::Cancelled).
pub trait ContentReader: Send {
    /// Lit au plus `buf.len()` octets ; `Ok(0)` signale la fin du contenu
    /// (si `buf` n'est pas vide).
    ///
    /// # Errors
    /// Erreur catégorisée du stockage (déconnexion, accès refusé, annulation…).
    /// Après une erreur, l'état du flux est indéterminé : ne pas reprendre la lecture.
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, StorageError>;
}

/// Connecteur de stockage.
///
/// # Contrat
/// - [`capabilities`](Self::capabilities) décrit honnêtement ce que le
///   connecteur sait faire ; toute opération non déclarée échoue avec
///   [`StorageErrorKind::NotSupported`](crate::StorageErrorKind::NotSupported) ;
/// - le connecteur ne choisit aucune règle de synchronisation ;
/// - les secrets ne figurent ni dans les erreurs, ni dans les capacités ;
/// - aucun accès ne sort de la racine de l'extrémité : un [`ResourcePath`]
///   est toujours relatif à cette racine, et le connecteur refait les
///   contrôles propres à son protocole ou à l'OS avant tout accès.
///
/// # Modèle d'exécution
/// Les opérations sont **synchrones et bloquantes**. Un appelant async doit
/// les exécuter sur un fil dédié (`spawn_blocking` ou équivalent) ; un
/// connecteur fondé sur un protocole async encapsule son propre exécuteur.
/// L'annulation est coopérative ([`CancellationToken`]) : elle est consultée
/// entre deux étapes bornées, jamais au milieu d'un appel système.
///
/// # Périmètre
/// Phase 02 : opérations **sans mutation** (`list`, `stat`, `open_read`).
/// Phase 04 : mutations élémentaires ([`create_directory`](Self::create_directory),
/// [`open_write`](Self::open_write) en deux temps, [`delete`](Self::delete)
/// non récursive). Elles ont une implémentation par défaut qui répond
/// [`NotSupported`](crate::StorageErrorKind::NotSupported) : un connecteur
/// en lecture seule n'a rien à écrire et ne peut pas muter par mégarde.
/// Le connecteur exécute une mutation demandée ; il ne décide jamais qu'elle
/// doit avoir lieu (c'est l'exécuteur qui revérifie les préconditions juste
/// avant).
pub trait StorageConnector: Send + Sync {
    /// Maximum number of independent directory listings that may be interleaved
    /// concurrently on this instance, including their pagination cursors.
    /// The conservative default serializes listings. This never authorizes writes.
    fn listing_concurrency(&self) -> std::num::NonZeroUsize {
        std::num::NonZeroUsize::MIN
    }

    /// Identifiant du type de connecteur (`local`, `sftp`…).
    fn connector_id(&self) -> &ConnectorId;

    /// Capacités effectives de ce connecteur.
    fn capabilities(&self) -> &StorageCapabilities;

    /// Liste une page des enfants directs de `directory`.
    ///
    /// `page` vaut `None` pour la première page, puis le jeton renvoyé par
    /// la page précédente.
    ///
    /// # Préconditions
    /// `directory` désigne un répertoire existant (la racine est
    /// [`ResourcePath::root`]).
    ///
    /// # Errors
    /// [`NotFound`](crate::StorageErrorKind::NotFound),
    /// [`PermissionDenied`](crate::StorageErrorKind::PermissionDenied),
    /// [`Disconnected`](crate::StorageErrorKind::Disconnected) (montage absent,
    /// réseau), [`Cancelled`](crate::StorageErrorKind::Cancelled)… Une erreur
    /// n'est **jamais** convertie en page vide, et une entrée non
    /// représentable est signalée par une [`ListingIssue`](crate::ListingIssue),
    /// jamais ignorée en silence.
    fn list(
        &self,
        directory: &ResourcePath,
        page: Option<&PageToken>,
        cancel: &CancellationToken,
    ) -> Result<ListPage, StorageError>;

    /// Lit les métadonnées d'une entrée, sans suivre un lien symbolique final.
    ///
    /// # Errors
    /// [`NotFound`](crate::StorageErrorKind::NotFound) si l'entrée n'existe
    /// pas ; autres catégories selon la cause.
    fn stat(
        &self,
        path: &ResourcePath,
        cancel: &CancellationToken,
    ) -> Result<FileEntry, StorageError>;

    /// Ouvre le contenu d'un fichier ordinaire en lecture, sous forme de flux.
    ///
    /// # Errors
    /// [`NotSupported`](crate::StorageErrorKind::NotSupported) si la lecture
    /// n'est pas déclarée ; une erreur si `path` n'est pas un fichier
    /// ordinaire (les liens symboliques ne sont pas suivis).
    fn open_read(
        &self,
        path: &ResourcePath,
        cancel: &CancellationToken,
    ) -> Result<Box<dyn ContentReader>, StorageError>;

    /// Crée **un** répertoire ; son parent doit exister (pas de création
    /// implicite des ancêtres).
    ///
    /// # Errors
    /// [`Conflict`](crate::StorageErrorKind::Conflict) si une entrée existe
    /// déjà à ce chemin ; [`NotFound`](crate::StorageErrorKind::NotFound) si
    /// le parent manque ; [`NotSupported`](crate::StorageErrorKind::NotSupported)
    /// si la capacité [`StorageFeature::CreateDirectory`] n'est pas déclarée
    /// (implémentation par défaut).
    fn create_directory(
        &self,
        path: &ResourcePath,
        cancel: &CancellationToken,
    ) -> Result<FileEntry, StorageError> {
        let _ = cancel;
        Err(
            StorageError::not_supported(StorageOperation::CreateDirectory)
                .with_connector(self.connector_id().clone())
                .with_path(path.clone()),
        )
    }

    /// Ouvre l'écriture en deux temps du fichier `path` (voir
    /// [`ContentWriter`]) ; le parent doit exister. Rien n'est visible à
    /// `path` avant [`ContentWriter::commit`].
    ///
    /// # Errors
    /// [`NotSupported`](crate::StorageErrorKind::NotSupported) si la capacité
    /// [`StorageFeature::Write`] n'est pas déclarée (implémentation par
    /// défaut) ; [`NotFound`](crate::StorageErrorKind::NotFound) si le
    /// parent manque ; autres catégories selon la cause.
    fn open_write(
        &self,
        path: &ResourcePath,
        options: &WriteOptions,
        cancel: &CancellationToken,
    ) -> Result<Box<dyn ContentWriter>, StorageError> {
        let _ = (options, cancel);
        Err(StorageError::not_supported(StorageOperation::Write)
            .with_connector(self.connector_id().clone())
            .with_path(path.clone()))
    }

    /// Supprime l'entrée `path`, qui doit être de nature `expected` : un
    /// fichier ordinaire, ou un répertoire **vide** (jamais de suppression
    /// récursive). Un lien symbolique n'est ni supprimé ni suivi.
    ///
    /// # Errors
    /// [`Conflict`](crate::StorageErrorKind::Conflict) si la nature diffère
    /// de `expected` ou si le répertoire n'est pas vide ;
    /// [`NotFound`](crate::StorageErrorKind::NotFound) si l'entrée n'existe
    /// pas ; [`NotSupported`](crate::StorageErrorKind::NotSupported) si la
    /// capacité [`StorageFeature::Delete`] n'est pas déclarée (implémentation
    /// par défaut) ou pour une autre nature qu'un fichier ou un répertoire.
    fn delete(
        &self,
        path: &ResourcePath,
        expected: EntryKind,
        cancel: &CancellationToken,
    ) -> Result<(), StorageError> {
        let _ = (expected, cancel);
        Err(StorageError::not_supported(StorageOperation::Delete)
            .with_connector(self.connector_id().clone())
            .with_path(path.clone()))
    }
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

    /// Double de test : connecteur sans contenu, qui ne déclare que `List`.
    struct ListOnlyFake {
        id: ConnectorId,
        caps: StorageCapabilities,
    }

    impl StorageConnector for ListOnlyFake {
        fn connector_id(&self) -> &ConnectorId {
            &self.id
        }

        fn capabilities(&self) -> &StorageCapabilities {
            &self.caps
        }

        fn list(
            &self,
            _directory: &ResourcePath,
            _page: Option<&PageToken>,
            _cancel: &CancellationToken,
        ) -> Result<ListPage, StorageError> {
            Ok(ListPage::default())
        }

        fn stat(
            &self,
            _path: &ResourcePath,
            _cancel: &CancellationToken,
        ) -> Result<FileEntry, StorageError> {
            ensure_supported(self, StorageFeature::Stat, StorageOperation::Stat)?;
            Err(StorageError::new(
                StorageErrorKind::NotFound,
                StorageOperation::Stat,
            ))
        }

        fn open_read(
            &self,
            _path: &ResourcePath,
            _cancel: &CancellationToken,
        ) -> Result<Box<dyn ContentReader>, StorageError> {
            ensure_supported(self, StorageFeature::Read, StorageOperation::Read)?;
            Err(StorageError::new(
                StorageErrorKind::NotFound,
                StorageOperation::Read,
            ))
        }
    }

    fn fake() -> Box<dyn StorageConnector> {
        Box::new(ListOnlyFake {
            id: ConnectorId::new("fake").unwrap(),
            caps: StorageCapabilities::none().with_features([StorageFeature::List]),
        })
    }

    #[test]
    fn supported_feature_passes() {
        let connector = fake();
        assert!(
            ensure_supported(
                connector.as_ref(),
                StorageFeature::List,
                StorageOperation::List
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
    fn undeclared_operations_return_not_supported() {
        let connector = fake();
        let cancel = CancellationToken::new();
        let path = ResourcePath::parse("a").unwrap();
        let error = connector.open_read(&path, &cancel).err().unwrap();
        assert_eq!(error.kind(), StorageErrorKind::NotSupported);
        let error = connector.stat(&path, &cancel).unwrap_err();
        assert_eq!(error.kind(), StorageErrorKind::NotSupported);
    }

    #[test]
    fn mutations_are_not_supported_by_default() {
        let connector = fake();
        let cancel = CancellationToken::new();
        let path = ResourcePath::parse("a").unwrap();
        let error = connector.create_directory(&path, &cancel).unwrap_err();
        assert_eq!(error.kind(), StorageErrorKind::NotSupported);
        assert_eq!(error.operation(), StorageOperation::CreateDirectory);
        let error = connector
            .open_write(&path, &WriteOptions::new(), &cancel)
            .err()
            .unwrap();
        assert_eq!(error.kind(), StorageErrorKind::NotSupported);
        assert_eq!(error.operation(), StorageOperation::Write);
        let error = connector
            .delete(&path, EntryKind::File, &cancel)
            .unwrap_err();
        assert_eq!(error.kind(), StorageErrorKind::NotSupported);
        assert_eq!(error.operation(), StorageOperation::Delete);
        assert_eq!(error.path(), Some(&path));
    }

    #[test]
    fn trait_objects_are_send_and_sync() {
        fn assert_send_sync<T: Send + Sync + ?Sized>() {}
        fn assert_send<T: Send + ?Sized>() {}
        assert_send_sync::<dyn StorageConnector>();
        assert_send::<dyn ContentReader>();
        assert_send::<dyn ContentWriter>();
    }
}
