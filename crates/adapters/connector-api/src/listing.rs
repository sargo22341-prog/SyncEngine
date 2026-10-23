//! Pages de listing, jetons de pagination et anomalies d'entrées.

use std::fmt;

use sync_core::{FileEntry, PathError};

use crate::error::StorageErrorKind;

/// Jeton opaque de continuation d'un listing paginé.
///
/// Seul le connecteur qui l'a émis sait l'interpréter ; l'appelant le
/// renvoie tel quel pour obtenir la page suivante du **même** répertoire.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PageToken(String);

impl PageToken {
    /// Construit un jeton (usage réservé aux connecteurs).
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Valeur brute du jeton.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Raison pour laquelle un nom d'entrée ne peut pas devenir un `ResourcePath`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum NameIssue {
    /// Nom non représentable en UTF-8 (octets arbitraires Linux, UTF-16
    /// non apparié Windows).
    NonUtf8,
    /// Nom refusé par les invariants de `ResourcePath` (par exemple un `\`
    /// dans un nom de fichier Linux).
    Invalid(PathError),
    /// Nom valide pour le stockage mais impossible à désigner sans ambiguïté
    /// par l'API de la plateforme (par exemple `CON` ou `a.` sous Windows).
    PlatformAmbiguous,
}

/// Anomalie rencontrée sur une entrée d'un répertoire listé.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ListingIssueKind {
    /// L'entrée existe mais son nom n'est pas représentable : elle est
    /// **absente** de la page.
    UnrepresentableName(NameIssue),
    /// L'entrée existe mais sa nature n'a pas pu être lue : elle est
    /// **absente** de la page.
    EntryUnreadable(StorageErrorKind),
    /// L'entrée figure dans la page mais ses métadonnées sont inconnues.
    MetadataUnavailable(StorageErrorKind),
}

/// Anomalie d'une entrée de listing.
///
/// Une anomalie qui omet une entrée ([`ListingIssue::omits_entry`]) rend le
/// listing du répertoire **incomplet** : l'absence d'un nom dans ce
/// répertoire ne prouve alors plus rien.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListingIssue {
    display_name: String,
    kind: ListingIssueKind,
}

impl ListingIssue {
    /// Construit une anomalie.
    ///
    /// `display_name` est une représentation lisible, éventuellement avec
    /// perte, destinée **uniquement** au diagnostic : elle ne doit jamais
    /// servir à désigner l'entrée dans une opération.
    #[must_use]
    pub fn new(display_name: impl Into<String>, kind: ListingIssueKind) -> Self {
        Self {
            display_name: display_name.into(),
            kind,
        }
    }

    /// Nom lisible (diagnostic seulement).
    #[must_use]
    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    /// Nature de l'anomalie.
    #[must_use]
    pub fn kind(&self) -> &ListingIssueKind {
        &self.kind
    }

    /// Vrai si l'entrée concernée est absente de la page.
    #[must_use]
    pub fn omits_entry(&self) -> bool {
        !matches!(self.kind, ListingIssueKind::MetadataUnavailable(_))
    }
}

impl fmt::Display for ListingIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = &self.display_name;
        match &self.kind {
            ListingIssueKind::UnrepresentableName(NameIssue::NonUtf8) => {
                write!(f, "nom non UTF-8 ignoré : {name:?}")
            }
            ListingIssueKind::UnrepresentableName(NameIssue::Invalid(error)) => {
                write!(f, "nom non représentable ignoré : {name:?} ({error})")
            }
            ListingIssueKind::UnrepresentableName(NameIssue::PlatformAmbiguous) => {
                write!(f, "nom ambigu sur cette plateforme ignoré : {name:?}")
            }
            ListingIssueKind::EntryUnreadable(kind) => {
                write!(f, "entrée illisible ignorée : {name:?} ({kind:?})")
            }
            ListingIssueKind::MetadataUnavailable(kind) => {
                write!(f, "métadonnées inconnues : {name:?} ({kind:?})")
            }
        }
    }
}

/// Page de résultats d'un listing de répertoire.
///
/// Chaque entrée est un **enfant direct** du répertoire listé. Les entrées
/// ne sont pas forcément triées.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListPage {
    entries: Vec<FileEntry>,
    issues: Vec<ListingIssue>,
    next: Option<PageToken>,
}

impl ListPage {
    /// Construit une page ; `next` est `None` pour la dernière page.
    #[must_use]
    pub fn new(
        entries: Vec<FileEntry>,
        issues: Vec<ListingIssue>,
        next: Option<PageToken>,
    ) -> Self {
        Self {
            entries,
            issues,
            next,
        }
    }

    /// Entrées de la page.
    #[must_use]
    pub fn entries(&self) -> &[FileEntry] {
        &self.entries
    }

    /// Anomalies rencontrées sur cette page.
    #[must_use]
    pub fn issues(&self) -> &[ListingIssue] {
        &self.issues
    }

    /// Jeton de la page suivante, `None` si c'est la dernière.
    #[must_use]
    pub fn next_page(&self) -> Option<&PageToken> {
        self.next.as_ref()
    }

    /// Décompose la page en `(entrées, anomalies, page suivante)`.
    #[must_use]
    pub fn into_parts(self) -> (Vec<FileEntry>, Vec<ListingIssue>, Option<PageToken>) {
        (self.entries, self.issues, self.next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_metadata_issue_keeps_entry() {
        let kept = ListingIssue::new(
            "a",
            ListingIssueKind::MetadataUnavailable(StorageErrorKind::PermissionDenied),
        );
        assert!(!kept.omits_entry());
        let omitted = ListingIssue::new(
            "b\u{fffd}",
            ListingIssueKind::UnrepresentableName(NameIssue::NonUtf8),
        );
        assert!(omitted.omits_entry());
        assert!(omitted.to_string().contains("non UTF-8"));
        let unreadable = ListingIssue::new(
            "c",
            ListingIssueKind::EntryUnreadable(StorageErrorKind::Transient),
        );
        assert!(unreadable.omits_entry());
    }

    #[test]
    fn page_parts() {
        let page = ListPage::new(Vec::new(), Vec::new(), Some(PageToken::new("p2")));
        assert_eq!(page.next_page().map(PageToken::as_str), Some("p2"));
        let (entries, issues, next) = page.into_parts();
        assert!(entries.is_empty() && issues.is_empty());
        assert_eq!(next, Some(PageToken::new("p2")));
        assert_eq!(ListPage::default().next_page(), None);
    }
}
