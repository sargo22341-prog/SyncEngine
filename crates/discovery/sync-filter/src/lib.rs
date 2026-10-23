//! Filtres d'inclusion et d'exclusion.
//!
//! Les règles ([`FilterRules`]) sont sérialisables et destinées à être
//! enregistrées dans un profil ; elles sont compilées en un [`Filter`] pur,
//! sans E/S. Le même jeu de règles doit être utilisé pour la simulation et
//! l'exécution : le scanner recopie les règles utilisées dans son inventaire
//! et la comparaison refuse deux inventaires filtrés différemment.
//!
//! # Sémantique des motifs
//! - Les motifs sont des globs appliqués aux **chemins logiques**
//!   (séparateur `/`, jamais `\`), indépendamment de l'OS.
//! - Un motif **sans** `/` s'applique au **nom** de l'entrée, à toute
//!   profondeur : `*.tmp` exclut `a.tmp` et `x/y/b.tmp`.
//! - Un motif **avec** `/` s'applique au chemin complet depuis la racine :
//!   `build/*.o` exclut `build/a.o` mais pas `src/build/a.o` ; `**/cache`
//!   désigne `cache` à toute profondeur.
//! - `*` et `?` ne traversent pas `/` ; `**` couvre zéro ou plusieurs segments ;
//!   `[abc]`, `{a,b}` et `\` (échappement) sont pris en charge.
//! - Respect de la casse par défaut ; [`FilterRules::case_insensitive`]
//!   compare motifs et chemins mis en minuscules (`str::to_lowercase`,
//!   Unicode) : approximation qui ne reproduit pas exactement la table de
//!   casse de NTFS.
//!
//! # Ordre d'évaluation
//! 1. Un **répertoire** est exclu s'il correspond à `exclude_directories` :
//!    il n'est pas parcouru et son contenu n'est pas inventorié.
//! 2. Un **fichier** (ou lien, ou autre entrée non-répertoire) est exclu s'il
//!    correspond à `exclude_files`.
//! 3. Sinon, si `include_files` est non vide, un fichier qui n'y correspond
//!    pas est exclu. Les listes d'inclusion ne s'appliquent jamais aux
//!    répertoires (sinon `*.txt` empêcherait de descendre dans `docs/`).
//!
//! L'exclusion l'emporte toujours sur l'inclusion.
//!
//! ```
//! use sync_core::{EntryKind, ResourcePath};
//! use sync_filter::{Filter, FilterDecision, FilterRules};
//!
//! let rules = FilterRules::new()
//!     .exclude_file("*.tmp")
//!     .exclude_directory("**/node_modules");
//! let filter = Filter::new(rules)?;
//! let path = ResourcePath::parse("web/node_modules")?;
//! assert!(matches!(filter.evaluate(&path, EntryKind::Directory), FilterDecision::Exclude(_)));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use globset::{Glob, GlobBuilder, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};
use sync_core::{EntryKind, ResourcePath};
use thiserror::Error;

/// Liste de règles concernée.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleList {
    /// `include_files`.
    IncludeFiles,
    /// `exclude_files`.
    ExcludeFiles,
    /// `exclude_directories`.
    ExcludeDirectories,
}

/// Erreur de compilation d'un jeu de règles.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum FilterError {
    /// Motif vide.
    #[error("motif vide ({list:?} n°{index})")]
    EmptyPattern {
        /// Liste concernée.
        list: RuleList,
        /// Position du motif dans la liste.
        index: usize,
    },
    /// Motif commençant par `/` : les motifs sont relatifs à la racine.
    #[error("motif absolu refusé ({list:?} n°{index}) : retirer le '/' initial")]
    AbsolutePattern {
        /// Liste concernée.
        list: RuleList,
        /// Position du motif dans la liste.
        index: usize,
    },
    /// Syntaxe glob invalide.
    #[error("motif invalide ({list:?} n°{index}) : {reason}")]
    InvalidPattern {
        /// Liste concernée.
        list: RuleList,
        /// Position du motif dans la liste.
        index: usize,
        /// Explication de l'analyseur.
        reason: String,
    },
}

/// Règles de filtrage sérialisables (voir la documentation du crate).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilterRules {
    #[serde(default)]
    include_files: Vec<String>,
    #[serde(default)]
    exclude_files: Vec<String>,
    #[serde(default)]
    exclude_directories: Vec<String>,
    #[serde(default)]
    case_insensitive: bool,
}

