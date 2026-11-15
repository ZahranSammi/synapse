//! Global packages tracking, discovery, syncing, and cross-version migration.

use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::core::switcher;
use crate::error::{Result, SynapseError};
use crate::platform::paths;

const BUILTIN_NODE_MODULES: &[&str] = &["npm", "corepack"];
const BUILTIN_NODE_BINS: &[&str] = &["node", "npm", "npx", "corepack"];

/// Validates that a package name is safe to store in a manifest and pass to a package manager.
pub fn is_valid_package_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '@' | '_' | '.' | '/' | '-'))
}

/// Parses a `packages.txt` manifest, skipping comments/blank lines, validating, and deduplicating.
pub fn parse_manifest(content: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut packages = Vec::new();

    for line in content.lines() {
        let trimmed = line.split('#').next().unwrap_or("").trim();
        if is_valid_package_name(trimmed) && seen.insert(trimmed.to_string()) {
            packages.push(trimmed.to_string());
        }
    }

    packages
}

/// Loads global package names from `path` (or returns empty if the file does not exist).
pub fn load_manifest_at(path: &Path) -> Result<Vec<String>> {
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let content = fs::read_to_string(path)?;
    Ok(parse_manifest(&content))
}

/// Saves a deduplicated, sorted list of global package names to `path`.
pub fn save_manifest_at(path: &Path, packages: &[String]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let unique: BTreeSet<&str> = packages
        .iter()
        .map(|s| s.trim())
        .filter(|s| is_valid_package_name(s))
        .collect();

    let mut content = unique.into_iter().collect::<Vec<_>>().join("\n");
    if !content.is_empty() {
        content.push('\n');
    }
    fs::write(path, content)?;
    Ok(())
}

/// Discovers globally installed npm packages inside a Node.js version directory.
pub fn discover_node_globals_in_dir(version_dir: &Path) -> Result<Vec<String>> {
    let modules_dir = version_dir.join("lib").join("node_modules");
    if !modules_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut packages = BTreeSet::new();
    for entry in fs::read_dir(&modules_dir)? {
        let entry = entry?;
        let name = match entry.file_name().to_str() {
            Some(n) => n.to_string(),
            None => continue,
        };
        if name.starts_with('.') || BUILTIN_NODE_MODULES.contains(&name.as_str()) {
            continue;
        }
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        if name.starts_with('@') {
            for sub_entry in fs::read_dir(&path)? {
                let sub_entry = sub_entry?;
                let sub_name = match sub_entry.file_name().to_str() {
                    Some(n) => n.to_string(),
                    None => continue,
                };
                if sub_name.starts_with('.') || !sub_entry.path().is_dir() {
                    continue;
                }
                let scoped = format!("{}/{}", name, sub_name);
                if is_valid_package_name(&scoped) {
                    packages.insert(scoped);
                }
            }
        } else if is_valid_package_name(&name) {
            packages.insert(name);
        }
    }

    Ok(packages.into_iter().collect())
}

/// Extracts dependency names from `dependencies` / `require` keys of a JSON manifest (`package.json` or `composer.json`).
pub fn discover_package_json_deps(pkg_json_path: &Path) -> Result<Vec<String>> {
    if !pkg_json_path.is_file() {
        return Ok(Vec::new());
    }
    let content = fs::read_to_string(pkg_json_path)?;
    let parsed: serde_json::Value = serde_json::from_str(&content).map_err(|e| {
        SynapseError::Extraction(format!(
            "Failed to parse {}: {}",
            pkg_json_path.display(),
            e
        ))
    })?;

    let mut deps = BTreeSet::new();
    for section in ["dependencies", "require"] {
        if let Some(obj) = parsed.get(section).and_then(|v| v.as_object()) {
            for key in obj.keys() {
                if is_valid_package_name(key) {
                    deps.insert(key.clone());
                }
            }
        }
    }

    Ok(deps.into_iter().collect())
}

