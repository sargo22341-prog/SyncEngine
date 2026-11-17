//! Runtime credentials a profile needs, checked before any connection.
//!
//! Secrets never live in profiles: they are read from the process
//! environment (`SYNCENGINE_SECRET_<référence>`), and SFTP authenticates
//! through the SSH agent (`SSH_AUTH_SOCK` under Unix). An interface launched
//! from a desktop menu, or an agent started by systemd, does not inherit the
//! variables of a terminal: this check names what is missing, before a
//! preview fails on it (audit Linux M4). Values are never read here beyond
//! presence, and never returned.

use serde::Serialize;
use sync_profiles::{Profile, RemoteEndpoint};

/// Variable expected by a profile and absent from this process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MissingCredential {
    /// Environment variable name (never its value).
    pub variable: String,
    /// What it is used for, in plain words.
    pub purpose: &'static str,
}

/// Variables `profile` needs at run time that `present` does not report.
///
/// `present(name)` tells whether the variable is set and non-empty; use
/// [`missing_credentials`] for this process.
#[must_use]
pub fn missing_credentials_with(
    profile: &Profile,
    present: &dyn Fn(&str) -> bool,
) -> Vec<MissingCredential> {
    let mut missing = Vec::new();
    for remote in [&profile.remote_source, &profile.remote_destination]
        .into_iter()
        .flatten()
    {
        let (variable, purpose) = match remote {
            RemoteEndpoint::Sftp { .. } if cfg!(unix) => (
                "SSH_AUTH_SOCK".to_owned(),
                "agent SSH (clé de connexion SFTP)",
            ),
            RemoteEndpoint::Webdav { credential_ref, .. }
            | RemoteEndpoint::Ftp { credential_ref, .. }
            | RemoteEndpoint::Ftps { credential_ref, .. }
            | RemoteEndpoint::Onedrive { credential_ref, .. } => (
                format!("SYNCENGINE_SECRET_{credential_ref}"),
                "mot de passe ou jeton du serveur",
            ),
            _ => continue,
        };
        if !present(&variable)
            && !missing
                .iter()
                .any(|m: &MissingCredential| m.variable == variable)
        {
            missing.push(MissingCredential { variable, purpose });
        }
    }
    missing
}

/// [`missing_credentials_with`] for the environment of this process.
#[must_use]
pub fn missing_credentials(profile: &Profile) -> Vec<MissingCredential> {
    missing_credentials_with(profile, &|name| {
        std::env::var_os(name).is_some_and(|value| !value.is_empty())
    })
}

/// Message for a missing variable, shared by the preview error (only the
/// network connectors read secrets).
#[cfg(any(
    feature = "sftp",
    feature = "webdav",
    feature = "ftp",
    feature = "ftps",
    feature = "onedrive",
    test
))]
pub(crate) fn missing_message(variable: &str) -> String {
    format!(
        "{variable} absente de ce processus : une interface lancée depuis le menu ou un agent systemd n’hérite pas des variables d’un terminal (voir le guide, « Secrets »)"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(remote: RemoteEndpoint) -> Profile {
        Profile {
            id: sync_core::ProfileId::new("p").unwrap_or_else(|_| unreachable!()),
            name: "p".into(),
            group: String::new(),
            mode: sync_plan::SyncMode::Backup,
            source: "/src".into(),
            destination: std::path::PathBuf::new(),
            remote_source: None,
            remote_destination: Some(remote),
            filter: sync_filter::FilterRules::new(),
            allow_delete: false,
            compare: sync_profiles::CompareMethod::Content,
            trash: sync_profiles::TrashPolicy::disabled(),
        }
    }

    #[test]
    fn a_missing_secret_is_named_never_its_value() {
        let p = profile(RemoteEndpoint::Webdav {
            root: "https://example.test/dav/".into(),
            credential_ref: "NAS".into(),
        });
        let missing = missing_credentials_with(&p, &|_| false);
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].variable, "SYNCENGINE_SECRET_NAS");
        assert!(missing_credentials_with(&p, &|name| name == "SYNCENGINE_SECRET_NAS").is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn sftp_needs_the_ssh_agent_socket() {
        let p = profile(RemoteEndpoint::Sftp {
            address: "127.0.0.1:22".parse().unwrap_or_else(|_| unreachable!()),
            host: "nas".into(),
            username: "u".into(),
            known_hosts: "/k".into(),
            root: "/r".into(),
        });
        let missing = missing_credentials_with(&p, &|_| false);
        assert_eq!(missing[0].variable, "SSH_AUTH_SOCK");
        assert!(missing_credentials_with(&p, &|name| name == "SSH_AUTH_SOCK").is_empty());
        assert!(missing_message("SSH_AUTH_SOCK").contains("SSH_AUTH_SOCK"));
    }
}
