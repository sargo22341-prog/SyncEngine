//! Contrat réseau séparé FTPS sur serveur isolé.
#![cfg(feature = "ftps")]
use connector_ftps::{Credentials, FtpsConnector, Options};
include!("../../../../../tests/support/network_contract.rs");
fn opts() -> Options {
    Options {
        address: std::env::var("SYNCENGINE_FTP_ADDR")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(([127, 0, 0, 1], 1).into()),
        root: "/".into(),
    }
}
fn credentials() -> Result<Credentials, connector_api::StorageError> {
    Credentials::new("fixture", "fixture-only")
}
#[test]
#[ignore = "serveur isolé requis : scripts/test_phase08_ftp.py"]
fn isolated_ftps_listing_reuse() -> Result<(), Box<dyn std::error::Error>> {
    let pem = std::fs::read(std::env::var_os("SYNCENGINE_FTP_CERT").ok_or("no cert")?)?;
    let source = FtpsConnector::new(opts(), credentials()?, "localhost".into())?
        .with_root_certificate(&pem)?;
    exercise_listing_reuse(&source)
}
#[test]
#[ignore = "serveur isolé requis : scripts/test_phase08_ftp.py"]
fn isolated_ftps_contract() -> Result<(), Box<dyn std::error::Error>> {
    let untrusted = FtpsConnector::new(opts(), credentials()?, "localhost".into())?;
    assert!(
        untrusted
            .stat(&ResourcePath::root(), &CancellationToken::new())
            .is_err()
    );
    let pem = std::fs::read(std::env::var_os("SYNCENGINE_FTP_CERT").ok_or("no cert")?)?;
    let source = FtpsConnector::new(opts(), credentials()?, "localhost".into())?
        .with_root_certificate(&pem)?;
    let destination = (FtpsConnector::new(opts(), credentials()?, "localhost".into())?
        .with_root_certificate(&pem)?)
    .with_exclusive_mutations();
    assert_eq!(source.connector_id().as_str(), "ftps");
    exercise(&source, &destination)
}
#[test]
fn invalid_credentials_never_expose_secrets() {
    let err = Credentials::new("injection\r\nUSER", "fixture-secret").err();
    assert!(err.is_some());
    assert!(!format!("{err:?}").contains("fixture-secret"));
}
