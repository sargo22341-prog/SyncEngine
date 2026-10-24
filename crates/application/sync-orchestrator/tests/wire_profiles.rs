//! Application/provider boundary on the existing isolated FTP/FTPS fixtures.
//! No personal credentials, accounts or remote server is ever used here.
#[cfg(any(feature = "ftp", feature = "ftps"))]
fn profile_cycle(tls: bool) -> Result<(), Box<dyn std::error::Error>> {
    use sync_profiles::{Profile, RemoteEndpoint};
    let temp = tempfile::tempdir()?;
    let state = temp.path().join("state");
    let destination = temp.path().join("destination");
    std::fs::create_dir(&state)?;
    std::fs::create_dir(&destination)?;
    let app =
        sync_orchestrator::Application::open_with_lock_root(&state, temp.path().join("locks"))?;
    let address = std::env::var("SYNCENGINE_FTP_ADDR")?.parse()?;
    let remote = if tls {
        RemoteEndpoint::Ftps {
            address,
            tls_domain: "localhost".into(),
            username: "fixture".into(),
            root: "/".into(),
            credential_ref: "FTP_FIXTURE".into(),
            root_certificate: Some(std::env::var("SYNCENGINE_FTP_CERT")?.into()),
            exclusive_writers: false,
        }
    } else {
        RemoteEndpoint::Ftp {
            address,
            username: "fixture".into(),
            root: "/".into(),
            credential_ref: "FTP_FIXTURE".into(),
            acknowledge_plaintext: true,
            exclusive_writers: false,
        }
    };
    let mut p = Profile {
        id: sync_core::ProfileId::new("network-fixture")?,
        name: "Isolated transport".into(),
        group: String::new(),
        mode: sync_plan::SyncMode::Backup,
        source: std::path::PathBuf::new(),
        destination: destination.clone(),
        remote_source: Some(remote),
        remote_destination: None,
        filter: sync_filter::FilterRules::new().include_file("file"),
        allow_delete: false,
        compare: sync_profiles::CompareMethod::Content,
    };
    app.profile_save(p.clone())?;
    let view = app.preview(p.id.clone())?;
    assert_eq!(view["summary"]["copy"], 1);
    let id = view["plan"]["id"].as_str().ok_or("plan id")?;
    app.execute(p.id.clone(), id.into(), true, false)?;
    assert_eq!(std::fs::read(destination.join("file"))?, b"abc");
    // A network traversal without a strong consistency guarantee must never
    // justify deleting a local file, even with an explicit mirror policy.
    std::fs::write(destination.join("precious"), b"keep")?;
    p.filter = sync_filter::FilterRules::new()
        .include_file("file")
        .include_file("precious");
    p.mode = sync_plan::SyncMode::Mirror;
    p.allow_delete = true;
    app.profile_save(p.clone())?;
    let view = app.preview(p.id.clone())?;
    assert_eq!(view["summary"]["delete"], 0);
    let id = view["plan"]["id"].as_str().ok_or("plan id")?;
    app.execute(p.id.clone(), id.into(), true, false)?;
    assert_eq!(std::fs::read(destination.join("precious"))?, b"keep");
    Ok(())
}

#[cfg(feature = "ftp")]
#[test]
#[ignore = "isolated fixture required: scripts/test_phase08_ftp.py"]
fn isolated_ftp_profile() -> Result<(), Box<dyn std::error::Error>> {
    profile_cycle(false)
}

#[cfg(feature = "ftps")]
#[test]
#[ignore = "isolated TLS fixture required: scripts/test_phase08_ftp.py"]
fn isolated_ftps_profile() -> Result<(), Box<dyn std::error::Error>> {
    profile_cycle(true)
}
