//! GitHub release lookup and streamed, verified download.
//!
//! Blocking `reqwest` (already used by the WebDAV and OneDrive connectors):
//! every call is meant to run on a worker thread, never on the interface
//! thread. No token or account is used; the public API allows 60 anonymous
//! requests per hour and address, which one check per opening respects.

use crate::error::{ErrorKind, UpdateError, fail, io_at};
use crate::files::{DOWNLOAD, remove_quietly};
use crate::release::{checksum_for, release_notes};
use crate::version::{Target, parse_tag, short};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

/// Repository whose releases are installed.
const REPOSITORY: &str = "sargo22341-prog/SyncEngine";
/// Largest accepted API answer.
const MAX_API_BYTES: u64 = 2 * 1024 * 1024;
/// Largest accepted archive (current archives weigh a few tens of MiB).
pub(crate) const MAX_ARCHIVE_BYTES: u64 = 1024 * 1024 * 1024;

/// Where releases are read and downloaded from.
#[derive(Debug, Clone)]
pub struct UpdateSource {
    latest_url: String,
    /// Every archive address must start with this prefix.
    download_prefix: String,
    /// Human release page, followed by `tag/<tag>`.
    page_prefix: String,
    client: reqwest::blocking::Client,
}

fn http_error(error: reqwest::Error) -> UpdateError {
    let kind = if error.is_connect() || error.is_timeout() {
        ErrorKind::Offline
    } else {
        ErrorKind::Http
    };
    // Redirected download addresses carry signed parameters: never shown.
    let message = match kind {
        ErrorKind::Offline => format!(
            "GitHub injoignable (hors ligne ?) : {}",
            error.without_url()
        ),
        _ => format!("échange HTTP impossible : {}", error.without_url()),
    };
    fail(kind, message)
}

impl UpdateSource {
    /// Public GitHub releases of SyncEngine, HTTPS only (redirects to the
    /// GitHub download servers are followed, still in HTTPS).
    ///
    /// # Errors
    /// [`ErrorKind::Http`] when the TLS client cannot be built.
    pub fn github() -> Result<Self, UpdateError> {
        Self::build(
            format!("https://api.github.com/repos/{REPOSITORY}/releases/latest"),
            format!("https://github.com/{REPOSITORY}/releases/download/"),
            format!("https://github.com/{REPOSITORY}/releases/"),
            true,
        )
    }

    /// Local test server (`http://127.0.0.1:<port>` or `http://localhost:<port>`)
    /// serving `/releases/latest` and archives under `/download/`. Refuses any
    /// other host, so that clear-text HTTP never reaches the network.
    ///
    /// # Errors
    /// [`ErrorKind::Unsupported`] for a non-loopback base.
    pub fn loopback(base: &str) -> Result<Self, UpdateError> {
        let base = base.trim_end_matches('/');
        let host = base.strip_prefix("http://").unwrap_or_default();
        let host = host.split(':').next().unwrap_or_default();
        if !matches!(host, "127.0.0.1" | "localhost") {
            return Err(fail(
                ErrorKind::Unsupported,
                "source locale limitée à 127.0.0.1 ou localhost",
            ));
        }
        Self::build(
            format!("{base}/releases/latest"),
            format!("{base}/download/"),
            format!("{base}/releases/"),
            false,
        )
    }

