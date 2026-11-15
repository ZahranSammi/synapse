//! Composer runtime provider: resolves versions via GitHub Releases API,
//! downloads official `composer.phar`, verifies SHA256 digest when available,
//! and installs it as an executable `composer` binary.

use std::fs;
use std::path::PathBuf;

use indicatif::{ProgressBar, ProgressStyle};
use serde::Deserialize;

use crate::error::{Result, SynapseError};
use crate::platform::{checksum, paths};
use crate::provider::{RemoteVersion, RuntimeProvider};

const COMPOSER_RELEASES_API: &str = "https://api.github.com/repos/composer/composer/releases";

#[derive(Debug, Clone, Deserialize)]
pub struct ComposerRelease {
    pub tag_name: String,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub assets: Vec<ComposerAsset>,
}

impl ComposerRelease {
    pub fn clean_version(&self) -> &str {
        self.tag_name.strip_prefix('v').unwrap_or(&self.tag_name)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ComposerAsset {
    pub name: String,
    pub browser_download_url: String,
    pub digest: Option<String>,
}

/// Provider for official Composer releases (`composer/composer`).
#[derive(Debug, Default)]
pub struct ComposerProvider;

impl ComposerProvider {
    pub fn new() -> Self {
        Self
    }

    fn client(&self) -> Result<reqwest::Client> {
        let user_agent = format!("synapse/{}", env!("CARGO_PKG_VERSION"));
        let client = reqwest::Client::builder()
            .user_agent(user_agent)
            .connect_timeout(std::time::Duration::from_secs(15))
            .build()?;
        Ok(client)
    }

    async fn fetch_releases(&self) -> Result<Vec<ComposerRelease>> {
        let client = self.client()?;
        let mut all_releases = Vec::new();

        for page in 1usize.. {
            let url = format!("{}?per_page=100&page={}", COMPOSER_RELEASES_API, page);
            let resp = client.get(&url).send().await?.error_for_status()?;
            let releases: Vec<ComposerRelease> = resp.json().await?;
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

    async fn fetch_release_by_tag(&self, version: &str) -> Result<ComposerRelease> {
        let client = self.client()?;
        let url = format!("{}/tags/{}", COMPOSER_RELEASES_API, version);
        let resp = client.get(&url).send().await?.error_for_status()?;
        let release: ComposerRelease = resp.json().await?;
        Ok(release)
    }
}

impl RuntimeProvider for ComposerProvider {
    fn name(&self) -> &'static str {
        "composer"
    }

    fn binary_names(&self) -> &'static [&'static str] {
        &["composer"]
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
            let url = format!("{}?per_page=100&page={}", COMPOSER_RELEASES_API, page);
            let resp = client.get(&url).send().await?.error_for_status()?;
            let releases: Vec<ComposerRelease> = resp.json().await?;
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

        if target_dir.join("composer").is_file() {
            println!(
                "Composer v{} is already installed at {}",
                resolved_version,
                target_dir.display()
            );
            return Ok(target_dir);
        }

        let release = self.fetch_release_by_tag(&resolved_version).await?;
        let asset = release
            .assets
            .iter()
            .find(|a| a.name == "composer.phar")
            .ok_or_else(|| {
                SynapseError::VersionNotFound(
                    format!("{} (asset composer.phar)", resolved_version),
                    self.name().to_string(),
                )
            })?;

        let client = self.client()?;
        let mut expected_hash = asset
            .digest
            .as_ref()
            .and_then(|d| d.strip_prefix("sha256:"))
            .map(|h| h.to_lowercase());

        if expected_hash.is_none() {
            let sha_url = release
                .assets
                .iter()
                .find(|a| a.name == "composer.phar.sha256sum")
                .map(|a| a.browser_download_url.clone())
                .unwrap_or_else(|| {
                    format!(
                        "https://getcomposer.org/download/{}/composer.phar.sha256sum",
                        resolved_version
                    )
                });
            if let Ok(sha_resp) = client.get(&sha_url).send().await
                && sha_resp.status().is_success()
                && let Ok(text) = sha_resp.text().await
                && let Some(token) = text.split_whitespace().next()
                && token.len() == 64
                && token.chars().all(|c| c.is_ascii_hexdigit())
            {
                expected_hash = Some(token.to_lowercase());
            }
        }

        let expected_hash = expected_hash.ok_or_else(|| {
            SynapseError::ChecksumNotFound(
                "composer.phar".to_string(),
                format!("Composer release v{}", resolved_version),
            )
        })?;

        let temp_dir = paths::runtime_dir(self.name())?.join(format!(
            ".tmp_{}_{}",
            resolved_version,
            std::process::id()
        ));
        if temp_dir.exists() {
            fs::remove_dir_all(&temp_dir)?;
        }
        fs::create_dir_all(&temp_dir)?;

        let mut resp = client
            .get(&asset.browser_download_url)
            .send()
            .await?
            .error_for_status()?;
        let total_size = resp.content_length().unwrap_or(0);

        let pb = ProgressBar::new(total_size);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
                .unwrap_or_else(|_| ProgressStyle::default_bar())
                .progress_chars("#>-"),
        );

        let staged_bin = temp_dir.join("composer");
        {
            use std::io::Write;
            let mut file = fs::File::create(&staged_bin)?;
            while let Some(chunk) = resp.chunk().await? {
                file.write_all(&chunk)?;
                pb.inc(chunk.len() as u64);
            }
        }
        pb.finish_with_message("Downloaded");

        println!("Verifying SHA256 checksum...");
        let bytes = fs::read(&staged_bin)?;
        checksum::verify_sha256(&bytes, &expected_hash, "composer.phar")?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&staged_bin, fs::Permissions::from_mode(0o755))?;
        }

