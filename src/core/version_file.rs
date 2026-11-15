//! Per-project version file parser and writer (`.node-version`, `.nvmrc`,
//! `.bun-version`, `.php-version`, `.composer-version`, `.postgres-version`,
//! `.nginx-version`, `.synapse-version`).

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Result, SynapseError};
use crate::platform::paths;

const SUPPORTED_RUNTIMES: &[&str] = &["node", "bun", "php", "composer", "postgres", "nginx"];

/// Strips leading `"bun-v"`, `'v'`, or `'V'` and whitespace from a version string.
fn clean_version_str(raw: &str) -> String {
    let trimmed = raw.trim().trim_matches(|c| c == '"' || c == '\'');
    let stripped = trimmed
        .strip_prefix("bun-v")
        .or_else(|| trimmed.strip_prefix('v'))
        .or_else(|| trimmed.strip_prefix('V'))
        .unwrap_or(trimmed);
    stripped.to_string()
}

/// Returns the default pin filename to create for a runtime.
pub fn pin_filename(runtime: &str) -> Result<&'static str> {
    match runtime {
        "node" => Ok(".node-version"),
        "bun" => Ok(".bun-version"),
        "php" => Ok(".php-version"),
        "composer" => Ok(".composer-version"),
        "postgres" => Ok(".postgres-version"),
        "nginx" => Ok(".nginx-version"),
        _ => Err(SynapseError::UnsupportedRuntime(runtime.to_string())),
    }
}

/// Returns candidate single-runtime version filenames in priority order.
pub fn candidate_filenames(runtime: &str) -> Result<&'static [&'static str]> {
    match runtime {
        "node" => Ok(&[".node-version", ".nvmrc"]),
        "bun" => Ok(&[".bun-version"]),
        "php" => Ok(&[".php-version"]),
        "composer" => Ok(&[".composer-version"]),
        "postgres" => Ok(&[".postgres-version"]),
        "nginx" => Ok(&[".nginx-version"]),
        _ => Err(SynapseError::UnsupportedRuntime(runtime.to_string())),
    }
}

/// Parses a single-runtime version file (e.g. `.node-version`, `.nvmrc`, `.bun-version`).
/// Skips empty lines and `#` comments, takes the first value, and normalizes prefixes.
pub fn parse_single_version_content(content: &str) -> Option<String> {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let cleaned = clean_version_str(trimmed);
        if !cleaned.is_empty() {
            return Some(cleaned);
        }
    }
    None
}

/// Parses `.synapse-version` content (TOML table or simple `runtime = version` / `runtime version` lines).
pub fn parse_synapse_version_content(content: &str) -> Vec<(String, String)> {
    if let Ok(table) = toml::from_str::<toml::Table>(content) {
        let mut pairs = Vec::new();
        for (key, value) in table {
            if SUPPORTED_RUNTIMES.contains(&key.as_str())
                && let Some(raw_ver) = value.as_str()
            {
                let ver = clean_version_str(raw_ver);
                if !ver.is_empty() {
                    pairs.push((key, ver));
                }
            }
        }
        if !pairs.is_empty() {
            return pairs;
        }
    }

    let mut pairs = Vec::new();
    for line in content.lines() {
        let without_comment = line.split('#').next().unwrap_or("").trim();
        if without_comment.is_empty() {
            continue;
        }
        let parsed = if let Some((k, v)) = without_comment.split_once('=') {
            Some((k.trim(), v.trim()))
        } else {
            let mut parts = without_comment.split_whitespace();
            match (parts.next(), parts.next()) {
                (Some(k), Some(v)) => Some((k, v)),
                _ => None,
            }
        };

        if let Some((runtime, raw_ver)) = parsed
            && SUPPORTED_RUNTIMES.contains(&runtime)
        {
            let ver = clean_version_str(raw_ver);
            if !ver.is_empty() {
                pairs.push((runtime.to_string(), ver));
            }
        }
    }
    pairs
}

/// Writes a pin file (e.g. `.node-version`) in `dir` after validating `version`.
pub fn write_pin_file(dir: &Path, runtime: &str, version: &str) -> Result<PathBuf> {
    paths::validate_version_str(version)?;
    let filename = pin_filename(runtime)?;
    let path = dir.join(filename);
    fs::write(&path, format!("{}\n", version))?;
    Ok(path)
}

/// Searches `start_dir` and its parent directories for a pinned version of `runtime`.
pub fn find_pinned_version(start_dir: &Path, runtime: &str) -> Result<Option<(String, PathBuf)>> {
    let candidates = candidate_filenames(runtime)?;

    for ancestor in start_dir.ancestors() {
        let synapse_file = ancestor.join(".synapse-version");
        if synapse_file.is_file() {
            let content = fs::read_to_string(&synapse_file)?;
            for (rt, ver) in parse_synapse_version_content(&content) {
                if rt == runtime {
                    paths::validate_version_str(&ver)?;
                    return Ok(Some((ver, synapse_file)));
                }
            }
        }

        for name in candidates {
            let candidate_path = ancestor.join(name);
            if candidate_path.is_file() {
                let content = fs::read_to_string(&candidate_path)?;
                if let Some(ver) = parse_single_version_content(&content) {
                    paths::validate_version_str(&ver)?;
                    return Ok(Some((ver, candidate_path)));
                }
            }
        }
    }

    Ok(None)
}

