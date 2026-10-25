//! Contrat réseau séparé FTP sur serveur isolé.
#![cfg(feature = "ftp")]
use connector_ftp::{Credentials, FtpConnector, Options};
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
fn isolated_ftp_listing_reuse() -> Result<(), Box<dyn std::error::Error>> {
    exercise_listing_reuse(&FtpConnector::new(opts(), credentials()?, true)?)
}
#[test]
#[ignore = "serveur isolé requis : scripts/test_phase08_ftp.py"]
fn isolated_ftp_contract() -> Result<(), Box<dyn std::error::Error>> {
    let source = FtpConnector::new(opts(), credentials()?, true)?;
    let destination = (FtpConnector::new(opts(), credentials()?, true)?).with_exclusive_mutations();
    assert_eq!(source.connector_id().as_str(), "ftp");
    exercise(&source, &destination)
}
#[test]
fn invalid_credentials_never_expose_secrets() {
    let err = Credentials::new("injection\r\nUSER", "fixture-secret").err();
    assert!(err.is_some());
    assert!(!format!("{err:?}").contains("fixture-secret"));
}
