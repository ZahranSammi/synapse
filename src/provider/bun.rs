//! Bun.js runtime provider: resolves versions via GitHub releases API,
//! downloads prebuilt zip archives, verifies SHA256 digests, and extracts binaries.

use std::fs;
use std::path::PathBuf;

use indicatif::{ProgressBar, ProgressStyle};
use serde::Deserialize;

use crate::error::{Result, SynapseError};
use crate::platform::paths::{self, Arch};
use crate::platform::{archive, checksum};
use crate::provider::{RemoteVersion, RuntimeProvider};

const GITHUB_API_BASE: &str = "https://api.github.com/repos/oven-sh/bun/releases";
const MAX_PREALLOC_BYTES: usize = 64 * 1024 * 1024;
const MAX_DOWNLOAD_BYTES: usize = 250 * 1024 * 1024;

/// Release metadata structure returned by GitHub Releases API.
#[derive(Debug, Clone, Deserialize)]
pub struct BunRelease {
    pub tag_name: String, // e.g. "bun-v1.4.2"
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub assets: Vec<BunAsset>,
}

impl BunRelease {
    /// Returns the version string stripped of leading 'bun-v' or 'v'.
    pub fn clean_version(&self) -> &str {
        self.tag_name
            .strip_prefix("bun-v")
            .or_else(|| self.tag_name.strip_prefix('v'))
            .unwrap_or(&self.tag_name)
    }
}

/// Asset metadata structure in a GitHub Release.
#[derive(Debug, Clone, Deserialize)]
pub struct BunAsset {
    pub name: String, // e.g. "bun-linux-x64.zip"
    pub browser_download_url: String,
    pub digest: Option<String>, // e.g. "sha256:36368faef..."
}

/// Returns the Bun release zip filename for the given OS and architecture.
pub fn target_asset_name_for(os: &str, arch: Arch) -> Result<&'static str> {
    match (os, arch) {
        ("linux", Arch::X64) => Ok("bun-linux-x64.zip"),
        ("linux", Arch::Arm64) => Ok("bun-linux-aarch64.zip"),
        ("windows", Arch::X64) => Ok("bun-windows-x64.zip"),
        ("windows", Arch::Arm64) => Ok("bun-windows-aarch64.zip"),
        _ => Err(SynapseError::UnsupportedArchitecture(format!(
            "{}-{}",
            os,
            arch.as_str()
        ))),
    }
}

/// Provider for official Bun releases.
#[derive(Debug, Default)]
pub struct BunProvider;

impl BunProvider {
    pub fn new() -> Self {
        Self
    }

    /// Creates a reqwest Client configured with User-Agent header and connect timeout.
    fn client(&self) -> Result<reqwest::Client> {
        let user_agent = format!("synapse/{}", env!("CARGO_PKG_VERSION"));
        let client = reqwest::Client::builder()
            .user_agent(user_agent)
            .connect_timeout(std::time::Duration::from_secs(15))
            .build()?;
        Ok(client)
    }

    /// Returns the target asset filename for the detected system OS and architecture.
    fn target_asset_name(&self) -> Result<&'static str> {
        target_asset_name_for(std::env::consts::OS, paths::detect_arch()?)
    }

    /// Fetches all releases across GitHub API pages.
    async fn fetch_releases(&self) -> Result<Vec<BunRelease>> {
        let client = self.client()?;
        let mut all_releases = Vec::new();

        for page in 1usize.. {
            let url = format!("{}?per_page=100&page={}", GITHUB_API_BASE, page);
            let resp = client.get(&url).send().await?.error_for_status()?;
            let releases: Vec<BunRelease> = resp.json().await?;
            if releases.is_empty() {
                break;
            }
            let count = releases.len();
            all_releases.extend(releases);
            if count < 100 {
                break;
            }
        }

        Ok(all_releases)
    }

    /// Fetches release details for a specific tag (e.g. "bun-v1.4.2").
    async fn fetch_release_by_tag(&self, version: &str) -> Result<BunRelease> {
        let client = self.client()?;
        let url = format!("{}/tags/bun-v{}", GITHUB_API_BASE, version);
        let resp = client.get(&url).send().await?.error_for_status()?;
        let release: BunRelease = resp.json().await?;
        Ok(release)
    }
}

