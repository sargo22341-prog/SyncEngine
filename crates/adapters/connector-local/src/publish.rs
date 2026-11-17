//! Publication sans écrasement d'un dépôt temporaire (`CommitMode::CreateNew`).
//!
//! Ordre des méthodes, de la plus sûre à la moins sûre :
//!
//! 1. **Linux** : `renameat2(…, RENAME_NOREPLACE)` — atomique, sans lien
//!    physique ; pris en charge par ext4, xfs, btrfs, tmpfs, exFAT et FAT du
//!    noyau, FUSE ≥ 7.23 (dont `exfat-fuse`). Repli seulement si le noyau ou
//!    le système de fichiers ne connaît pas l'option (`EINVAL`, `ENOSYS`,
//!    `EOPNOTSUPP`).
//! 2. Lien physique (`linkat`) puis retrait du temporaire — atomique aussi ;
//!    seule méthode hors Linux (comportement antérieur inchangé). Sous Linux,
//!    repli si le système de fichiers n'a pas de liens physiques (`EPERM`,
//!    `EOPNOTSUPP`, `ENOSYS`, `EMLINK`).
//! 3. **Linux, dernier recours** : réservation du nom cible par une création
//!    exclusive (`O_CREAT|O_EXCL`, fichier vide), puis renommage du
//!    temporaire par-dessus. Une cible déjà présente est toujours refusée.
//!    **Limite documentée** : entre la réservation et le renommage, un
//!    programme tiers qui ouvrirait *notre* fichier vide sans création
//!    exclusive et y écrirait perdrait ce qu'il a écrit. Le verrou de
//!    destination de SyncEngine exclut une seconde exécution, pas les
//!    autres programmes.
//!
//! Une cible existante n'est **jamais** écrasée : l'erreur rendue est
//! `AlreadyExists` (classée `Conflict`).

use std::io;

use cap_std::fs::{Dir, OpenOptions};

/// Méthode qui a publié le fichier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Published {
    /// `renameat2(RENAME_NOREPLACE)` : le temporaire est devenu la cible.
    RenamedNoReplace,
    /// Lien physique : la cible existe, le temporaire reste à retirer.
    Linked,
    /// Réservation puis renommage : le temporaire est devenu la cible.
    Reserved,
}

/// Publie `staging` sous `target` dans `parent`, sans jamais écraser.
pub(crate) fn create_new(parent: &Dir, staging: &str, target: &str) -> io::Result<Published> {
    chain(
        || rename_noreplace(parent, staging, target),
        || parent.hard_link(staging, parent, target),
        || reserve_then_rename(parent, staging, target),
    )
}

/// Enchaînement des méthodes (séparé pour tester les replis par injection).
fn chain(
    rename: impl FnOnce() -> io::Result<()>,
    link: impl FnOnce() -> io::Result<()>,
    reserve: impl FnOnce() -> io::Result<()>,
) -> io::Result<Published> {
    if cfg!(target_os = "linux") {
        match rename() {
            Ok(()) => return Ok(Published::RenamedNoReplace),
            Err(error) if rename_unsupported(&error) => {}
            Err(error) => return Err(error),
        }
        match link() {
            Ok(()) => return Ok(Published::Linked),
            Err(error) if links_unsupported(&error) => {}
            Err(error) => return Err(error),
        }
        reserve().map(|()| Published::Reserved)
    } else {
        // Hors Linux : comportement antérieur, sans repli (échec fermé).
        let _ = (rename, reserve);
        link().map(|()| Published::Linked)
    }
}

/// Codes `errno` Linux.
const EPERM: i32 = 1;
const EINVAL: i32 = 22;
const EMLINK: i32 = 31;
const ENOSYS: i32 = 38;
const EOPNOTSUPP: i32 = 95;

fn rename_unsupported(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(EINVAL | ENOSYS | EOPNOTSUPP))
}

fn links_unsupported(error: &io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(EPERM | ENOSYS | EOPNOTSUPP | EMLINK)
    )
}

#[cfg(target_os = "linux")]
fn rename_noreplace(parent: &Dir, staging: &str, target: &str) -> io::Result<()> {
    use rustix::fs::{RenameFlags, renameat_with};
    renameat_with(parent, staging, parent, target, RenameFlags::NOREPLACE).map_err(io::Error::from)
}

#[cfg(not(target_os = "linux"))]
fn rename_noreplace(_: &Dir, _: &str, _: &str) -> io::Result<()> {
    Err(io::Error::from(io::ErrorKind::Unsupported))
}

