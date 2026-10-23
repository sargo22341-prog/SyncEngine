//! Horodatages et précision déclarée.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use thiserror::Error;

const NANOS_PER_SEC: u32 = 1_000_000_000;

/// Erreur de construction d'un [`Timestamp`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum TimestampError {
    /// Le nombre de nanosecondes n'est pas inférieur à 10⁹.
    #[error("nanosecondes hors plage : {nanos} (doit être < 1 000 000 000)")]
    NanosOutOfRange {
        /// Valeur reçue.
        nanos: u32,
    },
    /// L'instant n'est pas représentable sur 64 bits de secondes.
    #[error("instant hors de la plage représentable")]
    OutOfRange,
}

/// Instant UTC, exprimé depuis l'époque Unix (1970-01-01T00:00:00Z).
///
/// # Sémantique
/// - toujours UTC, sans fuseau ni heure locale ;
/// - les secondes intercalaires ne sont pas modélisées (comme `SystemTime`) ;
/// - `secs` peut être négatif (instant antérieur à 1970) ; `nanos` est
///   toujours dans `0..1_000_000_000`, l'ordre est donc chronologique ;
/// - la valeur seule ne dit rien de sa fiabilité : la précision réelle est
///   portée à part par [`TimestampPrecision`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "RawTimestamp", into = "RawTimestamp")]
pub struct Timestamp {
    secs: i64,
    nanos: u32,
}

#[derive(Serialize, Deserialize)]
struct RawTimestamp {
    secs: i64,
    nanos: u32,
}

impl TryFrom<RawTimestamp> for Timestamp {
    type Error = TimestampError;

    fn try_from(raw: RawTimestamp) -> Result<Self, Self::Error> {
        Self::new(raw.secs, raw.nanos)
    }
}

impl From<Timestamp> for RawTimestamp {
    fn from(ts: Timestamp) -> Self {
        Self {
            secs: ts.secs,
            nanos: ts.nanos,
        }
    }
}

impl Timestamp {
    /// L'époque Unix.
    pub const UNIX_EPOCH: Self = Self { secs: 0, nanos: 0 };

    /// Construit un horodatage.
    ///
    /// # Errors
    /// [`TimestampError::NanosOutOfRange`] si `nanos >= 1_000_000_000`.
    pub fn new(secs: i64, nanos: u32) -> Result<Self, TimestampError> {
        if nanos >= NANOS_PER_SEC {
            return Err(TimestampError::NanosOutOfRange { nanos });
        }
        Ok(Self { secs, nanos })
    }

    /// Convertit un `SystemTime`, y compris antérieur à l'époque.
    ///
    /// # Errors
    /// [`TimestampError::OutOfRange`] si l'instant dépasse ±2⁶³ secondes.
    pub fn from_system_time(time: SystemTime) -> Result<Self, TimestampError> {
        match time.duration_since(UNIX_EPOCH) {
            Ok(after) => {
                let secs =
                    i64::try_from(after.as_secs()).map_err(|_| TimestampError::OutOfRange)?;
                Self::new(secs, after.subsec_nanos())
            }
            Err(before) => {
                let before = before.duration();
                let mut secs = i64::try_from(before.as_secs())
                    .map_err(|_| TimestampError::OutOfRange)?
                    .checked_neg()
                    .ok_or(TimestampError::OutOfRange)?;
                let mut nanos = before.subsec_nanos();
                if nanos > 0 {
                    secs = secs.checked_sub(1).ok_or(TimestampError::OutOfRange)?;
                    nanos = NANOS_PER_SEC - nanos;
                }
                Self::new(secs, nanos)
            }
        }
    }

    /// Instant courant selon l'horloge système locale.
    ///
    /// # Errors
    /// [`TimestampError::OutOfRange`] si l'horloge système est aberrante.
    pub fn now() -> Result<Self, TimestampError> {
        Self::from_system_time(SystemTime::now())
    }

    /// Convertit vers `SystemTime`, `None` si non représentable sur la plateforme.
    #[must_use]
    pub fn to_system_time(self) -> Option<SystemTime> {
        let nanos = Duration::from_nanos(u64::from(self.nanos));
        if self.secs >= 0 {
            UNIX_EPOCH
                .checked_add(Duration::from_secs(self.secs.unsigned_abs()))?
                .checked_add(nanos)
        } else {
            UNIX_EPOCH
                .checked_sub(Duration::from_secs(self.secs.unsigned_abs()))?
                .checked_add(nanos)
        }
    }

    /// Secondes depuis l'époque (partie entière, arrondie vers -∞).
    #[must_use]
    pub fn secs(self) -> i64 {
        self.secs
    }

