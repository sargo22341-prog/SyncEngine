//! Analyse des arguments et textes de la CLI.

use std::ffi::OsString;
use std::fmt;

/// Commande reconnue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Command {
    /// Afficher l'aide.
    Help,
    /// Afficher la version et la cible compilée.
    Version,
}

/// Erreur d'utilisation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ParseError {
    /// Commande inconnue.
    UnknownCommand(String),
    /// Argument en trop après une commande.
    UnexpectedArgument(String),
    /// Argument non représentable en UTF-8.
    NonUtf8Argument,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownCommand(command) => write!(f, "commande inconnue « {command} »"),
            Self::UnexpectedArgument(argument) => {
                write!(f, "argument inattendu « {argument} »")
            }
            Self::NonUtf8Argument => f.write_str("argument non UTF-8"),
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
    let command = match first.as_str() {
        "help" | "--help" | "-h" => Command::Help,
        "version" | "--version" | "-V" => Command::Version,
        _ => return Err(ParseError::UnknownCommand(first)),
    };
    match args.next() {
        None => Ok(command),
        Some(extra) => Err(ParseError::UnexpectedArgument(
            extra.to_string_lossy().into_owned(),
        )),
    }
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
        "syncengine {} — squelette de la phase 01\n\
         \n\
         UTILISATION :\n    syncengine <COMMANDE>\n\
         \n\
         COMMANDES :\n\
         \x20   help      Affiche cette aide (aussi -h, --help)\n\
         \x20   version   Affiche la version et la cible compilée (aussi -V, --version)\n\
         \n\
         Aucune commande de sauvegarde, miroir, synchronisation ou simulation\n\
         n'est encore disponible : cette version ne lit ni ne modifie aucun fichier.\n",
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

    #[test]
    fn version_mentions_target() {
        let text = version_text();
        assert!(text.starts_with(concat!("syncengine ", env!("CARGO_PKG_VERSION"))));
        assert!(text.contains(std::env::consts::OS));
        assert!(text.contains(std::env::consts::ARCH));
    }

    #[test]
    fn help_states_no_file_operation() {
        assert!(help_text().contains("ne lit ni ne modifie aucun fichier"));
    }
}
