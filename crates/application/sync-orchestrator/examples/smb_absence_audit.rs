//! Native SMB absence audit: explicit read-only profile preview, or tiny fixtures.
//! Never executes a real user's profile. Fixture mode requires new isolated roots.
#[cfg(all(windows, feature = "smb"))]
mod audit {
    use std::{error::Error, io::Write, path::PathBuf, time::Instant};
    use sync_core::ProfileId;
    use sync_orchestrator::Application;
    use sync_profiles::{Profile, RemoteEndpoint};

    fn preview(
        app: &Application,
        profile: &Profile,
        label: &str,
    ) -> Result<String, Box<dyn Error>> {
        let start = Instant::now();
        let value = app.preview_paged(profile.id.clone())?;
        writeln!(
            std::io::stdout().lock(),
            "{label}: {:.2?} source_complete={} destination_complete={} summary={} consistency_warnings={}",
            start.elapsed(),
            value["plan"]["source"]["complete"],
            value["plan"]["destination"]["complete"],
            value["summary"],
            app.snapshot()?.scan_consistency_warnings
        )?;
        Ok(value["plan"]["id"]
            .as_str()
            .ok_or("missing plan id")?
            .to_owned())
    }
    pub(super) fn run() -> Result<(), Box<dyn Error>> {
        let args: Vec<_> = std::env::args().skip(1).collect();
        if args.len() == 3 && args[0] == "preview" {
            let profile: Profile = serde_json::from_slice(&std::fs::read(&args[1])?)?;
            if !matches!(&profile.remote_source, Some(RemoteEndpoint::Smb { .. })) {
                return Err("SMB source required".into());
            }
            let root = PathBuf::from(&args[2]);
            if !root.is_absolute() {
                return Err("new absolute state required".into());
            }
            std::fs::create_dir(&root)?;
            let app = Application::open_with_lock_root(&root, root.with_extension("locks"))?;
            app.profile_save(profile.clone())?;
            let plan = preview(&app, &profile, "profile preview only")?;
            let mut causes = std::collections::BTreeMap::new();
            let mut offset = 0;
            loop {
                let page = app.plan_page(&plan, offset, "", "skip")?;
                let rows = page["operations"].as_array().ok_or("missing page")?;
                for row in rows {
                    *causes.entry(row["reason"].to_string()).or_insert(0usize) += 1;
                }
                offset += rows.len();
                if rows.is_empty() || offset >= page["total"].as_u64().unwrap_or(0) as usize {
                    break;
                }
            }
            writeln!(std::io::stdout().lock(), "skipped_reasons={causes:?}")?;
            // No execute call exists on this branch.
            return Ok(());
        }
        if args.len() != 4 || args[0] != "fixture" || args[3] != "--confirm-fixture" {
            return Err("usage: smb_absence_audit preview PROFILE_JSON NEW_STATE | fixture NEW_UNC_CHILD NEW_LOCAL_WORK --confirm-fixture".into());
        }
        let source = PathBuf::from(&args[1]);
        let work = PathBuf::from(&args[2]);
        if !source.is_absolute()
            || !source.to_string_lossy().starts_with(r"\\")
            || !source
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with(".syncengine-presence-"))
            || !work.is_absolute()
            || work.exists()
            || source.exists()
        {
            return Err(
                "new absolute isolated work and new .syncengine-presence-* UNC child required"
                    .into(),
            );
        }
        std::fs::create_dir(&work)?;
        std::fs::create_dir(&source)?;
        let state = work.join("state");
        let dest = work.join("destination");
        std::fs::create_dir(&state)?;
        std::fs::create_dir(&dest)?;
        std::fs::write(source.join("keep.txt"), b"keep")?;
        std::fs::write(dest.join("keep.txt"), b"keep")?;
        std::fs::OpenOptions::new()
            .write(true)
            .open(dest.join("keep.txt"))?
            .set_modified(std::fs::metadata(source.join("keep.txt"))?.modified()?)?;
        std::fs::create_dir_all(dest.join("old/deep"))?;
        for path in ["obsolete.txt", "old/a", "old/deep/b"] {
            std::fs::write(dest.join(path), b"old fixture")?;
        }
        let app = Application::open_with_lock_root(&state, work.join("locks"))?;
        let profile = Profile {
            id: ProfileId::new("smb-presence-fixture")?,
            name: "SMB presence fixture".into(),
            group: String::new(),
            mode: sync_plan::SyncMode::Mirror,
            source: source.clone(),
            destination: dest.clone(),
            remote_source: Some(RemoteEndpoint::Smb {
                root: source.clone(),
            }),
            remote_destination: None,
            filter: sync_filter::FilterRules::new(),
            allow_delete: true,
            compare: sync_profiles::CompareMethod::SizeAndDate,
        };
        app.profile_save(profile.clone())?;
        let plan = preview(&app, &profile, "normal absence")?;
        let deletions = app.plan_page(&plan, 0, "", "delete")?;
        if deletions["total"] != 5 {
            return Err("expected exactly five fixture deletions".into());
        }
        let result = app.execute(profile.id.clone(), plan, true, true)?;
        if result["totals"]["done"] != 5
            || dest.join("old").exists()
            || dest.join("obsolete.txt").exists()
        {
            return Err("fixture mirror did not converge".into());
        }
        std::fs::write(dest.join("reappeared.txt"), b"old destination")?;
        let plan = preview(&app, &profile, "absence before reappearance")?;
        std::fs::write(source.join("reappeared.txt"), b"new source content")?;
        let result = app.execute(profile.id.clone(), plan, true, true)?;
        if result["totals"]["refused"] != 1
            || std::fs::read(dest.join("reappeared.txt"))? != b"old destination"
        {
            return Err("reappeared source was not protected".into());
        }
        let plan = preview(&app, &profile, "fresh preview")?;
        let result = app.execute(profile.id.clone(), plan, true, false)?;
        if result["status"] != "succeeded"
            || std::fs::read(dest.join("reappeared.txt"))? != b"new source content"
        {
            return Err("fresh preview did not converge".into());
        }
        writeln!(
            std::io::stdout().lock(),
            "fixture passed: five safe deletions, one reappearance refused, fresh preview converged; payload < 1 KiB"
        )?;
        Ok(())
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(all(windows, feature = "smb"))]
    {
        audit::run()
    }
    #[cfg(not(all(windows, feature = "smb")))]
    {
        Err("Windows with feature smb required".into())
    }
}
