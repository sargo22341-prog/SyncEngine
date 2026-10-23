//! Point d'entrée de la CLI `syncengine`.
//!
//! Phase 04 : aide, version, trois commandes **en lecture seule** —
//! `scan` (inventaire d'un dossier local), `compare` (différences entre
//! deux dossiers locaux), `preview` (plan d'une sauvegarde ou d'un miroir,
//! simulé) —, `execute` (exécution **réelle** du même plan, qui modifie la
//! destination, sur confirmation explicite et avec journal durable) et
//! `journal` (diagnostic d'un journal, en lecture seule).

mod cli;
mod discovery;
mod execute;
mod preview;
mod profiles;

use std::io::{self, Write};
use std::process::ExitCode;

use cli::{Command, ParseError};
use discovery::exit;

/// Code de sortie pour une erreur d'utilisation.
const EXIT_USAGE: u8 = 2;

static CANCELLATION: std::sync::LazyLock<sync_core::CancellationToken> =
    std::sync::LazyLock::new(sync_core::CancellationToken::new);
fn cancellation() -> sync_core::CancellationToken {
    CANCELLATION.clone()
}
fn main() -> ExitCode {
    if let Err(e) = ctrlc::set_handler(|| CANCELLATION.cancel()) {
        return write_or_fail(
            &mut io::stderr().lock(),
            &format!("signal handler: {e}\n"),
            ExitCode::FAILURE,
        );
    }
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.first().is_some_and(|s| s == "profile") {
        return match profiles::run(&args[1..]) {
            Ok(code) => ExitCode::from(code),
            Err(e) => write_or_fail(
                &mut io::stderr().lock(),
                &format!("syncengine: {e}\n"),
                ExitCode::FAILURE,
            ),
        };
    }
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
        Command::Preview {
            source,
            destination,
            options,
            preview,
        } => preview::preview_command(&mut out, &source, &destination, &options, &preview),
        Command::Execute {
            source,
            destination,
            options,
            preview,
            execute,
        } => execute::execute_command(
            &mut out,
            &source,
            &destination,
            &options,
            &preview,
            &execute,
        ),
        Command::Journal { path } => execute::journal_command(&mut out, &path),
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
