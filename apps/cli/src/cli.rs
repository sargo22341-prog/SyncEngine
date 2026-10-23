//! Analyse des arguments et textes de la CLI.

use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;

use sync_plan::{OverwritePolicy, SyncMode};

/// Options communes de découverte (filtres, empreintes).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DiscoveryArgs {
    /// Calculer les empreintes BLAKE3 (lecture complète des fichiers).
    pub(crate) hash: bool,
    /// Motifs `--include`.
    pub(crate) include_files: Vec<String>,
    /// Motifs `--exclude`.
    pub(crate) exclude_files: Vec<String>,
    /// Motifs `--exclude-dir`.
    pub(crate) exclude_directories: Vec<String>,
    /// `--ignore-case` : motifs insensibles à la casse.
    pub(crate) ignore_case: bool,
}

/// Options de la commande `preview` (hors filtres).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PreviewArgs {
    /// Mode de synchronisation (`--mode`, sauvegarde par défaut).
    pub(crate) mode: SyncMode,
    /// Politique d'écrasement (`--overwrite`) ; défaut propre au mode.
    pub(crate) overwrite: Option<OverwritePolicy>,
    /// `--allow-delete` : autorise explicitement les suppressions (miroir).
    pub(crate) allow_delete: bool,
    /// `--max-delete <N>` : plafond absolu de suppressions.
    pub(crate) max_delete: Option<u64>,
    /// `--max-delete-percent <P>` : plafond relatif (défaut 50).
    pub(crate) max_delete_percent: Option<u8>,
    /// `--allow-empty-source` : suppressions admises si la source est vide.
    pub(crate) allow_empty_source: bool,
    /// `--all` : affiche aussi les chemins équivalents.
    pub(crate) show_all: bool,
    /// `--json` : écrit le plan JSON au lieu du texte.
    pub(crate) json: bool,
}

impl Default for PreviewArgs {
    fn default() -> Self {
        Self {
            mode: SyncMode::Backup,
            overwrite: None,
            allow_delete: false,
            max_delete: None,
            max_delete_percent: None,
            allow_empty_source: false,
            show_all: false,
            json: false,
        }
    }
}

/// Commande reconnue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Command {
    /// Afficher l'aide.
    Help,
    /// Afficher la version et la cible compilée.
    Version,
    /// Inventorier un dossier local (lecture seule).
    Scan {
        /// Dossier à inventorier.
        root: PathBuf,
        /// Options de découverte.
        options: DiscoveryArgs,
    },
    /// Comparer deux dossiers locaux (lecture seule).
    Compare {
        /// Dossier de gauche.
        left: PathBuf,
        /// Dossier de droite.
        right: PathBuf,
        /// Options de découverte, identiques des deux côtés.
        options: DiscoveryArgs,
        /// Afficher aussi les chemins équivalents.
        show_all: bool,
    },
    /// Prévisualiser le plan d'une sauvegarde ou d'un miroir (simulation).
    Preview {
        /// Dossier source.
        source: PathBuf,
        /// Dossier destination.
        destination: PathBuf,
        /// Options de découverte, identiques des deux côtés.
        options: DiscoveryArgs,
        /// Options de planification et d'affichage.
        preview: PreviewArgs,
    },
}

/// Erreur d'utilisation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ParseError {
    /// Commande inconnue.
    UnknownCommand(String),
    /// Argument en trop après une commande.
    UnexpectedArgument(String),
    /// Argument non représentable en UTF-8 (hors chemins de dossiers).
    NonUtf8Argument,
    /// Option inconnue pour cette commande.
    UnknownOption(String),
    /// Option sans valeur.
    MissingValue(String),
    /// Dossier attendu manquant.
    MissingPath(&'static str),
    /// Valeur d'option invalide.
    InvalidValue {
        /// Option concernée.
        option: String,
        /// Valeur refusée.
        value: String,
    },
    /// Option qui n'a de sens qu'avec `--allow-delete`.
    RequiresAllowDelete(String),
    /// `--allow-delete` hors du mode miroir.
    DeleteRequiresMirror,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownCommand(command) => write!(f, "commande inconnue « {command} »"),
            Self::UnexpectedArgument(argument) => {
                write!(f, "argument inattendu « {argument} »")
            }
            Self::NonUtf8Argument => f.write_str("argument non UTF-8"),
            Self::UnknownOption(option) => write!(f, "option inconnue « {option} »"),
            Self::MissingValue(option) => write!(f, "valeur manquante après « {option} »"),
            Self::MissingPath(what) => write!(f, "{what} manquant"),
            Self::InvalidValue { option, value } => {
                write!(f, "valeur « {value} » invalide pour « {option} »")
            }
            Self::RequiresAllowDelete(option) => {
                write!(f, "« {option} » n'a de sens qu'avec « --allow-delete »")
            }
            Self::DeleteRequiresMirror => f.write_str(
                "« --allow-delete » exige « --mode mirror » : une sauvegarde ne supprime jamais",
            ),
        }
    }
}

