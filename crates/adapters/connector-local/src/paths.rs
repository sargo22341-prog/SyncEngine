//! Correspondance entre chemins logiques et chemins de l'OS.
//!
//! Un [`ResourcePath`] garantit déjà l'absence de `..`, `.`, `/` initial,
//! `\` et NUL. Ce module ajoute les contrôles propres à la plateforme et
//! refuse de traverser un lien symbolique (ou une jonction Windows) situé
//! sous la racine.

use std::path::{Path, PathBuf};

use connector_api::{StorageError, StorageErrorKind, StorageOperation};
use sync_core::ResourcePath;

use crate::errors::local_error;

/// Noms de périphériques réservés par Win32, avec ou sans extension.
const WINDOWS_RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$", "COM0", "COM1", "COM2", "COM3", "COM4",
    "COM5", "COM6", "COM7", "COM8", "COM9", "COM¹", "COM²", "COM³", "LPT0", "LPT1", "LPT2", "LPT3",
    "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9", "LPT¹", "LPT²", "LPT³",
];

/// Vrai si `name` ne peut pas être désigné sans ambiguïté par l'API Win32.
///
/// Sont refusés : caractères interdits (`<>:"|?*`, contrôles), noms de
/// périphériques réservés (`CON`, `NUL.txt`…) et noms finissant par `.` ou
/// espace (Win32 les tronque silencieusement : `a.` désignerait `a`).
/// Le `:` est refusé en particulier parce qu'il désigne un flux de données
/// alternatif NTFS.
#[must_use]
pub(crate) fn is_ambiguous_on_windows(name: &str) -> bool {
    if name
        .chars()
        .any(|ch| matches!(ch, '<' | '>' | ':' | '"' | '|' | '?' | '*') || u32::from(ch) < 32)
    {
        return true;
    }
    if name.ends_with('.') || name.ends_with(' ') {
        return true;
    }
    let stem = name.split('.').next().unwrap_or(name).trim_end_matches(' ');
    WINDOWS_RESERVED
        .iter()
        .any(|reserved| stem.eq_ignore_ascii_case(reserved))
}

/// Vrai si le nom est inutilisable sur la plateforme courante.
pub(crate) fn is_ambiguous_here(name: &str) -> bool {
    cfg!(windows) && is_ambiguous_on_windows(name)
}

/// Construit le chemin OS de `path` sous `root`, sans accès disque.
///
/// # Errors
/// `NotSupported` si un segment est ambigu sur cette plateforme.
pub(crate) fn to_os_path(
    root: &Path,
    path: &ResourcePath,
    operation: StorageOperation,
) -> Result<PathBuf, StorageError> {
    let mut os_path = root.to_path_buf();
    for segment in path.segments() {
        if is_ambiguous_here(segment) {
            return Err(local_error(
                StorageErrorKind::NotSupported,
                operation,
                path,
                "nom inutilisable sans ambiguïté sur cette plateforme",
            ));
        }
        os_path.push(segment);
    }
    Ok(os_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_ambiguous_names() {
        for name in [
            "CON",
            "con",
            "nul.txt",
            "COM1",
            "lpt9.tar.gz",
            "a:b",
            "a?",
            "x*",
            "fin.",
            "fin ",
            "tab\tname",
            "AUX .txt",
        ] {
            assert!(is_ambiguous_on_windows(name), "{name:?}");
        }
        for name in ["CONSOLE", "com10", "a.b", ".gitignore", "café", "nul_x"] {
            assert!(!is_ambiguous_on_windows(name), "{name:?}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn os_path_rejects_alternate_data_stream() {
        let root = Path::new(r"C:\racine");
        let ads = ResourcePath::parse("fichier.txt:flux").unwrap();
        let error = to_os_path(root, &ads, StorageOperation::Read).unwrap_err();
        assert_eq!(error.kind(), StorageErrorKind::NotSupported);
    }

    #[test]
    fn os_path_stays_under_root() {
        let root = std::env::temp_dir().join("racine");
        let path = ResourcePath::parse("a/b.txt").unwrap();
        let os_path = to_os_path(&root, &path, StorageOperation::Stat).unwrap();
        assert!(os_path.starts_with(&root));
        assert_eq!(os_path, root.join("a").join("b.txt"));
        assert_eq!(
            to_os_path(&root, &ResourcePath::root(), StorageOperation::List).unwrap(),
            root
        );
    }
}
