//! Times a complete preview and execution through the application facade.
//! The destination must be a new empty folder and the state folder a new
//! folder of its own: this example writes into both unless `preview-only` is
//! supplied as the fifth argument (then only the isolated state is written).

use std::error::Error;
use std::io::Write;
use std::path::PathBuf;
use std::time::Instant;

use sync_core::ProfileId;
use sync_filter::FilterRules;
use sync_orchestrator::Application;
use sync_plan::SyncMode;
use sync_profiles::{CompareMethod, Profile};

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !(3..=5).contains(&args.len()) {
        return Err(
            "usage: facade_timing SOURCE EMPTY_DESTINATION NEW_STATE [content|size] [preview-only]"
                .into(),
        );
    }
    let canonical = |p: &String| PathBuf::from(p).canonicalize();
    let (source, destination, state) = (
        canonical(&args[0])?,
        canonical(&args[1])?,
        canonical(&args[2])?,
    );
    if std::fs::read_dir(&destination)?.next().is_some() {
        return Err("destination must be empty".into());
    }
    let compare = match args.get(3).map(String::as_str) {
        Some("size") => CompareMethod::SizeAndDate,
        None | Some("content") => CompareMethod::Content,
        Some(_) => return Err("comparison must be content or size".into()),
    };
    let preview_only = match args.get(4).map(String::as_str) {
        None => false,
        Some("preview-only") => true,
        Some(_) => return Err("fifth argument must be preview-only".into()),
    };
    let mut out = std::io::stdout().lock();
    let app = Application::open_with_lock_root(&state, state.with_extension("locks"))?;
    let id = ProfileId::new("timing")?;
    app.profile_save(Profile {
        id: id.clone(),
        name: "timing".into(),
        group: String::new(),
        mode: SyncMode::Mirror,
        source,
        destination,
        remote_source: None,
        remote_destination: None,
        filter: FilterRules::new(),
        allow_delete: false,
        compare,
        trash: sync_profiles::TrashPolicy::legacy(),
    })?;
    for round in ["first copy", "unchanged"] {
        let start = Instant::now();
        let preview = app.preview_paged(id.clone())?;
        writeln!(
            out,
            "{round}: preview {:>8.2?}  {}",
            start.elapsed(),
            preview["summary"]
        )?;
        if preview_only {
            break;
        }
        let plan = preview["plan"]["id"].as_str().ok_or("plan id")?.to_owned();
        let start = Instant::now();
        let report = app.execute(id.clone(), plan, true, false)?;
        writeln!(
            out,
            "{round}: execute {:>8.2?}  {}",
            start.elapsed(),
            report["totals"]
        )?;
    }
    Ok(())
}
