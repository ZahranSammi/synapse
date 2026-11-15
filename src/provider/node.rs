//! Node.js runtime provider: resolves versions, downloads prebuilt binaries,
//! verifies SHA256 checksums, and extracts .tar.xz archives.

use std::fs;
use std::path::PathBuf;

use indicatif::{ProgressBar, ProgressStyle};
use serde::Deserialize;

use crate::error::{Result, SynapseError};
use crate::platform::paths::{self, Arch};
use crate::platform::{archive, checksum};
use crate::provider::{RemoteVersion, RuntimeProvider};

const NODE_DIST_INDEX_URL: &str = "https://nodejs.org/dist/index.json";
const NODE_DIST_BASE_URL: &str = "https://nodejs.org/dist";
const MAX_PREALLOC_BYTES: usize = 64 * 1024 * 1024;
const MAX_DOWNLOAD_BYTES: usize = 250 * 1024 * 1024;

/// Release metadata structure returned by `https://nodejs.org/dist/index.json`.
#[derive(Debug, Clone, Deserialize)]
pub struct NodeRelease {
    pub version: String, // e.g. "v22.11.0"
    #[serde(default)]
    pub files: Vec<String>, // e.g. ["linux-x64", "linux-arm64", "src", ...]
    #[serde(default)]
    pub lts: serde_json::Value, // false or string such as "Jod"
}

impl NodeRelease {
    /// Returns true if this release is marked as LTS.
    pub fn is_lts(&self) -> bool {
        match &self.lts {
            serde_json::Value::Bool(b) => *b,
            serde_json::Value::String(_) => true,
            _ => false,
        }
    }

    /// Returns the version string stripped of the leading 'v'.
    pub fn clean_version(&self) -> &str {
        self.version.strip_prefix('v').unwrap_or(&self.version)
    }
}

/// Returns `(index_file_token, archive_arch_label, archive_ext)` for the given OS and arch.
pub fn target_artifact_spec_for(
    os: &str,
    arch: Arch,
) -> Result<(&'static str, &'static str, &'static str)> {
    match (os, arch) {
        ("linux", Arch::X64) => Ok(("linux-x64", "linux-x64", "tar.xz")),
        ("linux", Arch::Arm64) => Ok(("linux-arm64", "linux-arm64", "tar.xz")),
        ("windows", Arch::X64) => Ok(("win-x64-zip", "win-x64", "zip")),
        ("windows", Arch::Arm64) => Ok(("win-arm64-zip", "win-arm64", "zip")),
        _ => Err(SynapseError::UnsupportedArchitecture(format!(
            "{}-{}",
            os,
            arch.as_str()
        ))),
    }
}

/// Provider for official Node.js releases.
#[derive(Debug, Default)]
pub struct NodeProvider;

impl NodeProvider {
    pub fn new() -> Self {
        Self
    }

    fn target_artifact_spec(&self) -> Result<(&'static str, &'static str, &'static str)> {
        target_artifact_spec_for(std::env::consts::OS, paths::detect_arch()?)
    }

    fn client(&self) -> Result<reqwest::Client> {
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(15))
            .build()?;
        Ok(client)
    }

    /// Fetches all release metadata from `nodejs.org/dist/index.json`.
    async fn fetch_index(&self) -> Result<Vec<NodeRelease>> {
        let client = self.client()?;
        let releases = client
            .get(NODE_DIST_INDEX_URL)
            .send()
            .await?
            .json::<Vec<NodeRelease>>()
            .await?;
        Ok(releases)
    }
}