/// Discovers globally installed packages for a runtime and version.
pub fn discover_installed_globals(runtime: &str, version: &str) -> Result<Vec<String>> {
    match runtime {
        "node" => {
            let version_dir = paths::runtime_version_dir("node", version)?;
            discover_node_globals_in_dir(&version_dir)
        }
        "bun" => {
            let Some(home) = dirs::home_dir() else {
                return Ok(Vec::new());
            };
            let bun_global_pkg = home.join(".bun").join("install").join("global").join("package.json");
            discover_package_json_deps(&bun_global_pkg)
        }
        "composer" => {
            let Some(home) = dirs::home_dir() else {
                return Ok(Vec::new());
            };
            let xdg_composer = home.join(".config").join("composer").join("composer.json");
            let legacy_composer = home.join(".composer").join("composer.json");
            let composer_json = if xdg_composer.is_file() {
                xdg_composer
            } else {
                legacy_composer
            };
            let deps = discover_package_json_deps(&composer_json)?;
            Ok(deps
                .into_iter()
                .filter(|d| d != "php" && !d.starts_with("ext-"))
                .collect())
        }
        _ => Ok(Vec::new()),
    }
}

/// Links non-builtin binaries from a Node.js version into `~/.synapse/bin/`.
pub fn link_node_global_bins(version_dir: &Path, bin_dir: &Path) -> Result<Vec<String>> {
    let src_bin_dir = if cfg!(windows) {
        version_dir.to_path_buf()
    } else {
        version_dir.join("bin")
    };
    if !src_bin_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut linked = BTreeSet::new();
    for entry in fs::read_dir(&src_bin_dir)? {
        let entry = entry?;
        let name = match entry.file_name().to_str() {
            Some(n) => n.to_string(),
            None => continue,
        };
        let stem = name.strip_suffix(".cmd").or_else(|| name.strip_suffix(".exe")).unwrap_or(&name);
        if name.starts_with('.') || BUILTIN_NODE_BINS.contains(&stem) {
            continue;
        }
        let file_type = entry.file_type()?;
        if !file_type.is_file() && !file_type.is_symlink() {
            continue;
        }

        #[cfg(unix)]
        {
            let target = PathBuf::from(format!("../current/node/bin/{}", name));
            let link_path = bin_dir.join(&name);
            switcher::atomic_symlink(&target, &link_path)?;
        }

        #[cfg(windows)]
        {
            if name.ends_with(".cmd") {
                let shim_path = bin_dir.join(&name);
                let content = format!("@echo off\r\ncall \"%~dp0..\\current\\node\\{}\" %*\r\n", name);
                fs::write(shim_path, content)?;
            } else {
                continue;
            }
        }

        linked.insert(name);
    }

    Ok(linked.into_iter().collect())
}

fn validate_globals_runtime(runtime: &str) -> Result<()> {
    match runtime {
        "node" | "bun" | "composer" => Ok(()),
        _ => Err(SynapseError::UnsupportedRuntime(runtime.to_string())),
    }
}

fn prepend_path_env(cmd: &mut Command, extra_dirs: &[PathBuf]) {
    let mut paths_vec: Vec<PathBuf> = extra_dirs.to_vec();
    if let Some(existing) = std::env::var_os("PATH") {
        paths_vec.extend(std::env::split_paths(&existing));
    }
    if let Ok(joined) = std::env::join_paths(paths_vec) {
        cmd.env("PATH", joined);
    }
}

