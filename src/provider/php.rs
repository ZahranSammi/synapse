//! PHP runtime provider: resolves versions via static-php-cli directory index,
//! downloads prebuilt static Linux binaries (.tar.gz), and extracts the `php` executable.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use indicatif::{ProgressBar, ProgressStyle};
use semver::Version;

use crate::error::{Result, SynapseError};
use crate::platform::archive;
use crate::platform::paths::{self, Arch};
use crate::provider::{RemoteVersion, RuntimeProvider};

const STATIC_PHP_BASE_URL: &str = "https://dl.static-php.dev/static-php-cli/common/";
const WINDOWS_PHP_RELEASES_URL: &str = "https://windows.php.net/downloads/releases/";

/// Provider for prebuilt PHP CLI releases (`static-php-cli` on Linux, `windows.php.net` on Windows).
#[derive(Debug, Default)]
pub struct PhpProvider;

impl PhpProvider {
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

    /// Returns the target OS + architecture suffix for Linux static-php-cli artifacts.
    fn target_linux_platform(&self) -> Result<&'static str> {
        match paths::detect_arch()? {
            Arch::X64 => Ok("linux-x86_64"),
            Arch::Arm64 => Ok("linux-aarch64"),
        }
    }

    async fn fetch_available_versions(&self) -> Result<Vec<String>> {
        match std::env::consts::OS {
            "linux" => {
                let target = self.target_linux_platform()?;
                let client = self.client()?;
                let html = client
                    .get(STATIC_PHP_BASE_URL)
                    .send()
                    .await?
                    .error_for_status()?
                    .text()
                    .await?;
                Ok(parse_versions_from_index(&html, target))
            }
            "windows" => {
                let releases = self.fetch_windows_releases().await?;
                Ok(releases.into_iter().map(|(v, _)| v).collect())
            }
            other => Err(SynapseError::UnsupportedArchitecture(other.to_string())),
        }
    }

    async fn fetch_windows_releases(&self) -> Result<Vec<(String, String)>> {
        let arch = paths::detect_arch()?;
        let client = self.client()?;
        let html = client
            .get(WINDOWS_PHP_RELEASES_URL)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        Ok(parse_windows_php_index(&html, arch))
    }
}

