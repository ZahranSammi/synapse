//! Nginx runtime provider: resolves versions from `jirutka/nginx-binaries` (Linux)
//! or `nginx.org/download/` (Windows), downloads standalone binaries/archives,
//! and installs into `~/.synapse/runtimes/nginx/<version>`.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use indicatif::{ProgressBar, ProgressStyle};
use semver::Version;

use crate::error::{Result, SynapseError};
use crate::platform::archive;
use crate::platform::paths::{self, Arch};
use crate::provider::{RemoteVersion, RuntimeProvider};

const LINUX_NGINX_BASE_URL: &str = "https://jirutka.github.io/nginx-binaries/";
const WINDOWS_NGINX_BASE_URL: &str = "https://nginx.org/download/";

/// Returns true if the Nginx version is on a Stable branch (even minor number, e.g. 1.24.x, 1.26.x).
pub fn is_stable_version(version: &str) -> bool {
    Version::parse(version)
        .map(|v| v.minor % 2 == 0)
        .unwrap_or(false)
}

/// Extracts and sorts descending all valid semver versions from `jirutka.github.io/nginx-binaries/` HTML.
pub fn parse_linux_nginx_index(html: &str, arch_str: &str) -> Vec<String> {
    let suffix = format!("-{}-linux", arch_str);
    let mut seen = HashSet::new();
    let mut parsed: Vec<(Version, String)> = Vec::new();

    let mut search_from = 0;
    while let Some(rel_idx) = html[search_from..].find(&suffix) {
        let suffix_start = search_from + rel_idx;
        let after_idx = suffix_start + suffix.len();
        let next_char = html[after_idx..].chars().next();
        let valid_boundary = !matches!(next_char, Some(c) if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));

        let before = &html[search_from..suffix_start];
        if valid_boundary
            && let Some(prefix_rel) = before.rfind("nginx-")
        {
            let ver_str = &before[prefix_rel + 6..];
            if !ver_str.is_empty()
                && ver_str
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
                && seen.insert(ver_str.to_string())
                && let Ok(v) = Version::parse(ver_str)
            {
                parsed.push((v, ver_str.to_string()));
            }
        }
        search_from = after_idx;
    }

    parsed.sort_by(|a, b| b.0.cmp(&a.0));
    parsed.into_iter().map(|(_, s)| s).collect()
}

/// Extracts and sorts descending all valid semver `.zip` versions from `nginx.org/download/` HTML.
pub fn parse_windows_nginx_index(html: &str) -> Vec<String> {
    let suffix = ".zip";
    let mut seen = HashSet::new();
    let mut parsed: Vec<(Version, String)> = Vec::new();

    let mut search_from = 0;
    while let Some(rel_idx) = html[search_from..].find(suffix) {
        let suffix_start = search_from + rel_idx;
        let after_idx = suffix_start + suffix.len();
        let next_char = html[after_idx..].chars().next();
        let valid_boundary = !matches!(next_char, Some(c) if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));

        let before = &html[search_from..suffix_start];
        if valid_boundary
            && let Some(prefix_rel) = before.rfind("nginx-")
        {
            let ver_str = &before[prefix_rel + 6..];
            if !ver_str.is_empty()
                && ver_str
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
                && seen.insert(ver_str.to_string())
                && let Ok(v) = Version::parse(ver_str)
            {
                parsed.push((v, ver_str.to_string()));
            }
        }
        search_from = after_idx;
    }

    parsed.sort_by(|a, b| b.0.cmp(&a.0));
    parsed.into_iter().map(|(_, s)| s).collect()
}

/// Resolves user version input (`"latest"`, `"lts"`, `"stable"`, `"1.26"`, `"1.26.2"`) from descending version list.
pub fn match_version(versions: &[String], input: &str) -> Option<String> {
    let trimmed = input.trim();
    let clean = trimmed.strip_prefix('v').unwrap_or(trimmed);

    if clean.eq_ignore_ascii_case("latest") {
        return versions.first().cloned();
    }

    if clean.eq_ignore_ascii_case("lts") || clean.eq_ignore_ascii_case("stable") {
        return versions.iter().find(|v| is_stable_version(v)).cloned();
    }

    for v in versions {
        if v == clean || v.starts_with(&format!("{}.", clean)) {
            return Some(v.clone());
        }
    }

    None
}

/// Provider for portable Nginx binaries (`jirutka/nginx-binaries` on Linux, `nginx.org` on Windows).
#[derive(Debug, Default)]
pub struct NginxProvider;

impl NginxProvider {
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

    fn linux_arch_str(&self) -> Result<&'static str> {
        match paths::detect_arch()? {
            Arch::X64 => Ok("x86_64"),
            Arch::Arm64 => Ok("aarch64"),
        }
    }

    async fn fetch_available_versions(&self) -> Result<Vec<String>> {
        let client = self.client()?;
        match std::env::consts::OS {
            "linux" => {
                let arch_str = self.linux_arch_str()?;
                let html = client
                    .get(LINUX_NGINX_BASE_URL)
                    .send()
                    .await?
                    .error_for_status()?
                    .text()
                    .await?;
                Ok(parse_linux_nginx_index(&html, arch_str))
            }
            "windows" => {
                let _ = paths::detect_arch()?;
                let html = client
                    .get(WINDOWS_NGINX_BASE_URL)
                    .send()
                    .await?
                    .error_for_status()?
                    .text()
                    .await?;
                Ok(parse_windows_nginx_index(&html))
            }
            other => Err(SynapseError::UnsupportedArchitecture(other.to_string())),
        }
    }
}

