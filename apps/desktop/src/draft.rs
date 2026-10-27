//! Editable drafts behind the step-by-step assistant. They convert to the
//! engine types (`Profile`, `Trigger`) and surface validation in French; the
//! facade repeats every check (roots, disjointness, consents) on save.

use crate::icons::Glyph;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use sync_core::ProfileId;
use sync_filter::FilterRules;
use sync_plan::SyncMode;
use sync_profiles::{CompareMethod, Profile, RemoteEndpoint};
use sync_scheduler::{MissedJobs, Schedule, Trigger};

/// Endpoint natures, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Provider {
    Local,
    Smb,
    Sftp,
    Webdav,
    Ftps,
    Ftp,
    Onedrive,
}

impl Provider {
    pub(crate) const ALL: [Self; 7] = [
        Self::Local,
        Self::Smb,
        Self::Sftp,
        Self::Webdav,
        Self::Ftps,
        Self::Ftp,
        Self::Onedrive,
    ];

    /// Key used by the facade's `providers()` list.
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Smb => "smb",
            Self::Sftp => "sftp",
            Self::Webdav => "webdav",
            Self::Ftps => "ftps",
            Self::Ftp => "ftp",
            Self::Onedrive => "onedrive",
        }
    }
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Local => "Dossier local",
            Self::Smb => "Partage réseau",
            Self::Sftp => "SFTP / SSH",
            Self::Webdav => "WebDAV",
            Self::Ftps => "FTPS",
            Self::Ftp => "FTP",
            Self::Onedrive => "OneDrive",
        }
    }
    pub(crate) fn hint(self) -> &'static str {
        match self {
            Self::Local => "Disque interne, USB, dossier monté",
            Self::Smb => "Chemin UNC ou partage monté",
            Self::Sftp => "Serveur SSH, clé via l’agent",
            Self::Webdav => "Collection HTTPS, jeton externe",
            Self::Ftps => "FTP chiffré par TLS",
            Self::Ftp => "Non chiffré, à éviter",
            Self::Onedrive => "Source expérimentale, lecture seule",
        }
    }
    pub(crate) fn glyph(self) -> Glyph {
        match self {
            Self::Local => Glyph::Folder,
            Self::Smb => Glyph::Network,
            Self::Sftp => Glyph::Terminal,
            Self::Webdav => Glyph::Globe,
            Self::Ftps => Glyph::Lock,
            Self::Ftp => Glyph::Server,
            Self::Onedrive => Glyph::Cloud,
        }
    }
    fn default_port(self) -> &'static str {
        match self {
            Self::Sftp => "22",
            Self::Ftp | Self::Ftps => "21",
            _ => "",
        }
    }
}

/// Field identifiers of the endpoint form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Field {
    Path,
    Ip,
    Port,
    HostName,
    Username,
    KnownHosts,
    Root,
    Url,
    CredentialRef,
    TlsDomain,
    RootCertificate,
    DriveId,
    DownloadHosts,
}

/// Form state of one side.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct EndpointDraft {
    pub(crate) provider: Option<Provider>,
    pub(crate) path: String,
    pub(crate) smb_root: String,
    pub(crate) ip: String,
    pub(crate) port: String,
    pub(crate) host_name: String,
    pub(crate) username: String,
    pub(crate) known_hosts: String,
    pub(crate) root: String,
    pub(crate) url: String,
    pub(crate) credential_ref: String,
    pub(crate) tls_domain: String,
    pub(crate) root_certificate: String,
    pub(crate) drive_id: String,
    pub(crate) download_hosts: String,
    pub(crate) acknowledge_plaintext: bool,
    pub(crate) exclusive_writers: bool,
}

impl EndpointDraft {
    pub(crate) fn from_profile(local: &Path, remote: Option<&RemoteEndpoint>) -> Self {
        let mut d = Self {
            path: local.to_str().unwrap_or("").to_owned(),
            provider: Some(Provider::Local),
            ..Self::default()
        };
        let split = |a: &SocketAddr| (a.ip().to_string(), a.port().to_string());
        match remote {
            None => {}
            Some(RemoteEndpoint::Smb { root }) => {
                d.provider = Some(Provider::Smb);
                d.smb_root = root.to_str().unwrap_or("").to_owned();
            }
            Some(RemoteEndpoint::Sftp {
                address,
                host,
                username,
                known_hosts,
                root,
            }) => {
                d.provider = Some(Provider::Sftp);
                (d.ip, d.port) = split(address);
                d.host_name.clone_from(host);
                d.username.clone_from(username);
                d.known_hosts = known_hosts.to_str().unwrap_or("").to_owned();
                d.root.clone_from(root);
            }
            Some(RemoteEndpoint::Webdav {
                root,
                credential_ref,
            }) => {
                d.provider = Some(Provider::Webdav);
                d.url.clone_from(root);
                d.credential_ref.clone_from(credential_ref);
            }
            Some(RemoteEndpoint::Ftp {
                address,
                username,
                root,
                credential_ref,
                acknowledge_plaintext,
                exclusive_writers,
            }) => {
                d.provider = Some(Provider::Ftp);
                (d.ip, d.port) = split(address);
                d.username.clone_from(username);
                d.root.clone_from(root);
                d.credential_ref.clone_from(credential_ref);
                d.acknowledge_plaintext = *acknowledge_plaintext;
                d.exclusive_writers = *exclusive_writers;
            }
            Some(RemoteEndpoint::Ftps {
                address,
                tls_domain,
                username,
                root,
                credential_ref,
                root_certificate,
                exclusive_writers,
            }) => {
                d.provider = Some(Provider::Ftps);
                (d.ip, d.port) = split(address);
                d.tls_domain.clone_from(tls_domain);
                d.username.clone_from(username);
                d.root.clone_from(root);
                d.credential_ref.clone_from(credential_ref);
                d.root_certificate = root_certificate
                    .as_ref()
                    .and_then(|p| p.to_str())
                    .unwrap_or("")
                    .to_owned();
                d.exclusive_writers = *exclusive_writers;
            }
            Some(RemoteEndpoint::Onedrive {
                drive_id,
                download_hosts,
                credential_ref,
            }) => {
                d.provider = Some(Provider::Onedrive);
                d.drive_id.clone_from(drive_id);
                d.download_hosts = download_hosts.join("\n");
                d.credential_ref.clone_from(credential_ref);
            }
        }
        d
    }

