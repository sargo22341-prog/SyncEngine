//! Local bidirectional preview/execution with durable state outside both trees.

use std::error::Error;
use std::io::{self, Write};
use std::path::PathBuf;

use connector_local::{LocalConnector, LocalOptions};
use sync_core::{CancellationToken, ExecutionId, PlanId, ProfileId, ResourcePath, Timestamp};
use sync_decisions::PlanContext;
use sync_executor::ExecutionOptions;
use sync_filter::Filter;
use sync_history::{History, RetentionPolicy};
use sync_journal::FileJournal;
use sync_plan::{DeletionGuard, DeletionPolicy, OverwritePolicy, PlanPolicy};
use sync_session::{SessionRequest, execute, preview};
use sync_versioning::VersionStore;

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() < 3 {
        return Err("usage: local SOURCE DESTINATION STATE_DIRECTORY [--execute] [--allow-delete] [--recover=EXECUTION_ID]".into());
    }
    let source = PathBuf::from(&args[0]).canonicalize()?;
    let destination = PathBuf::from(&args[1]).canonicalize()?;
    let state = PathBuf::from(&args[2]).canonicalize()?;
    for (a, b) in [
        (&source, &destination),
        (&source, &state),
        (&destination, &state),
    ] {
        if a.starts_with(b) || b.starts_with(a) {
            return Err("source, destination and state directory must be disjoint".into());
        }
    }
    if !source.is_dir() || !destination.is_dir() || !state.is_dir() {
        return Err("all three roots must be existing directories".into());
    }
    let mut apply = false;
    let mut delete = false;
    let mut recover = None;
    for arg in &args[3..] {
        let arg = arg.to_str().ok_or("non-UTF8 option")?;
        match arg {
            "--execute" => apply = true,
            "--allow-delete" => delete = true,
            _ if arg.starts_with("--recover=") => recover = Some(ExecutionId::new(&arg[10..])?),
            _ => return Err("unknown option".into()),
        }
    }
    if delete && !apply {
        return Err("--allow-delete requires --execute".into());
    }
    // Cover recovery acknowledgement as well as execution and publication.
    let _owner = sync_executor::execution_lock(&sync_executor::default_lock_root()?)?;
    let mut history = History::open(state.join("state.sqlite"))?;
    if let Some(execution) = recover {
        if apply {
            return Err("recovery and execution must be separate invocations".into());
        }
        history.mark_interrupted(&execution)?;
        writeln!(
            io::stdout().lock(),
            "Execution marked interrupted; old owner must be stopped. Re-run a fresh preview."
        )?;
        return Ok(());
    }
    let local_options = if apply {
        LocalOptions::new().with_mutations()
    } else {
        LocalOptions::new()
    };
    let left = LocalConnector::new(&source, local_options.clone())?;
    let right = LocalConnector::new(&destination, local_options)?;
    let now = Timestamp::now()?;
    let suffix = format!("{}-{}", now.secs(), now.subsec_nanos());
    let context = PlanContext {
        plan_id: PlanId::new(format!("plan-{suffix}"))?,
        profile: Some(ProfileId::new("local-bidirectional")?),
        created_at: now,
        source_location: source.to_str().ok_or("non-UTF8 source root")?.into(),
        destination_location: destination
            .to_str()
            .ok_or("non-UTF8 destination root")?
            .into(),
    };
    // Preview also shows default deletion refusals. This guard does not
    // allow emptying a peer or deleting over 50% without an API policy change.
    let policy = PlanPolicy::new(
        OverwritePolicy::SourceWins,
        DeletionPolicy::enabled(DeletionGuard::new()),
    );
    let cancel = CancellationToken::new();
    let signal_token = cancel.clone();
    ctrlc::set_handler(move || signal_token.cancel())?;
    let filter = Filter::include_all();
    let plan = preview(&history, &left, &right, &context, &filter, &policy, &cancel)?;
    let mut out = io::stdout().lock();
    writeln!(out, "{}", plan.to_json()?)?;
    if !apply {
        writeln!(
            out,
            "SIMULATION: synchronized files unchanged; reference database initialized/read only, no baseline published."
        )?;
        return Ok(());
    }
    let execution = ExecutionId::new(format!("run-{suffix}"))?;
    let mut journal = FileJournal::create(state.join(format!("{execution}.jsonl")))?;
    let versions = LocalConnector::new(&state, LocalOptions::new().with_mutations())?;
    let folder = ResourcePath::parse(&format!("versions/{execution}"))?;
    let store = VersionStore::new(&versions, folder)?;
    let result = execute(
        &mut history,
        SessionRequest {
            plan: &plan,
            source: &left,
            destination: &right,
            execution,
            journal: &mut journal,
            cancel: &cancel,
            options: ExecutionOptions::new().with_deletions_confirmed(delete),
            versions: Some(&store),
            events: None,
        },
        RetentionPolicy::default(),
    )?;
    writeln!(
        out,
        "status={:?}, baseline_updated={}, verification_error={:?}",
        result.execution.status(),
        result.baseline_updated,
        result.verification_error
    )?;
    if !result.baseline_updated {
        return Err(
            "execution did not establish a new verified baseline; inspect history and journal"
                .into(),
        );
    }
    Ok(())
}
