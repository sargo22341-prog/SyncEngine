use super::*;
use std::io::{self, Write};
use std::net::TcpListener;
use std::thread::{self, JoinHandle};

type TestResult = Result<(), Box<dyn std::error::Error>>;
type ServerHandle = JoinHandle<io::Result<Vec<String>>>;

#[test]
#[ignore = "serveur HTTPS éphémère requis : scripts/test_phase08_webdav.py"]
fn isolated_https_contract() -> TestResult {
    let url = std::env::var("SYNCENGINE_WEBDAV_URL")?;
    let mut connector = WebDavConnector::new(WebDavOptions { root: url }, "fixture-only")?;
    let cancel = CancellationToken::new();
    let root = ResourcePath::root();
    // Given an unknown CA, When production HTTPS connects, Then refuse it.
    assert!(connector.list(&root, None, &cancel).is_err());
    // Trust ONLY this ephemeral CA in a private test client; never disable TLS checks.
    let pem = std::fs::read(std::env::var_os("SYNCENGINE_WEBDAV_CERT").ok_or("no cert")?)?;
    connector.client = Client::builder()
        .https_only(true)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .timeout(Duration::from_secs(5))
        .add_root_certificate(reqwest::Certificate::from_pem(&pem)?)
        .build()?;
    assert_eq!(connector.list(&root, None, &cancel)?.entries().len(), 1);
    let mut reader = connector.open_read(&ResourcePath::parse("file")?, &cancel)?;
    let mut bytes = [0; 3];
    assert_eq!(reader.read(&mut bytes)?, 3);
    assert_eq!(&bytes, b"abc");
    let writable = connector.with_mutations();
    let path = ResourcePath::parse("uploaded")?;
    let mut writer = writable.open_write(&path, &connector_api::WriteOptions::new(), &cancel)?;
    writer.write(b"new-data")?;
    assert!(writable.stat(&path, &cancel).is_err());
    writer.finish()?;
    let stage = writer.staging_path().ok_or("no stage")?.clone();
    assert_eq!(writable.stat(&stage, &cancel)?.metadata().size(), Some(8));
    writer.commit(connector_api::CommitMode::CreateNew)?;
    assert!(writable.stat(&stage, &cancel).is_err());
    let mut conflict = writable.open_write(&path, &connector_api::WriteOptions::new(), &cancel)?;
    conflict.write(b"conflict")?;
    assert_eq!(
        conflict
            .commit(connector_api::CommitMode::CreateNew)
            .err()
            .ok_or("overwrote target")?
            .kind(),
        Kind::Conflict
    );
    let mut replace = writable.open_write(&path, &connector_api::WriteOptions::new(), &cancel)?;
    replace.write(b"replace")?;
    replace.commit(connector_api::CommitMode::ReplaceExisting)?;
    assert_eq!(writable.stat(&path, &cancel)?.metadata().size(), Some(7));
    let mut aborted = writable.open_write(
        &ResourcePath::parse("aborted")?,
        &connector_api::WriteOptions::new(),
        &cancel,
    )?;
    aborted.write(b"partial")?;
    aborted.finish()?;
    aborted.abort()?;
    assert!(
        writable
            .stat(&ResourcePath::parse("aborted")?, &cancel)
            .is_err()
    );
    writable.delete(&path, EntryKind::File, &cancel)?;
    assert_eq!(
        writable
            .delete(&root, EntryKind::Directory, &cancel)
            .err()
            .ok_or("recursive delete allowed")?
            .kind(),
        Kind::NotSupported
    );
    Ok(())
}

