//! Strict extraction of a verified archive into the staging folder.
//!
//! Only `<root>/<member>` regular files are accepted (and the `<root>/`
//! folder entry itself), each exactly once. Names are compared byte for byte
//! with the expected ones, before any interpretation as a path: absolute
//! paths, `..`, backslashes, links, devices and extra or missing entries all
//! reject the whole archive. Nothing outside the staging folder is written.

use crate::error::{ErrorKind, UpdateError, fail, io_at};
use crate::files::{STAGING, remove_quietly, remove_staging};
use crate::http::{Downloaded, MAX_ARCHIVE_BYTES, Progress};
use crate::version::{ArchiveFormat, Target, short};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Largest accepted extracted member (guards against decompression bombs).
const MAX_MEMBER_BYTES: u64 = MAX_ARCHIVE_BYTES;

/// Files extracted and checked, ready for [`crate::install`].
#[derive(Debug)]
pub struct Staged {
    pub(crate) dir: PathBuf,
    pub(crate) target: Target,
    /// Version being installed.
    pub version: String,
}

impl Staged {
    /// Removes the extracted files without installing them.
    ///
    /// # Errors
    /// The staging folder could not be removed.
    pub fn discard(self) -> Result<(), UpdateError> {
        remove_staging(&self.dir, self.target)
    }
}

fn invalid(reason: impl std::fmt::Display) -> UpdateError {
    fail(
        ErrorKind::InvalidArchive,
        format!("archive refusée : {reason}"),
    )
}

/// Expected members and whether each one was already seen.
struct Plan<'a> {
    root: String,
    members: Vec<(String, bool)>,
    staging: &'a Path,
    target: Target,
}

impl Plan<'_> {
    /// Classifies an entry name: `Ok(None)` for the root folder entry,
    /// `Ok(Some(index))` for an expected, not yet seen member.
    fn classify(&mut self, name: &[u8], is_dir: bool) -> Result<Option<usize>, UpdateError> {
        let shown = || short(&String::from_utf8_lossy(name));
        if is_dir {
            let root = self.root.as_bytes();
            return if name == root || name.strip_suffix(b"/") == Some(root) {
                Ok(None)
            } else {
                Err(invalid(format!("dossier inattendu « {} »", shown())))
            };
        }
        let member = name
            .strip_prefix(self.root.as_bytes())
            .and_then(|rest| rest.strip_prefix(b"/"))
            .and_then(|rest| self.members.iter().position(|(m, _)| m.as_bytes() == rest))
            .ok_or_else(|| invalid(format!("élément inattendu « {} »", shown())))?;
        let seen = &mut self.members[member].1;
        if *seen {
            return Err(invalid(format!("élément répété « {} »", shown())));
        }
        *seen = true;
        Ok(Some(member))
    }

    /// Writes one member into the staging folder (exclusive creation).
    fn write(
        &self,
        index: usize,
        reader: &mut dyn Read,
        progress: &Progress,
    ) -> Result<(), UpdateError> {
        let name = &self.members[index].0;
        let path = self.staging.join(name);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| io_at("création de", &path, e))?;
        let mut limited = reader.take(MAX_MEMBER_BYTES + 1);
        let mut buffer = vec![0_u8; 64 * 1024];
        let mut written: u64 = 0;
        loop {
            progress.checkpoint()?;
            let read = limited
                .read(&mut buffer)
                .map_err(|e| invalid(format!("lecture de « {name} » : {e}")))?;
            if read == 0 {
                break;
            }
            written += read as u64;
            if written > MAX_MEMBER_BYTES {
                return Err(invalid(format!("« {name} » trop volumineux")));
            }
            file.write_all(buffer.get(..read).unwrap_or_default())
                .map_err(|e| io_at("écriture de", &path, e))?;
        }
        file.sync_all()
            .map_err(|e| io_at("écriture de", &path, e))?;
        set_mode(&path, self.target.is_binary(name))
    }

    fn complete(&self) -> Result<(), UpdateError> {
        match self.members.iter().find(|(_, seen)| !seen) {
            Some((name, _)) => Err(invalid(format!("« {name} » manquant"))),
            None => Ok(()),
        }
    }
}

#[cfg(unix)]
fn set_mode(path: &Path, binary: bool) -> Result<(), UpdateError> {
    use std::os::unix::fs::PermissionsExt;
    let mode = if binary { 0o755 } else { 0o644 };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .map_err(|e| io_at("droits de", path, e))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _binary: bool) -> Result<(), UpdateError> {
    Ok(())
}