    /// Empty local folder, the most common choice.
    pub(crate) fn local() -> Self {
        Self {
            provider: Some(Provider::Local),
            ..Self::default()
        }
    }

    pub(crate) fn select(&mut self, provider: Provider) {
        if self.port.is_empty() || self.provider.is_some_and(|p| p.default_port() == self.port) {
            provider.default_port().clone_into(&mut self.port);
        }
        self.provider = Some(provider);
    }

    pub(crate) fn value(&self, field: Field) -> &str {
        match field {
            Field::Path => &self.path,
            Field::Ip => &self.ip,
            Field::Port => &self.port,
            Field::HostName => &self.host_name,
            Field::Username => &self.username,
            Field::KnownHosts => &self.known_hosts,
            Field::Root => &self.root,
            Field::Url => &self.url,
            Field::CredentialRef => &self.credential_ref,
            Field::TlsDomain => &self.tls_domain,
            Field::RootCertificate => &self.root_certificate,
            Field::DriveId => &self.drive_id,
            Field::DownloadHosts => &self.download_hosts,
        }
    }

    pub(crate) fn set(&mut self, field: Field, value: String) {
        let value = if field == Field::CredentialRef {
            value.to_ascii_uppercase()
        } else {
            value
        };
        match field {
            Field::Path => {
                if self.provider == Some(Provider::Smb) {
                    self.smb_root = value;
                } else {
                    self.path = value;
                }
            }
            Field::Ip => self.ip = value,
            Field::Port => self.port = value,
            Field::HostName => self.host_name = value,
            Field::Username => self.username = value,
            Field::KnownHosts => self.known_hosts = value,
            Field::Root => self.root = value,
            Field::Url => self.url = value,
            Field::CredentialRef => self.credential_ref = value,
            Field::TlsDomain => self.tls_domain = value,
            Field::RootCertificate => self.root_certificate = value,
            Field::DriveId => self.drive_id = value,
            Field::DownloadHosts => self.download_hosts = value,
        }
    }

    /// Path shown in the local/SMB path field.
    pub(crate) fn path_value(&self) -> &str {
        if self.provider == Some(Provider::Smb) {
            &self.smb_root
        } else {
            &self.path
        }
    }

    fn address(&self) -> Result<SocketAddr, String> {
        let ip: IpAddr = self
            .ip
            .trim()
            .parse()
            .map_err(|_| "Adresse IP du serveur invalide (ex. 192.168.1.10)".to_owned())?;
        let port: u16 = self
            .port
            .trim()
            .parse()
            .ok()
            .filter(|p| *p > 0)
            .ok_or("Port invalide (1 à 65535)")?;
        Ok(SocketAddr::new(ip, port))
    }

