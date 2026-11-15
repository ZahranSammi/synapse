//! Runtime provider contracts and implementations.

pub mod node;
pub mod bun;
pub mod php;
pub mod composer;
pub mod postgres;
pub mod nginx;

use std::path::PathBuf;
use crate::error::Result;

/// Metadata for an available remote runtime version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteVersion {
    pub version: String,
    pub lts: Option<String>,
}

/// Trait defining the contract for all runtime providers (Node.js, Bun, etc.).
#[allow(async_fn_in_trait)]
pub trait RuntimeProvider {
    /// Return the runtime identifier name (e.g. "node", "bun").
    fn name(&self) -> &'static str;

    /// Return list of binary executable names provided by this runtime (e.g. ["node", "npm", "npx"]).
    fn binary_names(&self) -> &'static [&'static str];
    /// Return the subdirectory containing binaries inside extracted folder (e.g. "bin" for Node, "" for Bun).
    fn bin_subdir(&self) -> &'static str;

    /// Fetch list of available versions from remote index.
    async fn list_remote(&self) -> Result<Vec<RemoteVersion>>;

    /// Resolve user input (e.g. "22", "20.18", "latest", "lts") to exact semver string (e.g. "22.11.0").
    async fn resolve_version(&self, input: &str) -> Result<String>;

    /// Download, verify checksum, and extract runtime version into ~/.synapse/runtimes/<runtime>/<version>.
    /// Returns the target installation path.
    async fn download(&self, version: &str) -> Result<PathBuf>;
}