fn run_package_manager_install(runtime: &str, active_ver: &str, packages: &[String]) -> Result<()> {
    if packages.is_empty() {
        return Ok(());
    }

    let synapse_bin = paths::bin_dir()?;
    let version_dir = paths::runtime_version_dir(runtime, active_ver)?;

    let mut cmd = match runtime {
        "node" => {
            if cfg!(windows) {
                let npm = paths::current_runtime_symlink("node")?.join("npm.cmd");
                let mut c = Command::new("cmd");
                c.arg("/C").arg(npm).args(["install", "-g", "--"]).args(packages);
                prepend_path_env(&mut c, &[version_dir, synapse_bin]);
                c
            } else {
                let npm = paths::current_runtime_symlink("node")?.join("bin").join("npm");
                let mut c = Command::new(npm);
                c.args(["install", "-g", "--"]).args(packages);
                prepend_path_env(&mut c, &[version_dir.join("bin"), synapse_bin]);
                c
            }
        }
        "bun" => {
            let bun = paths::current_runtime_symlink("bun")?.join(if cfg!(windows) { "bun.exe" } else { "bun" });
            let mut c = Command::new(bun);
            c.args(["add", "-g", "--"]).args(packages);
            prepend_path_env(&mut c, &[version_dir, synapse_bin]);
            c
        }
        "composer" => {
            let composer = paths::current_runtime_symlink("composer")?.join("composer");
            let mut c = if cfg!(windows) {
                let mut pc = Command::new("php");
                pc.arg(composer);
                pc
            } else {
                Command::new(composer)
            };
            c.args(["global", "require", "--"]).args(packages);
            prepend_path_env(&mut c, &[version_dir, synapse_bin]);
            c
        }
        _ => return Err(SynapseError::UnsupportedRuntime(runtime.to_string())),
    };

    let status = cmd.status()?;
    if !status.success() {
        return Err(SynapseError::Io(std::io::Error::other(format!(
            "Package manager sync for '{}' failed with exit status {}",
            runtime, status
        ))));
    }

    Ok(())
}

/// Syncs and installs tracked + discovered global packages into the active version of `runtime`.
pub fn sync_globals(runtime: &str) -> Result<Vec<String>> {
    validate_globals_runtime(runtime)?;

    let active_ver = switcher::get_current_version(runtime)?
        .ok_or_else(|| SynapseError::NotInstalled(runtime.to_string(), "active".to_string()))?;

    let manifest_path = paths::globals_manifest_path(runtime)?;
    let mut merged: BTreeSet<String> = load_manifest_at(&manifest_path)?.into_iter().collect();
    for pkg in discover_installed_globals(runtime, &active_ver)? {
        merged.insert(pkg);
    }

    let packages: Vec<String> = merged.into_iter().collect();
    save_manifest_at(&manifest_path, &packages)?;

    run_package_manager_install(runtime, &active_ver, &packages)?;

    if runtime == "node" {
        let version_dir = paths::runtime_version_dir("node", &active_ver)?;
        let bin_dir = paths::bin_dir()?;
        link_node_global_bins(&version_dir, &bin_dir)?;
    }

    Ok(packages)
}

fn resolve_installed_version(runtime: &str, input: &str) -> Result<String> {
    let clean = input
        .trim()
        .strip_prefix("bun-v")
        .or_else(|| input.trim().strip_prefix('v'))
        .or_else(|| input.trim().strip_prefix('V'))
        .unwrap_or(input.trim());

    let base = paths::runtime_dir(runtime)?;
    if !base.is_dir() {
        return Err(SynapseError::NotInstalled(
            runtime.to_string(),
            clean.to_string(),
        ));
    }

    let mut installed = Vec::new();
    for entry in fs::read_dir(base)? {
        let entry = entry?;
        if entry.file_type()?.is_dir()
            && let Some(name) = entry.file_name().to_str()
            && !name.starts_with('.')
        {
            installed.push(name.to_string());
        }
    }

    installed.sort_by(|a, b| {
        match (semver::Version::parse(a), semver::Version::parse(b)) {
            (Ok(va), Ok(vb)) => vb.cmp(&va),
            _ => b.cmp(a),
        }
    });

    installed
        .into_iter()
        .find(|v| v == clean || v.starts_with(&format!("{}.", clean)))
        .ok_or_else(|| SynapseError::NotInstalled(runtime.to_string(), clean.to_string()))
}

fn runtime_bin_spec(runtime: &str) -> Result<(&'static [&'static str], &'static str)> {
    match runtime {
        "node" => Ok((&["node", "npm", "npx"], "bin")),
        "bun" => Ok((&["bun", "bunx"], "")),
        "composer" => Ok((&["composer"], "")),
        _ => Err(SynapseError::UnsupportedRuntime(runtime.to_string())),
    }
}