    /// Builds the local path and optional remote endpoint of this side.
    /// # Errors
    /// French message for the first invalid field.
    pub(crate) fn build(&self) -> Result<(PathBuf, Option<RemoteEndpoint>), String> {
        let provider = self
            .provider
            .ok_or("Choisissez la nature de l’emplacement")?;
        let required = |v: &str, what: &str| {
            if v.trim().is_empty() {
                Err(format!("{what} requis"))
            } else {
                Ok(v.trim().to_owned())
            }
        };
        let absolute = |v: &str, what: &str| -> Result<PathBuf, String> {
            let p = PathBuf::from(required(v, what)?);
            if p.is_absolute() {
                Ok(p)
            } else {
                Err(format!("{what} : chemin absolu requis"))
            }
        };
        let remote_root = |v: &str| -> Result<String, String> {
            let r = required(v, "Dossier distant")?;
            if r.starts_with('/')
                && !r.split('/').any(|s| s == ".." || s == ".")
                && !r.contains('\\')
            {
                Ok(r)
            } else {
                Err("Dossier distant : chemin POSIX absolu, ex. /sauvegardes".into())
            }
        };
        let reference = |v: &str| -> Result<String, String> {
            let r = required(v, "Référence du secret")?;
            if r.len() <= 64
                && r.bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
            {
                Ok(r)
            } else {
                Err("Référence du secret : lettres majuscules, chiffres et _ uniquement".into())
            }
        };
        let local = PathBuf::from(self.path.trim());
        let remote = match provider {
            Provider::Local => return Ok((absolute(&self.path, "Dossier")?, None)),
            Provider::Smb => {
                let root = absolute(&self.smb_root, "Partage")?;
                // The engine opens Windows shares by UNC only; a drive letter
                // (even a mapped one) would be refused at simulation time.
                if cfg!(windows) && !is_unc(&root) {
                    return Err("Partage : chemin réseau \\\\serveur\\partage requis ; pour un disque de ce PC, choisissez « Dossier local »".into());
                }
                RemoteEndpoint::Smb { root }
            }
            Provider::Sftp => RemoteEndpoint::Sftp {
                address: self.address()?,
                host: required(&self.host_name, "Nom d’hôte vérifié")?,
                username: required(&self.username, "Utilisateur")?,
                known_hosts: absolute(&self.known_hosts, "Fichier known_hosts")?,
                root: remote_root(&self.root)?,
            },
            Provider::Webdav => {
                let url = required(&self.url, "URL")?;
                if !url.starts_with("https://") {
                    return Err("URL WebDAV : https:// requis".into());
                }
                RemoteEndpoint::Webdav {
                    root: url,
                    credential_ref: reference(&self.credential_ref)?,
                }
            }
            Provider::Ftp => {
                if !self.acknowledge_plaintext {
                    return Err("FTP : acceptez explicitement la transmission non chiffrée".into());
                }
                RemoteEndpoint::Ftp {
                    address: self.address()?,
                    username: required(&self.username, "Utilisateur")?,
                    root: remote_root(&self.root)?,
                    credential_ref: reference(&self.credential_ref)?,
                    acknowledge_plaintext: true,
                    exclusive_writers: self.exclusive_writers,
                }
            }
            Provider::Ftps => RemoteEndpoint::Ftps {
                address: self.address()?,
                tls_domain: required(&self.tls_domain, "Nom du certificat TLS")?,
                username: required(&self.username, "Utilisateur")?,
                root: remote_root(&self.root)?,
                credential_ref: reference(&self.credential_ref)?,
                root_certificate: if self.root_certificate.trim().is_empty() {
                    None
                } else {
                    Some(absolute(&self.root_certificate, "Certificat CA")?)
                },
                exclusive_writers: self.exclusive_writers,
            },
            Provider::Onedrive => RemoteEndpoint::Onedrive {
                drive_id: required(&self.drive_id, "Identifiant du lecteur")?,
                download_hosts: self
                    .download_hosts
                    .split(|c: char| c == '\n' || c == ',' || c.is_whitespace())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect(),
                credential_ref: reference(&self.credential_ref)?,
            },
        };
        remote
            .validate()
            .map_err(|_| "Configuration distante refusée par le moteur".to_owned())?;
        Ok((local, Some(remote)))
    }

    /// Last path component, used to suggest a profile name.
    pub(crate) fn short_name(&self) -> String {
        let raw = match self.provider {
            Some(Provider::Local) | None => self.path.as_str(),
            Some(Provider::Smb) => self.smb_root.as_str(),
            Some(Provider::Webdav) => self.url.as_str(),
            Some(Provider::Onedrive) => "OneDrive",
            _ => self.root.as_str(),
        };
        raw.trim_end_matches(['/', '\\'])
            .rsplit(['/', '\\'])
            .find(|s| !s.is_empty())
            .unwrap_or(raw)
            .to_owned()
    }
}

fn is_unc(path: &Path) -> bool {
    use std::path::{Component, Prefix};
    matches!(path.components().next(), Some(Component::Prefix(p))
        if matches!(p.kind(), Prefix::UNC(..) | Prefix::VerbatimUNC(..)))
}

/// Quick filter presets offered in the assistant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Preset {
    pub(crate) label: &'static str,
    pub(crate) hint: &'static str,
    pub(crate) files: &'static [&'static str],
    pub(crate) directories: &'static [&'static str],
}

pub(crate) const PRESETS: [Preset; 4] = [
    Preset {
        label: "Fichiers temporaires",
        hint: "*.tmp, ~$*, *.part…",
        files: &["*.tmp", "~$*", "*.part", "*.swp"],
        directories: &[],
    },
    Preset {
        label: "Fichiers système",
        hint: "Thumbs.db, desktop.ini, .DS_Store",
        files: &["Thumbs.db", "desktop.ini", ".DS_Store"],
        directories: &[],
    },
    Preset {
        label: "Dossiers de développement",
        hint: "node_modules, target, .git",
        files: &[],
        directories: &["node_modules", "target", ".git"],
    },
    Preset {
        label: "Caches et corbeilles",
        hint: ".cache, $RECYCLE.BIN…",
        files: &[],
        directories: &[".cache", "$RECYCLE.BIN", "System Volume Information"],
    },
];

