//! Analyse des arguments et textes de la CLI.

use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;

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
            let (paths, options, _) = parse_discovery(args, false)?;
            let [root] = expect_paths(paths, &["dossier à inventorier"])?;
            Ok(Command::Scan { root, options })
        }
        "compare" => {
            let (paths, options, show_all) = parse_discovery(args, true)?;
            let [left, right] = expect_paths(paths, &["dossier de gauche", "dossier de droite"])?;
            Ok(Command::Compare {
                left,
                right,
                options,
                show_all,
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

/// Sépare dossiers (positionnels, éventuellement non UTF-8) et options.
fn parse_discovery(
    args: impl Iterator<Item = OsString>,
    allow_all: bool,
) -> Result<(Vec<PathBuf>, DiscoveryArgs, bool), ParseError> {
    let mut paths = Vec::new();
    let mut options = DiscoveryArgs::default();
    let mut show_all = false;
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
        let mut value = |target: &mut Vec<String>| -> Result<(), ParseError> {
            let value = args
                .next()
                .ok_or_else(|| ParseError::MissingValue(option.clone()))?
                .into_string()
                .map_err(|_| ParseError::NonUtf8Argument)?;
            target.push(value);
            Ok(())
        };
        match option.as_str() {
            "--hash" => options.hash = true,
            "--ignore-case" => options.ignore_case = true,
            "--include" => value(&mut options.include_files)?,
            "--exclude" => value(&mut options.exclude_files)?,
            "--exclude-dir" => value(&mut options.exclude_directories)?,
            "--all" if allow_all => show_all = true,
            _ => return Err(ParseError::UnknownOption(option)),
        }
    }
    Ok((paths, options, show_all))
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
        "syncengine {} — phase 02 : inventaire et comparaison en lecture seule\n\
         \n\
         UTILISATION :\n\
         \x20   syncengine scan <DOSSIER> [OPTIONS]\n\
         \x20   syncengine compare <GAUCHE> <DROITE> [OPTIONS] [--all]\n\
         \x20   syncengine help | version\n\
         \n\
         COMMANDES :\n\
         \x20   scan      Inventorie un dossier local et signale les zones illisibles\n\
         \x20   compare   Compare deux dossiers locaux et affiche les différences\n\
         \x20   help      Affiche cette aide (aussi -h, --help)\n\
         \x20   version   Affiche la version et la cible compilée (aussi -V, --version)\n\
         \n\
         OPTIONS :\n\
         \x20   --hash               Compare aussi le contenu (BLAKE3, relit chaque fichier)\n\
         \x20   --include <MOTIF>    Ne garde que les fichiers correspondants (répétable)\n\
         \x20   --exclude <MOTIF>    Écarte les fichiers correspondants (répétable)\n\
         \x20   --exclude-dir <MOTIF> Ne parcourt pas les dossiers correspondants (répétable)\n\
         \x20   --ignore-case        Motifs insensibles à la casse\n\
         \x20   --all                (compare) Affiche aussi les chemins équivalents\n\
         \n\
         Motifs : glob sur le chemin relatif avec « / » ; sans « / », sur le nom seul\n\
         (« *.tmp », « build/**/*.o », « **/node_modules »).\n\
         \n\
         CODES DE SORTIE :\n\
         \x20   0  inventaire complet ; aucune différence (compare)\n\
         \x20   1  différences constatées (compare)\n\
         \x20   2  erreur d'utilisation\n\
         \x20   3  inventaire incomplet ou constats incertains : rien n'est garanti\n\
         \x20   4  échec (dossier inaccessible, sortie impossible)\n\
         \n\
         Simulation uniquement : SyncEngine lit les dossiers indiqués mais ne crée,\n\
         ne modifie et ne supprime jamais aucun fichier dans cette version.\n\
         Aucune commande de sauvegarde, miroir ou synchronisation n'existe encore.\n",
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
    }
}
