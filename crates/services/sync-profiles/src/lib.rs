//! Local and optional remote profile configuration, separate from observed synchronization state.
//! SQLite migrations v0→v1→v2→v3 are transactional; unknown/corrupt stores are refused.
//! No secrets or scheduling configuration are stored here. All I/O is blocking.

mod endpoints;
pub use endpoints::RemoteEndpoint;

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use sync_core::ProfileId;
use sync_filter::{Filter, FilterRules};
use sync_plan::SyncMode;
use thiserror::Error;

/// How a one-way preview establishes that two files present on both sides
/// are identical. Never changes deletion safeguards nor execution checks
/// (size and date preconditions, verified copy, versions).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompareMethod {
    /// BLAKE3 content of the files whose sizes are equal on both sides
    /// (different sizes already prove a difference). Rereads all common
    /// content: slow on large trees, detects a change that keeps size and
    /// date. Required by bidirectional profiles. Default of profiles saved
    /// before this setting existed.
    #[default]
    Content,
    /// Size and modification date at the coarser declared precision, like
    /// FreeFileSync's default: only the directory listings are read. A
    /// modification keeping both size and date is not detected.
    SizeAndDate,
}

impl CompareMethod {
    fn is_content(&self) -> bool {
        *self == Self::Content
    }
}

/// Editable configuration. Validation is repeated before every save and use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    /// Stable identity, independent of its display name.
    pub id: ProfileId,
    /// Nonempty display name.
    pub name: String,
    /// Optional display group.
    pub group: String,
    /// Implemented synchronization strategy.
    pub mode: SyncMode,
    /// Absolute OS source path when `remote_source` is absent; otherwise unused.
    pub source: PathBuf,
    /// Absolute OS destination path when `remote_destination` is absent.
    pub destination: PathBuf,
    /// Optional remote source. Absent means the original local source path.
    /// Local path is retained for backward-compatible editing, ignored when remote.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_source: Option<RemoteEndpoint>,
    /// Optional remote destination; OneDrive is forbidden as a receiver.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_destination: Option<RemoteEndpoint>,
    /// Identical filters used for both preview and execution.
    pub filter: FilterRules,
    /// Explicit destructive policy; safeguards remain enabled.
    pub allow_delete: bool,
    /// File identity rule of one-way previews. Omitted from the stored JSON
    /// when `Content`, so such rows stay byte-for-byte those of version 2.
    #[serde(default, skip_serializing_if = "CompareMethod::is_content")]
    pub compare: CompareMethod,
}

impl Profile {
    /// Checks configuration, without assuming roots are currently accessible.
    /// # Errors
    /// Invalid name, path, filter, mode or destructive backup policy.
    pub fn validate(&self) -> Result<(), ProfileError> {
        if self.name.trim().is_empty() || self.name.len() > 200 || self.group.len() > 200 {
            return Err(ProfileError::Invalid("nom ou groupe invalide"));
        }
        if (self.remote_source.is_none() && !self.source.is_absolute())
            || (self.remote_destination.is_none() && !self.destination.is_absolute())
        {
            return Err(ProfileError::Invalid("chemins absolus requis"));
        }
        if self.source.to_str().is_none() || self.destination.to_str().is_none() {
            return Err(ProfileError::Invalid(
                "racines non UTF-8 non prises en charge",
            ));
        }
        for remote in [&self.remote_source, &self.remote_destination]
            .into_iter()
            .flatten()
        {
            remote.validate()?;
        }
        if matches!(
            self.remote_destination,
            Some(RemoteEndpoint::Onedrive { .. })
        ) || (self.mode == SyncMode::Bidirectional
            && (self.remote_source.is_some() || self.remote_destination.is_some()))
        {
            return Err(ProfileError::Invalid(
                "OneDrive source seule ; bidirectionnel distant non qualifi?",
            ));
        }
        if self.mode == SyncMode::Bidirectional && self.compare != CompareMethod::Content {
            return Err(ProfileError::Invalid(
                "le bidirectionnel exige la comparaison par contenu",
            ));
        }
        match self.mode {
            SyncMode::Backup if self.allow_delete => {
                return Err(ProfileError::Invalid("une sauvegarde ne supprime jamais"));
            }
            SyncMode::Backup | SyncMode::Mirror | SyncMode::Bidirectional => (),
            _ => return Err(ProfileError::Invalid("mode non pris en charge")),
        }
        Filter::new(self.filter.clone()).map_err(|_| ProfileError::Invalid("filtres invalides"))?;
        Ok(())
    }
}

