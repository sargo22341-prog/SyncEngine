//! Measures history views on an explicitly isolated state copy; no sync runs.
use std::io::Write;
use std::{error::Error, path::PathBuf, time::Instant};
use sync_core::ProfileId;
use sync_orchestrator::{Application, HistoryFilter};
fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 3 {
        return Err("usage: history_filter_timing ISOLATED_STATE PROFILE EXECUTION".into());
    }
    let root = PathBuf::from(&args[0]);
    if !root.is_absolute() {
        return Err("absolute isolated state required".into());
    }
    let start = Instant::now();
    let app = Application::open_with_lock_root(&root, root.with_extension("locks"))?;
    let mut out = std::io::stdout().lock();
    writeln!(out, "open_ms={}", start.elapsed().as_millis())?;
    for filter in [
        HistoryFilter::Problems,
        HistoryFilter::Refused,
        HistoryFilter::Failed,
        HistoryFilter::Unfinished,
        HistoryFilter::Skipped,
        HistoryFilter::Problems,
    ] {
        let start = Instant::now();
        let page = app.history_page_filtered(ProfileId::new(&args[1])?, &args[2], 0, filter, "")?;
        writeln!(
            out,
            "{filter:?} total={} ms={}",
            page["total"],
            start.elapsed().as_millis()
        )?;
    }
    Ok(())
}