/// Extracts the verified archive into `<dir>/syncengine-update.staging/`.
/// The downloaded archive is removed in every case; the staging folder is
/// removed on failure.
///
/// # Errors
/// [`ErrorKind::InvalidArchive`] for any unexpected, repeated or missing
/// entry; [`ErrorKind::Busy`] when the staging folder already exists;
/// [`ErrorKind::Cancelled`]; storage errors.
pub fn stage(downloaded: Downloaded, progress: &Progress) -> Result<Staged, UpdateError> {
    let staging = downloaded.dir.join(STAGING);
    if let Err(e) = std::fs::create_dir(&staging) {
        let error = if e.kind() == std::io::ErrorKind::AlreadyExists {
            fail(
                ErrorKind::Busy,
                format!(
                    "{} existe déjà : une autre mise à jour est en cours, ou un reste sera retiré au prochain démarrage",
                    staging.display()
                ),
            )
        } else {
            io_at("création de", &staging, e)
        };
        return Err(remove_quietly(&downloaded.path, error));
    }
    let target = downloaded.target;
    let mut plan = Plan {
        root: target.archive_root(&downloaded.version),
        members: target.members().into_iter().map(|m| (m, false)).collect(),
        staging: &staging,
        target,
    };
    let extracted = extract(&downloaded.path, target.format(), &mut plan, progress)
        .and_then(|()| plan.complete());
    let removed = crate::files::remove_file_if_present(&downloaded.path);
    let result = extracted.and(removed);
    match result {
        Ok(()) => Ok(Staged {
            dir: downloaded.dir,
            target,
            version: downloaded.version,
        }),
        Err(error) => match remove_staging(&downloaded.dir, target) {
            Ok(()) => Err(remove_quietly(&downloaded.path, error)),
            Err(extra) => Err(remove_quietly(&downloaded.path, error)
                .also(format!("dossier d’extraction non retiré : {extra}"))),
        },
    }
}

fn extract(
    archive: &Path,
    format: ArchiveFormat,
    plan: &mut Plan<'_>,
    progress: &Progress,
) -> Result<(), UpdateError> {
    let file = std::fs::File::open(archive).map_err(|e| io_at("ouverture de", archive, e))?;
    let reader = std::io::BufReader::new(file);
    match format {
        ArchiveFormat::Zip => extract_zip(reader, plan, progress),
        ArchiveFormat::TarGz => extract_tar(reader, plan, progress),
    }
}

fn extract_zip(
    reader: std::io::BufReader<std::fs::File>,
    plan: &mut Plan<'_>,
    progress: &Progress,
) -> Result<(), UpdateError> {
    let mut zip = zip::ZipArchive::new(reader).map_err(invalid)?;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).map_err(invalid)?;
        if entry.encrypted() {
            return Err(invalid("élément chiffré"));
        }
        if entry.is_symlink() {
            return Err(invalid(format!(
                "lien symbolique « {} »",
                short(&String::from_utf8_lossy(entry.name_raw()))
            )));
        }
        let name = entry.name_raw().to_vec();
        if let Some(member) = plan.classify(&name, entry.is_dir())? {
            if entry.size() > MAX_MEMBER_BYTES {
                return Err(invalid("élément trop volumineux"));
            }
            // Reading to the end also checks the CRC-32 of the entry.
            plan.write(member, &mut entry, progress)?;
        }
    }
    Ok(())
}

fn extract_tar(
    reader: std::io::BufReader<std::fs::File>,
    plan: &mut Plan<'_>,
    progress: &Progress,
) -> Result<(), UpdateError> {
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(reader));
    for entry in tar.entries().map_err(invalid)? {
        let mut entry = entry.map_err(invalid)?;
        let kind = entry.header().entry_type();
        let name = entry.path_bytes().into_owned();
        let is_dir = match kind {
            tar::EntryType::Regular => false,
            tar::EntryType::Directory => true,
            _ => {
                return Err(invalid(format!(
                    "« {} » n’est pas un fichier ordinaire",
                    short(&String::from_utf8_lossy(&name))
                )));
            }
        };
        if let Some(member) = plan.classify(&name, is_dir)? {
            plan.write(member, &mut entry, progress)?;
        }
    }
    Ok(())
}