impl RuntimeProvider for NginxProvider {
    fn name(&self) -> &'static str {
        "nginx"
    }

    fn binary_names(&self) -> &'static [&'static str] {
        &["nginx"]
    }

    fn bin_subdir(&self) -> &'static str {
        ""
    }

    async fn list_remote(&self) -> Result<Vec<RemoteVersion>> {
        let versions = self.fetch_available_versions().await?;
        Ok(versions
            .into_iter()
            .map(|version| {
                let lts = if is_stable_version(&version) {
                    Some("Stable".to_string())
                } else {
                    None
                };
                RemoteVersion { version, lts }
            })
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

        let nginx_bin_name = if is_windows { "nginx.exe" } else { "nginx" };
        if target_dir.join(nginx_bin_name).is_file() {
            println!(
                "Nginx v{} is already installed at {}",
                resolved_version,
                target_dir.display()
            );
            return Ok(target_dir);
        }

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

        if is_windows {
            let file_name = format!("nginx-{}.zip", resolved_version);
            let url = format!("{}{}", WINDOWS_NGINX_BASE_URL, file_name);
            let mut resp = client.get(&url).send().await?.error_for_status()?;
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
            archive::extract_zip(archive_file, &extract_dir)?;

            if !extract_dir.join("nginx.exe").is_file() {
                return Err(SynapseError::Extraction(format!(
                    "Expected 'nginx.exe' inside {}, but it was not found",
                    file_name
                )));
            }

            if let Some(parent) = target_dir.parent() {
                fs::create_dir_all(parent)?;
            }
            if target_dir.exists() {
                fs::remove_dir_all(&target_dir)?;
            }
            fs::rename(&extract_dir, &target_dir)?;
            let _ = fs::remove_dir_all(&temp_dir);
        } else if std::env::consts::OS == "linux" {
            let arch_str = self.linux_arch_str()?;
            let url = format!(
                "{}nginx-{}-{}-linux",
                LINUX_NGINX_BASE_URL, resolved_version, arch_str
            );
            let mut resp = client.get(&url).send().await?.error_for_status()?;
            let total_size = resp.content_length().unwrap_or(0);

            let pb = ProgressBar::new(total_size);
            pb.set_style(
                ProgressStyle::default_bar()
                    .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
                    .unwrap_or_else(|_| ProgressStyle::default_bar())
                    .progress_chars("#>-"),
            );

            let staged_bin = temp_dir.join("nginx");
            {
                use std::io::Write;
                let mut file = fs::File::create(&staged_bin)?;
                while let Some(chunk) = resp.chunk().await? {
                    file.write_all(&chunk)?;
                    pb.inc(chunk.len() as u64);
                }
            }
            pb.finish_with_message("Downloaded");

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
        } else {
            let _ = fs::remove_dir_all(&temp_dir);
            return Err(SynapseError::UnsupportedArchitecture(
                std::env::consts::OS.to_string(),
            ));
        }

        Ok(target_dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_linux_nginx_index() {
        let html = r#"
            <a href="nginx-1.24.0-x86_64-linux">nginx-1.24.0-x86_64-linux</a>
            <a href="nginx-1.24.0-x86_64-linux.sha1">sha1</a>
            <a href="nginx-1.26.2-x86_64-linux">nginx-1.26.2-x86_64-linux</a>
            <a href="nginx-1.27.3-x86_64-linux">nginx-1.27.3-x86_64-linux</a>
            <a href="nginx-1.27.3-aarch64-linux">nginx-1.27.3-aarch64-linux</a>
            <a href="nginx-1.26.2-x86_64-linux">dup</a>
        "#;

        let x64 = parse_linux_nginx_index(html, "x86_64");
        assert_eq!(x64, vec!["1.27.3", "1.26.2", "1.24.0"]);

        let arm64 = parse_linux_nginx_index(html, "aarch64");
        assert_eq!(arm64, vec!["1.27.3"]);
    }

    #[test]
    fn test_parse_windows_nginx_index() {
        let html = r#"
            <a href="nginx-1.26.2.tar.gz">nginx-1.26.2.tar.gz</a>
            <a href="nginx-1.26.2.zip">nginx-1.26.2.zip</a>
            <a href="nginx-1.26.2.zip.asc">nginx-1.26.2.zip.asc</a>
            <a href="nginx-1.27.3.zip">nginx-1.27.3.zip</a>
            <a href="nginx-1.24.0.zip">nginx-1.24.0.zip</a>
            <a href="nginx-1.26.2.zip">dup</a>
        "#;

        let versions = parse_windows_nginx_index(html);
        assert_eq!(versions, vec!["1.27.3", "1.26.2", "1.24.0"]);
    }

    #[test]
    fn test_match_version_nginx() {
        let versions = vec![
            "1.27.3".to_string(),
            "1.26.2".to_string(),
            "1.25.5".to_string(),
            "1.24.0".to_string(),
        ];

        assert!(is_stable_version("1.26.2"));
        assert!(!is_stable_version("1.27.3"));

        assert_eq!(match_version(&versions, "latest"), Some("1.27.3".to_string()));
        assert_eq!(match_version(&versions, "stable"), Some("1.26.2".to_string()));
        assert_eq!(match_version(&versions, "lts"), Some("1.26.2".to_string()));
        assert_eq!(match_version(&versions, "1.26"), Some("1.26.2".to_string()));
        assert_eq!(match_version(&versions, "v1.24.0"), Some("1.24.0".to_string()));
        assert_eq!(match_version(&versions, "1.22"), None);
    }
}
