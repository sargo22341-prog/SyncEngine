//! Connecteur SMB natif, feature `smb` désactivée par défaut.
#[cfg(feature = "smb")]
mod mounted;
#[cfg(feature = "smb")]
pub use mounted::SmbConnector;
