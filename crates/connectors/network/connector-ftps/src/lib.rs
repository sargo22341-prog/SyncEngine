//! Connecteur FTPS explicite, activé avec `ftps` ; mutations explicitement exclusives.
#[cfg(feature = "ftps")]
mod provider {
    use connector_api::{
        ContentReader, ContentWriter, ListPage, PageToken, StorageCapabilities, StorageConnector,
        StorageError, WriteOptions,
    };
    /// Identifiants mémoire fournis par un service externe.
    pub use connector_ftp_transport::Credentials;
    use connector_ftp_transport::Endpoint;
    /// Configuration non secrète du provider.
    pub use connector_ftp_transport::Options;
    use sync_core::{CancellationToken, ConnectorId, EntryKind, FileEntry, ResourcePath};
    /// Provider FTPS explicite, distinct des autres protocoles.
    pub struct FtpsConnector(Endpoint);
    impl FtpsConnector {
        /// Ajoute une CA privée explicitement approuvée, sans contourner TLS.
        /// # Errors
        /// Certificat PEM invalide.
        pub fn with_root_certificate(self, pem: &[u8]) -> Result<Self, StorageError> {
            Ok(Self(self.0.with_root_certificate(pem)?))
        }
        /// Valide la configuration sans accès réseau.
        /// # Errors
        /// Configuration invalide ; FTPS explicite n'effectue jamais de repli vers un autre protocole.
        pub fn new(
            options: Options,
            credentials: Credentials,
            tls_domain: String,
        ) -> Result<Self, StorageError> {
            Ok(Self(Endpoint::ftps(options, credentials, tls_domain)?))
        }
        /// Autorise les mutations sous verrou serveur. L'appelant garantit aussi
        /// l'exclusion des ?crivains externes ignorant le verrou SyncEngine.
        #[must_use]
        pub fn with_exclusive_mutations(self) -> Self {
            Self(self.0.with_exclusive_mutations())
        }
    }
    impl StorageConnector for FtpsConnector {
        fn connector_id(&self) -> &ConnectorId {
            self.0.connector_id()
        }
        fn capabilities(&self) -> &StorageCapabilities {
            self.0.capabilities()
        }
        fn list(
            &self,
            p: &ResourcePath,
            t: Option<&PageToken>,
            c: &CancellationToken,
        ) -> Result<ListPage, StorageError> {
            self.0.list(p, t, c)
        }
        fn stat(&self, p: &ResourcePath, c: &CancellationToken) -> Result<FileEntry, StorageError> {
            self.0.stat(p, c)
        }
        fn open_read(
            &self,
            p: &ResourcePath,
            c: &CancellationToken,
        ) -> Result<Box<dyn ContentReader>, StorageError> {
            self.0.open_read(p, c)
        }
        fn open_write(
            &self,
            p: &ResourcePath,
            o: &WriteOptions,
            c: &CancellationToken,
        ) -> Result<Box<dyn ContentWriter>, StorageError> {
            self.0.open_write(p, o, c)
        }
        fn create_directory(
            &self,
            p: &ResourcePath,
            c: &CancellationToken,
        ) -> Result<FileEntry, StorageError> {
            self.0.create_directory(p, c)
        }
        fn delete(
            &self,
            p: &ResourcePath,
            k: EntryKind,
            c: &CancellationToken,
        ) -> Result<(), StorageError> {
            self.0.delete(p, k, c)
        }
    }
}
#[cfg(feature = "ftps")]
pub use provider::{Credentials, FtpsConnector, Options};
