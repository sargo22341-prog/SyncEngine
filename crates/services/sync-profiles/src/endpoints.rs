//! Persistable connection metadata. Authentication values never belong here.
use crate::ProfileError;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::PathBuf;

/// Provider-specific configuration, independent of compiled connector features.
/// Unsupported builds refuse use explicitly and preserve the stored configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "snake_case", deny_unknown_fields)]
pub enum RemoteEndpoint {
    /// SSH agent authentication; strictly verified known_hosts.
    Sftp {
        /// Resolved network address (DNS resolution is external and bounded).
        address: SocketAddr,
        /// Expected host key name.
        host: String,
        /// SSH account name.
        username: String,
        /// Explicit trust file, never auto-written.
        known_hosts: PathBuf,
        /// Absolute POSIX root.
        root: String,
    },
    /// HTTPS DAV with a runtime bearer token.
    Webdav {
        /// HTTPS collection URL, without userinfo, query or fragment.
        root: String,
        /// Environment reference suffix; value comes from `SYNCENGINE_SECRET_<ref>`.
        credential_ref: String,
    },
    /// Explicitly acknowledged unencrypted FTP.
    Ftp {
        /// Resolved server address.
        address: SocketAddr,
        /// Account name.
        username: String,
        /// Absolute remote root.
        root: String,
        /// Runtime password reference, never the password.
        credential_ref: String,
        /// Explicit consent to unencrypted credentials and content.
        acknowledge_plaintext: bool,
        /// Affirmation that all other writers are excluded when mutating.
        exclusive_writers: bool,
    },
    /// Explicit TLS FTP; certificate verification is mandatory.
    Ftps {
        /// Resolved server address.
        address: SocketAddr,
        /// Verified TLS server name.
        tls_domain: String,
        /// Account name.
        username: String,
        /// Absolute remote root.
        root: String,
        /// Runtime password reference.
        credential_ref: String,
        /// Optional private CA file, without modifying system trust.
        root_certificate: Option<PathBuf>,
        /// Affirmation that all other writers are excluded when mutating.
        exclusive_writers: bool,
    },
    /// OS-authenticated UNC/CIFS root (Linux remains read only).
    Smb {
        /// Existing OS share root.
        root: PathBuf,
    },
    /// Experimental Graph source, without write/delta/snapshot guarantees.
    Onedrive {
        /// Explicit drive identifier.
        drive_id: String,
        /// Exact trusted Microsoft download hosts; no wildcard.
        download_hosts: Vec<String>,
        /// Runtime access token reference (externally obtained/refreshed).
        credential_ref: String,
    },
}
impl RemoteEndpoint {
    /// Validates metadata without making a network connection or reading secrets.
    /// # Errors
    /// Invalid trust, path, URL, credential reference or missing FTP consent.
    pub fn validate(&self) -> Result<(), ProfileError> {
        let invalid =
            || ProfileError::Invalid("configuration distante invalide (aucun secret admis)");
        let remote_root = |root: &str| {
            root.starts_with('/')
                && !root.contains(['\r', '\n', '\\'])
                && !root.split('/').any(|s| s == ".." || s == ".")
        };
        let secret = match self {
            Self::Sftp {
                host,
                username,
                known_hosts,
                root,
                ..
            } => {
                if host.is_empty()
                    || username.is_empty()
                    || !known_hosts.is_absolute()
                    || !remote_root(root)
                {
                    return Err(invalid());
                }
                None
            }
            Self::Webdav {
                root,
                credential_ref,
            } => {
                let url = url::Url::parse(root).map_err(|_| invalid())?;
                if url.scheme() != "https"
                    || url.host_str().is_none()
                    || !url.username().is_empty()
                    || url.password().is_some()
                    || url.query().is_some()
                    || url.fragment().is_some()
                {
                    return Err(invalid());
                }
                Some(credential_ref)
            }
            Self::Ftp {
                root,
                username,
                credential_ref,
                acknowledge_plaintext,
                ..
            } => {
                if !remote_root(root) || username.is_empty() || !acknowledge_plaintext {
                    return Err(invalid());
                }
                Some(credential_ref)
            }
            Self::Ftps {
                root,
                username,
                tls_domain,
                credential_ref,
                root_certificate,
                ..
            } => {
                if !remote_root(root)
                    || username.is_empty()
                    || tls_domain.is_empty()
                    || root_certificate.as_ref().is_some_and(|p| !p.is_absolute())
                {
                    return Err(invalid());
                }
                Some(credential_ref)
            }
            Self::Smb { root } => {
                if !root.is_absolute() {
                    return Err(invalid());
                }
                None
            }
            Self::Onedrive {
                drive_id,
                download_hosts,
                credential_ref,
            } => {
                if drive_id.is_empty()
                    || !drive_id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"!_-".contains(&b))
                    || download_hosts.len() > 32
                    || download_hosts.iter().any(|h| {
                        h.is_empty()
                            || !h
                                .bytes()
                                .all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b))
                    })
                {
                    return Err(invalid());
                }
                Some(credential_ref)
            }
        };
        if let Some(reference) = secret
            && (reference.is_empty()
                || reference.len() > 64
                || !reference
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_'))
        {
            return Err(invalid());
        }
        if serde_json::to_vec(self)?.len() > 16 * 1024 {
            return Err(invalid());
        }
        Ok(())
    }

    /// Stable non-secret plan identity, bound to all connection/trust metadata.
    /// Credential references are names only. No runtime token is serialized.
    /// # Errors
    /// Invalid metadata or serialization failure.
    pub fn location(&self) -> Result<String, ProfileError> {
        self.validate()?;
        Ok(serde_json::to_string(self)?)
    }
}
