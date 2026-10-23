//! Connecteur WebDAV HTTPS bloquant, activé avec la feature `webdav`.
//! Lecture seule par défaut ; mutations conditionnelles par opt-in explicite.
#![cfg_attr(feature = "webdav", doc = include_str!("../README.md"))]
#[cfg(feature = "webdav")]
mod mutations;
#[cfg(feature = "webdav")]
mod paging;
#[cfg(feature = "webdav")]
mod protocol;
#[cfg(feature = "webdav")]
mod webdav;
#[cfg(feature = "webdav")]
pub use webdav::{WebDavConnector, WebDavOptions};