impl RuntimeProvider for PhpProvider {
    fn name(&self) -> &'static str {
        "php"
    }

    fn binary_names(&self) -> &'static [&'static str] {
        &["php"]
    }

    fn bin_subdir(&self) -> &'static str {
        ""
    }

    async fn list_remote(&self) -> Result<Vec<RemoteVersion>> {
        let versions = self.fetch_available_versions().await?;
        Ok(versions
            .into_iter()
            .map(|version| RemoteVersion { version, lts: None })
            .collect())
    }

    async fn resolve_version(&self, input: &str) -> Result<String> {
        let versions = self.fetch_available_versions().await?;
        match_version(&versions, input)
            .ok_or_else(|| SynapseError::VersionNotFound(input.to_string(), self.name().to_string()))
    }

    async fn download(&self, version_input: &str) -> Result<PathBuf> {
        let is_windows = std::env::consts::OS == "windows";
        let resolved_version = self.resolve_version(version_input).await?;
        let target_dir = paths::runtime_version_dir(self.name(), &resolved_version)?;

        let php_bin_name = if is_windows { "php.exe" } else { "php" };
        if target_dir.join(php_bin_name).is_file() {
            println!(
                "PHP v{} is already installed at {}",
                resolved_version,
                target_dir.display()
            );
            return Ok(target_dir);
        }

        let (file_name, download_url) = if is_windows {
            let win_releases = self.fetch_windows_releases().await?;
            let (_, file_name) = win_releases
                .into_iter()
                .find(|(v, _)| v == &resolved_version)
                .ok_or_else(|| {
                    SynapseError::VersionNotFound(
                        resolved_version.clone(),
                        self.name().to_string(),
                    )
                })?;
            let url = format!("{}{}", WINDOWS_PHP_RELEASES_URL, file_name);
            (file_name, url)
        } else if std::env::consts::OS == "linux" {
            let target = self.target_linux_platform()?;
            let file_name = format!("php-{}-cli-{}.tar.gz", resolved_version, target);
            let url = format!("{}{}", STATIC_PHP_BASE_URL, file_name);
            (file_name, url)
        } else {
            return Err(SynapseError::UnsupportedArchitecture(
                std::env::consts::OS.to_string(),
            ));
        };

        let temp_dir = paths::runtime_dir(self.name())?.join(format!(
            ".tmp_{}_{}",
            resolved_version,
            std::process::id()
        ));
        if temp_dir.exists() {
            fs::remove_dir_all(&temp_dir)?;
        }
        fs::create_dir_all(&temp_dir)?;

        let client = self.client()?;
        let mut resp = client.get(&download_url).send().await?.error_for_status()?;
        let total_size = resp.content_length().unwrap_or(0);

        let pb = ProgressBar::new(total_size);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
                .unwrap_or_else(|_| ProgressStyle::default_bar())
                .progress_chars("#>-"),
        );

        let archive_path = temp_dir.join(&file_name);
        {
            use std::io::Write;
            let mut file = fs::File::create(&archive_path)?;
            while let Some(chunk) = resp.chunk().await? {
                file.write_all(&chunk)?;
                pb.inc(chunk.len() as u64);
            }
        }
        pb.finish_with_message("Downloaded");

        println!("Extracting {}...", file_name);
        let extract_dir = temp_dir.join("extracted");
        fs::create_dir_all(&extract_dir)?;

        let archive_file = fs::File::open(&archive_path)?;
        if is_windows {
            archive::extract_zip_opts(archive_file, &extract_dir, false)?;
        } else {
            archive::extract_tar_gz(archive_file, &extract_dir, false)?;
        }

        let php_bin = extract_dir.join(php_bin_name);
        if !php_bin.is_file() {
            return Err(SynapseError::Extraction(format!(
                "Expected '{}' binary inside {}, but it was not found",
                php_bin_name, file_name
            )));
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&php_bin, fs::Permissions::from_mode(0o755))?;
        }

        if let Some(parent) = target_dir.parent() {
            fs::create_dir_all(parent)?;
        }
        if target_dir.exists() {
            fs::remove_dir_all(&target_dir)?;
        }
        fs::rename(&extract_dir, &target_dir)?;
        let _ = fs::remove_dir_all(&temp_dir);

        Ok(target_dir)
    }
}

/// Extracts and sorts descending all valid semver versions from the static-php-cli HTML index.
pub fn parse_versions_from_index(html: &str, target: &str) -> Vec<String> {
    let suffix = format!("-cli-{}.tar.gz", target);
    let mut seen = HashSet::new();
    let mut parsed: Vec<(Version, String)> = Vec::new();

    let mut search_from = 0;
    while let Some(rel_idx) = html[search_from..].find(&suffix) {
        let suffix_start = search_from + rel_idx;
        let before = &html[search_from..suffix_start];
        if let Some(prefix_rel) = before.rfind("php-") {
            let ver_str = &before[prefix_rel + 4..];
            if !ver_str.is_empty()
                && ver_str.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
                && seen.insert(ver_str.to_string())
                && let Ok(v) = Version::parse(ver_str)
            {
                parsed.push((v, ver_str.to_string()));
            }
        }
        search_from = suffix_start + suffix.len();
    }

    parsed.sort_by(|a, b| b.0.cmp(&a.0));
    parsed.into_iter().map(|(_, s)| s).collect()
}

/// Extracts and sorts descending `(version, filename)` pairs from `windows.php.net/downloads/releases/` HTML.
pub fn parse_windows_php_index(html: &str, arch: Arch) -> Vec<(String, String)> {
    let arch_suffix = match arch {
        Arch::X64 => "-x64.zip",
        Arch::Arm64 => "-arm64.zip",
    };
    let marker = "-nts-Win32-";
    let mut seen = HashSet::new();
    let mut parsed: Vec<(Version, String, String)> = Vec::new();

    let mut search_from = 0;
    while let Some(rel_idx) = html[search_from..].find(marker) {
        let marker_start = search_from + rel_idx;
        let before = &html[search_from..marker_start];
        let after_start = marker_start + marker.len();
        let tail = &html[after_start..];
        let end_rel = tail
            .find(|c: char| matches!(c, '"' | '\'' | '<' | '>' | ' ' | '\t' | '\r' | '\n'))
            .unwrap_or(tail.len());
        let suffix_part = &tail[..end_rel];

        if suffix_part.ends_with(arch_suffix)
            && let Some(prefix_rel) = before.rfind("php-")
        {
            let ver_str = &before[prefix_rel + 4..];
            if !ver_str.is_empty()
                && ver_str.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
                && !suffix_part.contains('/')
                && seen.insert(ver_str.to_string())
                && let Ok(v) = Version::parse(ver_str)
            {
                let filename = format!("php-{}{}{}", ver_str, marker, suffix_part);
                parsed.push((v, ver_str.to_string(), filename));
            }
        }
        search_from = after_start;
    }

    parsed.sort_by(|a, b| b.0.cmp(&a.0));
    parsed.into_iter().map(|(_, v, f)| (v, f)).collect()
}

