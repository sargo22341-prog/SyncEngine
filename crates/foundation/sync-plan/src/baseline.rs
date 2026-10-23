//! Portable reference state, independent of its persistence implementation.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sync_core::{ConnectorId, EntryKind, HashAlgorithm, ProfileId, ResourcePath};
use sync_filter::FilterRules;
use thiserror::Error;

use crate::EntrySnapshot;

/// Exact identity of a synchronization pair. Locations must be canonical,
/// stable root identities, never display aliases or credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateBinding {
    /// Profile owning this state.
    pub profile: ProfileId,
    /// Physical source connector.
    pub source_connector: ConnectorId,
    /// Physical destination connector.
    pub destination_connector: ConnectorId,
    /// Canonical source root identity.
    pub source: String,
    /// Canonical destination root identity.
    pub destination: String,
    /// Exact filters used by both scans.
    pub filter: FilterRules,
}

/// Invalid reference state; never silently replaced by an empty baseline.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("invalid baseline: {0}")]
pub struct BaselineError(pub &'static str);

impl sync_errors::ClassifiedError for BaselineError {
    fn class(&self) -> sync_errors::ErrorClass {
        sync_errors::ErrorClass::Integrity
    }
}

/// Last globally converged state. `None` entries are durable tombstones.
/// Tombstones have no automatic expiry; history pruning cannot remove them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawBaseline")]
pub struct Baseline {
    binding: StateBinding,
    generation: u64,
    entries: BTreeMap<ResourcePath, Option<EntrySnapshot>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBaseline {
    binding: StateBinding,
    generation: u64,
    entries: BTreeMap<ResourcePath, Option<EntrySnapshot>>,
}

impl TryFrom<RawBaseline> for Baseline {
    type Error = BaselineError;
    fn try_from(raw: RawBaseline) -> Result<Self, Self::Error> {
        Self::new(raw.binding, raw.generation, raw.entries)
    }
}

impl Baseline {
    /// Constructs validated reference state.
    ///
    /// # Errors
    /// Rejects generation zero, root entries, unsupported kinds or files
    /// without size and BLAKE3. Callers must establish convergence first.
    pub fn new(
        binding: StateBinding,
        generation: u64,
        entries: BTreeMap<ResourcePath, Option<EntrySnapshot>>,
    ) -> Result<Self, BaselineError> {
        if generation == 0 {
            return Err(BaselineError("zero generation"));
        }
        if binding.source.is_empty()
            || binding.destination.is_empty()
            || binding.source == binding.destination
        {
            return Err(BaselineError("invalid endpoint identities"));
        }
        for (path, entry) in &entries {
            if path.is_root() {
                return Err(BaselineError("root entry"));
            }
            if let Some(entry) = entry {
                match entry.kind() {
                    EntryKind::Directory => {}
                    EntryKind::File
                        if entry.metadata().size().is_some()
                            && entry
                                .metadata()
                                .hash()
                                .is_some_and(|h| h.algorithm() == HashAlgorithm::Blake3) => {}
                    _ => return Err(BaselineError("unsupported or unhashed entry")),
                }
            }
        }
        Ok(Self {
            binding,
            generation,
            entries,
        })
    }

    /// Identity that must match a new run exactly.
    #[must_use]
    pub fn binding(&self) -> &StateBinding {
        &self.binding
    }
    /// Monotonic successful-publication generation.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }
    /// Entries, including tombstones (absence proven in a converged run).
    #[must_use]
    pub fn entries(&self) -> &BTreeMap<ResourcePath, Option<EntrySnapshot>> {
        &self.entries
    }
}

/// Content identity, never based on timestamps. Directories compare by
/// kind only; callers must separately check every descendant.
#[must_use]
pub fn same_content(left: &EntrySnapshot, right: &EntrySnapshot) -> bool {
    if left.kind() != right.kind() {
        return false;
    }
    match left.kind() {
        EntryKind::Directory => true,
        EntryKind::File => {
            left.metadata().size().is_some()
                && left.metadata().size() == right.metadata().size()
                && left.metadata().hash().is_some_and(|hash| {
                    hash.algorithm() == HashAlgorithm::Blake3
                        && Some(hash) == right.metadata().hash()
                })
        }
        _ => false,
    }
}
