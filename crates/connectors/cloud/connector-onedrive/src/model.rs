use connector_api::{StorageError, StorageErrorKind as Kind, StorageOperation as Op};
use serde::Deserialize;
use serde_json::Value;
use sync_core::{EntryKind, FileEntry, FileMetadata, ResourcePath};

use crate::transport::error;

#[derive(Deserialize)]
pub(super) struct Parent {
    pub id: Option<String>,
    #[serde(rename = "driveId")]
    pub drive: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct Item {
    pub id: String,
    pub name: String,
    pub size: Option<u64>,
    #[serde(rename = "lastModifiedDateTime")]
    pub modified: Option<String>,
    #[serde(rename = "eTag")]
    pub etag: Option<String>,
    pub file: Option<Value>,
    pub folder: Option<Value>,
    #[serde(rename = "parentReference")]
    pub parent: Option<Parent>,
    #[serde(rename = "remoteItem")]
    pub remote: Option<Value>,
    pub deleted: Option<Value>,
    pub package: Option<Value>,
}
impl Item {
    pub(super) fn entry(&self, path: &ResourcePath, op: Op) -> Result<FileEntry, StorageError> {
        if self.id.is_empty()
            || self.name.len() > 1024
            || self.etag.as_ref().is_some_and(|e| e.len() > 1024)
            || self.id.len() > 1024
            || self.remote.is_some()
            || self.deleted.is_some()
            || self.package.is_some()
        {
            return Err(error(Kind::NotSupported, op, path));
        }
        let kind = match (&self.file, &self.folder) {
            (Some(f), None) if f.is_object() => EntryKind::File,
            (None, Some(f)) if f.is_object() => EntryKind::Directory,
            _ => return Err(error(Kind::NotSupported, op, path)),
        };
        let mut metadata = if kind == EntryKind::File {
            FileMetadata::unknown()
                .with_size(self.size.ok_or_else(|| error(Kind::Permanent, op, path))?)
        } else {
            FileMetadata::unknown()
        };
        if let Some(value) = &self.modified {
            if value.len() > 128 {
                return Err(error(Kind::Permanent, op, path));
            }
            let date = chrono::DateTime::parse_from_rfc3339(value)
                .map_err(|_| error(Kind::Permanent, op, path))?;
            let digits = value.split_once('.').map_or(0, |(_, fraction)| {
                fraction.bytes().take_while(|c| c.is_ascii_digit()).count()
            });
            let precision = match digits {
                0..=2 => sync_core::TimestampPrecision::Second,
                3..=5 => sync_core::TimestampPrecision::Millisecond,
                6 => sync_core::TimestampPrecision::Microsecond,
                7..=8 => sync_core::TimestampPrecision::HundredNanoseconds,
                _ => sync_core::TimestampPrecision::Nanosecond,
            };
            let time = sync_core::Timestamp::new(date.timestamp(), date.timestamp_subsec_nanos())
                .map_err(|_| error(Kind::Permanent, op, path))?;
            metadata = metadata.with_modified(time, precision);
        }
        Ok(FileEntry::new(path.clone(), kind, metadata))
    }
    pub(super) fn child(
        &self,
        parent: &str,
        drive: &str,
        path: &ResourcePath,
        op: Op,
    ) -> Result<FileEntry, StorageError> {
        if !self
            .parent
            .as_ref()
            .is_some_and(|p| p.id.as_deref() == Some(parent) && p.drive.as_deref() == Some(drive))
        {
            return Err(error(Kind::Conflict, op, path));
        }
        self.entry(path, op)
    }
}

#[derive(Deserialize)]
pub(super) struct Page {
    pub value: Vec<Item>,
    #[serde(rename = "@odata.nextLink")]
    pub next: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listing_date_is_optional_but_malformed_dates_fail() {
        let base = serde_json::json!({"id":"1", "name":"a", "size":3, "file":{}});
        let item: Item = serde_json::from_value(base.clone()).unwrap();
        assert!(item.entry(&ResourcePath::root(), Op::List).is_ok());
        for date in ["2026-10-25T12:00:00.123Z", "invalid"] {
            let mut value = base.clone();
            value["lastModifiedDateTime"] = date.into();
            let item: Item = serde_json::from_value(value).unwrap();
            assert_eq!(
                item.entry(&ResourcePath::root(), Op::List).is_ok(),
                date != "invalid"
            );
        }
    }
}