/// Analyse les arguments (sans le nom du programme).
///
/// Sans argument, l'aide est affichée.
pub(crate) fn parse<I>(args: I) -> Result<Command, ParseError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut args = args.into_iter();
    let Some(first) = args.next() else {
        return Ok(Command::Help);
    };
    let first = first
        .into_string()
        .map_err(|_| ParseError::NonUtf8Argument)?;
    match first.as_str() {
        "help" | "--help" | "-h" => no_more(args, Command::Help),
        "version" | "--version" | "-V" => no_more(args, Command::Version),
        "scan" => {
            let (paths, options) = parse_discovery(args, |_, _| Ok(false))?;
            let [root] = expect_paths(paths, &["dossier à inventorier"])?;
            Ok(Command::Scan { root, options })
        }
        "compare" => {
            let mut show_all = false;
            let (paths, options) = parse_discovery(args, |option, _| {
                let known = option == "--all";
                show_all |= known;
                Ok(known)
            })?;
            let [left, right] = expect_paths(paths, &["dossier de gauche", "dossier de droite"])?;
            Ok(Command::Compare {
                left,
                right,
                options,
                show_all,
            })
        }
        "preview" => {
            let mut preview = PreviewArgs::default();
            let (paths, options) = parse_discovery(args, |option, value| {
                preview_option(&mut preview, option, value)
            })?;
            let [source, destination] =
                expect_paths(paths, &["dossier source", "dossier destination"])?;
            check_preview(&preview)?;
            Ok(Command::Preview {
                source,
                destination,
                options,
                preview,
            })
        }
        _ => Err(ParseError::UnknownCommand(first)),
    }
}

fn no_more(
    mut args: impl Iterator<Item = OsString>,
    command: Command,
) -> Result<Command, ParseError> {
    match args.next() {
        None => Ok(command),
        Some(extra) => Err(ParseError::UnexpectedArgument(
            extra.to_string_lossy().into_owned(),
        )),
    }
}