/// Presentation preferences, persisted independently of synchronization policy.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preferences {
    /// Use a denser profile/differences table.
    pub compact: bool,
}

/// Explicit configuration/persistence failure; never interpreted as an empty list.
#[derive(Debug, Error)]
pub enum ProfileError {
    /// SQLite read/write/migration error.
    #[error("profils SQLite : {0}")]
    Sql(#[from] rusqlite::Error),
    /// Serialized data is invalid.
    #[error("format des profils : {0}")]
    Format(#[from] serde_json::Error),
    /// Unsupported schema or invalid input.
    #[error("configuration : {0}")]
    Invalid(&'static str),
}

/// Versioned store. The caller places it outside synchronized trees.
pub struct Profiles(Connection);
impl Profiles {
    /// Opens an identified store, migrating empty v0 or additive v1/v2 to v3.
    /// # Errors
    /// Unreadable, corrupt, foreign or future database; migration failure.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ProfileError> {
        let mut connection = Connection::open(path)?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        let check: String = connection.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        if check != "ok" {
            return Err(ProfileError::Invalid("base corrompue"));
        }
        let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let version: u32 = tx.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        let app: u32 = tx.query_row("PRAGMA application_id", [], |r| r.get(0))?;
        if version == 0 && app == 0 {
            let tables: i64 = tx.query_row(
                "SELECT count(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'",
                [],
                |r| r.get(0),
            )?;
            if tables != 0 {
                return Err(ProfileError::Invalid("base étrangère"));
            }
            tx.execute_batch(include_str!("../migrations/001_initial.sql"))?;
        } else if !matches!(version, 1..=3) || app != 1_398_361_680 {
            return Err(ProfileError::Invalid("version ou application inconnue"));
        }
        // Additive JSON fields: legacy configurations remain byte-for-byte intact.
        // Version 2 prevents old applications silently opening new remote
        // profiles, version 3 profiles compared by size and date (`compare`).
        tx.execute_batch("PRAGMA user_version=3;")?;
        tx.commit()?;
        connection.execute_batch("PRAGMA synchronous=FULL;")?;
        Ok(Self(connection))
    }

    /// Lists validated configurations in display-name order.
    /// # Errors
    /// Read error or invalid stored profile (not silently skipped).
    pub fn list(&self) -> Result<Vec<Profile>, ProfileError> {
        let mut stmt = self
            .0
            .prepare("SELECT config FROM profiles ORDER BY json_extract(config,'$.name'),id")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.map(|row| {
            let profile: Profile = serde_json::from_str(&row?)?;
            profile.validate()?;
            Ok(profile)
        })
        .collect()
    }

    /// Inserts or replaces one validated configuration in a single SQL transaction.
    /// # Errors
    /// Invalid configuration or persistence failure; old row is retained on failure.
    pub fn save(&self, profile: &Profile) -> Result<(), ProfileError> {
        profile.validate()?;
        self.0.execute("INSERT INTO profiles VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET config=excluded.config", params![profile.id.as_str(), serde_json::to_string(profile)?])?;
        Ok(())
    }

    /// Deletes configuration only; history and content versions are retained.
    /// # Errors
    /// Persistence failure.
    pub fn remove(&self, id: &ProfileId) -> Result<(), ProfileError> {
        self.0
            .execute("DELETE FROM profiles WHERE id=?1", [id.as_str()])?;
        Ok(())
    }

    /// Reads presentation preferences, defaulting only when no row exists.
    /// # Errors
    /// Read/format failure.
    pub fn preferences(&self) -> Result<Preferences, ProfileError> {
        let json: Option<String> = self
            .0
            .query_row("SELECT config FROM preferences WHERE id=1", [], |r| {
                r.get(0)
            })
            .optional()?;
        Ok(json
            .map(|j| serde_json::from_str(&j))
            .transpose()?
            .unwrap_or_default())
    }

    /// Saves presentation preferences atomically.
    /// # Errors
    /// Persistence/serialization failure.
    pub fn save_preferences(&self, preferences: &Preferences) -> Result<(), ProfileError> {
        self.0.execute("INSERT INTO preferences VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET config=excluded.config", [serde_json::to_string(preferences)?])?;
        Ok(())
    }
}