/// Searches `start_dir` and its parent directories for all pinned supported runtimes.
pub fn find_all_pinned_versions(start_dir: &Path) -> Result<Vec<(String, String, PathBuf)>> {
    let mut found = Vec::new();
    for &runtime in SUPPORTED_RUNTIMES {
        if let Some((version, path)) = find_pinned_version(start_dir, runtime)? {
            found.push((runtime.to_string(), version, path));
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_single_version_content() {
        let content = "\n# Project Node version\n  v20.18.0  \n";
        assert_eq!(
            parse_single_version_content(content),
            Some("20.18.0".to_string())
        );

        let bun_content = "bun-v1.4.2\n";
        assert_eq!(
            parse_single_version_content(bun_content),
            Some("1.4.2".to_string())
        );

        assert_eq!(parse_single_version_content("# only comments\n  \n"), None);
    }

    #[test]
    fn test_parse_synapse_version_content() {
        let toml_content = r#"
# Synapse workspace versions
node = "v22.11.0"
bun = "bun-v1.4.2"
php = "8.4.12"
"#;
        let parsed = parse_synapse_version_content(toml_content);
        assert!(parsed.contains(&("node".to_string(), "22.11.0".to_string())));
        assert!(parsed.contains(&("bun".to_string(), "1.4.2".to_string())));
        assert!(parsed.contains(&("php".to_string(), "8.4.12".to_string())));

        let plain_content =
            "node 20.18.0\ncomposer = 2.8.3 # comment\npostgres = \"17.2.0\"\nnginx = \"1.26.2\"\n";
        let parsed_plain = parse_synapse_version_content(plain_content);
        assert_eq!(
            parsed_plain,
            vec![
                ("node".to_string(), "20.18.0".to_string()),
                ("composer".to_string(), "2.8.3".to_string()),
                ("postgres".to_string(), "17.2.0".to_string()),
                ("nginx".to_string(), "1.26.2".to_string()),
            ]
        );
    }

    #[test]
    fn test_postgres_and_nginx_pin_files() {
        let base =
            std::env::temp_dir().join(format!("synapse_pg_nginx_pin_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();

        let pg_pin = write_pin_file(&base, "postgres", "17.2.0").unwrap();
        assert_eq!(pg_pin, base.join(".postgres-version"));
        assert_eq!(
            find_pinned_version(&base, "postgres").unwrap(),
            Some(("17.2.0".to_string(), pg_pin))
        );

        let nginx_pin = write_pin_file(&base, "nginx", "1.26.2").unwrap();
        assert_eq!(nginx_pin, base.join(".nginx-version"));
        assert_eq!(
            find_pinned_version(&base, "nginx").unwrap(),
            Some(("1.26.2".to_string(), nginx_pin))
        );

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn test_write_and_find_pinned_version_in_ancestors() {
        let base = std::env::temp_dir().join(format!("synapse_pin_test_{}", std::process::id()));
        let child = base.join("apps").join("web");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&child).unwrap();

        let written = write_pin_file(&base, "node", "22.11.0").unwrap();
        assert_eq!(written, base.join(".node-version"));

        let found = find_pinned_version(&child, "node").unwrap();
        assert_eq!(found, Some(("22.11.0".to_string(), base.join(".node-version"))));

        // .synapse-version in child takes precedence for child lookup
        let synapse_path = child.join(".synapse-version");
        fs::write(&synapse_path, "node = \"20.18.0\"\nbun = \"1.4.2\"\n").unwrap();

        let found_all = find_all_pinned_versions(&child).unwrap();
        assert_eq!(
            found_all,
            vec![
                ("node".to_string(), "20.18.0".to_string(), synapse_path.clone()),
                ("bun".to_string(), "1.4.2".to_string(), synapse_path.clone()),
            ]
        );

        // Traversal attempt in pin file is rejected
        let evil_dir = base.join("evil");
        fs::create_dir_all(&evil_dir).unwrap();
        fs::write(evil_dir.join(".node-version"), "../../etc").unwrap();
        let err = find_pinned_version(&evil_dir, "node").unwrap_err();
        assert!(matches!(err, SynapseError::InvalidVersion(_)));

        // Writing traversal version is rejected
        let write_err = write_pin_file(&base, "node", "../bad").unwrap_err();
        assert!(matches!(write_err, SynapseError::InvalidVersion(_)));

        let _ = fs::remove_dir_all(&base);
    }
}