/// Dernier recours (voir la limite dans la documentation du module).
fn reserve_then_rename(parent: &Dir, staging: &str, target: &str) -> io::Result<()> {
    let mut exclusive = OpenOptions::new();
    exclusive.write(true).create_new(true);
    // Échoue en `AlreadyExists` si la cible existe : jamais d'écrasement.
    drop(parent.open_with(target, &exclusive)?);
    let still_ours = parent
        .symlink_metadata(target)
        .is_ok_and(|meta| meta.is_file() && !meta.file_type().is_symlink() && meta.len() == 0);
    if !still_ours {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "target changed after reservation",
        ));
    }
    let Err(error) = parent.rename(staging, parent, target) else {
        return Ok(());
    };
    // Retire notre réservation vide, jamais un contenu tiers ; un échec de ce
    // retrait est rapporté avec l'erreur d'origine.
    let empty = parent
        .symlink_metadata(target)
        .is_ok_and(|meta| meta.is_file() && meta.len() == 0);
    if empty && let Err(cleanup) = parent.remove_file(target) {
        return Err(io::Error::new(
            error.kind(),
            format!("{error}; empty reservation left in place: {cleanup}"),
        ));
    }
    Err(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn os(code: i32) -> io::Error {
        io::Error::from_raw_os_error(code)
    }

    fn temp() -> (tempfile::TempDir, Dir) {
        let dir = tempfile::tempdir().unwrap();
        let handle = Dir::open_ambient_dir(dir.path(), cap_std::ambient_authority()).unwrap();
        (dir, handle)
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn unsupported_rename_falls_back_to_a_link() {
        let linked = Cell::new(false);
        let result = chain(
            || Err(os(EINVAL)),
            || {
                linked.set(true);
                Ok(())
            },
            || panic!("réservation inutile"),
        );
        assert_eq!(result.unwrap(), Published::Linked);
        assert!(linked.get());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn filesystem_without_links_falls_back_to_reservation() {
        for code in [EPERM, EOPNOTSUPP, ENOSYS, EMLINK] {
            let result = chain(|| Err(os(ENOSYS)), || Err(os(code)), || Ok(()));
            assert_eq!(result.unwrap(), Published::Reserved, "{code}");
        }
    }

    #[test]
    fn an_existing_target_is_never_retried_with_a_weaker_method() {
        let later = Cell::new(0);
        let result = chain(
            || Err(io::Error::from(io::ErrorKind::AlreadyExists)),
            || {
                later.set(later.get() + 1);
                Err(io::Error::from(io::ErrorKind::AlreadyExists))
            },
            || {
                later.set(later.get() + 10);
                Ok(())
            },
        );
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        // Linux : arrêt dès le premier refus ; ailleurs : le lien seul.
        assert!(later.get() <= 1);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn access_denied_is_not_mistaken_for_missing_support() {
        let result = chain(|| Err(os(13)), || panic!("lien"), || panic!("réservation"));
        assert_eq!(result.unwrap_err().raw_os_error(), Some(13));
    }

    #[test]
    fn reservation_publishes_and_refuses_an_existing_target() {
        let (dir, handle) = temp();
        std::fs::write(dir.path().join("stage"), b"neuf").unwrap();
        reserve_then_rename(&handle, "stage", "cible").unwrap();
        assert_eq!(std::fs::read(dir.path().join("cible")).unwrap(), b"neuf");
        assert!(!dir.path().join("stage").exists());

        std::fs::write(dir.path().join("stage"), b"autre").unwrap();
        let error = reserve_then_rename(&handle, "stage", "cible").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(dir.path().join("cible")).unwrap(), b"neuf");
        assert_eq!(std::fs::read(dir.path().join("stage")).unwrap(), b"autre");
    }

    #[test]
    fn create_new_publishes_and_never_overwrites() {
        let (dir, handle) = temp();
        std::fs::write(dir.path().join("stage"), b"neuf").unwrap();
        let method = create_new(&handle, "stage", "cible").unwrap();
        if cfg!(target_os = "linux") {
            assert_eq!(method, Published::RenamedNoReplace);
            assert!(!dir.path().join("stage").exists());
        }
        std::fs::write(dir.path().join("stage2"), b"autre").unwrap();
        let error = create_new(&handle, "stage2", "cible").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(dir.path().join("cible")).unwrap(), b"neuf");
    }
}
