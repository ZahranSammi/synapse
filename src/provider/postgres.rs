//! PostgreSQL runtime provider: resolves prebuilt portable PostgreSQL distributions
//! via `theseus-rs/postgresql-binaries` GitHub Releases, verifies SHA-256 checksums,
//! and extracts binaries into `~/.synapse/runtimes/postgres/<version>`.

use std::fs;
use std::path::PathBuf;

use indicatif::{ProgressBar, ProgressStyle};
use serde::Deserialize;

use crate::error::{Result, SynapseError};
use crate::platform::{archive, checksum, paths::{self, Arch}};
use crate::provider::{RemoteVersion, RuntimeProvider};

const POSTGRES_RELEASES_API: &str =
    "https://api.github.com/repos/theseus-rs/postgresql-binaries/releases";

#[derive(Debug, Clone, Deserialize)]
pub struct PostgresRelease {
    pub tag_name: String,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub assets: Vec<PostgresAsset>,
}

impl PostgresRelease {
    pub fn clean_version(&self) -> &str {
        self.tag_name.strip_prefix('v').unwrap_or(&self.tag_name)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct PostgresAsset {
    pub name: String,
    pub browser_download_url: String,
    pub digest: Option<String>,
}

/// Returns the Rust/LLVM target triple used in `theseus-rs/postgresql-binaries` asset names.
pub fn target_triple_for(os: &str, arch: Arch) -> Result<&'static str> {
    match (os, arch) {
        ("linux", Arch::X64) => Ok("x86_64-unknown-linux-gnu"),
        ("linux", Arch::Arm64) => Ok("aarch64-unknown-linux-gnu"),
        ("windows", Arch::X64) => Ok("x86_64-pc-windows-msvc"),
        ("windows", Arch::Arm64) => Ok("aarch64-pc-windows-msvc"),
        (other, _) => Err(SynapseError::UnsupportedArchitecture(other.to_string())),
    }
}

/// Resolves user version input (`"latest"`, `"17"`, `"17.2"`, `"17.2.0"`) from PostgreSQL releases.
pub fn match_version(releases: &[PostgresRelease], input: &str) -> Option<String> {
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

/// Provider for portable PostgreSQL releases (`theseus-rs/postgresql-binaries`).
#[derive(Debug, Default)]
pub struct PostgresProvider;

impl PostgresProvider {
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

    async fn fetch_releases(&self) -> Result<Vec<PostgresRelease>> {
        let client = self.client()?;
        let mut all_releases = Vec::new();

        for page in 1usize.. {
            let url = format!("{}?per_page=100&page={}", POSTGRES_RELEASES_API, page);
            let resp = client.get(&url).send().await?.error_for_status()?;
            let releases: Vec<PostgresRelease> = resp.json().await?;
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

    async fn fetch_release_for_version(&self, version: &str) -> Result<PostgresRelease> {
        let client = self.client()?;
        let tag_url = format!("{}/tags/{}", POSTGRES_RELEASES_API, version);
        if let Ok(resp) = client.get(&tag_url).send().await
            && resp.status().is_success()
            && let Ok(release) = resp.json::<PostgresRelease>().await
        {
            return Ok(release);
        }

        let releases = self.fetch_releases().await?;
        releases
            .into_iter()
            .find(|r| !r.draft && !r.prerelease && r.clean_version() == version)
            .ok_or_else(|| SynapseError::VersionNotFound(version.to_string(), "postgres".to_string()))
    }
}

impl RuntimeProvider for PostgresProvider {
    fn name(&self) -> &'static str {
        "postgres"
    }

    fn binary_names(&self) -> &'static [&'static str] {
        &["postgres", "psql", "pg_ctl", "initdb", "pg_dump", "createdb", "dropdb"]
    }

    fn bin_subdir(&self) -> &'static str {
        "bin"
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
            let url = format!("{}?per_page=100&page={}", POSTGRES_RELEASES_API, page);
            let resp = client.get(&url).send().await?.error_for_status()?;
            let releases: Vec<PostgresRelease> = resp.json().await?;
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
        let is_windows = std::env::consts::OS == "windows";
        let resolved_version = self.resolve_version(version_input).await?;
        let target_dir = paths::runtime_version_dir(self.name(), &resolved_version)?;

        let pg_bin_name = if is_windows { "postgres.exe" } else { "postgres" };
        if target_dir.join(self.bin_subdir()).join(pg_bin_name).is_file() {
            println!(
                "PostgreSQL v{} is already installed at {}",
                resolved_version,
                target_dir.display()
            );
            return Ok(target_dir);
        }

        let arch = paths::detect_arch()?;
        let triple = target_triple_for(std::env::consts::OS, arch)?;
        let asset_name = format!("postgresql-{}-{}.tar.gz", resolved_version, triple);

        let release = self.fetch_release_for_version(&resolved_version).await?;
        let asset = release
            .assets
            .iter()
            .find(|a| a.name == asset_name)
            .ok_or_else(|| {
                SynapseError::VersionNotFound(
                    format!("{} (asset {})", resolved_version, asset_name),
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
            let sha_asset_name = format!("{}.sha256", asset_name);
            if let Some(sha_asset) = release.assets.iter().find(|a| a.name == sha_asset_name)
                && let Ok(sha_resp) = client.get(&sha_asset.browser_download_url).send().await
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
                asset_name.clone(),
                format!("PostgreSQL release v{}", resolved_version),
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

        let archive_path = temp_dir.join(&asset_name);
        {
            use std::io::Write;
            let mut file = fs::File::create(&archive_path)?;
            while let Some(chunk) = resp.chunk().await? {
                file.write_all(&chunk)?;
                pb.inc(chunk.len() as u64);
            }
        }
        pb.finish_with_message("Downloaded");

        println!("Verifying SHA256 checksum...");
        let archive_bytes = fs::read(&archive_path)?;
        checksum::verify_sha256(&archive_bytes, &expected_hash, &asset_name)?;

        println!("Extracting {}...", asset_name);
        let extract_dir = temp_dir.join("extracted");
        fs::create_dir_all(&extract_dir)?;
        let archive_file = fs::File::open(&archive_path)?;
        archive::extract_tar_gz(archive_file, &extract_dir, false)?;

        let mut install_root = extract_dir.clone();
        if !install_root.join(self.bin_subdir()).join(pg_bin_name).is_file() {
            let mut nested_root = None;
            for entry in fs::read_dir(&extract_dir)? {
                let entry = entry?;
                let candidate = entry.path();
                if candidate.is_dir() && candidate.join(self.bin_subdir()).join(pg_bin_name).is_file() {
                    nested_root = Some(candidate);
                    break;
                }
            }
            match nested_root {
                Some(root) => install_root = root,
                None => {
                    return Err(SynapseError::Extraction(format!(
                        "Expected 'bin/{}' inside {}, but it was not found",
                        pg_bin_name, asset_name
                    )));
                }
            }
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let bin_folder = install_root.join(self.bin_subdir());
            if bin_folder.is_dir() {
                for entry in fs::read_dir(&bin_folder)? {
                    let entry = entry?;
                    let path = entry.path();
                    if path.is_file() {
                        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o755));
                    }
                }
            }
        }

        if let Some(parent) = target_dir.parent() {
            fs::create_dir_all(parent)?;
        }
        if target_dir.exists() {
            fs::remove_dir_all(&target_dir)?;
        }
        fs::rename(&install_root, &target_dir)?;
        let _ = fs::remove_dir_all(&temp_dir);

        Ok(target_dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_postgres_target_triple_for() {
        assert_eq!(
            target_triple_for("linux", Arch::X64).unwrap(),
            "x86_64-unknown-linux-gnu"
        );
        assert_eq!(
            target_triple_for("linux", Arch::Arm64).unwrap(),
            "aarch64-unknown-linux-gnu"
        );
        assert_eq!(
            target_triple_for("windows", Arch::X64).unwrap(),
            "x86_64-pc-windows-msvc"
        );
        assert_eq!(
            target_triple_for("windows", Arch::Arm64).unwrap(),
            "aarch64-pc-windows-msvc"
        );
        assert!(matches!(
            target_triple_for("macos", Arch::X64),
            Err(SynapseError::UnsupportedArchitecture(_))
        ));
    }

    #[test]
    fn test_match_version_postgres() {
        let releases = vec![
            PostgresRelease {
                tag_name: "17.3.0-rc1".to_string(),
                draft: false,
                prerelease: true,
                assets: vec![],
            },
            PostgresRelease {
                tag_name: "17.2.0".to_string(),
                draft: false,
                prerelease: false,
                assets: vec![],
            },
            PostgresRelease {
                tag_name: "v16.6.0".to_string(),
                draft: false,
                prerelease: false,
                assets: vec![],
            },
            PostgresRelease {
                tag_name: "15.10.0".to_string(),
                draft: false,
                prerelease: false,
                assets: vec![],
            },
        ];

        assert_eq!(match_version(&releases, "latest"), Some("17.2.0".to_string()));
        assert_eq!(match_version(&releases, "17"), Some("17.2.0".to_string()));
        assert_eq!(match_version(&releases, "16"), Some("16.6.0".to_string()));
        assert_eq!(match_version(&releases, "v16.6"), Some("16.6.0".to_string()));
        assert_eq!(match_version(&releases, "15.10.0"), Some("15.10.0".to_string()));
        assert_eq!(match_version(&releases, "14"), None);
    }
}
