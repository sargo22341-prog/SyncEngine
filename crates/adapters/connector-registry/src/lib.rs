//! Registre statique option A, sans DLL, runtime plugin ou secrets persistés.
//! Les applications obtiennent les choix compilés avec [`available`] puis
//! construisent une instance via [`Endpoint::build`]. Les options sont runtime :
//! aucun format de profil ni magasin de mots de passe n'est introduit ici.
use connector_api::{StorageConnector, StorageError};
use std::sync::Arc;

/// Protocole distinct, jamais confondu par une factory de repli.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    /// SSH/SFTP avec known_hosts strict.
    Sftp,
    /// DAV sur HTTPS uniquement.
    WebDav,
    /// FTP clair après acquittement explicite.
    Ftp,
    /// FTP sur TLS explicite sans repli clair.
    Ftps,
    /// Partage natif OS UNC/CIFS.
    Smb,
}
/// Liste exacte des providers activés à la compilation ; aucune capacité fictive.
#[must_use]
pub fn available() -> &'static [Provider] {
    &[
        #[cfg(feature = "sftp")]
        Provider::Sftp,
        #[cfg(feature = "webdav")]
        Provider::WebDav,
        #[cfg(feature = "ftp")]
        Provider::Ftp,
        #[cfg(feature = "ftps")]
        Provider::Ftps,
        #[cfg(feature = "smb")]
        Provider::Smb,
    ]
}

/// Demande de factory en mémoire seulement, volontairement sans Debug/Serialize.
/// Les secrets proviennent du service d'identifiants de l'appelant.
pub enum Endpoint {
    /// Source ou destination SSH/SFTP.
    #[cfg(feature = "sftp")]
    Sftp {
        /// Options sans secret, y compris le fichier de confiance.
        options: connector_sftp::SftpOptions,
        /// Service externe ; None sélectionne l'agent SSH.
        authentication: Option<Box<dyn connector_sftp::SftpAuthentication>>,
        /// Autorisation explicite des mutations.
        mutations: bool,
    },
    /// Source ou destination WebDAV HTTPS.
    #[cfg(feature = "webdav")]
    WebDav {
        /// URL de collection sans identifiants intégrés.
        options: connector_webdav::WebDavOptions,
        /// Secret éphémère, jamais sérialisé.
        bearer: String,
        /// Autorisation explicite des mutations.
        mutations: bool,
    },
    /// FTP clair, distinct de FTPS.
    #[cfg(feature = "ftp")]
    Ftp {
        /// Adresse et racine non secrètes.
        options: connector_ftp::Options,
        /// Identifiants runtime non sérialisables.
        credentials: connector_ftp::Credentials,
        /// Acquittement du transport non chiffré.
        acknowledge_plaintext: bool,
        /// Contrat d'exclusion des écrivains externes aux verrous SyncEngine.
        exclusive_mutations: bool,
    },
    /// FTPS explicite, validation TLS avant authentification.
    #[cfg(feature = "ftps")]
    Ftps {
        /// Adresse et racine non secrètes.
        options: connector_ftps::Options,
        /// Identifiants runtime non sérialisables.
        credentials: connector_ftps::Credentials,
        /// Nom d'hôte à vérifier dans le certificat.
        tls_domain: String,
        /// CA privée explicitement approuvée, jamais TLS désactivé.
        root_certificate: Option<Vec<u8>>,
        /// Contrat d'exclusion des écrivains externes aux verrous SyncEngine.
        exclusive_mutations: bool,
    },
    /// SMB via le système déjà authentifié.
    #[cfg(feature = "smb")]
    Smb {
        /// Racine UNC Windows ou montage CIFS Linux existant.
        root: std::path::PathBuf,
        /// Mutations Windows seulement ; refus explicite Linux.
        mutations: bool,
    },
}
impl Endpoint {
    /// Construit une extrémité dont la cohérence forte a été validée par l'appelant.
    /// Ce choix permet aux scans complets de prouver une absence. Ne pas utiliser
    /// pour un backend éventuel, non caractérisé ou qui masque des erreurs de listing.
    /// # Errors
    /// Identiques à [`Self::build`].
    pub fn build_with_verified_strong_listing(
        self,
    ) -> Result<Arc<dyn StorageConnector>, StorageError> {
        let inner = self.build()?;
        let mut caps = inner.capabilities().clone();
        caps.listing_consistency = connector_api::ListingConsistency::Strong;
        Ok(Arc::new(VerifiedListing { inner, caps }))
    }
    /// Construit un connecteur possédé, utilisable par les moteurs via le contrat.
    /// Aucun fallback de protocole ; les identifiants sont remis au provider choisi.
    /// # Errors
    /// Configuration, authentification ou disponibilité selon les préconditions
    /// documentées du provider ; aucune exception ne recopie le secret.
    pub fn build(self) -> Result<Arc<dyn StorageConnector>, StorageError> {
        match self {
            #[cfg(feature = "sftp")]
            Self::Sftp {
                options,
                authentication,
                mutations,
            } => {
                let c = match authentication {
                    Some(a) => connector_sftp::SftpConnector::with_authentication(options, a)?,
                    None => connector_sftp::SftpConnector::new(options)?,
                };
                Ok(Arc::new(if mutations { c.with_mutations() } else { c }))
            }
            #[cfg(feature = "webdav")]
            Self::WebDav {
                options,
                bearer,
                mutations,
            } => {
                let c = connector_webdav::WebDavConnector::new(options, &bearer)?;
                Ok(Arc::new(if mutations { c.with_mutations() } else { c }))
            }
            #[cfg(feature = "ftp")]
            Self::Ftp {
                options,
                credentials,
                acknowledge_plaintext,
                exclusive_mutations,
            } => {
                let c =
                    connector_ftp::FtpConnector::new(options, credentials, acknowledge_plaintext)?;
                Ok(Arc::new(if exclusive_mutations {
                    c.with_exclusive_mutations()
                } else {
                    c
                }))
            }
            #[cfg(feature = "ftps")]
            Self::Ftps {
                options,
                credentials,
                tls_domain,
                root_certificate,
                exclusive_mutations,
            } => {
                let mut c = connector_ftps::FtpsConnector::new(options, credentials, tls_domain)?;
                if let Some(ca) = root_certificate {
                    c = c.with_root_certificate(&ca)?;
                }
                Ok(Arc::new(if exclusive_mutations {
                    c.with_exclusive_mutations()
                } else {
                    c
                }))
            }
            #[cfg(feature = "smb")]
            Self::Smb { root, mutations } => {
                Ok(Arc::new(connector_smb::SmbConnector::new(root, mutations)?))
            }
        }
    }
}