/// Migrates global packages from `from_version` to `to_version` (or the currently active version).
pub fn migrate_globals(
    runtime: &str,
    from_version: &str,
    to_version: Option<&str>,
) -> Result<(String, String, Vec<String>)> {
    validate_globals_runtime(runtime)?;

    let resolved_from = resolve_installed_version(runtime, from_version)?;
    let discovered = discover_installed_globals(runtime, &resolved_from)?;

    let manifest_path = paths::globals_manifest_path(runtime)?;
    let mut merged: BTreeSet<String> = load_manifest_at(&manifest_path)?.into_iter().collect();
    for pkg in discovered {
        merged.insert(pkg);
    }
    let merged_list: Vec<String> = merged.into_iter().collect();
    save_manifest_at(&manifest_path, &merged_list)?;

    let target_ver = match to_version {
        Some(to) => {
            let resolved_to = resolve_installed_version(runtime, to)?;
            if switcher::get_current_version(runtime)?.as_deref() != Some(&resolved_to) {
                let (bins, subdir) = runtime_bin_spec(runtime)?;
                switcher::switch(runtime, &resolved_to, bins, subdir)?;
            }
            resolved_to
        }
        None => switcher::get_current_version(runtime)?
            .ok_or_else(|| SynapseError::NotInstalled(runtime.to_string(), "active".to_string()))?,
    };

    let synced = sync_globals(runtime)?;
    Ok((resolved_from, target_ver, synced))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_and_save_manifest() {
        let raw = "\n# Global tools\npnpm\n@antfu/ni # helper\npnpm\n-evil-flag\n  typescript  \n";
        let parsed = parse_manifest(raw);
        assert_eq!(parsed, vec!["pnpm", "@antfu/ni", "typescript"]);

        let temp_dir =
            std::env::temp_dir().join(format!("synapse_manifest_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        let manifest = temp_dir.join("node").join("packages.txt");

        save_manifest_at(&manifest, &parsed).unwrap();
        let reloaded = load_manifest_at(&manifest).unwrap();
        assert_eq!(reloaded, vec!["@antfu/ni", "pnpm", "typescript"]);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_discover_node_globals_and_link_bins() {
        let temp_dir =
            std::env::temp_dir().join(format!("synapse_node_globals_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);

        let version_dir = temp_dir.join("runtimes").join("node").join("22.11.0");
        let modules_dir = version_dir.join("lib").join("node_modules");
        fs::create_dir_all(modules_dir.join("npm")).unwrap();
        fs::create_dir_all(modules_dir.join("corepack")).unwrap();
        fs::create_dir_all(modules_dir.join("pnpm")).unwrap();
        fs::create_dir_all(modules_dir.join("@antfu").join("ni")).unwrap();

        let globals = discover_node_globals_in_dir(&version_dir).unwrap();
        assert_eq!(globals, vec!["@antfu/ni", "pnpm"]);

        let version_bin = version_dir.join("bin");
        fs::create_dir_all(&version_bin).unwrap();
        for name in ["node", "npm", "npx", "pnpm", "ni"] {
            fs::write(version_bin.join(name), "#!/bin/sh\n").unwrap();
        }

        let synapse_bin = temp_dir.join("bin");
        fs::create_dir_all(&synapse_bin).unwrap();

        let linked = link_node_global_bins(&version_dir, &synapse_bin).unwrap();
        assert_eq!(linked, vec!["ni", "pnpm"]);
        assert_eq!(
            fs::read_link(synapse_bin.join("pnpm")).unwrap(),
            PathBuf::from("../current/node/bin/pnpm")
        );
        assert_eq!(
            fs::read_link(synapse_bin.join("ni")).unwrap(),
            PathBuf::from("../current/node/bin/ni")
        );
        assert!(!synapse_bin.join("node").exists());

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_discover_package_json_deps() {
        let temp_dir =
            std::env::temp_dir().join(format!("synapse_pkgjson_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let pkg_json = temp_dir.join("package.json");
        fs::write(
            &pkg_json,
            r#"{
  "dependencies": {
    "prettier": "^3.0.0",
    "@biomejs/biome": "1.9.4",
    "-bad": "1.0.0"
  }
}"#,
        )
        .unwrap();

        let deps = discover_package_json_deps(&pkg_json).unwrap();
        assert_eq!(deps, vec!["@biomejs/biome", "prettier"]);

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
