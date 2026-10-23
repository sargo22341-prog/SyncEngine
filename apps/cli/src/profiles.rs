//! Profile-backed CLI client of the same facade as GUI and agent.
use std::io::Write;
use std::path::Path;
use sync_core::ProfileId;
use sync_orchestrator::Application;

pub(crate) fn run(args: &[std::ffi::OsString]) -> Result<u8, Box<dyn std::error::Error>> {
    let usage = "profile list|providers STATE | profile save STATE FILE.json | profile preview STATE ID | profile run STATE ID --confirm [--confirm-delete]";
    if args.len() < 2 {
        return Err(usage.into());
    }
    let command = args[0].to_str().ok_or(usage)?;
    let app = Application::open(Path::new(&args[1]))?;
    let mut code = 0;
    let value = match (command, args.len()) {
        ("list", 2) => serde_json::to_value(app.profiles_list()?)?,
        ("providers", 2) => serde_json::to_value(app.providers())?,
        ("save", 3) => {
            let file = Path::new(&args[2]);
            if file.metadata()?.len() > 64 * 1024 {
                return Err("profile file exceeds 64 KiB".into());
            }
            let profile = serde_json::from_slice(&std::fs::read(file)?)?;
            app.profile_save(profile)?;
            serde_json::json!({"saved":true})
        }
        ("preview", 3) => app.preview(ProfileId::new(args[2].to_str().ok_or(usage)?)?)?,
        ("run", 4 | 5) => {
            let flags: Vec<_> = args[3..].iter().map(|a| a.to_str()).collect();
            if !flags.contains(&Some("--confirm"))
                || flags
                    .iter()
                    .any(|f| !matches!(f, Some("--confirm" | "--confirm-delete")))
            {
                return Err(usage.into());
            }
            let id = ProfileId::new(args[2].to_str().ok_or(usage)?)?;
            let approved = app
                .profiles_list()?
                .into_iter()
                .find(|p| p.id == id)
                .ok_or("profile missing")?;
            let token = crate::cancellation();
            let report = app.execute_scheduled(
                &approved,
                flags.contains(&Some("--confirm-delete")),
                &token,
            )?;
            let execution = report
                .get("execution")
                .filter(|v| v.is_object())
                .unwrap_or(&report);
            if execution["status"] != "succeeded"
                || report.get("baseline_updated").is_some_and(|v| v != true)
            {
                code = 1;
            }
            report
        }
        _ => return Err(usage.into()),
    };
    writeln!(
        std::io::stdout().lock(),
        "{}",
        serde_json::to_string_pretty(&value)?
    )?;
    Ok(code)
}
