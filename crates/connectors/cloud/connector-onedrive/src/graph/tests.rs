use super::*;
use serde_json::json;
use std::sync::atomic::AtomicUsize;
use sync_credentials::{AccessToken, CredentialError};

type TestResult = Result<(), Box<dyn std::error::Error>>;
struct Tokens {
    calls: AtomicUsize,
}
impl AccessTokenProvider for Tokens {
    fn access_token(&self, cancel: &CancellationToken) -> Result<AccessToken, CredentialError> {
        if cancel.is_cancelled() {
            return Err(CredentialError::Cancelled);
        }
        self.calls.fetch_add(1, Ordering::Relaxed);
        AccessToken::new("fixture-only-secret".into())
    }
}
fn options(drive: &str) -> OneDriveOptions {
    OneDriveOptions {
        drive_id: drive.into(),
        download_hosts: vec!["downloads.example.test".into()],
    }
}
fn connector(drive: &str) -> Result<OneDriveConnector, StorageError> {
    OneDriveConnector::new(
        options(drive),
        Arc::new(Tokens {
            calls: AtomicUsize::new(0),
        }),
    )
}
#[test]
fn capabilities_are_prudent_and_mutations_never_connect() -> TestResult {
    let c = connector("drive")?;
    let root = ResourcePath::root();
    let cancel = CancellationToken::new();
    assert!(c.capabilities().is_read_only());
    assert_eq!(
        c.capabilities().listing_consistency,
        connector_api::ListingConsistency::Unknown
    );
    assert!(c.capabilities().supports(StorageFeature::StreamingRead));
    assert!(c.capabilities().native_hashes.is_empty());
    assert_eq!(
        c.create_directory(&root, &cancel)
            .err()
            .ok_or("mkdir succeeded")?
            .kind(),
        Kind::NotSupported
    );
    assert_eq!(
        c.delete(&root, EntryKind::Directory, &cancel)
            .err()
            .ok_or("delete succeeded")?
            .kind(),
        Kind::NotSupported
    );
    assert_eq!(
        c.open_write(&root, &connector_api::WriteOptions::new(), &cancel)
            .err()
            .ok_or("write succeeded")?
            .kind(),
        Kind::NotSupported
    );
    let mut o = options("drive");
    o.download_hosts.clear();
    let c = OneDriveConnector::new(
        o,
        Arc::new(Tokens {
            calls: AtomicUsize::new(0),
        }),
    )?;
    assert!(!c.capabilities().supports(StorageFeature::Read));
    assert_eq!(
        c.open_read(&root, &cancel)
            .err()
            .ok_or("read succeeded")?
            .kind(),
        Kind::NotSupported
    );
    Ok(())
}
#[test]
fn invalid_configuration_is_rejected() {
    for name in ["x:content", "*", "a?b", "\r", "a|b"] {
        assert!(!valid_name(name));
    }
    for drive in ["", "..", "a/b", "https://evil", "a?secret", "a%2fb"] {
        assert!(connector(drive).is_err());
    }
    for host in [
        "127.0.0.1",
        "[::1]",
        "evil/path",
        "user@evil",
        "evil:443",
        "*.sharepoint.com",
        "UPPER.test",
    ] {
        let mut o = options("drive");
        o.download_hosts = vec![host.into()];
        assert!(
            OneDriveConnector::new(
                o,
                Arc::new(Tokens {
                    calls: AtomicUsize::new(0)
                })
            )
            .is_err()
        );
    }
}
#[test]
fn urls_preserve_unicode_and_confine_tokens_and_downloads() -> TestResult {
    let c = connector("b!drive")?;
    let u = c.url(&["items", "id:", "été #%.txt"])?;
    assert!(u.as_str().contains("%C3%A9t%C3%A9%20%23%25.txt"));
    let path = "/v1.0/drives/b!drive/items/id/children";
    for bad in [
        "https://evil.test/v1.0/drives/b!drive/items/id/children",
        "http://graph.microsoft.com/v1.0/drives/b!drive/items/id/children",
        "https://graph.microsoft.com/v1.0/me/drive",
        "https://graph.microsoft.com:444/v1.0/drives/b!drive/items/id/children",
        "https://user@graph.microsoft.com/v1.0/drives/b!drive/items/id/children",
    ] {
        assert!(!c.http.graph_url(&Url::parse(bad)?, path));
    }
    assert!(c.http.graph_url(
        &Url::parse(&format!(
            "https://graph.microsoft.com{path}?$skiptoken=opaque"
        ))?,
        path
    ));
    for bad in [
        "https://downloads.example.test.evil/a",
        "http://downloads.example.test/a",
        "https://downloads.example.test:444/a",
        "https://user@downloads.example.test/a",
        "https://127.0.0.1/a",
    ] {
        assert!(!c.http.download_url(&Url::parse(bad)?));
    }
    assert!(c.http.download_url(&Url::parse(
        "https://downloads.example.test/a?secret=value"
    )?));
    Ok(())
}
#[test]
fn aliases_unknown_kinds_and_malicious_names_are_not_absence() -> TestResult {
    let path = ResourcePath::parse("a")?;
    let good = json!({"id":"id", "name":"a", "size":3,"file":{},"parentReference":{"id":"root","driveId":"drive"}});
    let item: Item = serde_json::from_value(good.clone())?;
    assert_eq!(
        item.child("root", "drive", &path, Op::List)?
            .metadata()
            .size(),
        Some(3)
    );
    assert!(item.child("foreign", "drive", &path, Op::List).is_err());
    for field in ["remoteItem", "deleted", "package", "folder"] {
        let mut value = good.clone();
        value[field] = json!({});
        assert!(
            serde_json::from_value::<Item>(value)?
                .entry(&path, Op::Stat)
                .is_err()
        );
    }
    for name in ["../a", "a/b", "a\\b", "", "."] {
        assert!(ResourcePath::root().join(name).is_err());
    }
    assert!(serde_json::from_slice::<Page>(br#"{"value":["#).is_err());
    Ok(())
}
#[test]
fn cancellation_and_foreign_tokens_fail_without_network() -> TestResult {
    let c = connector("drive")?;
    let cancel = CancellationToken::new();
    let root = ResourcePath::root();
    assert_eq!(
        c.list(&root, Some(&PageToken::new("secret")), &cancel)
            .err()
            .ok_or("accepted token")?
            .kind(),
        Kind::Conflict
    );
    cancel.cancel();
    assert_eq!(
        c.connect(&cancel).err().ok_or("connected")?.kind(),
        Kind::Cancelled
    );
    assert_eq!(
        c.list(&root, None, &cancel).err().ok_or("listed")?.kind(),
        Kind::Cancelled
    );
    assert_eq!(
        c.open_read(&root, &cancel).err().ok_or("read")?.kind(),
        Kind::Cancelled
    );
    Ok(())
}

#[test]
#[ignore = "HTTPS fixture requise : scripts/test_phase09_onedrive.py"]
fn isolated_https_contract() -> TestResult {
    let base = Url::parse(&std::env::var("SYNCENGINE_ONEDRIVE_URL")?)?;
    let pem = std::fs::read(std::env::var_os("SYNCENGINE_ONEDRIVE_CERT").ok_or("missing CA")?)?;
    let trusted = reqwest::blocking::Client::builder()
        .https_only(true)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .timeout(std::time::Duration::from_secs(5))
        .add_root_certificate(reqwest::Certificate::from_pem(&pem)?)
        .build()?;
    let make = |drive: &str| -> Result<OneDriveConnector, StorageError> {
        let mut c = connector(drive)?;
        c.http.base = base.clone();
        c.http.client = trusted.clone();
        c.http.download_hosts = vec!["localhost".into()];
        Ok(c)
    };
    let cancel = CancellationToken::new();
    let root = ResourcePath::root();
    let mut untrusted = connector("drive")?;
    untrusted.http.base = base.clone();
    assert_eq!(
        untrusted
            .connect(&cancel)
            .err()
            .ok_or("trusted unknown CA")?
            .kind(),
        Kind::Disconnected
    );
    let c = make("drive")?;
    c.connect(&cancel)?;
    let first = c.list(&root, None, &cancel)?;
    assert_eq!(first.entries()[0].path().as_str(), "été #.txt");
    let token = first.next_page().ok_or("no page")?;
    assert!(make("drive")?.list(&root, Some(token), &cancel).is_err());
    let second = c.list(&root, Some(token), &cancel)?;
    assert_eq!(second.entries().len(), 1);
    assert!(second.next_page().is_none());
    assert!(c.list(&root, Some(token), &cancel).is_err());
    // Given a complete Graph children traversal with unknown consistency,
    // When the real scanner consumes it, Then absence remains Unknown.
    let inventory = sync_scanner::scan(
        &c,
        &sync_filter::Filter::include_all(),
        &sync_scanner::ScanOptions::new(),
        &cancel,
    )?;
    assert!(matches!(
        inventory.presence(&ResourcePath::parse("absent")?),
        sync_scanner::Presence::Unknown(_)
    ));
    assert!(matches!(
        sync_scanner::scan(
            &make("missing")?,
            &sync_filter::Filter::include_all(),
            &sync_scanner::ScanOptions::new(),
            &cancel
        ),
        Err(sync_scanner::ScanError::RootUnavailable(_))
    ));
    let file = ResourcePath::parse("été #.txt")?;
    assert_eq!(c.stat(&file, &cancel)?.metadata().size(), Some(3));
    let mut r = c.open_read(&file, &cancel)?;
    let mut content = Vec::new();
    let mut buf = [0; 2];
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        content.extend_from_slice(&buf[..n]);
    }
    assert_eq!(content, b"abc");
    for (drive, expected) in [
        ("unauth", Kind::Unauthenticated),
        ("denied", Kind::PermissionDenied),
        ("missing", Kind::NotFound),
        ("retry", Kind::Transient),
    ] {
        let err = make(drive)?
            .connect(&cancel)
            .err()
            .ok_or("accepted failure")?;
        assert_eq!(err.kind(), expected);
        assert!(!format!("{err:?}").contains("fixture-only-secret"));
    }
    for drive in ["foreign", "duplicate", "alias", "malformed", "traversal"] {
        let c = make(drive)?;
        let result = c.list(&root, None, &cancel);
        if drive == "duplicate" {
            let page = result?;
            assert!(c.list(&root, page.next_page(), &cancel).is_err());
        } else {
            assert!(result.is_err(), "{drive}");
        }
    }
    for drive in ["throttle", "date"] {
        assert!(matches!(
            make(drive)?
                .connect(&cancel)
                .err()
                .ok_or("ignored throttle")?
                .kind(),
            Kind::Throttled { .. }
        ));
    }
    make("backoff")?.connect(&cancel)?;
    for drive in ["short", "changed"] {
        let c = make(drive)?;
        let mut r = c.open_read(&file, &cancel)?;
        let mut failed = false;
        for _ in 0..10 {
            match r.read(&mut buf) {
                Err(_) => {
                    failed = true;
                    break;
                }
                Ok(0) => break,
                Ok(_) => {}
            }
        }
        assert!(failed, "{drive} was falsely complete");
        assert!(r.read(&mut buf).is_err());
    }
    let mut r = c.open_read(&file, &cancel)?;
    cancel.cancel();
    assert_eq!(
        r.read(&mut buf).err().ok_or("ignored cancellation")?.kind(),
        Kind::Cancelled
    );
    Ok(())
}
