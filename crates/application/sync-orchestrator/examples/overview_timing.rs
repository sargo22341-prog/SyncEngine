//! Times opening the facade and reading every history overview, as the
//! desktop does at start-up. Point it at a **copy** of a state folder: opening
//! migrates and maintains the attempt history in place.

use std::error::Error;
use std::io::Write;
use std::time::Instant;

use sync_orchestrator::Application;

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [state] = args.as_slice() else {
        return Err("usage: overview_timing COPIED_STATE_FOLDER".into());
    };
    let mut out = std::io::stdout().lock();
    let started = Instant::now();
    let app = Application::open(state)?;
    writeln!(out, "open: {:.2?}", started.elapsed())?;
    for pass in 1..=2 {
        let started = Instant::now();
        let mut runs = 0;
        for profile in app.profiles_list()? {
            runs += app.history_overview(profile.id)?.len();
        }
        writeln!(
            out,
            "overview pass {pass}: {runs} runs in {:.2?}",
            started.elapsed()
        )?;
    }
    Ok(())
}