impl FilterRules {
    /// Aucune règle : tout est inclus.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Ajoute un motif d'inclusion de fichiers.
    #[must_use]
    pub fn include_file(mut self, pattern: impl Into<String>) -> Self {
        self.include_files.push(pattern.into());
        self
    }

    /// Ajoute un motif d'exclusion de fichiers.
    #[must_use]
    pub fn exclude_file(mut self, pattern: impl Into<String>) -> Self {
        self.exclude_files.push(pattern.into());
        self
    }

    /// Ajoute un motif d'exclusion de répertoires (non parcourus).
    #[must_use]
    pub fn exclude_directory(mut self, pattern: impl Into<String>) -> Self {
        self.exclude_directories.push(pattern.into());
        self
    }

    /// Active ou non l'insensibilité à la casse des motifs.
    #[must_use]
    pub fn with_case_insensitive(mut self, value: bool) -> Self {
        self.case_insensitive = value;
        self
    }

    /// Motifs d'inclusion de fichiers.
    #[must_use]
    pub fn include_files(&self) -> &[String] {
        &self.include_files
    }

    /// Motifs d'exclusion de fichiers.
    #[must_use]
    pub fn exclude_files(&self) -> &[String] {
        &self.exclude_files
    }

    /// Motifs d'exclusion de répertoires.
    #[must_use]
    pub fn exclude_directories(&self) -> &[String] {
        &self.exclude_directories
    }

    /// Vrai si les motifs ignorent la casse.
    #[must_use]
    pub fn case_insensitive(&self) -> bool {
        self.case_insensitive
    }

    /// Vrai si aucune règle n'est définie.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.include_files.is_empty()
            && self.exclude_files.is_empty()
            && self.exclude_directories.is_empty()
    }
}

/// Raison d'une exclusion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum Exclusion {
    /// Reserved recovery staging/lock namespace, never synchronized or deleted
    /// by ordinary plans. Cleanup requires a separate explicit recovery policy.
    EngineInternal,
    /// Répertoire correspondant au motif `exclude_directories[rule]`.
    ExcludedDirectory {
        /// Position du premier motif correspondant.
        rule: usize,
    },
    /// Fichier correspondant au motif `exclude_files[rule]`.
    ExcludedFile {
        /// Position du premier motif correspondant.
        rule: usize,
    },
    /// Fichier ne correspondant à aucun motif de `include_files`.
    NotIncluded,
}

/// Résultat de l'évaluation d'une entrée.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum FilterDecision {
    /// L'entrée fait partie du périmètre.
    Include,
    /// L'entrée est hors périmètre ; pour un répertoire, son contenu aussi.
    Exclude(Exclusion),
}

impl FilterDecision {
    /// Vrai si l'entrée est incluse.
    #[must_use]
    pub fn is_included(self) -> bool {
        matches!(self, Self::Include)
    }
}

/// Motifs compilés d'une liste, séparés selon leur cible (nom ou chemin).
#[derive(Debug, Clone)]
struct CompiledList {
    /// Motifs sans `/`, appliqués au nom ; indices d'origine.
    by_name: GlobSet,
    name_index: Vec<usize>,
    /// Motifs avec `/`, appliqués au chemin complet ; indices d'origine.
    by_path: GlobSet,
    path_index: Vec<usize>,
    is_empty: bool,
}

impl CompiledList {
    fn compile(
        list: RuleList,
        patterns: &[String],
        case_insensitive: bool,
    ) -> Result<Self, FilterError> {
        let mut by_name = GlobSetBuilder::new();
        let mut by_path = GlobSetBuilder::new();
        let mut name_index = Vec::new();
        let mut path_index = Vec::new();
        for (index, pattern) in patterns.iter().enumerate() {
            if pattern.is_empty() {
                return Err(FilterError::EmptyPattern { list, index });
            }
            if pattern.starts_with('/') {
                return Err(FilterError::AbsolutePattern { list, index });
            }
            let pattern = fold(pattern, case_insensitive);
            let glob = build_glob(&pattern, case_insensitive).map_err(|error| {
                FilterError::InvalidPattern {
                    list,
                    index,
                    reason: error.kind().to_string(),
                }
            })?;
            if pattern.contains('/') {
                by_path.add(glob);
                path_index.push(index);
            } else {
                by_name.add(glob);
                name_index.push(index);
            }
        }
        let invalid = |error: globset::Error| FilterError::InvalidPattern {
            list,
            index: 0,
            reason: error.kind().to_string(),
        };
        Ok(Self {
            by_name: by_name.build().map_err(invalid)?,
            by_path: by_path.build().map_err(invalid)?,
            name_index,
            path_index,
            is_empty: patterns.is_empty(),
        })
    }

