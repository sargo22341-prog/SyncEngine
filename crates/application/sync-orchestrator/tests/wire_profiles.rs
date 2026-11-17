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
        trash: sync_profiles::TrashPolicy::legacy(),
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

/// C1 — Étant donné une destination FTP/FTPS vide et une source de
/// 3 fichiers, quand on simule une sauvegarde, alors 3 copies (et leur
/// dossier) sont prévues, 0 suppression, et l'exécution publie les 3 fichiers.
/// Un miroir ne déduit ensuite aucune suppression du listing distant.
#[cfg(any(feature = "ftp", feature = "ftps"))]
fn remote_destination_cycle(tls: bool) -> Result<(), Box<dyn std::error::Error>> {
    use sync_profiles::{Profile, RemoteEndpoint};
    let temp = tempfile::tempdir()?;
    let state = temp.path().join("state");
    let source = temp.path().join("source");
    std::fs::create_dir(&state)?;
    std::fs::create_dir_all(source.join("docs"))?;
    std::fs::write(source.join("a.txt"), b"alpha")?;
    std::fs::write(source.join("b.txt"), b"bravo")?;
    std::fs::write(source.join("docs").join("c.txt"), b"charlie")?;
    let app =
        sync_orchestrator::Application::open_with_lock_root(&state, temp.path().join("locks"))?;
    let address = std::env::var("SYNCENGINE_FTP_ADDR")?.parse()?;
    let root = "/c1-destination".to_owned();
    let remote = if tls {
        RemoteEndpoint::Ftps {
            address,
            tls_domain: "localhost".into(),
            username: "fixture".into(),
            root,
            credential_ref: "FTP_FIXTURE".into(),
            root_certificate: Some(std::env::var("SYNCENGINE_FTP_CERT")?.into()),
            exclusive_writers: true, // seul écrivain : exigé pour publier en FTP
        }
    } else {
        RemoteEndpoint::Ftp {
            address,
            username: "fixture".into(),
            root,
            credential_ref: "FTP_FIXTURE".into(),
            acknowledge_plaintext: true,
            exclusive_writers: true, // seul écrivain : exigé pour publier en FTP
        }
    };
    let mut p = Profile {
        id: sync_core::ProfileId::new("network-destination")?,
        name: "Isolated remote destination".into(),
        group: String::new(),
        mode: sync_plan::SyncMode::Backup,
        source: source.clone(),
        destination: std::path::PathBuf::new(),
        remote_source: None,
        remote_destination: Some(remote),
        filter: sync_filter::FilterRules::new(),
        allow_delete: false,
        compare: sync_profiles::CompareMethod::Content,
        trash: sync_profiles::TrashPolicy::legacy(),
    };
    app.profile_save(p.clone())?;
    let view = app.preview(p.id.clone())?;
    assert_eq!(view["summary"]["copy"], 4, "{view}");
    assert_eq!(view["summary"]["delete"], 0);
    let id = view["plan"]["id"].as_str().ok_or("plan id")?;
    app.execute(p.id.clone(), id.into(), true, false)?;
    let published = std::path::PathBuf::from(std::env::var("SYNCENGINE_FTP_DESTINATION")?);
    assert_eq!(std::fs::read(published.join("a.txt"))?, b"alpha");
    assert_eq!(std::fs::read(published.join("b.txt"))?, b"bravo");
    assert_eq!(
        std::fs::read(published.join("docs").join("c.txt"))?,
        b"charlie"
    );
    // Rejouer : plus rien à copier.
    let view = app.preview(p.id.clone())?;
    assert_eq!(view["summary"]["copy"], 0, "{view}");
    // Miroir : un fichier présent seulement à la destination distante n'est
    // jamais supprimé sur la foi d'un listing sans garantie de cohérence.
    std::fs::write(published.join("tiers.txt"), b"keep")?;
    p.mode = sync_plan::SyncMode::Mirror;
    p.allow_delete = true;
    app.profile_save(p.clone())?;
    let view = app.preview(p.id.clone())?;
    assert_eq!(view["summary"]["delete"], 0, "{view}");
    let id = view["plan"]["id"].as_str().ok_or("plan id")?;
    app.execute(p.id.clone(), id.into(), true, false)?;
    assert_eq!(std::fs::read(published.join("tiers.txt"))?, b"keep");
    Ok(())
}

#[cfg(feature = "ftp")]
#[test]
#[ignore = "isolated fixture required: scripts/test_phase08_ftp.py"]
fn isolated_ftp_remote_destination() -> Result<(), Box<dyn std::error::Error>> {
    remote_destination_cycle(false)
}