    /// Nanosecondes dans la seconde (`0..1_000_000_000`).
    #[must_use]
    pub fn subsec_nanos(self) -> u32 {
        self.nanos
    }
}

/// Précision avec laquelle un stockage conserve un horodatage.
///
/// Deux dates ne peuvent être comparées de façon fiable qu'à la plus
/// grossière des deux précisions ; `Unknown` interdit toute conclusion
/// fondée uniquement sur la date.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TimestampPrecision {
    /// Nanoseconde (ext4, Btrfs…).
    Nanosecond,
    /// 100 nanosecondes (NTFS).
    HundredNanoseconds,
    /// Microseconde.
    Microsecond,
    /// Milliseconde.
    Millisecond,
    /// Seconde (nombreux serveurs FTP/SFTP, API cloud).
    Second,
    /// Deux secondes (FAT/exFAT pour la date de modification).
    TwoSeconds,
    /// Précision inconnue ou non garantie.
    Unknown,
}

impl TimestampPrecision {
    /// Granularité sous forme de durée, `None` si inconnue.
    #[must_use]
    pub fn granularity(self) -> Option<Duration> {
        match self {
            Self::Nanosecond => Some(Duration::from_nanos(1)),
            Self::HundredNanoseconds => Some(Duration::from_nanos(100)),
            Self::Microsecond => Some(Duration::from_micros(1)),
            Self::Millisecond => Some(Duration::from_millis(1)),
            Self::Second => Some(Duration::from_secs(1)),
            Self::TwoSeconds => Some(Duration::from_secs(2)),
            Self::Unknown => None,
        }
    }

    /// La plus grossière des deux précisions (`Unknown` l'emporte toujours).
    #[must_use]
    pub fn coarsest(self, other: Self) -> Self {
        match (self.granularity(), other.granularity()) {
            (Some(a), Some(b)) => {
                if a >= b {
                    self
                } else {
                    other
                }
            }
            _ => Self::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_nanos() {
        assert!(Timestamp::new(0, 999_999_999).is_ok());
        assert_eq!(
            Timestamp::new(0, 1_000_000_000),
            Err(TimestampError::NanosOutOfRange {
                nanos: 1_000_000_000
            })
        );
    }

    #[test]
    fn system_time_round_trip_after_epoch() {
        // Multiple de 100 ns : `SystemTime` Windows a une résolution de 100 ns
        // et tronquerait une valeur plus fine.
        let time = UNIX_EPOCH + Duration::new(1_760_000_000, 123_456_700);
        let ts = Timestamp::from_system_time(time).unwrap();
        assert_eq!((ts.secs(), ts.subsec_nanos()), (1_760_000_000, 123_456_700));
        assert_eq!(ts.to_system_time(), Some(time));
    }

    #[test]
    fn system_time_round_trip_before_epoch() {
        // 1,25 s avant l'époque => secs = -2, nanos = 0,75 s.
        let time = UNIX_EPOCH - Duration::new(1, 250_000_000);
        let ts = Timestamp::from_system_time(time).unwrap();
        assert_eq!((ts.secs(), ts.subsec_nanos()), (-2, 750_000_000));
        assert_eq!(ts.to_system_time(), Some(time));
        assert!(ts < Timestamp::UNIX_EPOCH);
    }

    #[test]
    fn ordering_is_chronological() {
        let a = Timestamp::new(10, 999_999_999).unwrap();
        let b = Timestamp::new(11, 0).unwrap();
        assert!(a < b);
    }

    #[test]
    fn now_is_after_2020() {
        assert!(Timestamp::now().unwrap().secs() > 1_577_836_800);
    }

    #[test]
    fn serde_validates_nanos() {
        let ts = Timestamp::new(5, 7).unwrap();
        let json = serde_json::to_string(&ts).unwrap();
        assert_eq!(json, r#"{"secs":5,"nanos":7}"#);
        assert_eq!(serde_json::from_str::<Timestamp>(&json).unwrap(), ts);
        assert!(serde_json::from_str::<Timestamp>(r#"{"secs":5,"nanos":1000000000}"#).is_err());
    }

    #[test]
    fn coarsest_precision() {
        use TimestampPrecision::*;
        assert_eq!(Nanosecond.coarsest(TwoSeconds), TwoSeconds);
        assert_eq!(Second.coarsest(HundredNanoseconds), Second);
        assert_eq!(Millisecond.coarsest(Unknown), Unknown);
        assert_eq!(Unknown.coarsest(Second), Unknown);
    }
}
