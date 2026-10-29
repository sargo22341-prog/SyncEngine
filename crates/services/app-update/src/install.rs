//! Replacement of the program files by renaming, with rollback.
//!
//! Each current file is renamed `<name>.old` (Windows allows renaming a
//! running executable), then the extracted file is renamed into place. All
//! renames stay in the program folder, on one volume. The sequence is not
//! atomic as a whole: on failure, every step done is undone in reverse
//! order, and an incomplete undo is reported with the state of each file.

use crate::archive::Staged;
use crate::error::{ErrorKind, UpdateError, fail, io_at};
use crate::files::{OLD_SUFFIX, STAGING, remove_file_if_present, remove_staging};
use std::path::{Path, PathBuf};

/// Successful replacement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    /// New interface executable, to start before leaving the old one.
    pub desktop: PathBuf,
    /// Installed version.
    pub version: String,
    /// Non-fatal leftovers (staging folder not removed); retried at the next
    /// start by [`crate::cleanup`].
    pub warnings: Vec<String>,
}

/// One completed step, undone in reverse order on failure.
enum Step {
    /// `<name>` was renamed `<name>.old`.
    MovedAside(String),
    /// The extracted `<name>` was renamed into place.
    Placed(String),
}

/// Replaces the program files by the staged ones.
///
/// Leftover `.old` files are removed first; if one cannot be removed (an
/// older executable still running), nothing is changed.
///
/// # Errors
/// [`ErrorKind::Busy`] when a leftover `.old` cannot be removed;
/// [`ErrorKind::ReplaceFailed`] when a step failed and every file was
/// restored; [`ErrorKind::RollbackIncomplete`] when restoring failed too:
/// the message lists the files to check (mixed versions possible).
pub fn install(staged: Staged) -> Result<Installed, UpdateError> {
    let dir = staged.dir.clone();
    let members = staged.target.members();
    for member in &members {
        let old = dir.join(format!("{member}{OLD_SUFFIX}"));
        remove_file_if_present(&old).map_err(|e| {
            let error = fail(
                ErrorKind::Busy,
                format!(
                    "ancien fichier encore utilisé, rien n’a été remplacé : {}",
                    e.message()
                ),
            );
            discard_after(error, &staged)
        })?;
        let current = dir.join(member);
        if std::fs::symlink_metadata(&current).is_ok_and(|m| m.is_dir()) {
            let error = fail(
                ErrorKind::ReplaceFailed,
                format!(
                    "{} est un dossier : rien n’a été remplacé",
                    current.display()
                ),
            );
            return Err(discard_after(error, &staged));
        }
    }
    let mut steps = Vec::new();
    for member in &members {
        if let Err(error) = replace_one(&dir, member, &mut steps) {
            let error = rollback(&dir, steps, error);
            return Err(discard_after(error, &staged));
        }
    }
    let mut warnings = Vec::new();
    if let Err(e) = remove_staging(&dir, staged.target) {
        warnings.push(format!("dossier {STAGING} non retiré : {}", e.message()));
    }
    Ok(Installed {
        desktop: dir.join(staged.target.desktop_name()),
        version: staged.version,
        warnings,
    })
}

fn discard_after(error: UpdateError, staged: &Staged) -> UpdateError {
    match remove_staging(&staged.dir, staged.target) {
        Ok(()) => error,
        Err(extra) => error.also(format!(
            "dossier d’extraction non retiré : {}",
            extra.message()
        )),
    }
}

fn replace_one(dir: &Path, member: &str, steps: &mut Vec<Step>) -> Result<(), UpdateError> {
    let current = dir.join(member);
    let old = dir.join(format!("{member}{OLD_SUFFIX}"));
    let new = dir.join(STAGING).join(member);
    if std::fs::symlink_metadata(&current).is_ok() {
        std::fs::rename(&current, &old).map_err(|e| io_at("renommage de", &current, e))?;
        steps.push(Step::MovedAside(member.to_owned()));
    }
    std::fs::rename(&new, &current).map_err(|e| io_at("mise en place de", &current, e))?;
    steps.push(Step::Placed(member.to_owned()));
    Ok(())
}

fn rollback(dir: &Path, steps: Vec<Step>, cause: UpdateError) -> UpdateError {
    let mut problems = Vec::new();
    for step in steps.into_iter().rev() {
        let undone = match &step {
            Step::Placed(member) => {
                std::fs::rename(dir.join(member), dir.join(STAGING).join(member))
                    .map_err(|e| format!("{member} : nouvelle version restée en place ({e})"))
            }
            Step::MovedAside(member) => {
                std::fs::rename(dir.join(format!("{member}{OLD_SUFFIX}")), dir.join(member))
                    .map_err(|e| {
                        format!("{member} : ancienne version restée en {member}{OLD_SUFFIX} ({e})")
                    })
            }
        };
        if let Err(problem) = undone {
            problems.push(problem);
        }
    }
    if problems.is_empty() {
        fail(
            ErrorKind::ReplaceFailed,
            format!(
                "remplacement impossible ({}) ; tous les fichiers d’origine ont été restaurés",
                cause.message()
            ),
        )
    } else {
        fail(
            ErrorKind::RollbackIncomplete,
            format!(
                "remplacement impossible ({}) et restauration incomplète : le dossier {} peut mélanger deux versions. À vérifier : {}. Retéléchargez l’archive depuis la page de la release.",
                cause.message(),
                dir.display(),
                problems.join(" ; ")
            ),
        )
    }
}
