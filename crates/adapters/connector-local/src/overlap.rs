//! Détection de racines imbriquées par **identité** de dossier.
//!
//! Comparer des chemins canonisés ne suffit pas : une vue `mount --bind`
//! d'un dossier (Linux), un `subst` ou un lecteur mappé (Windows) donnent un
//! autre chemin pour le même dossier. Sous Unix, chaque racine est comparée
//! à chaque ancêtre de l'autre par `(st_dev, st_ino)`. Hors Unix, seule la
//! comparaison des chemins canonisés est faite (identité de volume et index
//! de fichier Windows : à vérifier sous Windows, non implémenté ici).

use std::io;
use std::path::Path;

/// Relation entre deux racines locales.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootOverlap {
    /// Le même dossier (éventuellement par deux chemins différents).
    Same,
    /// La première racine est dans la seconde.
    FirstInsideSecond,
    /// La seconde racine est dans la première.
    SecondInsideFirst,
}

/// Compare deux racines existantes : chemins canonisés (liens et jonctions
/// résolus) puis, sous Unix, identités des dossiers et de leurs ancêtres.
///
/// `Ok(None)` : racines disjointes. Exemple : `/srv/a` et `/srv/vue/b`, où
/// `/srv/vue` est une vue bind de `/srv/a`, donnent
/// `Some(RootOverlap::SecondInsideFirst)`.
///
/// # Errors
/// Erreur d'E/S si une racine ne peut pas être canonisée ou examinée
/// (l'appelant décide : une racine introuvable est signalée par le scan).
pub fn root_overlap(first: &Path, second: &Path) -> io::Result<Option<RootOverlap>> {
    let first = std::fs::canonicalize(first)?;
    let second = std::fs::canonicalize(second)?;
    if first == second {
        return Ok(Some(RootOverlap::Same));
    }
    if second.starts_with(&first) {
        return Ok(Some(RootOverlap::SecondInsideFirst));
    }
    if first.starts_with(&second) {
        return Ok(Some(RootOverlap::FirstInsideSecond));
    }
    identity_overlap(&first, &second, identity)
}

#[cfg(unix)]
type Identity = (u64, u64);

#[cfg(unix)]
fn identity(path: &Path) -> io::Result<Identity> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path)?;
    Ok((meta.dev(), meta.ino()))
}

#[cfg(not(unix))]
type Identity = ();

#[cfg(not(unix))]
fn identity(_: &Path) -> io::Result<Identity> {
    Ok(())
}

/// Relation par identité (séparée pour tester sans montage).
fn identity_overlap<I: PartialEq>(
    first: &Path,
    second: &Path,
    identify: impl Fn(&Path) -> io::Result<I>,
) -> io::Result<Option<RootOverlap>> {
    if !cfg!(unix) {
        return Ok(None);
    }
    let first_id = identify(first)?;
    let second_id = identify(second)?;
    if first_id == second_id {
        return Ok(Some(RootOverlap::Same));
    }
    let inside = |path: &Path, root: &I| -> io::Result<bool> {
        for ancestor in path.ancestors().skip(1) {
            if identify(ancestor)? == *root {
                return Ok(true);
            }
        }
        Ok(false)
    };
    if inside(second, &first_id)? {
        return Ok(Some(RootOverlap::SecondInsideFirst));
    }
    if inside(first, &second_id)? {
        return Ok(Some(RootOverlap::FirstInsideSecond));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Simule une vue bind : `/data/vue` a l'identité de `/data/src`.
    fn fake(path: &Path) -> io::Result<String> {
        Ok(path.to_string_lossy().replace("/data/vue", "/data/src"))
    }

    #[cfg(unix)]
    #[test]
    fn a_bind_view_inside_the_source_is_detected_by_identity() {
        let source = PathBuf::from("/data/src");
        let view = PathBuf::from("/data/vue/inner");
        assert_eq!(
            identity_overlap(&source, &view, fake).ok().flatten(),
            Some(RootOverlap::SecondInsideFirst)
        );
        assert_eq!(
            identity_overlap(&view, &source, fake).ok().flatten(),
            Some(RootOverlap::FirstInsideSecond)
        );
        let same = PathBuf::from("/data/vue");
        assert_eq!(
            identity_overlap(&source, &same, fake).ok().flatten(),
            Some(RootOverlap::Same)
        );
        let other = PathBuf::from("/data/ailleurs");
        assert_eq!(identity_overlap(&source, &other, fake).ok().flatten(), None);
    }

    #[test]
    fn plain_paths_keep_the_canonical_comparison() -> io::Result<()> {
        let dir = tempfile::tempdir()?;
        let inner = dir.path().join("a/b");
        std::fs::create_dir_all(&inner)?;
        let sibling = dir.path().join("c");
        std::fs::create_dir(&sibling)?;
        assert_eq!(
            root_overlap(dir.path(), &inner)?,
            Some(RootOverlap::SecondInsideFirst)
        );
        assert_eq!(
            root_overlap(&inner, dir.path())?,
            Some(RootOverlap::FirstInsideSecond)
        );
        assert_eq!(root_overlap(&inner, &inner)?, Some(RootOverlap::Same));
        assert_eq!(root_overlap(&inner, &sibling)?, None);
        Ok(())
    }
}
