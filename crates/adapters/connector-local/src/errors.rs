//! Conversion des erreurs d'E/S de l'OS en erreurs de stockage catégorisées.

use std::io;

use connector_api::{StorageError, StorageErrorKind, StorageOperation};
use sync_core::ResourcePath;

/// Codes d'erreur Windows signalant un support ou un partage injoignable.
#[cfg(windows)]
const DISCONNECTED_OS_CODES: &[i32] = &[
    21,   // ERROR_NOT_READY : lecteur non prêt (support retiré)
    51,   // ERROR_REM_NOT_LIST
    53,   // ERROR_BAD_NETPATH
    59,   // ERROR_UNEXP_NET_ERR
    64,   // ERROR_NETNAME_DELETED
    67,   // ERROR_BAD_NET_NAME
    1167, // ERROR_DEVICE_NOT_CONNECTED
];

/// Codes d'erreur Windows signalant un fichier verrouillé par un autre processus.
#[cfg(windows)]
const LOCKED_OS_CODES: &[i32] = &[
    32, // ERROR_SHARING_VIOLATION
    33, // ERROR_LOCK_VIOLATION
];

/// Codes `errno` Linux signalant un montage injoignable.
#[cfg(unix)]
const DISCONNECTED_OS_CODES: &[i32] = &[
    107, // ENOTCONN
    112, // EHOSTDOWN
    113, // EHOSTUNREACH
    116, // ESTALE (NFS)
];

#[cfg(unix)]
const LOCKED_OS_CODES: &[i32] = &[];

#[cfg(not(any(windows, unix)))]
const DISCONNECTED_OS_CODES: &[i32] = &[];

#[cfg(not(any(windows, unix)))]
const LOCKED_OS_CODES: &[i32] = &[];

/// Catégorie de stockage correspondant à une erreur d'E/S.
///
/// Règle de prudence : une erreur inconnue devient `Permanent`, jamais
/// `NotFound` ; seule une absence explicitement signalée par l'OS est `NotFound`.
pub(crate) fn classify(error: &io::Error) -> StorageErrorKind {
    if let Some(code) = error.raw_os_error() {
        if DISCONNECTED_OS_CODES.contains(&code) {
            return StorageErrorKind::Disconnected;
        }
        if LOCKED_OS_CODES.contains(&code) {
            return StorageErrorKind::Transient;
        }
    }
    match error.kind() {
        io::ErrorKind::NotFound => StorageErrorKind::NotFound,
        io::ErrorKind::AlreadyExists => StorageErrorKind::Conflict,
        io::ErrorKind::Unsupported => StorageErrorKind::NotSupported,
        io::ErrorKind::PermissionDenied => StorageErrorKind::PermissionDenied,
        io::ErrorKind::StorageFull | io::ErrorKind::QuotaExceeded => {
            StorageErrorKind::InsufficientSpace
        }
        io::ErrorKind::Interrupted | io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => {
            StorageErrorKind::Transient
        }
        io::ErrorKind::NotConnected
        | io::ErrorKind::ConnectionAborted
        | io::ErrorKind::ConnectionReset
        | io::ErrorKind::BrokenPipe
        | io::ErrorKind::HostUnreachable
        | io::ErrorKind::NetworkUnreachable
        | io::ErrorKind::NetworkDown
        | io::ErrorKind::StaleNetworkFileHandle => StorageErrorKind::Disconnected,
        _ => StorageErrorKind::Permanent,
    }
}

/// Construit une erreur de stockage à partir d'une erreur d'E/S.
///
/// Le message de l'OS est conservé comme cause ; le chemin absolu n'est pas
/// recopié (seul le chemin logique figure dans le contexte).
pub(crate) fn io_error(
    error: io::Error,
    operation: StorageOperation,
    path: &ResourcePath,
) -> StorageError {
    StorageError::new(classify(&error), operation)
        .with_connector(crate::connector_id())
        .with_path(path.clone())
        .with_source(error)
}

/// Erreur sans cause d'E/S (contrôle propre au connecteur).
pub(crate) fn local_error(
    kind: StorageErrorKind,
    operation: StorageOperation,
    path: &ResourcePath,
    message: &str,
) -> StorageError {
    StorageError::new(kind, operation)
        .with_connector(crate::connector_id())
        .with_path(path.clone())
        .with_message(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_common_kinds() {
        let cases = [
            (io::ErrorKind::NotFound, StorageErrorKind::NotFound),
            (
                io::ErrorKind::PermissionDenied,
                StorageErrorKind::PermissionDenied,
            ),
            (
                io::ErrorKind::StorageFull,
                StorageErrorKind::InsufficientSpace,
            ),
            (io::ErrorKind::TimedOut, StorageErrorKind::Transient),
            (io::ErrorKind::NotConnected, StorageErrorKind::Disconnected),
            (io::ErrorKind::InvalidData, StorageErrorKind::Permanent),
            (io::ErrorKind::Other, StorageErrorKind::Permanent),
        ];
        for (kind, expected) in cases {
            assert_eq!(classify(&io::Error::from(kind)), expected, "{kind:?}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn maps_windows_codes() {
        assert_eq!(
            classify(&io::Error::from_raw_os_error(21)),
            StorageErrorKind::Disconnected
        );
        assert_eq!(
            classify(&io::Error::from_raw_os_error(53)),
            StorageErrorKind::Disconnected
        );
        assert_eq!(
            classify(&io::Error::from_raw_os_error(32)),
            StorageErrorKind::Transient
        );
        assert_eq!(
            classify(&io::Error::from_raw_os_error(5)),
            StorageErrorKind::PermissionDenied
        );
    }

    #[cfg(unix)]
    #[test]
    fn maps_unix_codes() {
        assert_eq!(
            classify(&io::Error::from_raw_os_error(116)),
            StorageErrorKind::Disconnected
        );
        assert_eq!(
            classify(&io::Error::from_raw_os_error(13)),
            StorageErrorKind::PermissionDenied
        );
    }
}