/// Which filter list a pattern belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FilterList {
    ExcludeFiles,
    ExcludeDirectories,
    IncludeFiles,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FilterDraft {
    pub(crate) include_files: Vec<String>,
    pub(crate) exclude_files: Vec<String>,
    pub(crate) exclude_directories: Vec<String>,
    pub(crate) case_insensitive: bool,
}

impl FilterDraft {
    pub(crate) fn from_rules(rules: &FilterRules) -> Self {
        Self {
            include_files: rules.include_files().to_vec(),
            exclude_files: rules.exclude_files().to_vec(),
            exclude_directories: rules.exclude_directories().to_vec(),
            case_insensitive: rules.case_insensitive(),
        }
    }
    pub(crate) fn rules(&self) -> FilterRules {
        let mut rules = FilterRules::new().with_case_insensitive(self.case_insensitive);
        for p in &self.include_files {
            rules = rules.include_file(p.clone());
        }
        for p in &self.exclude_files {
            rules = rules.exclude_file(p.clone());
        }
        for p in &self.exclude_directories {
            rules = rules.exclude_directory(p.clone());
        }
        rules
    }
    pub(crate) fn list(&self, list: FilterList) -> &[String] {
        match list {
            FilterList::ExcludeFiles => &self.exclude_files,
            FilterList::ExcludeDirectories => &self.exclude_directories,
            FilterList::IncludeFiles => &self.include_files,
        }
    }
    fn list_mut(&mut self, list: FilterList) -> &mut Vec<String> {
        match list {
            FilterList::ExcludeFiles => &mut self.exclude_files,
            FilterList::ExcludeDirectories => &mut self.exclude_directories,
            FilterList::IncludeFiles => &mut self.include_files,
        }
    }
    pub(crate) fn add(&mut self, list: FilterList, pattern: &str) -> bool {
        let pattern = pattern.trim();
        let items = self.list_mut(list);
        if pattern.is_empty() || items.iter().any(|p| p == pattern) {
            return false;
        }
        items.push(pattern.to_owned());
        true
    }
    pub(crate) fn remove(&mut self, list: FilterList, index: usize) {
        let items = self.list_mut(list);
        if index < items.len() {
            items.remove(index);
        }
    }
    pub(crate) fn has_preset(&self, preset: &Preset) -> bool {
        preset
            .files
            .iter()
            .all(|p| self.exclude_files.iter().any(|x| x == p))
            && preset
                .directories
                .iter()
                .all(|p| self.exclude_directories.iter().any(|x| x == p))
    }
    pub(crate) fn toggle_preset(&mut self, preset: &Preset) {
        if self.has_preset(preset) {
            self.exclude_files
                .retain(|p| !preset.files.contains(&p.as_str()));
            self.exclude_directories
                .retain(|p| !preset.directories.contains(&p.as_str()));
        } else {
            for p in preset.files {
                self.add(FilterList::ExcludeFiles, p);
            }
            for p in preset.directories {
                self.add(FilterList::ExcludeDirectories, p);
            }
        }
    }
    pub(crate) fn count(&self) -> usize {
        self.include_files.len() + self.exclude_files.len() + self.exclude_directories.len()
    }
}

/// Trigger family chosen in the planning step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TriggerKind {
    Manual,
    Interval,
    Daily,
    Logon,
    LogonDelayed,
    ManagerStart,
}

impl TriggerKind {
    pub(crate) fn available(self) -> bool {
        match self {
            Self::Logon | Self::LogonDelayed => cfg!(windows),
            Self::ManagerStart => cfg!(target_os = "linux"),
            _ => true,
        }
    }
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Manual => "Manuellement",
            Self::Interval => "À intervalle régulier",
            Self::Daily => "Chaque jour",
            Self::Logon => "À l’ouverture de session",
            Self::LogonDelayed => "Après l’ouverture de session",
            Self::ManagerStart => "Au démarrage de la session",
        }
    }
    pub(crate) fn hint(self) -> &'static str {
        match self {
            Self::Manual => "Vous lancez vous-même",
            Self::Interval => "Toutes les N minutes",
            Self::Daily => "À heure fixe (UTC)",
            Self::Logon => "Windows, sans droits admin",
            Self::LogonDelayed => "Windows, avec un délai",
            Self::ManagerStart => "Gestionnaire utilisateur Linux",
        }
    }
    pub(crate) fn glyph(self) -> Glyph {
        match self {
            Self::Manual => Glyph::Play,
            Self::Interval => Glyph::Refresh,
            Self::Daily => Glyph::Calendar,
            Self::Logon | Self::LogonDelayed | Self::ManagerStart => Glyph::Monitor,
        }
    }
    pub(crate) const ALL: [Self; 6] = [
        Self::Manual,
        Self::Interval,
        Self::Daily,
        Self::Logon,
        Self::LogonDelayed,
        Self::ManagerStart,
    ];
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScheduleDraft {
    pub(crate) kind: TriggerKind,
    pub(crate) interval_minutes: String,
    pub(crate) hour: u8,
    pub(crate) minute: u8,
    pub(crate) delay_minutes: String,
    pub(crate) skip_missed: bool,
    pub(crate) approve_writes: bool,
    pub(crate) approve_deletions: bool,
    /// Presentation preference, not a consent: kept from the existing schedule.
    pub(crate) show_window: bool,
}

