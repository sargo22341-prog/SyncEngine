//! Chemin logique relatif à la racine d'un connecteur.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Séparateur logique des segments d'un [`ResourcePath`].
const SEPARATOR: char = '/';

/// Erreur de construction d'un [`ResourcePath`].
///
/// Les erreurs ne recopient pas le chemin complet (qui peut être sensible) :
/// elles indiquent seulement la position du segment fautif.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum PathError {
    /// Le chemin commence par `/` : seuls les chemins relatifs sont admis.
    #[error("chemin logique absolu refusé (commence par '/')")]
    Absolute,
    /// Un segment est vide (`a//b`, `a/` ou segment fourni vide).
    #[error("segment {index} vide")]
    EmptySegment {
        /// Position (base 0) du segment.
        index: usize,
    },
    /// Un segment vaut `.` ou `..` : refusé pour empêcher toute traversée.
    #[error("segment {index} relatif ('.' ou '..') refusé")]
    DotSegment {
        /// Position (base 0) du segment.
        index: usize,
    },
    /// Un segment contient un caractère interdit (`/`, `\` ou NUL).
    #[error("segment {index} : caractère interdit {ch:?}")]
    ForbiddenChar {
        /// Position (base 0) du segment.
        index: usize,
        /// Caractère refusé.
        ch: char,
    },
}

/// Chemin logique, relatif à la racine d'une extrémité de connecteur.
///
/// Ce n'est **pas** un chemin OS : il n'a ni lettre de lecteur, ni préfixe
/// UNC, ni séparateur dépendant de la plateforme. Sa forme textuelle utilise
/// `/` comme séparateur logique ; la conversion vers un chemin concret est de
/// la responsabilité du connecteur, qui doit refaire ses propres contrôles
/// (noms réservés Windows, `:` et flux alternatifs, longueur maximale…).
///
/// # Invariants
/// - la racine est le chemin vide (zéro segment) ;
/// - chaque segment est non vide, différent de `.` et `..`, et ne contient
///   ni `/`, ni `\`, ni NUL (protection contre la traversée, y compris sous
///   Windows où `\` est un séparateur) ;
/// - aucune normalisation implicite : casse et forme Unicode sont préservées
///   telles quelles ; l'égalité est une égalité exacte d'octets.
///
/// # Limite connue
/// Les segments sont en UTF-8. Un nom de fichier non représentable en UTF-8
/// (octets arbitraires Linux, UTF-16 non apparié Windows) ne peut pas être
/// représenté : le connecteur doit le signaler explicitement, jamais le
/// convertir avec perte ni l'ignorer silencieusement.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ResourcePath(String);

fn validate_segment(index: usize, segment: &str) -> Result<(), PathError> {
    if segment.is_empty() {
        return Err(PathError::EmptySegment { index });
    }
    if segment == "." || segment == ".." {
        return Err(PathError::DotSegment { index });
    }
    match segment.chars().find(|ch| matches!(ch, '/' | '\\' | '\0')) {
        Some(ch) => Err(PathError::ForbiddenChar { index, ch }),
        None => Ok(()),
    }
}

impl ResourcePath {
    /// Chemin racine (aucun segment).
    #[must_use]
    pub fn root() -> Self {
        Self(String::new())
    }

    /// Analyse une forme textuelle `a/b/c`. La chaîne vide désigne la racine.
    ///
    /// # Errors
    /// [`PathError::Absolute`] si la chaîne commence par `/`, sinon l'erreur
    /// du premier segment invalide.
    pub fn parse(value: &str) -> Result<Self, PathError> {
        if value.is_empty() {
            return Ok(Self::root());
        }
        if value.starts_with(SEPARATOR) {
            return Err(PathError::Absolute);
        }
        for (index, segment) in value.split(SEPARATOR).enumerate() {
            validate_segment(index, segment)?;
        }
        Ok(Self(value.to_owned()))
    }