    /// Plus petit indice d'origine d'un motif correspondant ; `path` et
    /// `name` sont déjà repliés si les règles ignorent la casse.
    fn first_match(&self, path: &str, name: &str) -> Option<usize> {
        let by_name = self
            .by_name
            .matches(name)
            .into_iter()
            .filter_map(|i| self.name_index.get(i).copied());
        let by_path = self
            .by_path
            .matches(path)
            .into_iter()
            .filter_map(|i| self.path_index.get(i).copied());
        by_name.chain(by_path).min()
    }
}

/// Met en minuscules si les règles ignorent la casse. Le repli intégré de
/// `globset` ne couvre que l'ASCII ; celui-ci couvre aussi `É`/`é`.
fn fold(value: &str, case_insensitive: bool) -> std::borrow::Cow<'_, str> {
    if case_insensitive {
        std::borrow::Cow::Owned(value.to_lowercase())
    } else {
        std::borrow::Cow::Borrowed(value)
    }
}

fn build_glob(pattern: &str, case_insensitive: bool) -> Result<Glob, globset::Error> {
    GlobBuilder::new(pattern)
        // `*` et `?` ne traversent pas `/`, sur toutes les plateformes.
        .literal_separator(true)
        // `\` est un échappement partout (il ne peut pas apparaître dans un
        // `ResourcePath`) : comportement identique sous Windows et Linux.
        .backslash_escape(true)
        .case_insensitive(case_insensitive)
        .build()
}

/// Filtre compilé, immuable et sans E/S.
#[derive(Debug, Clone)]
pub struct Filter {
    rules: FilterRules,
    include_files: CompiledList,
    exclude_files: CompiledList,
    exclude_directories: CompiledList,
}

impl Filter {
    /// Compile un jeu de règles.
    ///
    /// # Errors
    /// [`FilterError`] pour un motif vide, absolu ou syntaxiquement invalide.
    pub fn new(rules: FilterRules) -> Result<Self, FilterError> {
        let ci = rules.case_insensitive;
        Ok(Self {
            include_files: CompiledList::compile(RuleList::IncludeFiles, &rules.include_files, ci)?,
            exclude_files: CompiledList::compile(RuleList::ExcludeFiles, &rules.exclude_files, ci)?,
            exclude_directories: CompiledList::compile(
                RuleList::ExcludeDirectories,
                &rules.exclude_directories,
                ci,
            )?,
            rules,
        })
    }

    /// Filtre qui inclut tout.
    #[must_use]
    pub fn include_all() -> Self {
        let empty = CompiledList {
            by_name: GlobSet::empty(),
            name_index: Vec::new(),
            by_path: GlobSet::empty(),
            path_index: Vec::new(),
            is_empty: true,
        };
        Self {
            rules: FilterRules::new(),
            include_files: empty.clone(),
            exclude_files: empty.clone(),
            exclude_directories: empty,
        }
    }

    /// Règles d'origine (pour les recopier dans un inventaire ou un plan).
    #[must_use]
    pub fn rules(&self) -> &FilterRules {
        &self.rules
    }