impl RuntimeProvider for NodeProvider {
    fn name(&self) -> &'static str {
        "node"
    }

    fn binary_names(&self) -> &'static [&'static str] {
        &["node", "npm", "npx"]
    }

    fn bin_subdir(&self) -> &'static str {
        if cfg!(windows) { "" } else { "bin" }
    }

    async fn list_remote(&self) -> Result<Vec<RemoteVersion>> {
        let (arch_file_pattern, _, _) = self.target_artifact_spec()?;
        let releases = self.fetch_index().await?;
        let versions = releases
            .into_iter()
            .filter(|r| r.files.iter().any(|f| f == arch_file_pattern))
            .map(|r| {
                let lts = match &r.lts {
                    serde_json::Value::String(name) => Some(name.clone()),
                    serde_json::Value::Bool(true) => Some("LTS".to_string()),
                    _ => None,
                };
                RemoteVersion {
                    version: r.clean_version().to_string(),
                    lts,
                }
            })
            .collect();
        Ok(versions)
    }

    async fn resolve_version(&self, input: &str) -> Result<String> {
        let (arch_file_pattern, _, _) = self.target_artifact_spec()?;
        let releases = self.fetch_index().await?;
        match_version(&releases, input, arch_file_pattern)
            .ok_or_else(|| SynapseError::VersionNotFound(input.to_string(), self.name().to_string()))
    }

    async fn download(&self, version_input: &str) -> Result<PathBuf> {
        let resolved_version = self.resolve_version(version_input).await?;
        let target_dir = paths::runtime_version_dir(self.name(), &resolved_version)?;

        let installed_bin = if cfg!(windows) {
            target_dir.join("node.exe")
        } else {
            target_dir.join("bin").join("node")
        };
        if installed_bin.is_file() {
            println!("Node.js v{} is already installed at {}", resolved_version, target_dir.display());
            return Ok(target_dir);
        }

        let (_, arch_label, ext) = self.target_artifact_spec()?;
        let file_name = format!("node-v{}-{}.{}", resolved_version, arch_label, ext);
        let base_url = format!("{}/v{}", NODE_DIST_BASE_URL, resolved_version);
        let archive_url = format!("{}/{}", base_url, file_name);
        let shasums_url = format!("{}/SHASUMS256.txt", base_url);

        let client = self.client()?;

        // 1. Fetch checksum file
        let shasums_content = client.get(&shasums_url).send().await?.text().await?;
        let expected_checksum = checksum::parse_checksum(&shasums_content, &file_name).ok_or_else(|| {
            SynapseError::ChecksumNotFound(file_name.clone(), shasums_url.clone())
        })?;

        // 2. Download archive with progress bar
        let mut response = client.get(&archive_url).send().await?;
        if !response.status().is_success() {
            return Err(SynapseError::Extraction(format!(
                "Failed to download {}: HTTP {}",
                archive_url,
                response.status()
            )));
        }

        let total_size = response.content_length().unwrap_or(0);
        if total_size as usize > MAX_DOWNLOAD_BYTES {
            return Err(SynapseError::Extraction(format!(
                "Archive {} exceeds maximum allowed size ({} bytes)",
                file_name, MAX_DOWNLOAD_BYTES
            )));
        }
        let pb = ProgressBar::new(total_size);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta}) {msg}")
                .unwrap_or_else(|_| ProgressStyle::default_bar())
                .progress_chars("#>-"),
        );
        pb.set_message(format!("Downloading Node.js v{}", resolved_version));

        let prealloc = if total_size > 0 {
            (total_size as usize).min(MAX_PREALLOC_BYTES)
        } else {
            35 * 1024 * 1024
        };
        let mut buffer = Vec::with_capacity(prealloc);
        while let Some(chunk) = response.chunk().await? {
            if buffer.len().saturating_add(chunk.len()) > MAX_DOWNLOAD_BYTES {
                return Err(SynapseError::Extraction(format!(
                    "Archive {} exceeds maximum allowed download size ({} bytes)",
                    file_name, MAX_DOWNLOAD_BYTES
                )));
            }
            buffer.extend_from_slice(&chunk);
            pb.inc(chunk.len() as u64);
        }
        pb.finish_with_message(format!("Downloaded Node.js v{}", resolved_version));

        // 3. Verify SHA256 checksum
        checksum::verify_sha256(&buffer, &expected_checksum, &file_name)?;

        // 4. Extract to temporary folder then atomically rename
        paths::ensure_dirs()?;
        let temp_dir = paths::runtime_dir(self.name())?.join(format!(".tmp_{}_{}", resolved_version, std::process::id()));
        if temp_dir.exists() {
            let _ = fs::remove_dir_all(&temp_dir);
        }
        fs::create_dir_all(&temp_dir)?;

        if ext == "zip" {
            archive::extract_zip(std::io::Cursor::new(buffer), &temp_dir)?;
        } else {
            archive::extract_tar_xz(std::io::Cursor::new(buffer), &temp_dir)?;
        }

        // Set executable permissions for binaries
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let bin_path = temp_dir.join("bin");
            if let Ok(entries) = fs::read_dir(bin_path) {
                for entry in entries.flatten() {
                    if let Ok(metadata) = entry.metadata() {
                        let mut perms = metadata.permissions();
                        perms.set_mode(0o755);
                        let _ = fs::set_permissions(entry.path(), perms);
                    }
                }
            }
        }

        if target_dir.exists() {
            fs::remove_dir_all(&target_dir)?;
        }
        fs::rename(&temp_dir, &target_dir)?;

        Ok(target_dir)
    }
}