/// Whether the agent can open the follow-up window (see `syncengine-agent`):
/// Windows tasks run in the interactive session, systemd units do not.
pub(crate) fn window_supported() -> bool {
    cfg!(windows)
}

impl Default for ScheduleDraft {
    fn default() -> Self {
        Self {
            kind: TriggerKind::Manual,
            interval_minutes: "60".into(),
            hour: 12,
            minute: 0,
            delay_minutes: "2".into(),
            skip_missed: false,
            approve_writes: false,
            approve_deletions: false,
            show_window: false,
        }
    }
}

impl ScheduleDraft {
    /// Keeps trigger/display settings but never reuses stale authorizations.
    pub(crate) fn for_profile(profile: &Profile, schedule: Option<&Schedule>) -> Self {
        let mut draft = Self::from_state(schedule);
        if schedule.is_some_and(|s| s.approved != *profile) {
            draft.approve_writes = false;
            draft.approve_deletions = false;
        }
        draft
    }
    /// Restores an existing authorization for an unchanged profile.
    /// Callers must exclude stale authorizations.
    pub(crate) fn from_state(schedule: Option<&Schedule>) -> Self {
        let mut d = Self::default();
        let Some(schedule) = schedule else { return d };
        d.approve_writes = schedule.enabled;
        d.approve_deletions = schedule.enabled && schedule.allow_deletions;
        d.skip_missed = schedule.missed == MissedJobs::Skip;
        d.show_window = schedule.show_window && window_supported();
        match schedule.trigger {
            Trigger::Interval { seconds } => {
                d.kind = TriggerKind::Interval;
                d.interval_minutes = (seconds / 60).max(1).to_string();
            }
            Trigger::DailyUtc { hour, minute } => {
                d.kind = TriggerKind::Daily;
                d.hour = hour;
                d.minute = minute;
            }
            Trigger::Logon => d.kind = TriggerKind::Logon,
            Trigger::LogonDelayed { seconds } => {
                d.kind = TriggerKind::LogonDelayed;
                d.delay_minutes = (seconds / 60).max(1).to_string();
            }
            Trigger::UserManagerStart => d.kind = TriggerKind::ManagerStart,
        }
        d
    }

    /// `Ok(None)` means manual only.
    /// # Errors
    /// Invalid numbers or unavailable trigger on this OS.
    pub(crate) fn trigger(&self) -> Result<Option<Trigger>, String> {
        if !self.kind.available() {
            return Err("Déclencheur indisponible sur ce système".into());
        }
        let minutes = |v: &str, max: u32, what: &str| -> Result<u32, String> {
            v.trim()
                .parse::<u32>()
                .ok()
                .filter(|m| (1..=max).contains(m))
                .ok_or_else(|| format!("{what} : entre 1 et {max} minutes"))
        };
        Ok(match self.kind {
            TriggerKind::Manual => None,
            TriggerKind::Interval => Some(Trigger::Interval {
                seconds: minutes(&self.interval_minutes, 525_600, "Intervalle")? * 60,
            }),
            TriggerKind::Daily => {
                if self.hour > 23 || self.minute > 59 {
                    return Err("Heure invalide".into());
                }
                Some(Trigger::DailyUtc {
                    hour: self.hour,
                    minute: self.minute,
                })
            }
            TriggerKind::Logon => Some(Trigger::Logon),
            TriggerKind::LogonDelayed => Some(Trigger::LogonDelayed {
                seconds: minutes(&self.delay_minutes, 1_440, "Délai")? * 60,
            }),
            TriggerKind::ManagerStart => Some(Trigger::UserManagerStart),
        })
    }

    pub(crate) fn missed(&self) -> MissedJobs {
        if self.skip_missed {
            MissedJobs::Skip
        } else {
            MissedJobs::RunOnce
        }
    }
}

/// French description of a trigger.
pub(crate) fn trigger_label(trigger: &Trigger) -> String {
    match trigger {
        Trigger::Interval { seconds } => {
            let m = seconds / 60;
            if m % 1440 == 0 {
                format!("Tous les {} jour(s)", m / 1440)
            } else if m % 60 == 0 {
                format!("Toutes les {} h", m / 60)
            } else {
                format!("Toutes les {m} min")
            }
        }
        Trigger::DailyUtc { hour, minute } => format!("Chaque jour à {hour:02}:{minute:02} UTC"),
        Trigger::Logon => "À l’ouverture de session Windows".into(),
        Trigger::LogonDelayed { seconds } => {
            format!("{} min après l’ouverture de session", seconds / 60)
        }
        Trigger::UserManagerStart => "Au démarrage du gestionnaire utilisateur".into(),
    }
}

/// Next timed occurrence in Unix seconds (presentation only; the agent
/// remains the authority and polls about once a minute).
pub(crate) fn next_occurrence(trigger: &Trigger, created_at: i64, now: i64) -> Option<i64> {
    match trigger {
        Trigger::Interval { seconds } => {
            let period = i64::from(*seconds).max(60);
            if now < created_at {
                return Some(created_at);
            }
            let k = (now - created_at) / period + 1;
            Some(created_at + k * period)
        }
        Trigger::DailyUtc { hour, minute } => {
            let day = now.div_euclid(86_400) * 86_400;
            let at = day + i64::from(*hour) * 3600 + i64::from(*minute) * 60;
            Some(if at > now { at } else { at + 86_400 })
        }
        _ => None,
    }
}