    /// Évalue une entrée de nature `kind` située en `path`.
    ///
    /// La racine est toujours incluse.
    #[must_use]
    pub fn evaluate(&self, path: &ResourcePath, kind: EntryKind) -> FilterDecision {
        if path.is_root() {
            return FilterDecision::Include;
        }
        if path.segments().any(|segment| {
            let name = segment.to_ascii_lowercase();
            name == ".syncengine-lock"
                || (name.starts_with(".syncengine-") && name.ends_with(".partial"))
        }) {
            return FilterDecision::Exclude(Exclusion::EngineInternal);
        }
        let ci = self.rules.case_insensitive;
        let full = fold(path.as_str(), ci);
        let name = fold(path.file_name().unwrap_or_default(), ci);
        if kind == EntryKind::Directory {
            return match self.exclude_directories.first_match(&full, &name) {
                Some(rule) => FilterDecision::Exclude(Exclusion::ExcludedDirectory { rule }),
                None => FilterDecision::Include,
            };
        }
        if let Some(rule) = self.exclude_files.first_match(&full, &name) {
            return FilterDecision::Exclude(Exclusion::ExcludedFile { rule });
        }
        if !self.include_files.is_empty && self.include_files.first_match(&full, &name).is_none() {
            return FilterDecision::Exclude(Exclusion::NotIncluded);
        }
        FilterDecision::Include
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(value: &str) -> ResourcePath {
        ResourcePath::parse(value).unwrap()
    }

    fn eval(filter: &Filter, path: &str, kind: EntryKind) -> FilterDecision {
        filter.evaluate(&p(path), kind)
    }

    #[test]
    fn empty_rules_include_everything() {
        let filter = Filter::new(FilterRules::new()).unwrap();
        assert!(eval(&filter, "a/b.txt", EntryKind::File).is_included());
        assert!(eval(&filter, "a", EntryKind::Directory).is_included());
        assert!(filter.rules().is_empty());
        let all = Filter::include_all();
        assert!(eval(&all, "x/y", EntryKind::Symlink).is_included());
    }

    #[test]
    fn recovery_artifacts_cannot_be_reincluded_by_user_patterns() {
        let filter = Filter::new(FilterRules::new().include_file("**")).unwrap();
        for name in [
            ".syncengine-1.partial",
            "parent/.SYNCENGINE-2.PARTIAL/file",
            ".syncengine-lock/owner",
        ] {
            assert_eq!(
                eval(&filter, name, EntryKind::File),
                FilterDecision::Exclude(Exclusion::EngineInternal)
            );
        }
        assert!(eval(&filter, "user.partial", EntryKind::File).is_included());
        assert!(eval(&filter, ".syncengine-notes.txt", EntryKind::File).is_included());
    }

    #[test]
    fn name_patterns_apply_at_any_depth() {
        let filter = Filter::new(FilterRules::new().exclude_file("*.tmp")).unwrap();
        assert_eq!(
            eval(&filter, "a.tmp", EntryKind::File),
            FilterDecision::Exclude(Exclusion::ExcludedFile { rule: 0 })
        );
        assert!(!eval(&filter, "x/y/b.tmp", EntryKind::File).is_included());
        assert!(eval(&filter, "x/y/b.tmp.keep", EntryKind::File).is_included());
        // Les règles de fichiers ne touchent pas les répertoires.
        assert!(eval(&filter, "dir.tmp", EntryKind::Directory).is_included());
    }

    #[test]
    fn path_patterns_are_anchored_and_star_does_not_cross_separator() {
        let filter = Filter::new(FilterRules::new().exclude_file("build/*.o")).unwrap();
        assert!(!eval(&filter, "build/a.o", EntryKind::File).is_included());
        assert!(eval(&filter, "build/sub/a.o", EntryKind::File).is_included());
        assert!(eval(&filter, "src/build/a.o", EntryKind::File).is_included());

        let filter = Filter::new(FilterRules::new().exclude_file("build/**/*.o")).unwrap();
        assert!(!eval(&filter, "build/a.o", EntryKind::File).is_included());
        assert!(!eval(&filter, "build/sub/deep/a.o", EntryKind::File).is_included());
    }

    #[test]
    fn excluded_directories_by_name_or_path() {
        let rules = FilterRules::new()
            .exclude_directory("node_modules")
            .exclude_directory("out/cache");
        let filter = Filter::new(rules).unwrap();
        assert_eq!(
            eval(&filter, "web/app/node_modules", EntryKind::Directory),
            FilterDecision::Exclude(Exclusion::ExcludedDirectory { rule: 0 })
        );
        assert_eq!(
            eval(&filter, "out/cache", EntryKind::Directory),
            FilterDecision::Exclude(Exclusion::ExcludedDirectory { rule: 1 })
        );
        assert!(eval(&filter, "x/out/cache", EntryKind::Directory).is_included());
        // Un fichier nommé comme le répertoire exclu reste inclus.
        assert!(eval(&filter, "node_modules", EntryKind::File).is_included());
    }

    #[test]
    fn include_list_applies_to_files_only_and_exclusion_wins() {
        let rules = FilterRules::new()
            .include_file("*.txt")
            .include_file("*.md")
            .exclude_file("secret*.txt");
        let filter = Filter::new(rules).unwrap();
        assert!(eval(&filter, "docs/a.txt", EntryKind::File).is_included());
        assert!(eval(&filter, "README.md", EntryKind::File).is_included());
        assert_eq!(
            eval(&filter, "image.png", EntryKind::File),
            FilterDecision::Exclude(Exclusion::NotIncluded)
        );
        assert_eq!(
            eval(&filter, "secret-1.txt", EntryKind::File),
            FilterDecision::Exclude(Exclusion::ExcludedFile { rule: 0 })
        );
        assert!(eval(&filter, "docs", EntryKind::Directory).is_included());
        // Les liens sont filtrés comme des fichiers.
        assert!(!eval(&filter, "lien", EntryKind::Symlink).is_included());
    }

    #[test]
    fn case_sensitivity_is_explicit() {
        let sensitive = Filter::new(FilterRules::new().exclude_file("*.TMP")).unwrap();
        assert!(eval(&sensitive, "a.tmp", EntryKind::File).is_included());
        let insensitive = Filter::new(
            FilterRules::new()
                .exclude_file("*.TMP")
                .exclude_file("ÉTÉ*")
                .with_case_insensitive(true),
        )
        .unwrap();
        assert!(!eval(&insensitive, "a.tmp", EntryKind::File).is_included());
        assert!(!eval(&insensitive, "été.doc", EntryKind::File).is_included());
    }

    #[test]
    fn unicode_names_and_brackets() {
        let filter = Filter::new(
            FilterRules::new()
                .exclude_file("日本*")
                .exclude_file("rapport[0-9].pdf")
                .exclude_file("*.{bak,old}"),
        )
        .unwrap();
        assert!(!eval(&filter, "a/日本語.txt", EntryKind::File).is_included());
        assert!(!eval(&filter, "rapport7.pdf", EntryKind::File).is_included());
        assert!(eval(&filter, "rapportX.pdf", EntryKind::File).is_included());
        assert!(!eval(&filter, "x.old", EntryKind::File).is_included());
    }

    #[test]
    fn first_matching_rule_is_reported() {
        let filter = Filter::new(
            FilterRules::new()
                .exclude_file("docs/*.log")
                .exclude_file("*.log"),
        )
        .unwrap();
        assert_eq!(
            eval(&filter, "docs/a.log", EntryKind::File),
            FilterDecision::Exclude(Exclusion::ExcludedFile { rule: 0 })
        );
        assert_eq!(
            eval(&filter, "b.log", EntryKind::File),
            FilterDecision::Exclude(Exclusion::ExcludedFile { rule: 1 })
        );
    }

    #[test]
    fn root_is_always_included() {
        let filter = Filter::new(FilterRules::new().include_file("*.txt")).unwrap();
        assert!(
            filter
                .evaluate(&ResourcePath::root(), EntryKind::Directory)
                .is_included()
        );
    }

    #[test]
    fn invalid_rules_are_rejected() {
        assert_eq!(
            Filter::new(FilterRules::new().exclude_file("")).unwrap_err(),
            FilterError::EmptyPattern {
                list: RuleList::ExcludeFiles,
                index: 0
            }
        );
        assert_eq!(
            Filter::new(FilterRules::new().exclude_directory("/abs")).unwrap_err(),
            FilterError::AbsolutePattern {
                list: RuleList::ExcludeDirectories,
                index: 0
            }
        );
        assert!(matches!(
            Filter::new(FilterRules::new().include_file("ok").include_file("[z-a")),
            Err(FilterError::InvalidPattern {
                list: RuleList::IncludeFiles,
                index: 1,
                ..
            })
        ));
    }

    #[test]
    fn rules_round_trip_and_reject_unknown_fields() {
        let rules = FilterRules::new()
            .include_file("*.txt")
            .exclude_directory("tmp")
            .with_case_insensitive(true);
        let json = serde_json::to_string(&rules).unwrap();
        assert_eq!(serde_json::from_str::<FilterRules>(&json).unwrap(), rules);
        assert_eq!(
            serde_json::from_str::<FilterRules>("{}").unwrap(),
            FilterRules::new()
        );
        assert!(serde_json::from_str::<FilterRules>(r#"{"exclude":["x"]}"#).is_err());
    }
}