/// Matches user version input against a list of Node.js releases for the specified architecture.
pub fn match_version(releases: &[NodeRelease], input: &str, arch_target: &str) -> Option<String> {
    let trimmed = input.trim();
    let clean_input = trimmed
        .strip_prefix('v')
        .or_else(|| trimmed.strip_prefix('V'))
        .unwrap_or(trimmed);

    let eligible: Vec<&NodeRelease> = releases
        .iter()
        .filter(|r| r.files.iter().any(|f| f == arch_target))
        .collect();

    if eligible.is_empty() {
        return None;
    }

    if clean_input.eq_ignore_ascii_case("latest") {
        return Some(eligible.first()?.clean_version().to_string());
    }

    if clean_input.eq_ignore_ascii_case("lts") {
        let lts_release = eligible.iter().find(|r| r.is_lts())?;
        return Some(lts_release.clean_version().to_string());
    }

    // Direct exact match
    if let Some(exact) = eligible.iter().find(|r| r.clean_version() == clean_input) {
        return Some(exact.clean_version().to_string());
    }

    // Partial prefix match (e.g. "22" or "22.11")
    let prefix = format!("{}.", clean_input);
    let mut matching_semver: Vec<semver::Version> = eligible
        .iter()
        .filter(|r| r.clean_version().starts_with(&prefix))
        .filter_map(|r| semver::Version::parse(r.clean_version()).ok())
        .collect();

    if !matching_semver.is_empty() {
        matching_semver.sort();
        return matching_semver.pop().map(|v| v.to_string());
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_match_version() {
        let releases = vec![
            NodeRelease {
                version: "v23.1.0".to_string(),
                files: vec!["linux-x64".to_string(), "linux-arm64".to_string()],
                lts: serde_json::Value::Bool(false),
            },
            NodeRelease {
                version: "v22.11.0".to_string(),
                files: vec!["linux-x64".to_string(), "linux-arm64".to_string()],
                lts: serde_json::Value::String("Jod".to_string()),
            },
            NodeRelease {
                version: "v22.10.0".to_string(),
                files: vec!["linux-x64".to_string()],
                lts: serde_json::Value::String("Jod".to_string()),
            },
            NodeRelease {
                version: "v20.18.0".to_string(),
                files: vec!["linux-x64".to_string(), "linux-arm64".to_string()],
                lts: serde_json::Value::String("Iron".to_string()),
            },
        ];

        // "latest"
        assert_eq!(match_version(&releases, "latest", "linux-x64"), Some("23.1.0".to_string()));
        // "lts"
        assert_eq!(match_version(&releases, "lts", "linux-x64"), Some("22.11.0".to_string()));
        // Partial major "22" resolves to highest 22.x
        assert_eq!(match_version(&releases, "22", "linux-x64"), Some("22.11.0".to_string()));
        // Partial with leading 'v'
        assert_eq!(match_version(&releases, "v22", "linux-x64"), Some("22.11.0".to_string()));
        // Partial minor "22.10"
        assert_eq!(match_version(&releases, "22.10", "linux-x64"), Some("22.10.0".to_string()));
        // Exact version
        assert_eq!(match_version(&releases, "20.18.0", "linux-x64"), Some("20.18.0".to_string()));
        // Non-existent
        assert_eq!(match_version(&releases, "18", "linux-x64"), None);
    }

    #[test]
    fn test_node_target_artifact_spec() {
        assert_eq!(
            target_artifact_spec_for("linux", Arch::X64).unwrap(),
            ("linux-x64", "linux-x64", "tar.xz")
        );
        assert_eq!(
            target_artifact_spec_for("linux", Arch::Arm64).unwrap(),
            ("linux-arm64", "linux-arm64", "tar.xz")
        );
        assert_eq!(
            target_artifact_spec_for("windows", Arch::X64).unwrap(),
            ("win-x64-zip", "win-x64", "zip")
        );
        assert_eq!(
            target_artifact_spec_for("windows", Arch::Arm64).unwrap(),
            ("win-arm64-zip", "win-arm64", "zip")
        );
        assert!(target_artifact_spec_for("freebsd", Arch::X64).is_err());
    }
}
