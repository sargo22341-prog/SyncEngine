//! Aides partagées par les tests d'intégration du binaire `syncengine` :
//! exécution, arborescences temporaires, retrait de droits, relevé d'arbre.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

pub(crate) fn syncengine<I, S>(args: I) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    // Each synthetic invocation has its own account state, avoiding unrelated
    // test jobs contending with each other or a real application instance.
    let account = tempfile::tempdir().unwrap();
    Command::new(env!("CARGO_BIN_EXE_syncengine"))
        .env("LOCALAPPDATA", account.path())
        .env("XDG_STATE_HOME", account.path())
        .args(args)
        .output()
        .unwrap()
}

pub(crate) fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

pub(crate) fn write(root: &Path, relative: &str, content: &[u8]) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

/// Retire le droit de lister `path` le temps du test (rétabli au drop).
/// `None` si l'OS ne l'applique pas (exécution en root sous Linux).
pub(crate) struct DenyListing(std::path::PathBuf);

impl DenyListing {
    pub(crate) fn new(path: &Path) -> Option<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o000)).unwrap();
            if fs::read_dir(path).is_ok() {
                fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
                return None;
            }
        }
        #[cfg(windows)]
        {
            let output = Command::new("icacls")
                .arg(path)
                .args(["/deny", "*S-1-1-0:(RD)"])
                .output()
                .unwrap();
            assert!(output.status.success(), "icacls /deny : {output:?}");
        }
        Some(Self(path.to_path_buf()))
    }
}

impl Drop for DenyListing {
    #[expect(
        clippy::print_stderr,
        reason = "un Drop ne peut pas renvoyer d'erreur : l'échec est signalé explicitement"
    )]
    fn drop(&mut self) {
        if let Err(error) = restore_listing(&self.0) {
            eprintln!(
                "ATTENTION : droits non rétablis sur {} : {error}",
                self.0.display()
            );
        }
    }
}

/// Rétablit le droit de lister `path`.
fn restore_listing(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    }
    #[cfg(windows)]
    {
        let output = Command::new("icacls")
            .arg(path)
            .args(["/remove:d", "*S-1-1-0"])
            .output()?;
        if !output.status.success() {
            return Err(std::io::Error::other(format!(
                "icacls : {:?}",
                output.status
            )));
        }
    }
    Ok(())
}

/// Liste triée des chemins, tailles et dates de fichiers sous `root`.
pub(crate) fn snapshot(root: &Path) -> String {
    let mut lines = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for item in fs::read_dir(&dir).unwrap() {
            let path = item.unwrap().path();
            let meta = fs::metadata(&path).unwrap();
            if meta.is_dir() {
                lines.push(format!("D {}", path.display()));
                stack.push(path);
            } else {
                lines.push(format!(
                    "F {} {} {:?}",
                    path.display(),
                    meta.len(),
                    meta.modified().unwrap()
                ));
            }
        }
    }
    lines.sort();
    lines.join("\n")
}