/// C3 — Étant donné un serveur qui redirige toute collection sans barre
/// finale (Apache `DirectorySlash`), quand on crée, liste, lit et nettoie des
/// sous-collections, alors tout réussit sans suivre d'autre redirection.
#[test]
#[ignore = "serveur HTTPS éphémère requis : scripts/test_phase08_webdav.py"]
fn isolated_https_collections() -> TestResult {
    let url = std::env::var("SYNCENGINE_WEBDAV_URL")?;
    let mut connector = WebDavConnector::new(WebDavOptions { root: url }, "fixture-only")?;
    let pem = std::fs::read(std::env::var_os("SYNCENGINE_WEBDAV_CERT").ok_or("no cert")?)?;
    connector.client = Client::builder()
        .https_only(true)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .timeout(Duration::from_secs(5))
        .add_root_certificate(reqwest::Certificate::from_pem(&pem)?)
        .build()?;
    let writable = connector.with_mutations();
    let cancel = CancellationToken::new();
    let dir = ResourcePath::parse("dir1")?;
    let sub = ResourcePath::parse("dir1/sub")?;
    assert_eq!(
        writable.create_directory(&dir, &cancel)?.kind(),
        EntryKind::Directory
    );
    assert_eq!(writable.stat(&dir, &cancel)?.kind(), EntryKind::Directory);
    writable.create_directory(&sub, &cancel)?;
    let file = ResourcePath::parse("dir1/sub/f.txt")?;
    let mut writer = writable.open_write(&file, &connector_api::WriteOptions::new(), &cancel)?;
    writer.write(b"in a collection")?;
    writer.commit(connector_api::CommitMode::CreateNew)?;
    let listed = writable.list(&dir, None, &cancel)?;
    assert_eq!(listed.entries().len(), 1);
    assert_eq!(listed.entries()[0].kind(), EntryKind::Directory);
    let listed = writable.list(&sub, None, &cancel)?;
    assert_eq!(listed.entries().len(), 1, "staging collection left behind");
    let mut reader = writable.open_read(&file, &cancel)?;
    let mut bytes = [0; 32];
    assert_eq!(reader.read(&mut bytes)?, 15);
    Ok(())
}

fn response(href: &str, collection: bool) -> String {
    format!(
        "<d:response><d:href>{href}</d:href><d:propstat><d:prop><d:resourcetype>{}</d:resourcetype><d:getcontentlength>3</d:getcontentlength></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>",
        if collection { "<d:collection/>" } else { "" }
    )
}
fn multi(body: &str) -> String {
    format!("<d:multistatus xmlns:d=\"DAV:\">{body}</d:multistatus>")
}

#[test]
fn dav_parser_preserves_unicode_and_unknown_timestamps() -> TestResult {
    let root = Url::parse("https://example.test/dav/")?;
    let xml = multi(&(response("/dav/", true) + &response("/dav/%C3%A9t%C3%A9%20%23.txt", false)));
    let entries = parse(&root, &ResourcePath::root(), "1", &xml)?;
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[1].path().as_str(), "été #.txt");
    assert_eq!(entries[1].metadata().size(), Some(3));
    assert!(entries[1].metadata().modified().is_none());
    Ok(())
}

#[test]
fn dav_rejects_traversal_aliases_partial_and_foreign_responses() -> TestResult {
    let root = Url::parse("https://example.test/dav/")?;
    for href in [
        "/dav/../secret",
        "/dav/%2e%2e/secret",
        "/dav/a%2fb",
        "/dav/%252e%252e",
        "/dav/a%5cb",
        "/dav/a%00b",
        "/dav2/a",
        "//evil.test/dav/a",
        "https://evil.test/dav/a",
        "/dav/a?token=x",
        "/dav/a#fragment",
        "/dav/dir/child",
    ] {
        let xml = multi(&(response("/dav/", true) + &response(href, false)));
        assert!(
            parse(&root, &ResourcePath::root(), "1", &xml).is_err(),
            "unsafe href accepted"
        );
    }
    let good = response("/dav/", true);
    for xml in [
        "".to_owned(),
        multi(""),
        multi(&response("/dav/a", false)),
        multi(&(good.clone() + &good)),
        multi(&good.replace("200 OK", "403 Forbidden")),
        multi(&good).replace("DAV:", "evil:"),
        multi(&good.replace("<d:resourcetype><d:collection/></d:resourcetype>", "")),
        multi(&good.replace(">3<", ">NaN<")),
    ] {
        assert!(parse(&root, &ResourcePath::root(), "1", &xml).is_err());
    }
    let dtd = "<!DOCTYPE x [<!ENTITY a 'x'>]><d:multistatus xmlns:d='DAV:'>&a;</d:multistatus>";
    assert!(parse(&root, &ResourcePath::root(), "1", dtd).is_err());
    Ok(())
}