impl RuntimeProvider for BunProvider {
    fn name(&self) -> &'static str {
        "bun"
    }

    fn binary_names(&self) -> &'static [&'static str] {
        &["bun", "bunx"]
    }

    fn bin_subdir(&self) -> &'static str {
        ""
    }

    async fn list_remote(&self) -> Result<Vec<RemoteVersion>> {
        let releases = self.fetch_releases().await?;
        let versions = releases
            .into_iter()
            .filter(|r| !r.draft && !r.prerelease)
            .map(|r| RemoteVersion {
                version: r.clean_version().to_string(),
                lts: None,
            })
            .collect();
        Ok(versions)
    }

    async fn resolve_version(&self, input: &str) -> Result<String> {
        let client = self.client()?;
        let mut all_releases = Vec::new();

        for page in 1usize.. {
            let url = format!("{}?per_page=100&page={}", GITHUB_API_BASE, page);
            let resp = client.get(&url).send().await?.error_for_status()?;
            let releases: Vec<BunRelease> = resp.json().await?;
            if releases.is_empty() {
                break;
            }
            let count = releases.len();
            all_releases.extend(releases);

            if let Some(matched) = match_version(&all_releases, input) {
                return Ok(matched);
            }

            if count < 100 {
                break;
            }
        }

        Err(SynapseError::VersionNotFound(
            input.to_string(),
            self.name().to_string(),
        ))
    }

    async fn download(&self, version_input: &str) -> Result<PathBuf> {
        let resolved_version = self.resolve_version(version_input).await?;
        let target_dir = paths::runtime_version_dir(self.name(), &resolved_version)?;

        let bun_bin = target_dir.join(if cfg!(windows) { "bun.exe" } else { "bun" });
        if bun_bin.is_file() {
            println!(
                "Bun v{} is already installed at {}",
                resolved_version,
                target_dir.display()
            );
            return Ok(target_dir);
        }

        let asset_name = self.target_asset_name()?;
        let release = self.fetch_release_by_tag(&resolved_version).await?;
        let asset = release
            .assets
            .iter()
            .find(|a| a.name == asset_name)
            .ok_or_else(|| {
                SynapseError::Extraction(format!(
                    "Asset '{}' not found in Bun release v{}",
                    asset_name, resolved_version
                ))
            })?;

        let client = self.client()?;
        let mut expected_checksum = asset.digest.as_ref().and_then(|d| {
            d.strip_prefix("sha256:")
                .map(|h| h.to_lowercase())
        });

        if expected_checksum.is_none()
            && let Some(shasums_asset) = release.assets.iter().find(|a| a.name == "SHASUMS256.txt")
        {
            let shasums_text = client
                .get(&shasums_asset.browser_download_url)
                .send()
                .await?
                .error_for_status()?
                .text()
                .await?;
            expected_checksum = checksum::parse_checksum(&shasums_text, &asset.name);
        }

        let expected_checksum = expected_checksum.ok_or_else(|| {
            SynapseError::ChecksumNotFound(
                asset.name.clone(),
                format!("Bun release v{}", resolved_version),
            )
        })?;

        let mut response = client
            .get(&asset.browser_download_url)
            .send()
            .await?
            .error_for_status()?;

        let total_size = response.content_length().unwrap_or(0);
        if total_size as usize > MAX_DOWNLOAD_BYTES {
            return Err(SynapseError::Extraction(format!(
                "Asset {} exceeds maximum allowed size ({} bytes)",
                asset.name, MAX_DOWNLOAD_BYTES
            )));
        }
        let pb = ProgressBar::new(total_size);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta}) {msg}")
                .unwrap_or_else(|_| ProgressStyle::default_bar())
                .progress_chars("#>-"),
        );
        pb.set_message(format!("Downloading Bun v{}", resolved_version));

        let prealloc = if total_size > 0 {
            (total_size as usize).min(MAX_PREALLOC_BYTES)
        } else {
            50 * 1024 * 1024
        };
        let mut buffer = Vec::with_capacity(prealloc);
        while let Some(chunk) = response.chunk().await? {
            if buffer.len().saturating_add(chunk.len()) > MAX_DOWNLOAD_BYTES {
                return Err(SynapseError::Extraction(format!(
                    "Asset {} exceeds maximum allowed download size ({} bytes)",
                    asset.name, MAX_DOWNLOAD_BYTES
                )));
            }
            buffer.extend_from_slice(&chunk);
            pb.inc(chunk.len() as u64);
        }
        pb.finish_with_message(format!("Downloaded Bun v{}", resolved_version));

        checksum::verify_sha256(&buffer, &expected_checksum, &asset.name)?;

        // Extract to temporary folder then atomically rename
        paths::ensure_dirs()?;
        let temp_dir = paths::runtime_dir(self.name())?.join(format!(
            ".tmp_{}_{}",
            resolved_version,
            std::process::id()
        ));
        if temp_dir.exists() {
            let _ = fs::remove_dir_all(&temp_dir);
        }
        fs::create_dir_all(&temp_dir)?;

        archive::extract_zip(std::io::Cursor::new(buffer), &temp_dir)?;

        // Set executable permissions for bun binary
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let bun_path = temp_dir.join("bun");
            if bun_path.exists() {
                let _ = fs::set_permissions(&bun_path, fs::Permissions::from_mode(0o755));
            }

            // Create bunx symlink -> bun in the same directory
            let bunx_link = temp_dir.join("bunx");
            if bunx_link.exists() || bunx_link.is_symlink() {
                let _ = fs::remove_file(&bunx_link);
            }
            std::os::unix::fs::symlink("bun", &bunx_link)?;
        }

        if target_dir.exists() {
            fs::remove_dir_all(&target_dir)?;
        }
        fs::rename(&temp_dir, &target_dir)?;

        Ok(target_dir)
    }
}