        if let Some(parent) = target_dir.parent() {
            fs::create_dir_all(parent)?;
        }
        if target_dir.exists() {
            fs::remove_dir_all(&target_dir)?;
        }
        fs::rename(&temp_dir, &target_dir)?;

        Ok(target_dir)
    }
}

/// Resolves user version input (`"latest"`, `"2"`, `"2.8"`, `"2.8.3"`) against Composer releases.
pub fn match_version(releases: &[ComposerRelease], input: &str) -> Option<String> {
    let trimmed = input.trim();
    let clean = trimmed.strip_prefix('v').unwrap_or(trimmed);
    let is_latest = clean.eq_ignore_ascii_case("latest");

    for r in releases {
        if r.draft || r.prerelease {
            continue;
        }
        let ver = r.clean_version();
        if is_latest || ver == clean || ver.starts_with(&format!("{}.", clean)) {
            return Some(ver.to_string());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_match_version_composer() {
        let releases = vec![
            ComposerRelease {
                tag_name: "2.9.0-RC1".to_string(),
                draft: false,
                prerelease: true,
                assets: vec![],
            },
            ComposerRelease {
                tag_name: "2.8.3".to_string(),
                draft: false,
                prerelease: false,
                assets: vec![],
            },
            ComposerRelease {
                tag_name: "2.7.9".to_string(),
                draft: false,
                prerelease: false,
                assets: vec![],
            },
            ComposerRelease {
                tag_name: "1.10.27".to_string(),
                draft: false,
                prerelease: false,
                assets: vec![],
            },
        ];

        assert_eq!(match_version(&releases, "latest"), Some("2.8.3".to_string()));
        assert_eq!(match_version(&releases, "2"), Some("2.8.3".to_string()));
        assert_eq!(match_version(&releases, "2.7"), Some("2.7.9".to_string()));
        assert_eq!(match_version(&releases, "1"), Some("1.10.27".to_string()));
        assert_eq!(match_version(&releases, "v2.8.3"), Some("2.8.3".to_string()));
        assert_eq!(match_version(&releases, "3"), None);
    }
}
