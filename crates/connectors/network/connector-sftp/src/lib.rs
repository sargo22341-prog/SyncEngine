//! Connecteur SFTP bloquant, activé explicitement avec la feature `sftp`.
//! Lecture seule par défaut ; mutations par opt-in explicite.
#![cfg_attr(feature = "sftp", doc = include_str!("../README.md"))]
#[cfg(feature = "sftp")]
mod mutations;
#[cfg(feature = "sftp")]
mod paging;
#[cfg(feature = "sftp")]
mod sftp;
#[cfg(feature = "sftp")]
pub use sftp::{SftpAuthentication, SftpConnector, SftpOptions};