fn expect_paths<const N: usize>(
    paths: Vec<PathBuf>,
    names: &[&'static str; N],
) -> Result<[PathBuf; N], ParseError> {
    if let Some(extra) = paths.get(N) {
        return Err(ParseError::UnexpectedArgument(
            extra.to_string_lossy().into_owned(),
        ));
    }
    let count = paths.len();
    paths
        .try_into()
        .map_err(|_| ParseError::MissingPath(names.get(count).copied().unwrap_or("dossier")))
}

/// Lecture de la valeur d'une option.
type ValueReader<'a> = dyn FnMut() -> Result<String, ParseError> + 'a;

/// Sépare dossiers (positionnels, éventuellement non UTF-8) et options.
///
/// Les options de découverte sont traitées ici ; les autres sont confiées à
/// `extra`, qui répond `Ok(true)` s'il les reconnaît (et lit leur valeur au
/// besoin).
fn parse_discovery(
    args: impl Iterator<Item = OsString>,
    mut extra: impl FnMut(&str, &mut ValueReader<'_>) -> Result<bool, ParseError>,
) -> Result<(Vec<PathBuf>, DiscoveryArgs), ParseError> {
    let mut paths = Vec::new();
    let mut options = DiscoveryArgs::default();
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        let Some(text) = arg.to_str() else {
            // Un argument non UTF-8 ne peut être qu'un chemin de dossier.
            paths.push(PathBuf::from(arg));
            continue;
        };
        if !text.starts_with('-') || text == "-" {
            paths.push(PathBuf::from(arg));
            continue;
        }
        let option = text.to_owned();
        let mut value = || -> Result<String, ParseError> {
            args.next()
                .ok_or_else(|| ParseError::MissingValue(option.clone()))?
                .into_string()
                .map_err(|_| ParseError::NonUtf8Argument)
        };
        match option.as_str() {
            "--hash" => options.hash = true,
            "--ignore-case" => options.ignore_case = true,
            "--include" => options.include_files.push(value()?),
            "--exclude" => options.exclude_files.push(value()?),
            "--exclude-dir" => options.exclude_directories.push(value()?),
            _ => {
                if !extra(&option, &mut value)? {
                    return Err(ParseError::UnknownOption(option));
                }
            }
        }
    }
    Ok((paths, options))
}

fn invalid(option: &str, value: String) -> ParseError {
    ParseError::InvalidValue {
        option: option.to_owned(),
        value,
    }
}

/// Options propres à `preview`.
fn preview_option(
    preview: &mut PreviewArgs,
    option: &str,
    value: &mut ValueReader<'_>,
) -> Result<bool, ParseError> {
    match option {
        "--all" => preview.show_all = true,
        "--json" => preview.json = true,
        "--allow-delete" => preview.allow_delete = true,
        "--allow-empty-source" => preview.allow_empty_source = true,
        "--mode" => {
            let text = value()?;
            preview.mode = match text.as_str() {
                "backup" => SyncMode::Backup,
                "mirror" => SyncMode::Mirror,
                _ => return Err(invalid(option, text)),
            };
        }
        "--overwrite" => {
            let text = value()?;
            preview.overwrite = Some(match text.as_str() {
                "source-wins" => OverwritePolicy::SourceWins,
                "newer" => OverwritePolicy::NewerSourceOnly,
                "never" => OverwritePolicy::KeepDestination,
                _ => return Err(invalid(option, text)),
            });
        }
        "--max-delete" => {
            let text = value()?;
            let count = text.parse::<u64>().map_err(|_| invalid(option, text))?;
            preview.max_delete = Some(count);
        }
        "--max-delete-percent" => {
            let text = value()?;
            match text.parse::<u8>() {
                Ok(percent) if (1..=100).contains(&percent) => {
                    preview.max_delete_percent = Some(percent);
                }
                _ => return Err(invalid(option, text)),
            }
        }
        _ => return Ok(false),
    }
    Ok(true)
}

/// Cohérence des options de suppression.
fn check_preview(preview: &PreviewArgs) -> Result<(), ParseError> {
    if preview.allow_delete && preview.mode != SyncMode::Mirror {
        return Err(ParseError::DeleteRequiresMirror);
    }
    if !preview.allow_delete {
        let guard_option = if preview.max_delete.is_some() {
            Some("--max-delete")
        } else if preview.max_delete_percent.is_some() {
            Some("--max-delete-percent")
        } else if preview.allow_empty_source {
            Some("--allow-empty-source")
        } else {
            None
        };
        if let Some(option) = guard_option {
            return Err(ParseError::RequiresAllowDelete(option.to_owned()));
        }
    }
    Ok(())
}

/// Texte de version : nom, version du paquet, OS et architecture compilés.
pub(crate) fn version_text() -> String {
    let build = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    format!(
        "syncengine {} ({}-{}, {build})\n",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
    )
}

/// Texte d'aide.
pub(crate) fn help_text() -> String {
    format!(
        "syncengine {} — phase 03 : inventaire, comparaison et prévisualisation de plans\n\
         \n\
         UTILISATION :\n\
         \x20   syncengine scan <DOSSIER> [OPTIONS]\n\
         \x20   syncengine compare <GAUCHE> <DROITE> [OPTIONS] [--all]\n\
         \x20   syncengine preview <SOURCE> <DESTINATION> [OPTIONS] [OPTIONS DE PLAN]\n\
         \x20   syncengine help | version\n\
         \n\
         COMMANDES :\n\
         \x20   scan      Inventorie un dossier local et signale les zones illisibles\n\
         \x20   compare   Compare deux dossiers locaux et affiche les différences\n\
         \x20   preview   Calcule et affiche le plan d'une sauvegarde ou d'un miroir\n\
         \x20             (copier, remplacer, conserver, supprimer, conflit, ignorer)\n\
         \x20   help      Affiche cette aide (aussi -h, --help)\n\
         \x20   version   Affiche la version et la cible compilée (aussi -V, --version)\n\
         \n\
         OPTIONS :\n\
         \x20   --hash               Compare aussi le contenu (BLAKE3, relit chaque fichier)\n\
         \x20   --include <MOTIF>    Ne garde que les fichiers correspondants (répétable)\n\
         \x20   --exclude <MOTIF>    Écarte les fichiers correspondants (répétable)\n\
         \x20   --exclude-dir <MOTIF> Ne parcourt pas les dossiers correspondants (répétable)\n\
         \x20   --ignore-case        Motifs insensibles à la casse\n\
         \x20   --all                (compare, preview) Affiche aussi les chemins équivalents\n\
         \n\
         OPTIONS DE PLAN (preview) :\n\
         \x20   --mode backup|mirror       Sauvegarde (défaut, ne supprime jamais) ou miroir\n\
         \x20   --overwrite <POLITIQUE>    Fichiers différents : source-wins (défaut miroir),\n\
         \x20                              newer (défaut sauvegarde : conflit si la\n\
         \x20                              destination est plus récente), never (sauvegarde)\n\
         \x20   --allow-delete             (miroir) Autorise les suppressions, sous garde-fous\n\
         \x20   --max-delete <N>           Refuse toute suppression au-delà de N\n\
         \x20   --max-delete-percent <P>   Refuse toute suppression au-delà de P % des\n\
         \x20                              entrées de la destination (défaut 50)\n\
         \x20   --allow-empty-source       Admet des suppressions si la source est vide\n\
         \x20   --json                     Écrit le plan JSON versionné au lieu du texte\n\
         \n\
         Motifs : glob sur le chemin relatif avec « / » ; sans « / », sur le nom seul\n\
         (« *.tmp », « build/**/*.o », « **/node_modules »).\n\
         \n\
         CODES DE SORTIE :\n\
         \x20   0  inventaire complet ; aucune différence (compare) ; rien à faire (preview)\n\
         \x20   1  différences constatées (compare) ; opérations prévues (preview)\n\
         \x20   2  erreur d'utilisation (dont destination imbriquée dans la source)\n\
         \x20   3  inventaire incomplet, constats incertains, conflits ou chemins ignorés\n\
         \x20   4  échec (dossier inaccessible, sortie impossible) : aucun plan produit\n\
         \n\
         Simulation uniquement : SyncEngine lit les dossiers indiqués mais ne crée,\n\
         ne modifie et ne supprime jamais aucun fichier dans cette version.\n\
         « preview » affiche un plan ; aucune commande ne l'exécute encore.\n",
        env!("CARGO_PKG_VERSION")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn no_argument_shows_help() {
        assert_eq!(parse(args(&[])), Ok(Command::Help));
    }

    #[test]
    fn recognises_aliases() {
        for value in ["help", "--help", "-h"] {
            assert_eq!(parse(args(&[value])), Ok(Command::Help));
        }
        for value in ["version", "--version", "-V"] {
            assert_eq!(parse(args(&[value])), Ok(Command::Version));
        }
    }

    #[test]
    fn rejects_unknown_and_extra_arguments() {
        assert_eq!(
            parse(args(&["mirror"])),
            Err(ParseError::UnknownCommand("mirror".to_owned()))
        );
        assert_eq!(
            parse(args(&["version", "x"])),
            Err(ParseError::UnexpectedArgument("x".to_owned()))
        );
    }

    #[test]
    fn parses_scan_with_filters() {
        let command = parse(args(&[
            "scan",
            "/data",
            "--hash",
            "--exclude",
            "*.tmp",
            "--exclude-dir",
            "cache",
            "--include",
            "*.txt",
            "--ignore-case",
        ]))
        .unwrap();
        assert_eq!(
            command,
            Command::Scan {
                root: PathBuf::from("/data"),
                options: DiscoveryArgs {
                    hash: true,
                    include_files: vec!["*.txt".to_owned()],
                    exclude_files: vec!["*.tmp".to_owned()],
                    exclude_directories: vec!["cache".to_owned()],
                    ignore_case: true,
                },
            }
        );
    }

    #[test]
    fn parses_compare() {
        let command = parse(args(&["compare", "a", "b", "--all"])).unwrap();
        assert_eq!(
            command,
            Command::Compare {
                left: PathBuf::from("a"),
                right: PathBuf::from("b"),
                options: DiscoveryArgs::default(),
                show_all: true,
            }
        );
    }

    #[test]
    fn usage_errors_for_discovery_commands() {
        assert_eq!(
            parse(args(&["scan"])),
            Err(ParseError::MissingPath("dossier à inventorier"))
        );
        assert_eq!(
            parse(args(&["compare", "a"])),
            Err(ParseError::MissingPath("dossier de droite"))
        );
        assert_eq!(
            parse(args(&["scan", "a", "b"])),
            Err(ParseError::UnexpectedArgument("b".to_owned()))
        );
        assert_eq!(
            parse(args(&["scan", "a", "--exclude"])),
            Err(ParseError::MissingValue("--exclude".to_owned()))
        );
        assert_eq!(
            parse(args(&["scan", "a", "--all"])),
            Err(ParseError::UnknownOption("--all".to_owned()))
        );
        assert_eq!(
            parse(args(&["compare", "a", "b", "--delete"])),
            Err(ParseError::UnknownOption("--delete".to_owned()))
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_non_utf8_command() {
        use std::os::unix::ffi::OsStringExt;
        let raw = OsString::from_vec(vec![0x66, 0xff]);
        assert_eq!(parse(vec![raw]), Err(ParseError::NonUtf8Argument));
    }

    #[cfg(windows)]
    #[test]
    fn rejects_non_utf8_command() {
        use std::os::windows::ffi::OsStringExt;
        // Surrogate UTF-16 isolé : non convertible en UTF-8.
        let raw = OsString::from_wide(&[0x0066, 0xD800]);
        assert_eq!(parse(vec![raw]), Err(ParseError::NonUtf8Argument));
    }

    #[cfg(windows)]
    #[test]
    fn accepts_non_utf8_folder_path() {
        use std::os::windows::ffi::OsStringExt;
        let raw = OsString::from_wide(&[0x0066, 0xD800]);
        let command = parse(vec![OsString::from("scan"), raw.clone()]).unwrap();
        assert_eq!(
            command,
            Command::Scan {
                root: PathBuf::from(raw),
                options: DiscoveryArgs::default()
            }
        );
    }

    #[test]
    fn version_mentions_target() {
        let text = version_text();
        assert!(text.starts_with(concat!("syncengine ", env!("CARGO_PKG_VERSION"))));
        assert!(text.contains(std::env::consts::OS));
        assert!(text.contains(std::env::consts::ARCH));
    }

    #[test]
    fn help_states_read_only_simulation() {
        let help = help_text();
        assert!(help.contains("Simulation uniquement"));
        assert!(help.contains("ne supprime jamais aucun fichier"));
        assert!(help.contains("aucune commande ne l'exécute encore"));
    }

    #[test]
    fn parses_preview_with_plan_options() {
        let command = parse(args(&[
            "preview",
            "src",
            "dst",
            "--mode",
            "mirror",
            "--overwrite",
            "newer",
            "--allow-delete",
            "--max-delete",
            "10",
            "--max-delete-percent",
            "30",
            "--allow-empty-source",
            "--exclude",
            "*.tmp",
            "--all",
            "--json",
        ]))
        .unwrap();
        assert_eq!(
            command,
            Command::Preview {
                source: PathBuf::from("src"),
                destination: PathBuf::from("dst"),
                options: DiscoveryArgs {
                    exclude_files: vec!["*.tmp".to_owned()],
                    ..DiscoveryArgs::default()
                },
                preview: PreviewArgs {
                    mode: SyncMode::Mirror,
                    overwrite: Some(OverwritePolicy::NewerSourceOnly),
                    allow_delete: true,
                    max_delete: Some(10),
                    max_delete_percent: Some(30),
                    allow_empty_source: true,
                    show_all: true,
                    json: true,
                },
            }
        );
        assert_eq!(
            parse(args(&["preview", "a", "b"])).unwrap(),
            Command::Preview {
                source: PathBuf::from("a"),
                destination: PathBuf::from("b"),
                options: DiscoveryArgs::default(),
                preview: PreviewArgs::default(),
            }
        );
    }

    #[test]
    fn preview_deletion_options_are_explicit_and_consistent() {
        assert_eq!(
            parse(args(&["preview", "a", "b", "--allow-delete"])),
            Err(ParseError::DeleteRequiresMirror)
        );
        assert_eq!(
            parse(args(&[
                "preview",
                "a",
                "b",
                "--mode",
                "mirror",
                "--max-delete",
                "3"
            ])),
            Err(ParseError::RequiresAllowDelete("--max-delete".to_owned()))
        );
        assert_eq!(
            parse(args(&[
                "preview",
                "a",
                "b",
                "--mode",
                "mirror",
                "--allow-empty-source"
            ])),
            Err(ParseError::RequiresAllowDelete(
                "--allow-empty-source".to_owned()
            ))
        );
        for (option, value) in [
            ("--mode", "bidirectional"),
            ("--overwrite", "toujours"),
            ("--max-delete", "-1"),
            ("--max-delete-percent", "0"),
            ("--max-delete-percent", "101"),
        ] {
            assert_eq!(
                parse(args(&["preview", "a", "b", option, value])),
                Err(ParseError::InvalidValue {
                    option: option.to_owned(),
                    value: value.to_owned()
                }),
                "{option} {value}"
            );
        }
        assert_eq!(
            parse(args(&["preview", "a"])),
            Err(ParseError::MissingPath("dossier destination"))
        );
        assert_eq!(
            parse(args(&["compare", "a", "b", "--json"])),
            Err(ParseError::UnknownOption("--json".to_owned()))
        );
    }
}