/// Resolves user version input (`"latest"`, `"8"`, `"8.4"`, `"8.4.12"`) from descending version list.
pub fn match_version(versions: &[String], input: &str) -> Option<String> {
    let trimmed = input.trim();
    let clean = trimmed.strip_prefix('v').unwrap_or(trimmed);

    if clean.eq_ignore_ascii_case("latest") {
        return versions.first().cloned();
    }

    for v in versions {
        if v == clean || v.starts_with(&format!("{}.", clean)) {
            return Some(v.clone());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_static_php_index() {
        let html = r#"
            <a href="php-8.3.14-cli-linux-x86_64.tar.gz">php-8.3.14-cli-linux-x86_64.tar.gz</a>
            <a href="php-8.4.12-cli-linux-x86_64.tar.gz">php-8.4.12-cli-linux-x86_64.tar.gz</a>
            <a href="php-8.4.12-cli-linux-aarch64.tar.gz">php-8.4.12-cli-linux-aarch64.tar.gz</a>
            <a href="php-8.2.26-cli-linux-x86_64.tar.gz">php-8.2.26-cli-linux-x86_64.tar.gz</a>
            <a href="php-8.4.12-cli-linux-x86_64.tar.gz">duplicate</a>
        "#;

        let versions = parse_versions_from_index(html, "linux-x86_64");
        assert_eq!(versions, vec!["8.4.12", "8.3.14", "8.2.26"]);
    }

    #[test]
    fn test_match_version_php() {
        let versions = vec![
            "8.4.12".to_string(),
            "8.3.14".to_string(),
            "8.2.26".to_string(),
        ];

        assert_eq!(match_version(&versions, "latest"), Some("8.4.12".to_string()));
        assert_eq!(match_version(&versions, "8"), Some("8.4.12".to_string()));
        assert_eq!(match_version(&versions, "8.3"), Some("8.3.14".to_string()));
        assert_eq!(match_version(&versions, "v8.2.26"), Some("8.2.26".to_string()));
        assert_eq!(match_version(&versions, "7.4"), None);
    }

    #[test]
    fn test_parse_windows_php_index() {
        let html = r#"
            <a href="/downloads/releases/php-8.3.14-nts-Win32-vs16-x64.zip">php-8.3.14-nts-Win32-vs16-x64.zip</a>
            <a href="/downloads/releases/php-8.4.12-nts-Win32-vs17-x64.zip">php-8.4.12-nts-Win32-vs17-x64.zip</a>
            <a href="/downloads/releases/php-8.4.12-Win32-vs17-x64.zip">zts</a>
            <a href="/downloads/releases/php-8.4.12-nts-Win32-vs17-arm64.zip">arm64</a>
            <a href="/downloads/releases/php-8.2.26-nts-Win32-vs16-x64.zip">dup</a>
            <a href="/downloads/releases/php-8.4.12-nts-Win32-vs17-x64.zip">dup2</a>
        "#;

        let x64 = parse_windows_php_index(html, Arch::X64);
        assert_eq!(
            x64,
            vec![
                ("8.4.12".to_string(), "php-8.4.12-nts-Win32-vs17-x64.zip".to_string()),
                ("8.3.14".to_string(), "php-8.3.14-nts-Win32-vs16-x64.zip".to_string()),
                ("8.2.26".to_string(), "php-8.2.26-nts-Win32-vs16-x64.zip".to_string()),
            ]
        );

        let arm64 = parse_windows_php_index(html, Arch::Arm64);
        assert_eq!(
            arm64,
            vec![(
                "8.4.12".to_string(),
                "php-8.4.12-nts-Win32-vs17-arm64.zip".to_string()
            )]
        );
    }
}