#[cfg(feature = "ftps")]
#[test]
#[ignore = "isolated TLS fixture required: scripts/test_phase08_ftp.py"]
fn isolated_ftps_remote_destination() -> Result<(), Box<dyn std::error::Error>> {
    remote_destination_cycle(true)
}

/// C3 — WebDAV derrière un serveur qui redirige les collections sans barre
/// finale (Apache) : source distante avec sous-dossiers lue entièrement, et
/// destination distante (C1) recevant des copies dans des sous-collections.
#[cfg(feature = "webdav")]
fn webdav_endpoint(
    root: &str,
) -> Result<sync_profiles::RemoteEndpoint, Box<dyn std::error::Error>> {
    let base = std::env::var("SYNCENGINE_WEBDAV_URL")?;
    Ok(sync_profiles::RemoteEndpoint::Webdav {
        root: format!("{base}{root}/"),
        credential_ref: "DAV_FIXTURE".into(),
    })
}

#[cfg(feature = "webdav")]
#[test]
#[ignore = "isolated fixture required: scripts/test_phase08_webdav.py"]
fn isolated_webdav_remote_source_with_collections() -> Result<(), Box<dyn std::error::Error>> {
    use sync_profiles::Profile;
    let temp = tempfile::tempdir()?;
    let state = temp.path().join("state");
    let destination = temp.path().join("destination");
    std::fs::create_dir(&state)?;
    std::fs::create_dir(&destination)?;
    let app =
        sync_orchestrator::Application::open_with_lock_root(&state, temp.path().join("locks"))?;
    let p = Profile {
        id: sync_core::ProfileId::new("webdav-source")?,
        name: "WebDAV source".into(),
        group: String::new(),
        mode: sync_plan::SyncMode::Backup,
        source: std::path::PathBuf::new(),
        destination: destination.clone(),
        remote_source: Some(webdav_endpoint("src")?),
        remote_destination: None,
        filter: sync_filter::FilterRules::new(),
        allow_delete: false,
        compare: sync_profiles::CompareMethod::Content,
        trash: sync_profiles::TrashPolicy::legacy(),
    };
    app.profile_save(p.clone())?;
    let view = app.preview(p.id.clone())?;
    assert_eq!(view["summary"]["copy"], 3, "{view}");
    let id = view["plan"]["id"].as_str().ok_or("plan id")?;
    app.execute(p.id.clone(), id.into(), true, false)?;
    assert_eq!(std::fs::read(destination.join("a.txt"))?, b"alpha");
    assert_eq!(
        std::fs::read(destination.join("docs").join("b.md"))?,
        b"bravo"
    );
    Ok(())
}

#[cfg(feature = "webdav")]
#[test]
#[ignore = "isolated fixture required: scripts/test_phase08_webdav.py"]
fn isolated_webdav_remote_destination_with_collections() -> Result<(), Box<dyn std::error::Error>> {
    use sync_profiles::Profile;
    let temp = tempfile::tempdir()?;
    let state = temp.path().join("state");
    let source = temp.path().join("source");
    std::fs::create_dir(&state)?;
    std::fs::create_dir_all(source.join("docs").join("sub"))?;
    std::fs::write(source.join("a.txt"), b"alpha")?;
    std::fs::write(source.join("docs").join("b.txt"), b"bravo")?;
    std::fs::write(source.join("docs").join("sub").join("c.txt"), b"charlie")?;
    let app =
        sync_orchestrator::Application::open_with_lock_root(&state, temp.path().join("locks"))?;
    let p = Profile {
        id: sync_core::ProfileId::new("webdav-destination")?,
        name: "WebDAV destination".into(),
        group: String::new(),
        mode: sync_plan::SyncMode::Backup,
        source,
        destination: std::path::PathBuf::new(),
        remote_source: None,
        remote_destination: Some(webdav_endpoint("dst")?),
        filter: sync_filter::FilterRules::new(),
        allow_delete: false,
        compare: sync_profiles::CompareMethod::Content,
        trash: sync_profiles::TrashPolicy::legacy(),
    };
    app.profile_save(p.clone())?;
    let view = app.preview(p.id.clone())?;
    // a.txt, docs/, docs/b.txt, docs/sub/, docs/sub/c.txt
    assert_eq!(view["summary"]["copy"], 5, "{view}");
    assert_eq!(view["summary"]["delete"], 0);
    let id = view["plan"]["id"].as_str().ok_or("plan id")?;
    app.execute(p.id.clone(), id.into(), true, false)?;
    // Relecture : tout est présent, plus rien à copier.
    let view = app.preview(p.id.clone())?;
    assert_eq!(view["summary"]["copy"], 0, "{view}");
    Ok(())
}
