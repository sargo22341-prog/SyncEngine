//! Headless application client. Windows GUI subsystem prevents console flashes;
//! commands still write JSON to redirected stdout/stderr when handles exist.
#![cfg_attr(windows, windows_subsystem = "windows")]
use platform_api::{AgentInvocation, ScheduleAdapter, SessionEvent};
use std::io::Write;
use std::path::{Path, PathBuf};
use sync_core::{CancellationToken, ProfileId, Timestamp};
use sync_orchestrator::Application;
use sync_scheduler::{
    Completion, JobStatus, MissedJobs, Schedule, ScheduledApplication, Scheduler, Trigger, Wake,
};

mod resident;

type Failure = Box<dyn std::error::Error>;
const USAGE: &str = "syncengine-agent list STATE | authorize STATE PROFILE interval SECONDS|daily UTC_HOUR UTC_MINUTE|logon|logon-delay SECONDS|manager-start --approve-writes [--approve-deletions] [--skip-missed] [--show-window] | remove STATE PROFILE | acknowledge STATE PROFILE --diagnosed | tick STATE clock|logon|logon-delay SECONDS|manager-start | cancel STATE | clear-cancel STATE | export STATE OUTPUT windows|linux | resident STATE | stop STATE | resume STATE | status STATE";
fn main() {
    let result = run(std::env::args().skip(1).collect());
    let code = match result {
        Ok((value, code)) => {
            if writeln!(std::io::stdout().lock(), "{value}").is_err() {
                std::process::exit(1);
            }
            code
        }
        Err(e) => {
            let message = serde_json::json!({"error":e.to_string()});
            if writeln!(std::io::stderr().lock(), "{message}").is_err() {
                std::process::exit(1);
            }
            1
        }
    };
    std::process::exit(code);
}
fn run(args: Vec<String>) -> Result<(serde_json::Value, i32), Failure> {
    if args.len() < 2 {
        return Err(USAGE.into());
    }
    let command = args[0].as_str();
    let state = PathBuf::from(&args[1]).canonicalize()?;
    if !state.is_dir() {
        return Err("state directory required".into());
    }
    match command {
        "resident" if args.len() == 2 => return resident::run(&state),
        "status" if args.len() == 2 => {
            return Ok((
                serde_json::to_value(sync_orchestrator::resident_status(&state)?)?,
                0,
            ));
        }
        "stop" if args.len() == 2 => {
            std::fs::File::create(state.join("agent-stop.request"))?.sync_all()?;
            return Ok((serde_json::json!({"stop_requested":true}), 0));
        }
        "resume" if args.len() == 2 => {
            sync_orchestrator::clear_agent_stop(&state)?;
            sync_orchestrator::clear_agent_cancel(&state)?;
            return resident::run(&state);
        }
        "cancel" if args.len() == 2 => {
            sync_orchestrator::request_agent_cancel(&state)?;
            return Ok((serde_json::json!({"cancel_requested":true}), 0));
        }
        "clear-cancel" if args.len() == 2 => {
            sync_orchestrator::clear_agent_cancel(&state)?;
            return Ok((serde_json::json!({"cancel_requested":false}), 0));
        }
        "export" if args.len() == 4 => return export(&state, Path::new(&args[2]), &args[3]),
        _ => (),
    }
    let mut scheduler = Scheduler::open(state.join("scheduler.sqlite"))?;
    match command {
        "list" if args.len() == 2 => Ok((serde_json::to_value(scheduler.list()?)?, 0)),
        "remove" if args.len() == 3 => {
            scheduler.remove(&ProfileId::new(&args[2])?)?;
            Ok((serde_json::json!({"removed":true}), 0))
        }
        "acknowledge" if args.len() == 4 && args[3] == "--diagnosed" => {
            let id = ProfileId::new(&args[2])?;
            let app = Application::open(&state)?;
            app.acknowledge_interrupted(&id, true)?;
            if scheduler
                .list()?
                .iter()
                .any(|row| row.schedule.approved.id == id && row.running_slot.is_some())
            {
                scheduler.acknowledge(&id)?;
            }
            Ok((
                serde_json::json!({"interruption_acknowledged":true,"next_run_requires_fresh_scan":true}),
                0,
            ))
        }
        "authorize" => {
            if args.len() < 5 {
                return Err(USAGE.into());
            }
            let app = Application::open(&state)?;
            let profile = app
                .profiles_list()?
                .into_iter()
                .find(|p| p.id.as_str() == args[2])
                .ok_or("profile not found")?;
            let (trigger, index) = match args[3].as_str() {
                "interval" if args.len() > 5 => (
                    Trigger::Interval {
                        seconds: args[4].parse()?,
                    },
                    5,
                ),
                "daily" if args.len() > 6 => (
                    Trigger::DailyUtc {
                        hour: args[4].parse()?,
                        minute: args[5].parse()?,
                    },
                    6,
                ),
                "logon" => (Trigger::Logon, 4),
                "logon-delay" if args.len() > 5 => (
                    Trigger::LogonDelayed {
                        seconds: args[4].parse()?,
                    },
                    5,
                ),
                "manager-start" => (Trigger::UserManagerStart, 4),
                _ => return Err(USAGE.into()),
            };
            if matches!(trigger, Trigger::Logon | Trigger::LogonDelayed { .. }) && !cfg!(windows)
                || matches!(trigger, Trigger::UserManagerStart) && !cfg!(target_os = "linux")
            {
                return Err("session trigger NotSupported on this OS".into());
            }
            let flags = &args[index..];
            if !flags.iter().any(|f| f == "--approve-writes")
                || flags.iter().any(|f| {
                    !matches!(
                        f.as_str(),
                        "--approve-writes"
                            | "--approve-deletions"
                            | "--skip-missed"
                            | "--show-window"
                    )
                })
            {
                return Err("explicit --approve-writes required; unknown flag refused".into());
            }
            let approved = Schedule {
                approved: profile,
                trigger,
                created_at: Timestamp::now()?.secs(),
                enabled: true,
                allow_deletions: flags.iter().any(|f| f == "--approve-deletions"),
                missed: if flags.iter().any(|f| f == "--skip-missed") {
                    MissedJobs::Skip
                } else {
                    MissedJobs::RunOnce
                },
                show_window: flags.iter().any(|f| f == "--show-window"),
            };
            scheduler.save(&approved)?;
            Ok((serde_json::to_value(approved)?, 0))
        }
        "tick" if args.len() == 3 => {
            let _instance = sync_orchestrator::hold_agent_presence(&state)?;
            let wake = match args[2].as_str() {
                "clock" => Wake::Clock,
                "logon" if cfg!(windows) => Wake::Session(SessionEvent::Logon),
                "manager-start" if cfg!(target_os = "linux") => {
                    Wake::Session(SessionEvent::UserManagerStart)
                }
                _ => return Err("wake NotSupported".into()),
            };
            let app = match Application::open(&state) {
                Ok(app) => app,
                Err(e) if e.code == "busy" => {
                    return Ok((serde_json::json!({"deferred":"application_busy"}), 2));
                }
                Err(e) => return Err(e.into()),
            };
            let token = CancellationToken::new();
            let signal_token = token.clone();
            ctrlc::set_handler(move || signal_token.cancel())?;
            let receipts = dispatch(&state, &app, &mut scheduler, wake, &token, &token)?;
            let code = if token.is_cancelled()
                || receipts
                    .iter()
                    .any(|r| r.completion.status != JobStatus::Succeeded)
            {
                2
            } else {
                0
            };
            Ok((serde_json::to_value(receipts)?, code))
        }
        _ => Err(USAGE.into()),
    }
}
fn dispatch(
    state: &Path,
    app: &Application,
    scheduler: &mut Scheduler,
    wake: Wake,
    token: &CancellationToken,
    shutdown: &CancellationToken,
) -> Result<Vec<sync_scheduler::DispatchReceipt>, Failure> {
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let done_worker = done.clone();
    let worker_token = token.clone();
    let marker = sync_orchestrator::agent_cancel_marker(state);
    let stop_marker = state.join("agent-stop.request");
    let retire_marker = state.join("agent-retire.request");
    let shutdown = shutdown.clone();
    // A persistent marker also prevents new jobs after a stop request.
    let cancelled = || match marker.try_exists() {
        Ok(false) => false,
        Ok(true) | Err(_) => true,
    };
    if cancelled()
        || shutdown.is_cancelled()
        || !matches!(stop_marker.try_exists(), Ok(false))
        || !matches!(retire_marker.try_exists(), Ok(false))
    {
        token.cancel();
    }
    let watcher = std::thread::spawn(move || {
        while !done_worker.load(std::sync::atomic::Ordering::Acquire) {
            if shutdown.is_cancelled()
                || !matches!(retire_marker.try_exists(), Ok(false))
                || !matches!(stop_marker.try_exists(), Ok(false))
                || !matches!(marker.try_exists(), Ok(false))
            {
                worker_token.cancel();
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    });
    let now = Timestamp::now().map(|time| time.secs());
    let agent = Agent { app, state };
    let (stop, stopped) = std::sync::mpsc::channel::<()>();
    let result = std::thread::scope(|scope| {
        let publisher = app;
        // Live progress for windows (presentation only, about twice a
        // second); a failed publication never affects the job.
        scope.spawn(move || {
            loop {
                let finished = !matches!(
                    stopped.recv_timeout(std::time::Duration::from_millis(500)),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                );
                let _presentation_only = publisher.publish_live().is_ok();
                if finished {
                    break;
                }
            }
        });
        let result = match now {
            Ok(now) => scheduler.dispatch(now, wake, &agent, token),
            Err(_) => Err(sync_scheduler::SchedulerError::Invalid("clock unavailable")),
        };
        drop(stop);
        result
    });
    done.store(true, std::sync::atomic::Ordering::Release);
    watcher.join().map_err(|_| "cancellation observer failed")?;
    let receipts = result?;
    Ok(receipts)
}

/// Agent side of the application port: opens the desktop window when the
/// schedule asks for it, then runs the job through the ordinary facade.
struct Agent<'a> {
    app: &'a Application,
    state: &'a Path,
}
impl ScheduledApplication for Agent<'_> {
    fn run(&self, schedule: &Schedule, cancel: &CancellationToken) -> Completion {
        if schedule.show_window
            && let Err(e) = open_window(self.state)
        {
            // Presentation only: the job runs anyway; the failure is reported.
            let warning = serde_json::json!({"warning":format!("window: {e}")});
            let _stderr_closed = writeln!(std::io::stderr().lock(), "{warning}").is_err();
        }
        self.app.run(schedule, cancel)
    }
}
/// Starts the desktop window next to this executable, following the agent job.
/// Windows only: the tasks run in the interactive session of the user, while
/// a systemd user unit has no guaranteed graphical session and would stop the
/// window with the unit. An already open window follows the job by itself.
fn open_window(state: &Path) -> Result<(), Failure> {
    if !cfg!(windows) || sync_orchestrator::desktop_present(state)? {
        return Ok(());
    }
    let exe = std::env::current_exe()?.with_file_name(if cfg!(windows) {
        "syncengine-desktop.exe"
    } else {
        "syncengine-desktop"
    });
    // Detached on purpose: the window outlives this short-lived agent process
    // and the user closes it; Windows has no zombie processes to reap.
    std::process::Command::new(exe)
        .arg("--suivre-agent")
        .env("SYNCENGINE_STATE_DIR", state)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    Ok(())
}
fn export(state: &Path, output: &Path, target: &str) -> Result<(serde_json::Value, i32), Failure> {
    let invocation = AgentInvocation {
        executable: std::env::current_exe()?.canonicalize()?,
        state: state.into(),
    };
    let artifacts = match target {
        "windows" => platform_windows::WindowsScheduler.render(&invocation)?,
        "linux" => platform_linux::LinuxScheduler.render(&invocation)?,
        _ => return Err("target must be windows or linux".into()),
    };
    std::fs::create_dir_all(output)?;
    for artifact in &artifacts {
        // Never silently replace an existing installation definition.
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(output.join(artifact.name))?;
        file.write_all(artifact.content.as_bytes())?;
        file.sync_all()?;
    }
    Ok((
        serde_json::json!({"artifacts":artifacts.iter().map(|a|a.name).collect::<Vec<_>>(),"installed":false}),
        0,
    ))
}