    fn build(
        latest_url: String,
        download_prefix: String,
        page_prefix: String,
        https: bool,
    ) -> Result<Self, UpdateError> {
        let mut builder = reqwest::blocking::Client::builder()
            .user_agent(concat!("SyncEngine/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(15))
            // Per read for a streamed body: a stalled download fails, a slow one continues.
            .timeout(Duration::from_secs(60))
            .redirect(reqwest::redirect::Policy::limited(5))
            .retry(reqwest::retry::never())
            .referer(false);
        if https {
            builder = builder
                .https_only(true)
                .tls_version_min(reqwest::tls::Version::TLS_1_2);
        } else {
            builder = builder.no_proxy();
        }
        Ok(Self {
            latest_url,
            download_prefix,
            page_prefix,
            client: builder.build().map_err(http_error)?,
        })
    }
}

/// Outcome of [`check`].
#[derive(Debug, Clone)]
pub enum Check {
    /// The latest release is not newer than this build.
    UpToDate {
        /// Latest published version.
        latest: String,
    },
    /// A newer release exists but has no archive for this target.
    NoAsset {
        /// Latest published version.
        latest: String,
    },
    /// A newer release with an archive for this target.
    Available(Box<Update>),
}

/// Newer release that can be installed on this target.
#[derive(Debug, Clone)]
pub struct Update {
    /// Version, tag without `v`.
    pub version: String,
    /// Publication time as published (ISO 8601, UTC), when present.
    pub published_at: Option<String>,
    /// Release notes without the checksum block.
    pub notes: String,
    /// Human release page (manual download).
    pub page_url: String,
    target: Target,
    asset_name: String,
    asset_url: String,
    asset_size: u64,
    checksum: Result<[u8; 32], UpdateError>,
}

impl Update {
    /// Archive file name.
    #[must_use]
    pub fn asset_name(&self) -> &str {
        &self.asset_name
    }
    /// Announced archive size in bytes.
    #[must_use]
    pub fn asset_size(&self) -> u64 {
        self.asset_size
    }
    /// Why the published checksum is unusable (installing is then refused).
    #[must_use]
    pub fn checksum_problem(&self) -> Option<&UpdateError> {
        self.checksum.as_ref().err()
    }
}

#[derive(serde::Deserialize)]
struct ApiRelease {
    tag_name: String,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<ApiAsset>,
}

#[derive(serde::Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

fn status_error(response: &reqwest::blocking::Response) -> UpdateError {
    let status = response.status();
    let exhausted = response
        .headers()
        .get("x-ratelimit-remaining")
        .is_some_and(|v| v.as_bytes() == b"0");
    if status.as_u16() == 429 || (status.as_u16() == 403 && exhausted) {
        fail(
            ErrorKind::RateLimited,
            "limite de requêtes GitHub atteinte : réessayez dans une heure",
        )
    } else {
        fail(
            ErrorKind::Http,
            format!("GitHub a répondu {}", status.as_u16()),
        )
    }
}

/// Reads the latest published release and compares it with `current`.
///
/// Network, quota and format failures are errors, never « up to date ».
///
/// # Errors
/// [`ErrorKind::Offline`], [`ErrorKind::RateLimited`], [`ErrorKind::Http`],
/// [`ErrorKind::NoRelease`], or [`ErrorKind::InvalidRelease`] (invalid JSON or
/// tag, draft or pre-release, archive address outside the source).
pub fn check(source: &UpdateSource, current: &str, target: Target) -> Result<Check, UpdateError> {
    let response = source
        .client
        .get(&source.latest_url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .map_err(http_error)?;
    if response.status().as_u16() == 404 {
        return Err(fail(ErrorKind::NoRelease, "aucune version publiée"));
    }
    if !response.status().is_success() {
        return Err(status_error(&response));
    }
    let mut bytes = Vec::new();
    response
        .take(MAX_API_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| fail(ErrorKind::Offline, format!("réponse interrompue : {e}")))?;
    if bytes.len() as u64 > MAX_API_BYTES {
        return Err(fail(
            ErrorKind::InvalidRelease,
            "réponse GitHub trop volumineuse",
        ));
    }
    let release: ApiRelease = serde_json::from_slice(&bytes).map_err(|e| {
        fail(
            ErrorKind::InvalidRelease,
            format!("réponse GitHub illisible : {e}"),
        )
    })?;
    evaluate(source, release, current, target)
}

fn evaluate(
    source: &UpdateSource,
    release: ApiRelease,
    current: &str,
    target: Target,
) -> Result<Check, UpdateError> {
    if release.draft || release.prerelease {
        return Err(fail(
            ErrorKind::InvalidRelease,
            "la dernière release est un brouillon ou une pré-version",
        ));
    }
    let latest = parse_tag(&release.tag_name)?;
    let version = latest.to_string();
    if latest <= parse_tag(current)? {
        return Ok(Check::UpToDate { latest: version });
    }
    let asset_name = target.asset_name(&version);
    let Some(asset) = release.assets.into_iter().find(|a| a.name == asset_name) else {
        return Ok(Check::NoAsset { latest: version });
    };
    if !asset
        .browser_download_url
        .starts_with(&source.download_prefix)
    {
        return Err(fail(
            ErrorKind::InvalidRelease,
            "adresse de l’archive hors du dépôt attendu",
        ));
    }
    let body = release.body.unwrap_or_default();
    Ok(Check::Available(Box::new(Update {
        published_at: release.published_at,
        notes: release_notes(&body),
        page_url: format!("{}tag/{}", source.page_prefix, release.tag_name),
        checksum: checksum_for(&body, &asset_name),
        target,
        asset_url: asset.browser_download_url,
        asset_size: asset.size,
        asset_name,
        version,
    })))
}

/// Shared download progress and cancellation, polled by the interface.
#[derive(Debug, Default)]
pub struct Progress {
    done: AtomicU64,
    total: AtomicU64,
    cancel: AtomicBool,
}

impl Progress {
    /// Asks the running download or extraction to stop at the next block.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
    /// Whether cancellation was requested.
    #[must_use]
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }
    /// Bytes received so far.
    #[must_use]
    pub fn done(&self) -> u64 {
        self.done.load(Ordering::Relaxed)
    }
    /// Expected bytes (announced archive size).
    #[must_use]
    pub fn total(&self) -> u64 {
        self.total.load(Ordering::Relaxed)
    }
    pub(crate) fn checkpoint(&self) -> Result<(), UpdateError> {
        if self.cancelled() {
            Err(fail(ErrorKind::Cancelled, "mise à jour annulée"))
        } else {
            Ok(())
        }
    }
}

/// Archive downloaded into the program folder, size and SHA-256 verified.
#[derive(Debug)]
pub struct Downloaded {
    pub(crate) path: PathBuf,
    pub(crate) dir: PathBuf,
    pub(crate) target: Target,
    pub(crate) version: String,
}

impl Downloaded {
    /// Removes the downloaded archive without installing it.
    ///
    /// # Errors
    /// The file could not be removed.
    pub fn discard(self) -> Result<(), UpdateError> {
        crate::files::remove_file_if_present(&self.path)
    }
}

/// Streams the archive of `update` into `<dir>/syncengine-update.download`
/// (created exclusively), then verifies its size and SHA-256. On any failure,
/// including cancellation, the file is removed.
///
/// # Errors
/// [`ErrorKind::ChecksumMissing`] before any transfer when no usable checksum
/// is published; [`ErrorKind::Busy`] when the temporary file already exists;
/// [`ErrorKind::NotWritable`]; network errors as in [`check`];
/// [`ErrorKind::ChecksumMismatch`]; [`ErrorKind::Cancelled`].
pub fn download(
    source: &UpdateSource,
    update: &Update,
    dir: &Path,
    progress: &Progress,
) -> Result<Downloaded, UpdateError> {
    let expected = update.checksum.clone()?;
    if !update.asset_url.starts_with(&source.download_prefix) {
        return Err(fail(
            ErrorKind::InvalidRelease,
            "adresse de l’archive hors du dépôt attendu",
        ));
    }
    if update.asset_size == 0 || update.asset_size > MAX_ARCHIVE_BYTES {
        return Err(fail(
            ErrorKind::InvalidRelease,
            "taille d’archive annoncée invalide",
        ));
    }
    progress.total.store(update.asset_size, Ordering::Relaxed);
    progress.done.store(0, Ordering::Relaxed);
    let path = dir.join(DOWNLOAD);
    let file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(fail(
                ErrorKind::Busy,
                format!(
                    "{} existe déjà : une autre mise à jour est en cours, ou un reste sera retiré au prochain démarrage",
                    path.display()
                ),
            ));
        }
        Err(e) => return Err(io_at("création de", &path, e)),
    };
    match transfer(source, update, file, &path, progress, &expected) {
        Ok(()) => Ok(Downloaded {
            path,
            dir: dir.to_path_buf(),
            target: update.target,
            version: update.version.clone(),
        }),
        Err(error) => Err(remove_quietly(&path, error)),
    }
}