#[test]
fn configuration_and_diagnostics_never_expose_bearer() -> TestResult {
    for root in [
        "http://example.test/",
        "https://user:password@example.test/",
        "https://example.test/?token=x",
        "https://example.test/#x",
        "https://example.test/dav",
    ] {
        let err = WebDavConnector::new(
            WebDavOptions {
                root: root.to_owned(),
            },
            "fixture-secret",
        )
        .err()
        .ok_or("accepted invalid URL")?;
        assert!(!format!("{err:?} {err}").contains("fixture-secret"));
        assert!(!format!("{err:?} {err}").contains("password"));
    }
    let connector = WebDavConnector::new(
        WebDavOptions {
            root: "https://example.test/dav/".into(),
        },
        "fixture-secret",
    )?;
    assert!(connector.auth.is_sensitive());
    assert!(!format!("{:?}", connector.auth).contains("fixture-secret"));
    assert!(connector.capabilities().is_read_only());
    assert!(
        connector
            .capabilities()
            .supports(StorageFeature::StreamingRead)
    );
    assert_eq!(
        connector.url(&ResourcePath::parse("été #.txt")?)?.path(),
        "/dav/%C3%A9t%C3%A9%20%23.txt"
    );
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert_eq!(
        connector
            .list(&ResourcePath::root(), None, &cancel)
            .err()
            .ok_or("no cancellation")?
            .kind(),
        Kind::Cancelled
    );
    assert_eq!(
        connector
            .delete(&ResourcePath::root(), EntryKind::Directory, &cancel)
            .err()
            .ok_or("no refusal")?
            .kind(),
        Kind::NotSupported
    );
    Ok(())
}

// HTTP is available ONLY by replacing the private client from this test module.
// There is no production option or Cargo feature permitting plaintext credentials.
fn server(
    replies: Vec<(u16, String, String)>,
) -> Result<(WebDavConnector, ServerHandle), Box<dyn std::error::Error>> {
    let (connector, handle, _) = server_at(replies)?;
    Ok((connector, handle))
}

/// Like [`server`], also returning the fixture address; each recorded request
/// line ends with ` +auth` when an Authorization header was sent (the value
/// itself is never recorded).
fn server_at(
    replies: Vec<(u16, String, String)>,
) -> Result<(WebDavConnector, ServerHandle, std::net::SocketAddr), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let address = listener.local_addr()?;
    let record_auth = replies
        .iter()
        .any(|(status, _, _)| (300..400).contains(status));
    let handle = thread::spawn(move || {
        let mut requests = Vec::new();
        for (status, headers, body) in replies {
            let start = std::time::Instant::now();
            let (mut stream, _) = loop {
                match listener.accept() {
                    Ok(pair) => break pair,
                    Err(e)
                        if e.kind() == io::ErrorKind::WouldBlock
                            && start.elapsed() < Duration::from_secs(5) =>
                    {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(e) => return Err(e),
                }
            };
            // Windows sockets inherit the listener's non-blocking mode: a
            // request not yet arrived would end the fixture with WouldBlock
            // and reset the connection (intermittent wrong error kinds).
            stream.set_nonblocking(false)?;
            stream.set_read_timeout(Some(Duration::from_secs(3)))?;
            let mut bytes = Vec::new();
            let mut one = [0; 1];
            while !bytes.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut one)?;
                bytes.push(one[0]);
                if bytes.len() > 16_384 {
                    return Err(io::Error::other("oversized request"));
                }
            }
            let text = String::from_utf8(bytes).map_err(io::Error::other)?;
            let length = text
                .lines()
                .find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .and_then(|n| n.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            let mut payload = vec![0; length];
            stream.read_exact(&mut payload)?;
            // Store only the method and request path, never Authorization.
            let line = text.lines().next().unwrap_or_default().to_owned();
            let auth = text
                .lines()
                .any(|l| l.to_ascii_lowercase().starts_with("authorization:"));
            requests.push(if record_auth && auth {
                format!("{line} +auth")
            } else {
                line
            });
            let length_header = if headers.to_ascii_lowercase().contains("content-length:") {
                String::new()
            } else {
                format!("Content-Length: {}\r\n", body.len())
            };
            let headers = headers.replace("{addr}", &address.to_string());
            write!(
                stream,
                "HTTP/1.1 {status} Test\r\nConnection: close\r\n{headers}{length_header}\r\n{body}"
            )?;
        }
        Ok(requests)
    });
    let mut connector = WebDavConnector::new(
        WebDavOptions {
            root: "https://example.test/dav/".into(),
        },
        "fixture-secret",
    )?;
    connector.root = Url::parse(&format!("http://{address}/dav/"))?;
    connector.client = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .no_proxy()
        .timeout(Duration::from_secs(3))
        .build()?;
    Ok((connector, handle, address))
}

