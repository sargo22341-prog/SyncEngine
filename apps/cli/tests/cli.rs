//! Tests d'intégration du binaire `syncengine`.

use std::process::Command;

fn syncengine(args: &[&str]) -> std::io::Result<std::process::Output> {
    Command::new(env!("CARGO_BIN_EXE_syncengine"))
        .args(args)
        .output()
}

#[test]
fn version_exits_successfully() {
    let output = syncengine(&["--version"]).unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with(concat!("syncengine ", env!("CARGO_PKG_VERSION"))));
    assert!(output.stderr.is_empty());
}

#[test]
fn help_is_default() {
    let output = syncengine(&[]).unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("COMMANDES"));
}

#[test]
fn unknown_command_is_usage_error() {
    let output = syncengine(&["delete-everything"]).unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("commande inconnue"));
}