/// Whole assistant state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProfileDraft {
    pub(crate) existing: Option<ProfileId>,
    pub(crate) name: String,
    pub(crate) group: String,
    pub(crate) mode: Option<SyncMode>,
    pub(crate) source: EndpointDraft,
    pub(crate) destination: EndpointDraft,
    pub(crate) filters: FilterDraft,
    pub(crate) allow_delete: bool,
    /// One-way identity rule; bidirectional profiles always use content.
    pub(crate) compare: CompareMethod,
    pub(crate) schedule: ScheduleDraft,
    pub(crate) had_schedule: bool,
}

impl ProfileDraft {
    pub(crate) fn new(group: &str) -> Self {
        Self {
            existing: None,
            name: String::new(),
            group: group.to_owned(),
            mode: None,
            source: EndpointDraft::local(),
            destination: EndpointDraft::local(),
            filters: FilterDraft::default(),
            allow_delete: false,
            // New profiles: listings only, like FreeFileSync's default.
            compare: CompareMethod::SizeAndDate,
            schedule: ScheduleDraft::default(),
            had_schedule: false,
        }
    }

    pub(crate) fn edit(profile: &Profile, schedule: Option<&Schedule>) -> Self {
        Self {
            existing: Some(profile.id.clone()),
            name: profile.name.clone(),
            group: profile.group.clone(),
            mode: Some(profile.mode),
            source: EndpointDraft::from_profile(&profile.source, profile.remote_source.as_ref()),
            destination: EndpointDraft::from_profile(
                &profile.destination,
                profile.remote_destination.as_ref(),
            ),
            filters: FilterDraft::from_rules(&profile.filter),
            allow_delete: profile.allow_delete,
            compare: profile.compare,
            schedule: ScheduleDraft::for_profile(profile, schedule),
            had_schedule: schedule.is_some(),
        }
    }

    /// Duplicate under a fresh identity; schedules are never copied.
    pub(crate) fn duplicate(profile: &Profile) -> Self {
        let mut d = Self::edit(profile, None);
        d.existing = None;
        d.name = format!("{} (copie)", profile.name);
        d
    }

    pub(crate) fn suggested_name(&self) -> String {
        let (a, b) = (self.source.short_name(), self.destination.short_name());
        match (a.is_empty(), b.is_empty()) {
            (false, false) => format!("{a} vers {b}"),
            (false, true) => a,
            _ => "Nouveau profil".into(),
        }
    }

    /// Bidirectional reference states need every content hash.
    pub(crate) fn effective_compare(&self) -> CompareMethod {
        if self.mode == Some(SyncMode::Bidirectional) {
            CompareMethod::Content
        } else {
            self.compare
        }
    }

    /// The engine needs a local folder on at least one side.
    pub(crate) fn remote_pair_error(&self) -> Option<String> {
        let remote = |e: &EndpointDraft| e.provider.is_some_and(|p| p != Provider::Local);
        (remote(&self.source) && remote(&self.destination)).then(|| {
            "Source et destination sont toutes deux distantes : choisissez « Dossier local » pour l’une d’elles".to_owned()
        })
    }

    pub(crate) fn deletions_possible(&self) -> bool {
        matches!(self.mode, Some(SyncMode::Mirror | SyncMode::Bidirectional))
    }

    /// Builds the profile to save, with a stable new identity if needed.
    /// # Errors
    /// First validation failure, in French.
    pub(crate) fn build(&self) -> Result<Profile, String> {
        let mode = self.mode.ok_or("Choisissez un mode")?;
        let (source, remote_source) = self.source.build().map_err(|e| format!("Source — {e}"))?;
        let (destination, remote_destination) = self
            .destination
            .build()
            .map_err(|e| format!("Destination — {e}"))?;
        if let Some(e) = self.remote_pair_error() {
            return Err(e);
        }
        let name = self.name.trim();
        if name.is_empty() {
            return Err("Donnez un nom au profil".into());
        }
        let id = match &self.existing {
            Some(id) => id.clone(),
            None => new_profile_id()?,
        };
        let profile = Profile {
            id,
            name: name.to_owned(),
            group: self.group.trim().to_owned(),
            mode,
            source,
            destination,
            remote_source,
            remote_destination,
            filter: self.filters.rules(),
            allow_delete: self.allow_delete && self.deletions_possible(),
            compare: self.effective_compare(),
        };
        profile.validate().map_err(|e| match e {
            sync_profiles::ProfileError::Invalid(m) => describe(m),
            other => other.to_string(),
        })?;
        Ok(profile)
    }
}

fn describe(message: &str) -> String {
    if message.contains("OneDrive") {
        "OneDrive est une source uniquement, et le bidirectionnel exige deux dossiers locaux".into()
    } else if message.contains("filtres") {
        "Un motif de filtre est invalide".into()
    } else if message.contains("absolus") {
        "Les chemins doivent être absolus".into()
    } else {
        format!("Configuration refusée : {message}")
    }
}