fn join(
    handle: JoinHandle<io::Result<Vec<String>>>,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    handle
        .join()
        .map_err(|_| "fixture thread failed")?
        .map_err(Into::into)
}

#[test]
fn transport_retries_read_only_requests_and_streams() -> TestResult {
    let xml = multi(&response("/dav/file", false));
    let (connector, handle) = server(vec![
        (503, "Retry-After: 0\r\n".into(), "".into()),
        (207, "".into(), xml),
        (200, "".into(), "abc".into()),
    ])?;
    let mut reader =
        connector.open_read(&ResourcePath::parse("file")?, &CancellationToken::new())?;
    let mut bytes = [0; 2];
    assert_eq!(reader.read(&mut bytes)?, 2);
    assert_eq!(&bytes, b"ab");
    assert_eq!(reader.read(&mut bytes)?, 1);
    assert_eq!(bytes[0], b'c');
    assert_eq!(reader.read(&mut bytes)?, 0);
    assert_eq!(
        join(handle)?,
        [
            "PROPFIND /dav/file HTTP/1.1",
            "PROPFIND /dav/file HTTP/1.1",
            "GET /dav/file HTTP/1.1"
        ]
    );
    Ok(())
}

#[test]
fn transport_auth_redirect_rate_limits_and_partial_listing_fail_closed() -> TestResult {
    for (status, headers, body, expected) in [
        (
            401,
            "",
            "server echoed fixture-secret",
            Kind::Unauthenticated,
        ),
        (403, "", "", Kind::PermissionDenied),
        (404, "", "", Kind::NotFound),
        (302, "Location: https://evil.test/\r\n", "", Kind::Permanent),
        (
            429,
            "Retry-After: 60\r\n",
            "",
            Kind::Throttled {
                retry_after: Some(Duration::from_secs(60)),
            },
        ),
        (207, "", "<broken", Kind::Permanent),
    ] {
        let (connector, handle) = server(vec![(status, headers.into(), body.into())])?;
        let err = connector
            .list(&ResourcePath::root(), None, &CancellationToken::new())
            .err()
            .ok_or("error converted to empty listing")?;
        assert_eq!(err.kind(), expected);
        assert!(!format!("{err:?}").contains("fixture-secret"));
        assert_eq!(join(handle)?.len(), 1);
    }
    Ok(())
}

#[test]
fn retry_budget_is_bounded() -> TestResult {
    let (connector, handle) = server(vec![(500, "".into(), "".into()); 3])?;
    assert_eq!(
        connector
            .list(&ResourcePath::root(), None, &CancellationToken::new())
            .err()
            .ok_or("no error")?
            .kind(),
        Kind::Transient
    );
    assert_eq!(join(handle)?.len(), 3);
    Ok(())
}

#[test]
fn stream_cancellation_is_observed_before_next_read() -> TestResult {
    let (connector, handle) = server(vec![
        (207, "".into(), multi(&response("/dav/file", false))),
        (200, "".into(), "abc".into()),
    ])?;
    let cancel = CancellationToken::new();
    let mut reader = connector.open_read(&ResourcePath::parse("file")?, &cancel)?;
    cancel.cancel();
    assert_eq!(
        reader
            .read(&mut [0; 1])
            .err()
            .ok_or("ignored cancellation")?
            .kind(),
        Kind::Cancelled
    );
    assert_eq!(join(handle)?.len(), 2);
    Ok(())
}

#[test]
fn short_body_is_an_error_and_is_never_restarted() -> TestResult {
    let (connector, handle) = server(vec![
        (207, "".into(), multi(&response("/dav/file", false))),
        (200, "Content-Length: 8\r\n".into(), "abc".into()),
    ])?;
    let mut reader =
        connector.open_read(&ResourcePath::parse("file")?, &CancellationToken::new())?;
    let mut bytes = [0; 16];
    let mut failed = false;
    for _ in 0..4 {
        match reader.read(&mut bytes) {
            Err(err) => {
                assert_eq!(err.kind(), Kind::Disconnected);
                failed = true;
                break;
            }
            Ok(0) => break,
            Ok(_) => {}
        }
    }
    assert!(failed, "short response reported as complete");
    assert_eq!(join(handle)?.len(), 2);
    Ok(())
}