struct VerifiedListing {
    inner: Arc<dyn StorageConnector>,
    caps: connector_api::StorageCapabilities,
}
impl StorageConnector for VerifiedListing {
    fn connector_id(&self) -> &sync_core::ConnectorId {
        self.inner.connector_id()
    }
    fn capabilities(&self) -> &connector_api::StorageCapabilities {
        &self.caps
    }
    fn list(
        &self,
        p: &sync_core::ResourcePath,
        t: Option<&connector_api::PageToken>,
        c: &sync_core::CancellationToken,
    ) -> Result<connector_api::ListPage, StorageError> {
        self.inner.list(p, t, c)
    }
    fn stat(
        &self,
        p: &sync_core::ResourcePath,
        c: &sync_core::CancellationToken,
    ) -> Result<sync_core::FileEntry, StorageError> {
        self.inner.stat(p, c)
    }
    fn open_read(
        &self,
        p: &sync_core::ResourcePath,
        c: &sync_core::CancellationToken,
    ) -> Result<Box<dyn connector_api::ContentReader>, StorageError> {
        self.inner.open_read(p, c)
    }
    fn open_write(
        &self,
        p: &sync_core::ResourcePath,
        o: &connector_api::WriteOptions,
        c: &sync_core::CancellationToken,
    ) -> Result<Box<dyn connector_api::ContentWriter>, StorageError> {
        self.inner.open_write(p, o, c)
    }
    fn create_directory(
        &self,
        p: &sync_core::ResourcePath,
        c: &sync_core::CancellationToken,
    ) -> Result<sync_core::FileEntry, StorageError> {
        self.inner.create_directory(p, c)
    }
    fn delete(
        &self,
        p: &sync_core::ResourcePath,
        k: sync_core::EntryKind,
        c: &sync_core::CancellationToken,
    ) -> Result<(), StorageError> {
        self.inner.delete(p, k, c)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registrations_match_only_the_enabled_features() {
        assert_eq!(
            available().contains(&Provider::Sftp),
            cfg!(feature = "sftp")
        );
        assert_eq!(
            available().contains(&Provider::WebDav),
            cfg!(feature = "webdav")
        );
        assert_eq!(available().contains(&Provider::Ftp), cfg!(feature = "ftp"));
        assert_eq!(
            available().contains(&Provider::Ftps),
            cfg!(feature = "ftps")
        );
        assert_eq!(available().contains(&Provider::Smb), cfg!(feature = "smb"));
    }
    #[cfg(feature = "ftp")]
    #[test]
    fn ftp_factory_refuses_unacknowledged_plaintext() -> Result<(), Box<dyn std::error::Error>> {
        let endpoint = Endpoint::Ftp {
            options: connector_ftp::Options {
                address: ([127, 0, 0, 1], 1).into(),
                root: "/".into(),
            },
            credentials: connector_ftp::Credentials::new("fixture", "fixture-only")?,
            acknowledge_plaintext: false,
            exclusive_mutations: false,
        };
        assert!(endpoint.build().is_err());
        Ok(())
    }
}
