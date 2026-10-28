//! Throughput stability audit of real copies (large files). Reads SOURCE only;
//! writes only into NEW_DEST, which must not exist and is created here. Builds
//! a backup plan (creations only) of the files of SOURCE matching the given
//! name patterns, executes it with the facade's live options, samples the
//! block-level byte counter every 200 ms, then prints the 4 s windowed rates
//! computed as the desktop curve does (`apps/desktop/src/speed.rs`).
//! Copy WORKERS to WORKERS (1..16); the facade uses its default.
//! Use distinct source files per run: a file already read comes from the
//! OS cache, which hides the source disk.
//!
//! usage: copy_throughput SOURCE NEW_DEST WORKERS --confirm PATTERN...
use connector_local::{LocalConnector, LocalOptions};
use std::error::Error;
use std::io::Write;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::time::{Duration, Instant};
use sync_core::{CancellationToken, ExecutionId, PlanId, Timestamp};
use sync_decisions::{PlanContext, PlanStrategy, PlanningInput};
use sync_events::{EmitOutcome, Event, EventEmitter, EventKind, EventSink};
use sync_executor::{ExecutionOptions, ExecutionRequest, Executor, PlanExecutor};
use sync_filter::{Filter, FilterRules};
use sync_journal::FileJournal;
use sync_scanner::ScanOptions;

struct Counter(Arc<AtomicU64>);
impl EventSink for Counter {
    fn emit(&self, event: Event) -> EmitOutcome {
        if let EventKind::Progress(p) = event.kind() {
            self.0.fetch_max(p.bytes_done, Relaxed);
        }
        EmitOutcome::Delivered
    }
}

/// Same computation as the desktop curve: rate over a 4 s window, one
/// sample before the window kept, first second skipped.
fn windowed(samples: &[(Instant, u64)], started: Instant) -> Vec<f64> {
    let window = Duration::from_secs(4);
    let mut reference = 0;
    let mut out = Vec::new();
    for (index, &(at, count)) in samples.iter().enumerate() {
        while reference + 1 < index && samples[reference + 1].0 + window <= at {
            reference += 1;
        }
        if at.saturating_duration_since(started) < Duration::from_secs(1) || index == reference {
            continue;
        }
        let (since, base) = samples[reference];
        let elapsed = at.saturating_duration_since(since).as_secs_f64();
        if elapsed > 0.0 {
            out.push(count.saturating_sub(base) as f64 / elapsed / 1e6);
        }
    }
    out
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() < 5 || args[3] != "--confirm" {
        return Err("usage: copy_throughput SOURCE NEW_DEST WORKERS --confirm PATTERN...".into());
    }
    let source = PathBuf::from(&args[0]).canonicalize()?;
    let destination = PathBuf::from(&args[1]);
    let workers = NonZeroUsize::new(args[2].parse()?)
        .filter(|n| n.get() <= 16)
        .ok_or("workers 1..16")?;
    if destination.exists() || !destination.is_absolute() {
        return Err("NEW_DEST must be a new absolute path".into());
    }
    std::fs::create_dir(&destination)?;
    let destination = destination.canonicalize()?;
    if source.starts_with(&destination) || destination.starts_with(&source) {
        return Err("disjoint roots required".into());
    }
    let mut rules = FilterRules::new().with_case_insensitive(true);
    for pattern in &args[4..] {
        rules = rules.include_file(pattern.clone());
    }
    let filter = Filter::new(rules)?;
    let cancel = CancellationToken::new();
    let left = sync_scanner::scan(
        &LocalConnector::new(&source, LocalOptions::new())?,
        &filter,
        &ScanOptions::new(),
        &cancel,
    )?;
    let right = sync_scanner::scan(
        &LocalConnector::new(&destination, LocalOptions::new())?,
        &filter,
        &ScanOptions::new(),
        &cancel,
    )?;
    let comparison = sync_compare::compare(&left, &right, &sync_compare::CompareOptions::new())?;
    let context = PlanContext {
        plan_id: PlanId::new("throughput")?,
        profile: None,
        created_at: Timestamp::now()?,
        source_location: source.display().to_string(),
        destination_location: destination.display().to_string(),
    };
    let plan = sync_backup::BackupStrategy::new().build_plan(
        &PlanningInput::new(&comparison, &context),
        &sync_backup::BackupStrategy::default_policy(),
    )?;
    let summary = plan.summary();
    if summary.delete > 0 || summary.replace > 0 {
        return Err("unsafe plan".into());
    }
    let src = LocalConnector::new(&source, LocalOptions::new())?;
    let dst = LocalConnector::new(&destination, LocalOptions::new().with_mutations())?;
    let journal_dir = tempfile::tempdir()?;
    let mut journal = FileJournal::create(journal_dir.path().join("run.jsonl"))?;
    let counter = Arc::new(AtomicU64::new(0));
    let emitter = EventEmitter::new(
        ExecutionId::new("throughput-run")?,
        None,
        Arc::new(Counter(Arc::clone(&counter))),
    );
    let done = Arc::new(AtomicBool::new(false));
    let started = Instant::now();
    let sampler = {
        let (counter, done) = (Arc::clone(&counter), Arc::clone(&done));
        std::thread::spawn(move || {
            let mut samples = vec![(started, 0)];
            while !done.load(Relaxed) {
                std::thread::sleep(Duration::from_millis(200));
                samples.push((Instant::now(), counter.load(Relaxed)));
            }
            samples
        })
    };
    let request = ExecutionRequest::new(
        &plan,
        &src,
        &dst,
        ExecutionId::new("throughput-run")?,
        &mut journal,
        &cancel,
    )
    .with_options(
        ExecutionOptions::new()
            .with_copy_workers(workers)
            .with_progress_interval(sync_orchestrator::LIVE_PROGRESS_BYTES)
            .with_transfer_events(true),
    )
    .with_events(&emitter);
    let report = Executor.execute(request)?;
    let elapsed = started.elapsed();
    done.store(true, Relaxed);
    let samples = sampler.join().map_err(|_| "sampler failed")?;
    let mut rates = windowed(&samples, started);
    let curve: Vec<_> = rates.iter().map(|r| format!("{r:.0}")).collect();
    writeln!(std::io::stdout().lock(), "curve MB/s: {}", curve.join(" "))?;
    let mean = rates.iter().sum::<f64>() / rates.len().max(1) as f64;
    let sd =
        (rates.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / rates.len().max(1) as f64).sqrt();
    rates.sort_by(f64::total_cmp);
    let pct = |q: f64| {
        rates
            .get(((rates.len().saturating_sub(1)) as f64 * q).round() as usize)
            .copied()
            .unwrap_or(0.0)
    };
    writeln!(
        std::io::stdout().lock(),
        "status={:?} files={} bytes={} elapsed={elapsed:.2?} average={:.1} MB/s | curve: mean={mean:.1} sd={sd:.1} min={:.1} p10={:.1} p50={:.1} p90={:.1} max={:.1}",
        report.status(),
        summary.copy,
        summary.bytes_to_transfer,
        summary.bytes_to_transfer as f64 / elapsed.as_secs_f64() / 1e6,
        pct(0.0),
        pct(0.1),
        pct(0.5),
        pct(0.9),
        pct(1.0),
    )?;
    Ok(())
}