    /// Construit un chemin à partir de segments individuels.
    ///
    /// # Errors
    /// Erreur du premier segment invalide (un segment contenant `/` est refusé).
    pub fn from_segments<I, S>(segments: I) -> Result<Self, PathError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut path = Self::root();
        for segment in segments {
            path = path.join(segment.as_ref())?;
        }
        Ok(path)
    }

    /// Retourne un nouveau chemin prolongé d'un segment.
    ///
    /// # Errors
    /// Erreur si `segment` est invalide (il ne peut pas contenir `/`).
    pub fn join(&self, segment: &str) -> Result<Self, PathError> {
        validate_segment(self.depth(), segment)?;
        let mut value = String::with_capacity(self.0.len() + 1 + segment.len());
        value.push_str(&self.0);
        if !value.is_empty() {
            value.push(SEPARATOR);
        }
        value.push_str(segment);
        Ok(Self(value))
    }

    /// Vrai pour la racine.
    #[must_use]
    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    /// Nombre de segments (0 pour la racine).
    #[must_use]
    pub fn depth(&self) -> usize {
        self.segments().count()
    }

    /// Itère sur les segments, de la racine vers la feuille.
    pub fn segments(&self) -> impl Iterator<Item = &str> {
        self.0
            .split(SEPARATOR)
            .filter(|segment| !segment.is_empty())
    }

    /// Dernier segment, `None` pour la racine.
    #[must_use]
    pub fn file_name(&self) -> Option<&str> {
        self.segments().last()
    }

    /// Chemin parent, `None` pour la racine.
    #[must_use]
    pub fn parent(&self) -> Option<Self> {
        if self.is_root() {
            return None;
        }
        Some(match self.0.rfind(SEPARATOR) {
            Some(pos) => Self(self.0[..pos].to_owned()),
            None => Self::root(),
        })
    }

    /// Vrai si `self` est égal à `ancestor` ou situé sous lui.
    ///
    /// La comparaison se fait segment par segment (`a/bc` n'est pas sous
    /// `a/b`) et respecte la casse.
    #[must_use]
    pub fn starts_with(&self, ancestor: &Self) -> bool {
        let mut own = self.segments();
        ancestor
            .segments()
            .all(|segment| own.next() == Some(segment))
    }

    /// Forme textuelle avec `/` comme séparateur (vide pour la racine).
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ResourcePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for ResourcePath {
    type Err = PathError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl TryFrom<String> for ResourcePath {
    type Error = PathError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<ResourcePath> for String {
    fn from(path: ResourcePath) -> Self {
        path.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(value: &str) -> ResourcePath {
        ResourcePath::parse(value).unwrap()
    }

    #[test]
    fn root_is_empty() {
        let root = ResourcePath::root();
        assert!(root.is_root());
        assert_eq!(root.depth(), 0);
        assert_eq!(root.file_name(), None);
        assert_eq!(root.parent(), None);
        assert_eq!(p(""), root);
    }

    #[test]
    fn parses_relative_paths() {
        let path = p("docs/rapport 2026.txt");
        assert_eq!(path.depth(), 2);
        assert_eq!(
            path.segments().collect::<Vec<_>>(),
            ["docs", "rapport 2026.txt"]
        );
        assert_eq!(path.file_name(), Some("rapport 2026.txt"));
        assert_eq!(path.parent(), Some(p("docs")));
        assert_eq!(p("docs").parent(), Some(ResourcePath::root()));
    }

    #[test]
    fn rejects_traversal_and_malformed_paths() {
        assert_eq!(ResourcePath::parse("/etc"), Err(PathError::Absolute));
        assert_eq!(
            ResourcePath::parse("a/../b"),
            Err(PathError::DotSegment { index: 1 })
        );
        assert_eq!(
            ResourcePath::parse(".."),
            Err(PathError::DotSegment { index: 0 })
        );
        assert_eq!(
            ResourcePath::parse("./a"),
            Err(PathError::DotSegment { index: 0 })
        );
        assert_eq!(
            ResourcePath::parse("a//b"),
            Err(PathError::EmptySegment { index: 1 })
        );
        assert_eq!(
            ResourcePath::parse("a/"),
            Err(PathError::EmptySegment { index: 1 })
        );
        assert_eq!(
            ResourcePath::parse("a\\..\\b"),
            Err(PathError::ForbiddenChar { index: 0, ch: '\\' })
        );
        assert_eq!(
            ResourcePath::parse("a/b\0c"),
            Err(PathError::ForbiddenChar { index: 1, ch: '\0' })
        );
    }

    #[test]
    fn join_validates_segment() {
        let base = p("a");
        assert_eq!(base.join("b").unwrap(), p("a/b"));
        assert_eq!(ResourcePath::root().join("b").unwrap(), p("b"));
        assert_eq!(
            base.join("b/c"),
            Err(PathError::ForbiddenChar { index: 1, ch: '/' })
        );
        assert_eq!(base.join(".."), Err(PathError::DotSegment { index: 1 }));
        assert_eq!(base.join(""), Err(PathError::EmptySegment { index: 1 }));
    }

    #[test]
    fn from_segments_matches_parse() {
        assert_eq!(
            ResourcePath::from_segments(["x", "y", "z"]).unwrap(),
            p("x/y/z")
        );
        assert_eq!(
            ResourcePath::from_segments(Vec::<String>::new()).unwrap(),
            ResourcePath::root()
        );
    }

    #[test]
    fn preserves_unicode_and_case() {
        // NFC (é précomposé) et NFD (e + accent combinant) restent distincts :
        // aucune normalisation implicite.
        let nfc = p("caf\u{e9}");
        let nfd = p("cafe\u{301}");
        assert_ne!(nfc, nfd);
        assert_ne!(p("Readme.md"), p("README.md"));
        assert_eq!(p("日本/файл.txt").file_name(), Some("файл.txt"));
    }

    #[test]
    fn starts_with_is_segment_aware() {
        assert!(p("a/b/c").starts_with(&p("a/b")));
        assert!(p("a/b").starts_with(&p("a/b")));
        assert!(p("a").starts_with(&ResourcePath::root()));
        assert!(!p("a/bc").starts_with(&p("a/b")));
        assert!(!p("a").starts_with(&p("a/b")));
        assert!(!p("A/b").starts_with(&p("a")));
    }

    #[test]
    fn serde_validates_on_deserialize() {
        let path = p("a/b");
        let json = serde_json::to_string(&path).unwrap();
        assert_eq!(json, "\"a/b\"");
        assert_eq!(serde_json::from_str::<ResourcePath>(&json).unwrap(), path);
        assert!(serde_json::from_str::<ResourcePath>("\"../secret\"").is_err());
        assert!(serde_json::from_str::<ResourcePath>("\"/abs\"").is_err());
    }
}