/// Unique, valid profile identifier (`[A-Za-z0-9._-]`).
pub(crate) fn new_profile_id() -> Result<ProfileId, String> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| "horloge système invalide".to_owned())?;
    let value = format!(
        "profile-{:x}-{:x}-{:x}",
        now.as_secs(),
        now.subsec_nanos() ^ std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    ProfileId::new(value).map_err(|e| e.to_string())
}

/// Profile moved to another group (drag and drop, context menu).
pub(crate) fn regrouped(profile: &Profile, group: &str) -> Option<Profile> {
    let group = group.trim();
    if profile.group == group || group.len() > 200 {
        return None;
    }
    Some(Profile {
        group: group.to_owned(),
        ..profile.clone()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(path: &str) -> EndpointDraft {
        let mut d = EndpointDraft::default();
        d.select(Provider::Local);
        d.path = path.into();
        d
    }

    fn abs(name: &str) -> String {
        if cfg!(windows) {
            format!("C:\\data\\{name}")
        } else {
            format!("/data/{name}")
        }
    }

    #[test]
    fn local_backup_draft_builds_a_valid_profile() {
        let mut d = ProfileDraft::new("Travail");
        d.mode = Some(SyncMode::Backup);
        d.source = local(&abs("Documents"));
        d.destination = local(&abs("Sauvegardes"));
        d.allow_delete = true;
        assert_eq!(d.suggested_name(), "Documents vers Sauvegardes");
        assert!(d.build().is_err(), "name required");
        d.name = "  Docs  ".into();
        d.filters.toggle_preset(&PRESETS[0]);
        let p = d.build().unwrap();
        assert_eq!(p.name, "Docs");
        assert_eq!(p.group, "Travail");
        assert!(!p.allow_delete, "a backup never deletes");
        assert_eq!(p.filter.exclude_files().len(), 4);
        assert!(p.id.as_str().starts_with("profile-"));
        let again = ProfileDraft::edit(&p, None);
        assert_eq!(
            again.build().unwrap(),
            p,
            "round trip keeps identity and content"
        );
    }

    #[test]
    fn relative_paths_and_missing_choices_are_refused() {
        let mut d = ProfileDraft::new("");
        d.name = "x".into();
        assert!(d.build().unwrap_err().contains("mode"));
        d.mode = Some(SyncMode::Mirror);
        d.source = local("relatif");
        d.destination = local(&abs("b"));
        assert!(d.build().unwrap_err().starts_with("Source"));
    }

    #[test]
    fn onedrive_destination_and_remote_two_way_are_refused() {
        let mut d = ProfileDraft::new("");
        d.name = "x".into();
        d.mode = Some(SyncMode::Backup);
        d.source = local(&abs("a"));
        d.destination.select(Provider::Onedrive);
        d.destination.drive_id = "b!abc".into();
        d.destination.credential_ref = "ONEDRIVE".into();
        assert!(d.build().unwrap_err().contains("OneDrive"));
    }

    #[test]
    fn two_remote_sides_are_refused_before_saving() {
        let mut d = ProfileDraft::new("");
        d.name = "Musique".into();
        d.mode = Some(SyncMode::Mirror);
        d.source.select(Provider::Smb);
        d.source.smb_root = if cfg!(windows) {
            "\\\\nas\\musique".into()
        } else {
            "/mnt/nas/musique".into()
        };
        d.destination.select(Provider::Smb);
        d.destination.smb_root = if cfg!(windows) {
            "\\\\nas\\copie".into()
        } else {
            "/mnt/nas/copie".into()
        };
        assert!(d.remote_pair_error().is_some());
        assert!(d.build().unwrap_err().contains("Dossier local"));
        d.destination = local(&abs("Musique"));
        assert!(d.remote_pair_error().is_none());
        let p = d.build().unwrap();
        assert!(p.remote_source.is_some() && p.remote_destination.is_none());
    }

    #[cfg(windows)]
    #[test]
    fn windows_share_requires_a_unc_path() {
        let mut e = EndpointDraft::default();
        e.select(Provider::Smb);
        e.set(Field::Path, "F:\\Musique".into());
        assert!(e.build().unwrap_err().contains("Dossier local"));
        e.set(Field::Path, "\\\\Nas\\Musique".into());
        assert!(matches!(
            e.build(),
            Ok((_, Some(RemoteEndpoint::Smb { .. })))
        ));
    }

    #[test]
    fn ftp_requires_consent_and_ip_address() {
        let mut e = EndpointDraft::default();
        e.select(Provider::Ftp);
        assert_eq!(e.port, "21");
        e.ip = "nas.local".into();
        e.username = "u".into();
        e.root = "/b".into();
        e.set(Field::CredentialRef, "ftp_pass".into());
        assert_eq!(e.credential_ref, "FTP_PASS");
        assert!(e.build().unwrap_err().contains("non chiffrée"));
        e.acknowledge_plaintext = true;
        assert!(e.build().unwrap_err().contains("IP"));
        e.ip = "192.168.1.2".into();
        let (_, remote) = e.build().unwrap();
        assert!(matches!(remote, Some(RemoteEndpoint::Ftp { .. })));
        let back = EndpointDraft::from_profile(Path::new(""), remote.as_ref());
        assert_eq!(back.ip, "192.168.1.2");
        assert!(back.acknowledge_plaintext);
    }

    #[test]
    fn schedule_edit_restores_consent_only_for_the_approved_profile() {
        let mut d = ProfileDraft::new("");
        d.name = "Scheduled mirror".into();
        d.mode = Some(SyncMode::Mirror);
        d.source = local(&abs("source"));
        d.destination = local(&abs("destination"));
        d.allow_delete = true;
        let profile = d.build().unwrap();
        let mut schedule = Schedule {
            approved: profile.clone(),
            trigger: Trigger::Interval { seconds: 900 },
            missed: MissedJobs::Skip,
            created_at: 100,
            enabled: true,
            allow_deletions: true,
            show_window: true,
        };
        let restored = ScheduleDraft::for_profile(&profile, Some(&schedule));
        assert!(restored.approve_writes && restored.approve_deletions);
        assert!(restored.skip_missed);
        assert_eq!(restored.show_window, window_supported());
        assert_eq!(restored.trigger().unwrap(), Some(schedule.trigger.clone()));
        assert!(
            ProfileDraft::edit(&profile, Some(&schedule))
                .schedule
                .approve_writes
        );

        let mut changed = profile.clone();
        changed.destination = PathBuf::from(abs("another-destination"));
        let stale = ScheduleDraft::for_profile(&changed, Some(&schedule));
        assert!(!stale.approve_writes && !stale.approve_deletions);
        assert_eq!(stale.trigger().unwrap(), Some(schedule.trigger.clone()));
        schedule.enabled = false;
        let disabled = ScheduleDraft::for_profile(&profile, Some(&schedule));
        assert!(!disabled.approve_writes && !disabled.approve_deletions);
        let new = ScheduleDraft::for_profile(&profile, None);
        assert!(!new.approve_writes && !new.approve_deletions);
    }

    #[test]
    fn schedule_drafts_convert_and_never_preapprove() {
        let mut s = ScheduleDraft::default();
        assert_eq!(s.trigger().unwrap(), None);
        s.kind = TriggerKind::Interval;
        s.interval_minutes = "15".into();
        assert_eq!(
            s.trigger().unwrap(),
            Some(Trigger::Interval { seconds: 900 })
        );
        s.interval_minutes = "0".into();
        assert!(s.trigger().is_err());
        s.kind = TriggerKind::Daily;
        s.hour = 7;
        s.minute = 30;
        assert_eq!(
            s.trigger().unwrap(),
            Some(Trigger::DailyUtc {
                hour: 7,
                minute: 30
            })
        );
        assert_eq!(
            trigger_label(&Trigger::Interval { seconds: 7200 }),
            "Toutes les 2 h"
        );
        assert_eq!(
            trigger_label(&Trigger::Interval { seconds: 900 }),
            "Toutes les 15 min"
        );
    }

    #[test]
    fn next_occurrences_follow_interval_phase_and_daily_utc() {
        let t = Trigger::Interval { seconds: 3600 };
        assert_eq!(next_occurrence(&t, 1000, 1000), Some(4600));
        assert_eq!(next_occurrence(&t, 1000, 500), Some(1000));
        let d = Trigger::DailyUtc { hour: 1, minute: 0 };
        assert_eq!(next_occurrence(&d, 0, 0), Some(3600));
        assert_eq!(next_occurrence(&d, 0, 7200), Some(86_400 + 3600));
        assert_eq!(next_occurrence(&Trigger::Logon, 0, 0), None);
    }

    #[test]
    fn filters_and_presets_toggle_without_duplicates() {
        let mut f = FilterDraft::default();
        assert!(f.add(FilterList::ExcludeFiles, " *.log "));
        assert!(!f.add(FilterList::ExcludeFiles, "*.log"));
        f.toggle_preset(&PRESETS[2]);
        assert!(f.has_preset(&PRESETS[2]));
        assert_eq!(f.count(), 4);
        f.toggle_preset(&PRESETS[2]);
        assert_eq!(f.count(), 1);
        f.remove(FilterList::ExcludeFiles, 0);
        f.remove(FilterList::ExcludeFiles, 9);
        assert_eq!(f.count(), 0);
    }

    #[test]
    fn regrouping_changes_only_the_group() {
        let mut d = ProfileDraft::new("A");
        d.name = "n".into();
        d.mode = Some(SyncMode::Backup);
        d.source = local(&abs("s"));
        d.destination = local(&abs("d"));
        let p = d.build().unwrap();
        assert!(regrouped(&p, "A").is_none());
        let moved = regrouped(&p, " B ").unwrap();
        assert_eq!(moved.group, "B");
        assert_eq!(
            Profile {
                group: "A".into(),
                ..moved
            },
            p
        );
        let ids: std::collections::HashSet<_> =
            (0..50).map(|_| new_profile_id().unwrap()).collect();
        assert_eq!(ids.len(), 50);
    }
}
