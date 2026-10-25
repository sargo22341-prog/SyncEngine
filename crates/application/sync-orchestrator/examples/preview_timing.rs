//! Measures each preview stage on two local trees (read-only).

use std::error::Error;
use std::io::Write;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::time::Instant;

use connector_local::{LocalConnector, LocalOptions};
use sync_core::{CancellationToken, HashAlgorithm, PlanId, Timestamp};
use sync_decisions::{PlanContext, PlanStrategy, PlanningInput};
use sync_filter::{Filter, FilterRules};
use sync_plan::{DeletionGuard, DeletionPolicy, OverwritePolicy, PlanPolicy};
use sync_scanner::ScanOptions;

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        return Err(
            "usage: preview_timing SOURCE DESTINATION [full|lazy|none] [hash_workers] [reuse] [snapshot.json] [listing_workers]".into(),
        );
    }
    let source = PathBuf::from(&args[0]).canonicalize()?;
    let destination = PathBuf::from(&args[1]).canonicalize()?;
    let mode = args.get(2).map_or("none", String::as_str);
    let workers =
        NonZeroUsize::new(args.get(3).map_or(Ok(1), |w| w.parse())?).ok_or("workers > 0")?;
    let reuse = args.get(4).is_some_and(|a| a == "reuse");
    let mut out = std::io::stdout().lock();
    let cancel = CancellationToken::new();
    let filter = Filter::new(FilterRules::default())?;
    let hashing = ScanOptions::new()
        .with_hash(HashAlgorithm::Blake3)
        .with_hash_workers(workers);
    let listing_workers = NonZeroUsize::new(args.get(6).map_or(Ok(4), |w| w.parse())?)
        .ok_or("listing workers > 0")?;
    let options = if mode == "full" {
        hashing.clone()
    } else {
        ScanOptions::new()
    }
    .with_listing_workers(listing_workers);
    let local = if reuse {
        LocalOptions::new().with_handle_reuse()
    } else {
        LocalOptions::new()
    };
    let left = LocalConnector::new(&source, local.clone())?;
    let right = LocalConnector::new(&destination, local)?;

    let start = Instant::now();
    let (a, b) = std::thread::scope(|scope| {
        let a = scope.spawn(|| sync_scanner::scan(&left, &filter, &options, &cancel));
        let b = sync_scanner::scan(&right, &filter, &options, &cancel);
        (a.join(), b)
    });
    let (mut a, mut b) = (a.map_err(|_| "panic")??, b?);
    writeln!(
        out,
        "scan both        {:>8.2?}  {} / {} entries",
        start.elapsed(),
        a.len(),
        b.len()
    )?;
    writeln!(
        out,
        "source complete={} stats={:?} issues={}; destination complete={} stats={:?} issues={}",
        a.is_complete(),
        a.stats(),
        a.issues().len(),
        b.is_complete(),
        b.stats(),
        b.issues().len()
    )?;
    if mode == "lazy" {
        let start = Instant::now();
        let candidates = sync_compare::content_candidates(&a, &b);
        let (x, y) = std::thread::scope(|scope| {
            let x = scope.spawn(|| {
                sync_scanner::hash_selected(&left, a, &candidates, &hashing, &cancel, &|_, _| {})
            });
            let y =
                sync_scanner::hash_selected(&right, b, &candidates, &hashing, &cancel, &|_, _| {});
            (x.join(), y)
        });
        a = x.map_err(|_| "panic")??;
        b = y?;
        writeln!(
            out,
            "hash candidates  {:>8.2?}  {} files",
            start.elapsed(),
            candidates.len()
        )?;
    }
    let start = Instant::now();
    let report = sync_compare::compare(&a, &b, &sync_compare::CompareOptions::new())?;
    writeln!(
        out,
        "compare          {:>8.2?}  {:?}",
        start.elapsed(),
        report.summary()
    )?;
    let start = Instant::now();
    let context = PlanContext {
        plan_id: PlanId::new("timing")?,
        profile: None,
        created_at: Timestamp::now()?,
        source_location: source.display().to_string(),
        destination_location: destination.display().to_string(),
    };
    let policy = PlanPolicy::new(
        OverwritePolicy::SourceWins,
        DeletionPolicy::enabled(DeletionGuard::new()),
    );
    let input = PlanningInput::new(&report, &context);
    let plan = sync_mirror::MirrorStrategy::new().build_plan(&input, &policy)?;
    writeln!(
        out,
        "plan             {:>8.2?}  {:?}",
        start.elapsed(),
        plan.summary()
    )?;
    if let Some(snapshot) = args.get(5) {
        let mut file = std::io::BufWriter::new(std::fs::File::create_new(snapshot)?);
        serde_json::to_writer(
            &mut file,
            &serde_json::json!({
                "source": a.entries().collect::<Vec<_>>(),
                "destination": b.entries().collect::<Vec<_>>(),
                "source_issues": a.issues().iter().map(ToString::to_string).collect::<Vec<_>>(),
                "destination_issues": b.issues().iter().map(ToString::to_string).collect::<Vec<_>>(),
                "operations": plan.operations(),
            }),
        )?;
        file.flush()?;
    }
    Ok(())
}
