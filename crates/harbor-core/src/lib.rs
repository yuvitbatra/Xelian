//! harbor-core: manifest, lockfile, packaging, cache, and run pipeline.
//!
//! The CLI crate (`harbor-cli`) is a thin dispatcher over this library.

pub mod cache;
pub mod checksum;
pub mod errors;
pub mod init;
pub mod lockfile;
pub mod manifest;
pub mod package;
pub mod run;
pub mod validate;

/// Version of the harbor binary, recorded in `harbor.lock` (`harbor-version`).
pub const HARBOR_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The Package Format Specification versions this binary implements.
pub const SUPPORTED_SPEC_VERSIONS: &[u64] = &[1];
