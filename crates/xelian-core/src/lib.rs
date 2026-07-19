//! xelian-core: manifest, lockfile, packaging, cache, and run pipeline.
//!
//! The CLI crate (`xelian-cli`) is a thin dispatcher over this library.

pub mod auth;
pub mod cache;
pub mod checksum;
pub mod errors;
pub mod github;
pub mod init;
pub mod lockfile;
pub mod manifest;
pub mod package;
pub mod permissions;
pub mod registry_client;
pub mod run;
pub mod validate;

/// Version of the xelian binary, recorded in `xelian.lock` (`xelian-version`).
pub const XELIAN_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The Package Format Specification versions this binary implements.
pub const SUPPORTED_SPEC_VERSIONS: &[u64] = &[1];
