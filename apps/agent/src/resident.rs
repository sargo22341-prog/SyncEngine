//! Resident lifecycle. The synchronous engines run on this dedicated process;
//! waits use sleeping threads, never an async executor or a SQLite busy loop.
use super::*;
use std::time::Duration;
use sync_orchestrator::ResidentPhase;

pub(super) fn run(state: &Path) -> Result<(serde_json::Value, i32), Failure> {
    let _instance = sync_orchestrator::hold_agent_presence(state)?;
    let shutdown = CancellationToken::new();
    let signal = shutdown.clone();
    ctrlc::set_handler(move || signal.cancel())?;
    let result = serve(state, &shutdown);
    // A storage error is a process failure (eligible for native restart), while
    // business failures remain scheduler receipts and do not restart the agent.
    let phase = if result.is_ok() {
        ResidentPhase::Stopped
    } else {
        ResidentPhase::Error
    };
    let published = sync_orchestrator::publish_resident_status(state, phase);
    match (result, published) {
        (Err(cause), Err(observation)) => {
            return Err(format!("{cause}; status publication: {observation}").into());
        }
        (Err(cause), _) => return Err(cause),
        (Ok(()), Err(observation)) => return Err(observation.into()),
        (Ok(()), Ok(())) => (),
    }
    Ok((serde_json::json!({"stopped":true}), 0))
}
fn serve(state: &Path, shutdown: &CancellationToken) -> Result<(), Failure> {
    let mut scheduler = Scheduler::open(state.join("scheduler.sqlite"))?;
    #[cfg(windows)]
    {
        let (identity, logon_at) = platform_windows::interactive_session()?;
        scheduler.observe_session(&identity, logon_at)?;
    }
    let app = Application::open(state)?;
    let stop = state.join("agent-stop.request");
    let retire = state.join("agent-retire.request");
    while !shutdown.is_cancelled() && !stop.try_exists()? && !retire.try_exists()? {
        let token = CancellationToken::new();
        let paused = app.agent_cancel_pending();
        sync_orchestrator::publish_resident_status(
            state,
            if paused {
                ResidentPhase::Paused
            } else {
                ResidentPhase::Dispatching
            },
        )?;
        if !paused {
            let receipts =
                super::dispatch(state, &app, &mut scheduler, Wake::Clock, &token, shutdown)?;
            // One bounded latest result; no log file or per-loop output grows.
            if !receipts.is_empty() {
                sync_orchestrator::publish_resident_status(
                    state,
                    if receipts
                        .iter()
                        .any(|r| r.completion.status != JobStatus::Succeeded)
                    {
                        ResidentPhase::Attention
                    } else {
                        ResidentPhase::Idle
                    },
                )?;
            } else {
                sync_orchestrator::publish_resident_status(state, ResidentPhase::Idle)?;
            }
        }
        // Edits, clock jumps and wake are observed within five seconds when idle.
        // A busy job completes/cancels before the next sequential dispatch.
        for _ in 0..20 {
            if shutdown.is_cancelled() || stop.try_exists()? || retire.try_exists()? {
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
    Ok(())
}
