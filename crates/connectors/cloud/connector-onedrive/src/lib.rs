//! Source OneDrive expérimentale, compilée seulement avec la feature `onedrive`.
//! E/S bloquantes : appeler sur un worker. Aucune mutation distante.
#![doc = include_str!("../README.md")]
#[cfg(feature = "onedrive")]
mod graph;
#[cfg(feature = "onedrive")]
mod model;
#[cfg(feature = "onedrive")]
mod transport;
#[cfg(feature = "onedrive")]
pub use graph::{OneDriveConnector, OneDriveOptions};
