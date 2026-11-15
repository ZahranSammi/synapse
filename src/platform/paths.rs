//! Synapse directory paths and platform architecture detection.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Result, SynapseError};

/// Supported CPU architectures on Linux.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    X64,
    Arm64,
}

impl Arch {
    /// Return the architecture string used in release artifacts (e.g., Node.js "x64" / "arm64").
    pub fn as_str(&self) -> &'static str {
        match self {
            Arch::X64 => "x64",
            Arch::Arm64 => "arm64",
        }
    }
}

/// Detect the machine CPU architecture using `std::env::consts::ARCH`.
pub fn detect_arch() -> Result<Arch> {
    match std::env::consts::ARCH {
        "x86_64" => Ok(Arch::X64),
        "aarch64" => Ok(Arch::Arm64),
        other => Err(SynapseError::UnsupportedArchitecture(other.to_string())),
    }
}

/// Returns the base Synapse directory (`~/.synapse`).
pub fn synapse_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or(SynapseError::HomeDirNotFound)?;
    Ok(home.join(".synapse"))
}

/// Returns the shims/binaries directory (`~/.synapse/bin`).
pub fn bin_dir() -> Result<PathBuf> {
    Ok(synapse_dir()?.join("bin"))
}

/// Returns the runtimes base directory (`~/.synapse/runtimes`).
pub fn runtimes_dir() -> Result<PathBuf> {
    Ok(synapse_dir()?.join("runtimes"))
}

/// Returns the directory for a specific runtime (e.g., `~/.synapse/runtimes/node`).
pub fn runtime_dir(runtime: &str) -> Result<PathBuf> {
    Ok(runtimes_dir()?.join(runtime))
}

/// Validates that a version string is safe for use as a single path component.
pub fn validate_version_str(version: &str) -> Result<()> {
    if version.is_empty()
        || version == "."
        || version == ".."
        || !version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(SynapseError::InvalidVersion(version.to_string()));
    }
    Ok(())
}

/// Returns the directory for an installed runtime version (e.g., `~/.synapse/runtimes/node/22.11.0`).
pub fn runtime_version_dir(runtime: &str, version: &str) -> Result<PathBuf> {
    validate_version_str(version)?;
    Ok(runtime_dir(runtime)?.join(version))
}

/// Returns the current active symlinks directory (`~/.synapse/current`).
pub fn current_dir() -> Result<PathBuf> {
    Ok(synapse_dir()?.join("current"))
}

/// Returns the symlink path for the active version of a runtime (e.g., `~/.synapse/current/node`).
pub fn current_runtime_symlink(runtime: &str) -> Result<PathBuf> {
    Ok(current_dir()?.join(runtime))
}

/// Returns the global packages directory (`~/.synapse/globals`).
pub fn globals_dir() -> Result<PathBuf> {
    Ok(synapse_dir()?.join("globals"))
}

/// Returns the global packages manifest path for a runtime (e.g., `~/.synapse/globals/node/packages.txt`).
pub fn globals_manifest_path(runtime: &str) -> Result<PathBuf> {
    Ok(globals_dir()?.join(runtime).join("packages.txt"))
}

/// Returns the service data base directory (`~/.synapse/data`).
pub fn data_dir() -> Result<PathBuf> {
    Ok(synapse_dir()?.join("data"))
}

/// Returns the service data directory for a specific service (e.g., `~/.synapse/data/postgres`).
pub fn service_data_dir(service: &str) -> Result<PathBuf> {
    Ok(data_dir()?.join(service))
}

/// Creates all required Synapse base directories under `~/.synapse` if they do not already exist.
pub fn ensure_dirs() -> Result<()> {
    ensure_dirs_at(&synapse_dir()?)
}

/// Creates all required Synapse base directories under the specified base directory.
pub fn ensure_dirs_at(base: &Path) -> Result<()> {
    fs::create_dir_all(base.join("bin"))?;
    fs::create_dir_all(base.join("runtimes").join("node"))?;
    fs::create_dir_all(base.join("current"))?;
    fs::create_dir_all(base.join("globals"))?;
    fs::create_dir_all(base.join("data"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_arch() {
        // On x86_64 or aarch64 linux machines, detect_arch should succeed.
        let arch = detect_arch();
        assert!(arch.is_ok());
        let arch = arch.unwrap();
        assert!(arch == Arch::X64 || arch == Arch::Arm64);
        assert!(arch.as_str() == "x64" || arch.as_str() == "arm64");
    }

    #[test]
    fn test_ensure_dirs_at() {
        let temp_dir = std::env::temp_dir().join(format!("synapse_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);

        let res = ensure_dirs_at(&temp_dir);
        assert!(res.is_ok());

        assert!(temp_dir.join("bin").is_dir());
        assert!(temp_dir.join("runtimes").join("node").is_dir());
        assert!(temp_dir.join("current").is_dir());
        assert!(temp_dir.join("globals").is_dir());
        assert!(temp_dir.join("data").is_dir());

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_path_helpers() {
        let synapse = synapse_dir().expect("synapse_dir should resolve with home");
        assert_eq!(bin_dir().unwrap(), synapse.join("bin"));
        assert_eq!(runtimes_dir().unwrap(), synapse.join("runtimes"));
        assert_eq!(runtime_dir("node").unwrap(), synapse.join("runtimes").join("node"));
        assert_eq!(
            runtime_version_dir("node", "22.11.0").unwrap(),
            synapse.join("runtimes").join("node").join("22.11.0")
        );
        assert_eq!(current_dir().unwrap(), synapse.join("current"));
        assert_eq!(current_runtime_symlink("node").unwrap(), synapse.join("current").join("node"));
        assert_eq!(data_dir().unwrap(), synapse.join("data"));
        assert_eq!(
            service_data_dir("postgres").unwrap(),
            synapse.join("data").join("postgres")
        );
    }

    #[test]
    fn test_validate_version_str() {
        assert!(validate_version_str("22.11.0").is_ok());
        assert!(validate_version_str("27.0.0-alpha.0.1.0").is_ok());
        assert!(validate_version_str("1.4.2_rc1").is_ok());

        assert!(matches!(
            validate_version_str(""),
            Err(SynapseError::InvalidVersion(_))
        ));
        assert!(matches!(
            validate_version_str("."),
            Err(SynapseError::InvalidVersion(_))
        ));
        assert!(matches!(
            validate_version_str(".."),
            Err(SynapseError::InvalidVersion(_))
        ));
        assert!(matches!(
            validate_version_str("../etc"),
            Err(SynapseError::InvalidVersion(_))
        ));
        assert!(matches!(
            validate_version_str("22/../../foo"),
            Err(SynapseError::InvalidVersion(_))
        ));
        assert!(matches!(
            runtime_version_dir("node", "../etc"),
            Err(SynapseError::InvalidVersion(_))
        ));
    }
}