fn transfer(
    source: &UpdateSource,
    update: &Update,
    mut file: std::fs::File,
    path: &Path,
    progress: &Progress,
    expected: &[u8; 32],
) -> Result<(), UpdateError> {
    progress.checkpoint()?;
    let mut response = source
        .client
        .get(&update.asset_url)
        .header("Accept", "application/octet-stream")
        .send()
        .map_err(http_error)?;
    if !response.status().is_success() {
        return Err(status_error(&response));
    }
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    let mut received: u64 = 0;
    loop {
        progress.checkpoint()?;
        let read = response.read(&mut buffer).map_err(|e| {
            fail(
                ErrorKind::Offline,
                format!("téléchargement interrompu : {e}"),
            )
        })?;
        if read == 0 {
            break;
        }
        received += read as u64;
        if received > update.asset_size {
            return Err(fail(
                ErrorKind::ChecksumMismatch,
                "archive plus grande que la taille annoncée",
            ));
        }
        let chunk = buffer.get(..read).unwrap_or_default();
        hasher.update(chunk);
        file.write_all(chunk)
            .map_err(|e| io_at("écriture de", path, e))?;
        progress.done.store(received, Ordering::Relaxed);
    }
    if received != update.asset_size {
        return Err(fail(
            ErrorKind::ChecksumMismatch,
            format!(
                "archive incomplète : {received} octets reçus sur {}",
                update.asset_size
            ),
        ));
    }
    file.sync_all().map_err(|e| io_at("écriture de", path, e))?;
    let actual: [u8; 32] = hasher.finalize().into();
    if &actual != expected {
        return Err(fail(
            ErrorKind::ChecksumMismatch,
            format!(
                "empreinte SHA-256 de « {} » différente de celle publiée : archive refusée",
                short(&update.asset_name)
            ),
        ));
    }
    Ok(())
}