/// Matches user version input against a list of Bun releases.
pub fn match_version(releases: &[BunRelease], input: &str) -> Option<String> {
    let trimmed = input.trim();
    let clean_input = trimmed
        .strip_prefix("bun-v")
        .or_else(|| trimmed.strip_prefix('v'))
        .unwrap_or(trimmed);

    let eligible: Vec<&BunRelease> = releases
        .iter()
        .filter(|r| !r.draft && !r.prerelease)
        .collect();

    if eligible.is_empty() {
        return None;
    }

    if clean_input.eq_ignore_ascii_case("latest") {
        return Some(eligible.first()?.clean_version().to_string());
    }

    // Direct exact match
    if let Some(exact) = eligible.iter().find(|r| r.clean_version() == clean_input) {
        return Some(exact.clean_version().to_string());
    }

    // Partial prefix match (e.g. "1.4" or "1" resolves to highest matching semver)
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
    fn test_match_version_bun() {
        let releases = vec![
            BunRelease {
                tag_name: "bun-v1.4.3".to_string(),
                draft: false,
                prerelease: true,
                assets: vec![],
            },
            BunRelease {
                tag_name: "bun-v1.4.2".to_string(),
                draft: false,
                prerelease: false,
                assets: vec![],
            },
            BunRelease {
                tag_name: "bun-v1.4.1".to_string(),
                draft: false,
                prerelease: false,
                assets: vec![],
            },
            BunRelease {
                tag_name: "bun-v1.3.9".to_string(),
                draft: false,
                prerelease: false,
                assets: vec![],
            },
            BunRelease {
                tag_name: "bun-v1.0.0".to_string(),
                draft: false,
                prerelease: false,
                assets: vec![],
            },
        ];

        // "latest" skips prerelease
        assert_eq!(match_version(&releases, "latest"), Some("1.4.2".to_string()));
        // exact match
        assert_eq!(match_version(&releases, "1.4.1"), Some("1.4.1".to_string()));
        // with 'v' prefix
        assert_eq!(match_version(&releases, "v1.4.1"), Some("1.4.1".to_string()));
        // with 'bun-v' prefix
        assert_eq!(match_version(&releases, "bun-v1.4.1"), Some("1.4.1".to_string()));
        // partial match "1.4" -> highest 1.4.x
        assert_eq!(match_version(&releases, "1.4"), Some("1.4.2".to_string()));
        // partial match "1" -> highest 1.x
        assert_eq!(match_version(&releases, "1"), Some("1.4.2".to_string()));
        // non-existent
        assert_eq!(match_version(&releases, "2.0"), None);
    }

    #[test]
    fn test_bun_target_asset_name_for() {
        assert_eq!(
            target_asset_name_for("linux", Arch::X64).unwrap(),
            "bun-linux-x64.zip"
        );
        assert_eq!(
            target_asset_name_for("linux", Arch::Arm64).unwrap(),
            "bun-linux-aarch64.zip"
        );
        assert_eq!(
            target_asset_name_for("windows", Arch::X64).unwrap(),
            "bun-windows-x64.zip"
        );
        assert_eq!(
            target_asset_name_for("windows", Arch::Arm64).unwrap(),
            "bun-windows-aarch64.zip"
        );
        assert!(target_asset_name_for("freebsd", Arch::X64).is_err());
    }
}
