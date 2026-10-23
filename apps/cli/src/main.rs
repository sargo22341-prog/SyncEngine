//! Point d'entrée de la CLI `syncengine`.
//!
//! Phase 02 : aide, version, et deux commandes **en lecture seule** —
//! `scan` (inventaire d'un dossier local) et `compare` (différences entre
//! deux dossiers locaux). Aucune commande ne copie, ne modifie ni ne
//! supprime de fichier.

mod cli;
mod discovery;

use std::io::{self, Write};
use std::process::ExitCode;

use cli::{Command, ParseError};
use discovery::exit;

/// Code de sortie pour une erreur d'utilisation.
const EXIT_USAGE: u8 = 2;

fn main() -> ExitCode {
    match cli::parse(std::env::args_os().skip(1)) {
        Ok(command) => run(command),
        Err(error) => report_usage_error(&error),
    }
}

fn run(command: Command) -> ExitCode {
    let stdout = io::stdout();
    let mut out = io::BufWriter::new(stdout.lock());
    let result = match command {
        Command::Help => {
            return write_or_fail(&mut out, &cli::help_text(), ExitCode::SUCCESS);
        }
        Command::Version => {
            return write_or_fail(&mut out, &cli::version_text(), ExitCode::SUCCESS);
        }
        Command::Scan { root, options } => discovery::scan_command(&mut out, &root, &options),
        Command::Compare {
            left,
            right,
            options,
            show_all,
        } => discovery::compare_command(&mut out, &left, &right, &options, show_all),
    };
    let flushed = out.flush();
    match (result, flushed) {
        (Ok(code), Ok(())) => ExitCode::from(code),
        (Ok(_), Err(_)) => ExitCode::from(exit::FAILURE),
        (Err(error), _) => {
            let code = if error.is_usage() {
                EXIT_USAGE
            } else {
                exit::FAILURE
            };
            let text = format!("syncengine : {error}\n");
            write_or_fail(&mut io::stderr().lock(), &text, ExitCode::from(code))
        }
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
        Err(_) => ExitCode::from(exit::FAILURE),
    }
}
