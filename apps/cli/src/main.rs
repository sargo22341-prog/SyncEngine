//! Point d'entrée de la CLI `syncengine`.
//!
//! Squelette de phase 01 : seules l'aide et la version existent. Aucune
//! commande ne lit, ne copie ni ne supprime de fichier.

mod cli;

use std::io::{self, Write};
use std::process::ExitCode;

use cli::{Command, ParseError};

/// Code de sortie pour une erreur d'utilisation.
const EXIT_USAGE: u8 = 2;
/// Code de sortie si la sortie standard est inutilisable.
const EXIT_IO: u8 = 1;

fn main() -> ExitCode {
    match cli::parse(std::env::args_os().skip(1)) {
        Ok(command) => {
            let text = match command {
                Command::Help => cli::help_text(),
                Command::Version => cli::version_text(),
            };
            write_or_fail(&mut io::stdout().lock(), &text, ExitCode::SUCCESS)
        }
        Err(error) => report_usage_error(&error),
    }
}

fn report_usage_error(error: &ParseError) -> ExitCode {
    let text =
        format!("syncengine : {error}\nEssayez « syncengine help » pour la liste des commandes.\n");
    write_or_fail(&mut io::stderr().lock(), &text, ExitCode::from(EXIT_USAGE))
}

fn write_or_fail(out: &mut impl Write, text: &str, success: ExitCode) -> ExitCode {
    match out.write_all(text.as_bytes()).and_then(|()| out.flush()) {
        Ok(()) => success,
        // Sortie fermée (tube rompu…) : rien d'autre à signaler sans risque.
        Err(_) => ExitCode::from(EXIT_IO),
    }
}
