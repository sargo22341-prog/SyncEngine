//! Static provider composition, outside business engines. No token persistence.
use crate::{AppError, error, io};
use connector_api::StorageConnector;
use connector_local::{LocalConnector, LocalOptions};
use std::path::Path;
use std::sync::Arc;
use sync_core::CancellationToken;
use sync_profiles::RemoteEndpoint;

/// Exact compiled choices for presentation; OneDrive is always labeled experimental.
pub(super) fn available() -> Vec<&'static str> {
    let mut values = vec!["local"];
    for (enabled, name) in [
        (cfg!(feature = "sftp"), "sftp"),
        (cfg!(feature = "webdav"), "webdav"),
        (cfg!(feature = "ftp"), "ftp"),
        (cfg!(feature = "ftps"), "ftps"),
        (cfg!(feature = "smb"), "smb"),
        (cfg!(feature = "onedrive"), "onedrive"),
    ] {
        if enabled {
            values.push(name);
        }
    }
    values
}
pub(super) fn supported(remote: &RemoteEndpoint) -> Result<(), AppError> {
    let supported = match remote {
        RemoteEndpoint::Sftp { .. } => cfg!(feature = "sftp"),
        RemoteEndpoint::Webdav { .. } => cfg!(feature = "webdav"),
        RemoteEndpoint::Ftp { .. } => cfg!(feature = "ftp"),
        RemoteEndpoint::Ftps { .. } => cfg!(feature = "ftps"),
        RemoteEndpoint::Smb { .. } => cfg!(feature = "smb"),
        RemoteEndpoint::Onedrive { .. } => cfg!(feature = "onedrive"),
    };
    if supported {
        Ok(())
    } else {
        Err(error(
            "not_supported",
            "connecteur absent de cette compilation",
        ))
    }
}
#[cfg(any(
    feature = "webdav",
    feature = "ftp",
    feature = "ftps",
    feature = "onedrive"
))]
fn secret(reference: &str) -> Result<String, AppError> {
    std::env::var(format!("SYNCENGINE_SECRET_{reference}"))
        .ok()
        .filter(|v| !v.is_empty() && v.len() <= 64 * 1024)
        .ok_or_else(|| {
            error(
                "unauthenticated",
                "identifiant runtime indisponible ; renouveler l’authentification externe",
            )
        })
}
#[cfg(feature = "onedrive")]
struct Tokens(String);
#[cfg(feature = "onedrive")]
impl sync_credentials::AccessTokenProvider for Tokens {
    fn access_token(
        &self,
        cancel: &CancellationToken,
    ) -> Result<sync_credentials::AccessToken, sync_credentials::CredentialError> {
        if cancel.is_cancelled() {
            return Err(sync_credentials::CredentialError::Cancelled);
        }
        let value = secret(&self.0).map_err(|_| sync_credentials::CredentialError::Unavailable)?;
        sync_credentials::AccessToken::new(value)
    }
}
/// Read-only connector for a preview scan. A local endpoint reuses the
/// directory handles it has already reached (one open per directory instead
/// of one walk from the root per entry); execution builds fresh connectors
/// with [`build`], which re-walk every path before each precondition.
pub(super) fn build_for_scan(
    local: &Path,
    remote: Option<&RemoteEndpoint>,
    cancel: &CancellationToken,
) -> Result<Arc<dyn StorageConnector>, AppError> {
    match remote {
        None => Ok(Arc::new(
            LocalConnector::new(local, LocalOptions::new().with_handle_reuse()).map_err(io)?,
        )),
        Some(_) => build(local, remote, false, cancel),
    }
}

pub(super) fn build(
    local: &Path,
    remote: Option<&RemoteEndpoint>,
    mutations: bool,
    cancel: &CancellationToken,
) -> Result<Arc<dyn StorageConnector>, AppError> {
    let Some(remote) = remote else {
        return Ok(Arc::new(
            LocalConnector::new(
                local,
                if mutations {
                    LocalOptions::new().with_mutations()
                } else {
                    LocalOptions::new()
                },
            )
            .map_err(io)?,
        ));
    };
    remote.validate().map_err(io)?;
    supported(remote)?;
    if cancel.is_cancelled() {
        return Err(error("cancelled", "connexion annulée"));
    }
    match remote {
        #[cfg(feature = "sftp")]
        RemoteEndpoint::Sftp {
            address,
            host,
            username,
            known_hosts,
            root,
        } => connector_registry::Endpoint::Sftp {
            options: connector_registry::SftpOptions {
                address: *address,
                host: host.clone(),
                username: username.clone(),
                known_hosts: known_hosts.clone(),
                root: root.clone(),
            },
            authentication: None,
            mutations,
        }
        .build()
        .map_err(io),
        #[cfg(feature = "webdav")]
        RemoteEndpoint::Webdav {
            root,
            credential_ref,
        } => connector_registry::Endpoint::WebDav {
            options: connector_registry::WebDavOptions { root: root.clone() },
            bearer: secret(credential_ref)?,
            mutations,
        }
        .build()
        .map_err(io),
        #[cfg(feature = "ftp")]
        RemoteEndpoint::Ftp {
            address,
            root,
            username,
            credential_ref,
            acknowledge_plaintext,
            exclusive_writers,
        } => {
            if mutations && !exclusive_writers {
                return Err(error(
                    "confirmation_required",
                    "FTP exige l’exclusion explicite des autres écrivains",
                ));
            }
            connector_registry::Endpoint::Ftp {
                options: connector_registry::ftp::Options {
                    address: *address,
                    root: root.clone(),
                },
                credentials: connector_registry::ftp::Credentials::new(
                    username,
                    &secret(credential_ref)?,
                )
                .map_err(io)?,
                acknowledge_plaintext: *acknowledge_plaintext,
                exclusive_mutations: mutations,
            }
            .build()
            .map_err(io)
        }
        #[cfg(feature = "ftps")]
        RemoteEndpoint::Ftps {
            address,
            root,
            username,
            credential_ref,
            tls_domain,
            root_certificate,
            exclusive_writers,
        } => {
            if mutations && !exclusive_writers {
                return Err(error(
                    "confirmation_required",
                    "FTPS exige l’exclusion explicite des autres écrivains",
                ));
            }
            connector_registry::Endpoint::Ftps {
                options: connector_registry::ftps::Options {
                    address: *address,
                    root: root.clone(),
                },
                credentials: connector_registry::ftps::Credentials::new(
                    username,
                    &secret(credential_ref)?,
                )
                .map_err(io)?,
                tls_domain: tls_domain.clone(),
                root_certificate: root_certificate
                    .as_ref()
                    .map(std::fs::read)
                    .transpose()
                    .map_err(io)?,
                exclusive_mutations: mutations,
            }
            .build()
            .map_err(io)
        }
        #[cfg(feature = "smb")]
        RemoteEndpoint::Smb { root } => connector_registry::Endpoint::Smb {
            root: root.clone(),
            mutations,
        }
        .build()
        .map_err(io),
        #[cfg(feature = "onedrive")]
        RemoteEndpoint::Onedrive {
            drive_id,
            download_hosts,
            credential_ref,
        } if !mutations => connector_registry::onedrive::connect(
            connector_registry::onedrive::OneDriveOptions {
                drive_id: drive_id.clone(),
                download_hosts: download_hosts.clone(),
            },
            Arc::new(Tokens(credential_ref.clone())),
            cancel,
        )
        .map_err(io),
        _ => Err(error(
            "not_supported",
            "extrémité ou mutation non prise en charge",
        )),
    }
}
