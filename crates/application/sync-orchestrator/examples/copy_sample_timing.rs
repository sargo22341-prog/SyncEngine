//! Bounded real-copy audit. Never deletes or overwrites preexisting data. Requires an empty
//! destination, a new output folder and explicit --confirm. At most 1 GiB per
//! invocation, with a deterministic mix of small files and whole larger files.
use connector_api::{CommitMode, StorageConnector, WriteOptions};
use connector_local::{LocalConnector, LocalOptions};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::io::Write;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use sync_core::{CancellationToken, EntryKind, ExecutionId, PlanId, Timestamp};
use sync_decisions::{PlanContext, PlanStrategy, PlanningInput};
use sync_events::{EventEmitter, EventKind, bounded_channel};
use sync_executor::{ExecutionOptions, ExecutionRequest, Executor, PlanExecutor};
use sync_filter::Filter;
use sync_journal::FileJournal;
use sync_plan::{Action, ExecutionPlan, OperationSpec, PlanHeader};
use sync_scanner::ScanOptions;

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let large_only = args.get(6).is_some_and(|s| s == "--large-files");
    let replacements = args.get(6).is_some_and(|s| s == "--replacements");
    if !(args.len() == 6 || (args.len() == 7 && (large_only || replacements)))
        || args[5] != "--confirm"
    {
        return Err(
            "usage: copy_sample_timing SOURCE EMPTY_DEST NEW_OUTPUT WORKERS MAX_MIB --confirm [--large-files|--replacements]"
                .into(),
        );
    }
    let source = PathBuf::from(&args[0]).canonicalize()?;
    let destination = PathBuf::from(&args[1]).canonicalize()?;
    let output = PathBuf::from(&args[2]);
    let workers = NonZeroUsize::new(args[3].parse()?)
        .filter(|n| n.get() <= 16)
        .ok_or("workers 1..16")?;
    let limit: u64 = args[4].parse()?;
    if !(32..=1024).contains(&limit) {
        return Err("MAX_MIB must be 32..1024".into());
    }
    if source.starts_with(&destination)
        || destination.starts_with(&source)
        || output.exists()
        || !output.is_absolute()
    {
        return Err("disjoint roots and new absolute output required".into());
    }
    if std::fs::read_dir(&destination)?.next().is_some() {
        return Err("destination must be empty".into());
    }
    let limit = limit * 1024 * 1024;
    let cancel = CancellationToken::new();
    let preview_source = LocalConnector::new(&source, LocalOptions::new().with_handle_reuse())?;
    let preview_dest = LocalConnector::new(&destination, LocalOptions::new().with_handle_reuse())?;
    let start = Instant::now();
    let left = sync_scanner::scan(
        &preview_source,
        &Filter::include_all(),
        &ScanOptions::new(),
        &cancel,
    )?;
    let right = sync_scanner::scan(
        &preview_dest,
        &Filter::include_all(),
        &ScanOptions::new(),
        &cancel,
    )?;
    if !left.is_complete() || !right.is_complete() {
        return Err("incomplete inventory; no copy".into());
    }
    let comparison = sync_compare::compare(&left, &right, &sync_compare::CompareOptions::new())?;
    let context = PlanContext {
        plan_id: PlanId::new("sample-preview")?,
        profile: None,
        created_at: Timestamp::now()?,
        source_location: source.display().to_string(),
        destination_location: destination.display().to_string(),
    };
    let mut full = sync_backup::BackupStrategy::new().build_plan(
        &PlanningInput::new(&comparison, &context),
        &sync_backup::BackupStrategy::default_policy(),
    )?;
    let files: Vec<_> = full
        .operations()
        .iter()
        .filter(|o| o.action() == Action::Copy && o.kind() == EntryKind::File)
        .collect();
    let mut selected = BTreeSet::new();
    let mut bytes = 0_u64;
    // Small files exercise metadata latency; whole larger files exercise bandwidth.
    for op in files
        .iter()
        .copied()
        .filter(|o| o.bytes().is_some_and(|n| n > 0 && n <= 64 * 1024))
        .take(if large_only { 0 } else { 2000 })
    {
        if bytes + op.bytes().unwrap_or(0) > limit / 4 {
            break;
        }
        bytes += op.bytes().unwrap_or(0);
        selected.insert(op.path().clone());
    }
    let mut large: Vec<_> = files
        .iter()
        .copied()
        .filter(|o| o.bytes().is_some_and(|n| n > 64 * 1024 && n <= limit / 3))
        .collect();
    large.sort_by_key(|o| std::cmp::Reverse(o.bytes()));
    for op in large {
        let size = op.bytes().unwrap_or(0);
        if bytes + size <= limit * 9 / 10 {
            bytes += size;
            selected.insert(op.path().clone());
        }
        if selected.len() > 2100 {
            break;
        }
    }
    let file_count = selected.len();
    for file in selected.clone() {
        let mut parent = file.parent();
        while let Some(p) = parent.filter(|p| !p.is_root()) {
            parent = p.parent();
            selected.insert(p);
        }
    }
    if replacements {
        // Seed ONLY this invocation's empty destination with tiny synthetic
        // old versions. No original user file is overwritten to prepare a test.
        let seed = LocalConnector::new(&destination, LocalOptions::new().with_mutations())?;
        for op in full
            .operations()
            .iter()
            .filter(|op| selected.contains(op.path()))
        {
            if op.kind() == EntryKind::Directory {
                seed.create_directory(op.path(), &cancel)?;
            } else {
                let mut writer = seed.open_write(
                    op.path(),
                    &WriteOptions::new().with_modified(Some(Timestamp::new(1, 0)?)),
                    &cancel,
                )?;
                writer.write(b"old sample")?;
                writer.commit(CommitMode::CreateNew)?;
            }
        }
        let seeded = sync_scanner::scan(
            &preview_dest,
            &Filter::include_all(),
            &ScanOptions::new(),
            &cancel,
        )?;
        if !seeded.is_complete() {
            return Err("incomplete seeded destination".into());
        }
        let comparison =
            sync_compare::compare(&left, &seeded, &sync_compare::CompareOptions::new())?;
        full = sync_mirror::MirrorStrategy::new().build_plan(
            &PlanningInput::new(&comparison, &context),
            &sync_mirror::MirrorStrategy::default_policy(),
        )?;
    }
    let specs = full
        .operations()
        .iter()
        .filter(|o| selected.contains(o.path()))
        .map(|op| OperationSpec {
            path: op.path().clone(),
            action: op.action(),
            reason: op.reason().clone(),
            source: op.source().cloned(),
            destination: op.destination().cloned(),
        })
        .collect();
    let plan = ExecutionPlan::new(
        PlanHeader {
            id: PlanId::new("copy-sample")?,
            profile: None,
            mode: full.mode(),
            created_at: Timestamp::now()?,
            source: full.source().clone(),
            destination: full.destination().clone(),
            filter: full.filter_rules().clone(),
            policy: *full.policy(),
        },
        specs,
    )?;
    if plan.summary().delete > 0
        || (!replacements && plan.summary().replace > 0)
        || plan.summary().bytes_to_transfer > limit
    {
        return Err("unsafe sample plan".into());
    }
    std::fs::create_dir(&output)?;
    std::fs::write(output.join("plan.json"), plan.to_json()?)?;
    writeln!(
        std::io::stdout().lock(),
        "preview={:.2?} files={file_count} planned_bytes={bytes} workers={workers}",
        start.elapsed()
    )?;
    drop((left, right, full, comparison, preview_source, preview_dest));
    let source = LocalConnector::new(source, LocalOptions::new())?;
    let destination = LocalConnector::new(destination, LocalOptions::new().with_mutations())?;
    let versions_dir = output.join("versions");
    std::fs::create_dir(&versions_dir)?;
    let versions = LocalConnector::new(&versions_dir, LocalOptions::new().with_mutations())?;
    let store =
        sync_versioning::VersionStore::new(&versions, sync_core::ResourcePath::parse("sample")?)?;
    let mut journal = FileJournal::create(output.join("execution.jsonl"))?;
    let execution = ExecutionId::new("copy-sample-execution")?;
    let (sink, events) = bounded_channel(256);
    let emitter = EventEmitter::new(execution.clone(), None, Arc::new(sink));
    let monitor_cancel = cancel.clone();
    let rates = output.join("throughput.csv");
    let monitor = std::thread::spawn(move || -> std::io::Result<()> {
        let mut csv = std::io::BufWriter::new(std::fs::File::create_new(rates)?);
        writeln!(
            csv,
            "seconds,transferred_bytes,entries_done,megabytes_per_second"
        )?;
        let start = Instant::now();
        let mut last = (Instant::now(), 0_u64);
        for event in events {
            if let EventKind::Progress(p) = event.kind() {
                if p.bytes_done > limit {
                    monitor_cancel.cancel();
                }
                let elapsed = last.0.elapsed();
                if elapsed >= Duration::from_secs(1) {
                    let rate = p.bytes_done.saturating_sub(last.1) as f64
                        / elapsed.as_secs_f64()
                        / 1_000_000.0;
                    writeln!(
                        csv,
                        "{:.3},{},{},{rate:.3}",
                        start.elapsed().as_secs_f64(),
                        p.bytes_done,
                        p.entries_done
                    )?;
                    csv.flush()?;
                    last = (Instant::now(), p.bytes_done);
                }
            }
        }
        csv.flush()
    });
    let start = Instant::now();
    let mut request = ExecutionRequest::new(
        &plan,
        &source,
        &destination,
        execution,
        &mut journal,
        &cancel,
    )
    .with_options(
        ExecutionOptions::new()
            .with_copy_workers(workers)
            .with_progress_interval(1024 * 1024),
    )
    .with_events(&emitter);
    if replacements {
        request = request.with_versions(&store);
    }
    let report = Executor.execute(request);
    let elapsed = start.elapsed();
    drop(emitter);
    monitor.join().map_err(|_| "throughput monitor failed")??;
    let report = report?;
    std::fs::write(output.join("report.json"), serde_json::to_vec(&report)?)?;
    let mut refusals = BTreeMap::new();
    for op in report.outcomes() {
        if let sync_executor::OutcomeStatus::Refused { refusal } = op.status() {
            *refusals.entry(refusal.to_string()).or_insert(0_usize) += 1;
        }
    }
    writeln!(
        std::io::stdout().lock(),
        "execute={elapsed:.2?} status={:?} totals={:?} verified_MB_s={:.2} refusals={refusals:?}",
        report.status(),
        report.totals(),
        report.totals().bytes as f64 / elapsed.as_secs_f64() / 1_000_000.0
    )?;
    if report.status() != sync_core::ExecutionStatus::Succeeded || !report.leftovers().is_empty() {
        return Err("copy incomplete or failed; inspect report and journal".into());
    }
    Ok(())
}