// --- C3 : collections Apache (mod_dir DirectorySlash) -------------------------

fn redirect_to(location: &str) -> (u16, String, String) {
    (301, format!("Location: {location}\r\n"), String::new())
}

#[test]
fn collections_are_listed_with_a_trailing_slash() -> TestResult {
    let xml = multi(&(response("/dav/docs/", true) + &response("/dav/docs/a.txt", false)));
    let (connector, handle) = server(vec![(207, "".into(), xml)])?;
    let page = connector.list(
        &ResourcePath::parse("docs")?,
        None,
        &CancellationToken::new(),
    )?;
    assert_eq!(page.entries().len(), 1);
    assert_eq!(page.entries()[0].path().as_str(), "docs/a.txt");
    assert_eq!(join(handle)?, ["PROPFIND /dav/docs/ HTTP/1.1"]);
    Ok(())
}

#[test]
fn a_redirect_to_the_same_url_plus_slash_is_followed_once() -> TestResult {
    // Apache: PROPFIND /dav/docs → 301 to /dav/docs/ (absolute or relative).
    for location in ["http://{addr}/dav/docs/", "/dav/docs/"] {
        for status in [301, 308] {
            let (connector, handle) = server(vec![
                (status, format!("Location: {location}\r\n"), String::new()),
                (207, "".into(), multi(&response("/dav/docs/", true))),
            ])?;
            let entry = connector.stat(&ResourcePath::parse("docs")?, &CancellationToken::new())?;
            assert_eq!(entry.kind(), EntryKind::Directory);
            assert_eq!(
                join(handle)?,
                [
                    "PROPFIND /dav/docs HTTP/1.1 +auth",
                    "PROPFIND /dav/docs/ HTTP/1.1 +auth"
                ]
            );
        }
    }
    Ok(())
}

#[test]
fn any_other_redirect_is_refused_without_resending_credentials() -> TestResult {
    for location in [
        "http://evil.test/dav/docs/",
        "https://{addr}/dav/docs/",
        "http://{addr}/dav/other/",
        "http://{addr}/dav/docs/x",
        "http://{addr}/dav/docs/?token=x",
        "http://{addr}/dav/docs/#x",
        "http://{addr}/dav/docs",
        "//evil.test/dav/docs/",
        "/dav/docs//",
        "/dav/docs/../docs/",
    ] {
        let (connector, handle) = server(vec![redirect_to(location)])?;
        let err = connector
            .stat(&ResourcePath::parse("docs")?, &CancellationToken::new())
            .err()
            .ok_or("foreign redirect followed")?;
        assert_eq!(err.kind(), Kind::Permanent, "{location}");
        assert_eq!(
            join(handle)?,
            ["PROPFIND /dav/docs HTTP/1.1 +auth"],
            "{location}"
        );
    }
    Ok(())
}

#[test]
fn a_second_redirect_is_never_followed() -> TestResult {
    let (connector, handle) = server(vec![redirect_to("/dav/docs/"), redirect_to("/dav/docs//")])?;
    let err = connector
        .stat(&ResourcePath::parse("docs")?, &CancellationToken::new())
        .err()
        .ok_or("redirect loop followed")?;
    assert_eq!(err.kind(), Kind::Permanent);
    assert_eq!(join(handle)?.len(), 2);
    Ok(())
}

#[test]
fn created_collections_are_addressed_with_a_trailing_slash() -> TestResult {
    let (connector, handle) = server(vec![
        (207, "".into(), multi(&response("/dav/", true))),
        (201, "".into(), String::new()),
        redirect_to("/dav/dir1/"),
        (207, "".into(), multi(&response("/dav/dir1/", true))),
    ])?;
    let connector = connector.with_mutations();
    let entry =
        connector.create_directory(&ResourcePath::parse("dir1")?, &CancellationToken::new())?;
    assert_eq!(entry.kind(), EntryKind::Directory);
    assert_eq!(
        join(handle)?,
        [
            "PROPFIND /dav/ HTTP/1.1 +auth",
            "MKCOL /dav/dir1/ HTTP/1.1 +auth",
            "PROPFIND /dav/dir1 HTTP/1.1 +auth",
            "PROPFIND /dav/dir1/ HTTP/1.1 +auth"
        ]
    );
    Ok(())
}
