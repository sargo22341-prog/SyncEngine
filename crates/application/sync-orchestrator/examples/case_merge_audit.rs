//! Tiny, explicit NAS case-alias and growing-log copy fixture. No real profile.
#[cfg(all(windows, feature = "smb"))]
fn run() -> Result<(), Box<dyn std::error::Error>> {
    use std::io::{Read, Write};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 3 || args[2] != "--confirm-fixture" {
        return Err(
            "usage: case_merge_audit NEW_NAS_TESTSYNC_CHILD NEW_LOCAL_WORK --confirm-fixture"
                .into(),
        );
    }
    let source = PathBuf::from(&args[0]);
    let work = PathBuf::from(&args[1]);
    if !source.parent().is_some_and(|parent| {
        parent
            .to_string_lossy()
            .trim_end_matches('\\')
            .eq_ignore_ascii_case(r"\\nas\testsync")
    }) || !source
        .file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with(".syncengine-case-"))
        || source
            .components()
            .any(|p| matches!(p, std::path::Component::ParentDir))
        || source.exists()
        || !work.is_absolute()
        || work.exists()
    {
        return Err("new isolated roots under NAS testsync and local workspace required".into());
    }
    std::fs::create_dir(&source)?;
    std::fs::create_dir(&work)?;
    let dest = work.join("destination");
    std::fs::create_dir_all(source.join("People/B/bbno$"))?;
    std::fs::create_dir_all(dest.join("People/b/bbno$"))?;
    std::fs::write(
        source.join("People/B/bbno$/folder.jpg"),
        b"new fixture photograph",
    )?;
    std::fs::write(dest.join("People/b/bbno$/folder.jpg"), b"old")?;
    std::fs::write(source.join("People/B/unique.txt"), b"unique fixture")?;
    let log = source.join("live.log");
    {
        let mut writer = std::io::BufWriter::new(std::fs::File::create(&log)?);
        for _ in 0..4096 {
            writer.write_all(&[b'a'; 4096])?;
        }
        writer.flush()?;
    }
    std::fs::create_dir(work.join("state"))?;
    let app = sync_orchestrator::Application::open_with_lock_root(
        work.join("state"),
        work.join("locks"),
    )?;
    let profile = sync_profiles::Profile {
        id: sync_core::ProfileId::new("case-merge-fixture")?,
        name: "Case merge fixture".into(),
        group: String::new(),
        mode: sync_plan::SyncMode::Mirror,
        source: source.clone(),
        destination: dest.clone(),
        remote_source: Some(sync_profiles::RemoteEndpoint::Smb {
            root: source.clone(),
        }),
        remote_destination: None,
        filter: sync_filter::FilterRules::new(),
        allow_delete: true,
        compare: sync_profiles::CompareMethod::SizeAndDate,
    };
    app.profile_save(profile.clone())?;
    let preview = app.preview_paged(profile.id.clone())?;
    if preview["summary"]["skip"] != 0 || preview["summary"]["conflict"] != 0 {
        return Err(format!("fixture preview must be clean: {}", preview["summary"]).into());
    }
    let plan = preview["plan"]["id"]
        .as_str()
        .ok_or("plan id missing")?
        .to_owned();
    let stop = AtomicBool::new(false);
    let result = std::thread::scope(|scope| {
        let writer = scope.spawn(|| -> std::io::Result<u64> {
            let mut file = std::fs::OpenOptions::new().append(true).open(&log)?;
            let mut bytes = 0;
            while !stop.load(Ordering::Acquire) && bytes < 4 * 1024 * 1024 {
                file.write_all(&[b'b'; 4096])?;
                file.flush()?;
                bytes += 4096;
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            Ok(bytes)
        });
        let execution = app.execute(profile.id.clone(), plan, true, true);
        stop.store(true, Ordering::Release);
        let added = writer.join().map_err(|_| "writer thread failed")??;
        let result = execution?;
        writeln!(
            std::io::stdout().lock(),
            "live-copy result={} appended_bytes={added}",
            result["status"]
        )?;
        Ok::<_, Box<dyn std::error::Error>>(result)
    })?;
    if result["status"] != "succeeded"
        || std::fs::read(dest.join("People/b/bbno$/folder.jpg"))? != b"new fixture photograph"
        || std::fs::read(dest.join("People/b/unique.txt"))? != b"unique fixture"
    {
        return Err(format!("case or live-log copy failed: {result}").into());
    }
    // Verify the actual copied prefix, not only successful status.
    let mut expected = std::fs::File::open(&log)?;
    let mut copied = std::fs::File::open(dest.join("live.log"))?;
    let mut a = [0u8; 65536];
    let mut b = [0u8; 65536];
    let mut copied_bytes = 0;
    loop {
        let n = copied.read(&mut a)?;
        if n == 0 {
            break;
        }
        expected.read_exact(&mut b[..n])?;
        if a[..n] != b[..n] {
            return Err("copied log prefix differs".into());
        }
        copied_bytes += n;
    }
    std::fs::write(dest.join("obsolete-fixture.txt"), b"obsolete")?;
    let preview = app.preview_paged(profile.id.clone())?;
    let plan = preview["plan"]["id"]
        .as_str()
        .ok_or("plan id missing")?
        .to_owned();
    let result = app.execute(profile.id.clone(), plan, true, true)?;
    if result["status"] != "succeeded" || dest.join("obsolete-fixture.txt").exists() {
        return Err(format!("final fixture mirror failed: {result}").into());
    }
    let preview = app.preview_paged(profile.id.clone())?;
    if ["copy", "replace", "delete", "skip", "conflict"]
        .iter()
        .any(|key| preview["summary"][key] != 0)
    {
        return Err(format!("fixture did not converge: {}", preview["summary"]).into());
    }
    writeln!(
        std::io::stdout().lock(),
        "fixture passed: case alias copied, live prefix verified ({copied_bytes} bytes), mirror converged; total payload < 41 MiB"
    )?;
    Ok(())
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(all(windows, feature = "smb"))]
    {
        run()
    }
    #[cfg(not(all(windows, feature = "smb")))]
    {
        Err("Windows with smb required".into())
    }
}
