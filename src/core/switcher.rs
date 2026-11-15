//! Version switcher: updates active symlinks for runtimes and binary shims.

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{Result, SynapseError};
use crate::platform::paths;

/// Generates the contents of a Windows `.cmd` shim for a Synapse-managed runtime binary.
pub fn generate_windows_cmd_shim(runtime: &str, bin_name: &str, bin_subdir: &str) -> String {
    if runtime == "composer" {
        return "@echo off\r\nphp \"%~dp0..\\current\\composer\\composer\" %*\r\n".to_string();
    }
    if runtime == "bun" && bin_name == "bunx" {
        return "@echo off\r\n\"%~dp0..\\current\\bun\\bun.exe\" x %*\r\n".to_string();
    }
    if runtime == "node" && (bin_name == "npm" || bin_name == "npx") {
        let rel = if bin_subdir.is_empty() {
            format!("..\\current\\node\\{}.cmd", bin_name)
        } else {
            format!("..\\current\\node\\{}\\{}.cmd", bin_subdir, bin_name)
        };
        return format!("@echo off\r\ncall \"%~dp0{}\" %*\r\n", rel);
    }

    let rel = if bin_subdir.is_empty() {
        format!("..\\current\\{}\\{}.exe", runtime, bin_name)
    } else {
        format!("..\\current\\{}\\{}\\{}.exe", runtime, bin_subdir, bin_name)
    };
    format!("@echo off\r\n\"%~dp0{}\" %*\r\n", rel)
}

/// Atomically creates or replaces a symlink at `link_path` pointing to `target`.
///
/// Pattern:
/// 1. Create a temporary symlink in the same parent directory (e.g. `.tmp_symlink_<pid>_<nano>`).
/// 2. Atomically rename the temporary symlink over `link_path` (`std::fs::rename`).
#[cfg(unix)]
pub fn atomic_symlink(target: &Path, link_path: &Path) -> Result<()> {
    let parent = link_path.parent().ok_or_else(|| {
        SynapseError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("Path has no parent: {}", link_path.display()),
        ))
    })?;

    fs::create_dir_all(parent)?;

    let temp_name = format!(
        ".tmp_{}_{}_{}",
        link_path.file_name().and_then(|n| n.to_str()).unwrap_or("symlink"),
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let temp_link = parent.join(temp_name);

    if temp_link.exists() || temp_link.is_symlink() {
        let _ = fs::remove_file(&temp_link);
    }

    symlink(target, &temp_link)?;

    if let Err(err) = fs::rename(&temp_link, link_path) {
        let _ = fs::remove_file(&temp_link);
        return Err(SynapseError::Io(err));
    }

    Ok(())
}

/// Creates or replaces a Windows junction (for directories) or symlink/hardlink (for files).
#[cfg(windows)]
pub fn atomic_symlink(target: &Path, link_path: &Path) -> Result<()> {
    let parent = link_path.parent().ok_or_else(|| {
        SynapseError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("Path has no parent: {}", link_path.display()),
        ))
    })?;

    fs::create_dir_all(parent)?;

    let resolved_target = if target.is_relative() {
        parent.join(target)
    } else {
        target.to_path_buf()
    };

    if resolved_target.is_dir() {
        if link_path.exists() || link_path.is_symlink() {
            let _ = fs::remove_dir(link_path);
            let _ = fs::remove_file(link_path);
        }
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link_path)
            .arg(&resolved_target)
            .status()?;
        if !status.success() {
            return Err(SynapseError::Io(std::io::Error::other(format!(
                "Failed to create directory junction {} -> {}",
                link_path.display(),
                resolved_target.display()
            ))));
        }
        return Ok(());
    }

    if link_path.exists() || link_path.is_symlink() {
        let _ = fs::remove_file(link_path);
    }

    if std::os::windows::fs::symlink_file(target, link_path).is_err() {
        if fs::hard_link(&resolved_target, link_path).is_err() {
            fs::copy(&resolved_target, link_path)?;
        }
    }

    Ok(())
}

/// Maps a managed executable name (e.g. "node", "npm", "psql", "nginx") to `(runtime, bin_subdir)`.
pub fn runtime_for_binary(bin_name: &str) -> Option<(&'static str, &'static str)> {
    let clean = bin_name
        .strip_suffix(".exe")
        .or_else(|| bin_name.strip_suffix(".cmd"))
        .unwrap_or(bin_name);
    match clean {
        "node" | "npm" | "npx" => Some(("node", if cfg!(windows) { "" } else { "bin" })),
        "bun" | "bunx" => Some(("bun", "")),
        "php" => Some(("php", "")),
        "composer" => Some(("composer", "")),
        "postgres" | "psql" | "pg_ctl" | "initdb" | "pg_dump" | "createdb" | "dropdb" => {
            Some(("postgres", "bin"))
        }
        "nginx" => Some(("nginx", "")),
        _ => None,
    }
}

/// Returns binary executable names for a supported runtime.
pub fn runtime_binary_names(runtime: &str) -> Result<&'static [&'static str]> {
    match runtime {
        "node" => Ok(&["node", "npm", "npx"]),
        "bun" => Ok(&["bun", "bunx"]),
        "php" => Ok(&["php"]),
        "composer" => Ok(&["composer"]),
        "postgres" => Ok(&["postgres", "psql", "pg_ctl", "initdb", "pg_dump", "createdb", "dropdb"]),
        "nginx" => Ok(&["nginx"]),
        _ => Err(SynapseError::UnsupportedRuntime(runtime.to_string())),
    }
}

/// Lists installed versions of `runtime` under `base_dir`, sorted descending by semver.
pub fn list_installed_versions_at(base_dir: &Path, runtime: &str) -> Result<Vec<String>> {
    let runtime_dir = base_dir.join("runtimes").join(runtime);
    if !runtime_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut installed = Vec::new();
    for entry in fs::read_dir(&runtime_dir)? {
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

    Ok(installed)
}

/// Lists locally installed versions of `runtime` under `~/.synapse/runtimes/<runtime>`.
pub fn list_installed_versions(runtime: &str) -> Result<Vec<String>> {
    list_installed_versions_at(&paths::synapse_dir()?, runtime)
}

/// Uninstalls an installed runtime version (supporting partial matches) and cleans active shims if needed.
/// Returns `(resolved_version, removed_active_links)`.
pub fn uninstall_version(runtime: &str, version: &str) -> Result<(String, bool)> {
    let bin_names = runtime_binary_names(runtime)?;
    let clean_input = version
        .trim()
        .strip_prefix("bun-v")
        .or_else(|| version.trim().strip_prefix('v'))
        .or_else(|| version.trim().strip_prefix('V'))
        .unwrap_or(version.trim());

    let mut clean_version = clean_input.to_string();
    let mut target_dir = paths::runtime_version_dir(runtime, &clean_version)?;

    if !target_dir.exists() {
        let local_matches = list_installed_versions(runtime)?;
        if let Some(matched) = local_matches
            .into_iter()
            .find(|v| v == clean_input || v.starts_with(&format!("{}.", clean_input)))
        {
            clean_version = matched;
            target_dir = paths::runtime_version_dir(runtime, &clean_version)?;
        } else {
            return Err(SynapseError::NotInstalled(
                runtime.to_string(),
                clean_input.to_string(),
            ));
        }
    }

    let current = get_current_version(runtime)?;
    let was_active = current.as_deref() == Some(clean_version.as_str());
    if was_active {
        let current_symlink = paths::current_runtime_symlink(runtime)?;
        if current_symlink.exists() || current_symlink.is_symlink() {
            let _ = fs::remove_file(&current_symlink);
            let _ = fs::remove_dir(&current_symlink);
        }
        if let Ok(cdir) = paths::current_dir() {
            let _ = fs::remove_file(cdir.join(format!("{}.version", runtime)));
        }
        let bin_dir = paths::bin_dir()?;
        for bin_name in bin_names {
            let bin_shim = bin_dir.join(bin_name);
            if bin_shim.exists() || bin_shim.is_symlink() {
                let _ = fs::remove_file(&bin_shim);
            }
            let cmd_shim = bin_dir.join(format!("{}.cmd", bin_name));
            if cmd_shim.exists() {
                let _ = fs::remove_file(&cmd_shim);
            }
        }
        if runtime == "node" {
            let _ = crate::core::globals::cleanup_stale_node_global_bins(&bin_dir);
        }
    }

    fs::remove_dir_all(&target_dir)?;
    Ok((clean_version, was_active))
}

/// Resolves a version spec (exact or partial prefix like "20" or "20.18") against installed versions of `runtime` under `base_dir`.
pub fn match_installed_version_at(
    base_dir: &Path,
    runtime: &str,
    requested: &str,
) -> Result<Option<String>> {
    let trimmed = requested.trim();
    let clean = trimmed
        .strip_prefix("bun-v")
        .or_else(|| trimmed.strip_prefix('v'))
        .or_else(|| trimmed.strip_prefix('V'))
        .unwrap_or(trimmed);

    paths::validate_version_str(clean)?;

    let runtime_dir = base_dir.join("runtimes").join(runtime);
    if !runtime_dir.is_dir() {
        return Ok(None);
    }

    if runtime_dir.join(clean).is_dir() {
        return Ok(Some(clean.to_string()));
    }

    let installed = list_installed_versions_at(base_dir, runtime)?;
    let prefix = format!("{}.", clean);
    Ok(installed
        .into_iter()
        .find(|v| v == clean || v.starts_with(&prefix)))
}

/// Switches the active version of a runtime.
///
/// 1. Verifies that the requested version is installed under `~/.synapse/runtimes/<runtime>/<version>/`.
/// 2. Updates `~/.synapse/current/<runtime>` -> `../runtimes/<runtime>/<version>`.
/// 3. Updates `~/.synapse/bin/<bin>` (smart `synapse` shims on Unix, `.cmd` shims on Windows).
pub fn switch(runtime: &str, version: &str, binary_names: &[&str], bin_subdir: &str) -> Result<PathBuf> {
    paths::ensure_dirs()?;

    let target_version_dir = paths::runtime_version_dir(runtime, version)?;
    if !target_version_dir.exists() {
        return Err(SynapseError::NotInstalled(
            runtime.to_string(),
            version.to_string(),
        ));
    }

    let current_link = paths::current_runtime_symlink(runtime)?;

    #[cfg(unix)]
    {
        // Relative target for ~/.synapse/current/<runtime> -> ../runtimes/<runtime>/<version>
        let relative_current_target = PathBuf::from(format!("../runtimes/{}/{}", runtime, version));
        atomic_symlink(&relative_current_target, &current_link)?;

        let bin_dir = paths::bin_dir()?;
        let self_bin = bin_dir.join("synapse");
        if !self_bin.exists()
            && let Ok(exe) = std::env::current_exe()
            && exe.file_stem() == Some(std::ffi::OsStr::new("synapse"))
        {
            let _ = crate::cli::install_self_to_dir(&exe, &bin_dir);
        }

        let use_smart_shim = self_bin.is_file();
        for bin_name in binary_names {
            let bin_target = if use_smart_shim {
                PathBuf::from("synapse")
            } else if bin_subdir.is_empty() {
                PathBuf::from(format!("../current/{}/{}", runtime, bin_name))
            } else {
                PathBuf::from(format!("../current/{}/{}/{}", runtime, bin_subdir, bin_name))
            };
            let bin_link = bin_dir.join(bin_name);
            atomic_symlink(&bin_target, &bin_link)?;
        }
    }

    #[cfg(windows)]
    {
        atomic_symlink(&target_version_dir, &current_link)?;
        let version_marker = paths::current_dir()?.join(format!("{}.version", runtime));
        fs::write(version_marker, format!("{}\n", version))?;

        let bin_dir = paths::bin_dir()?;
        for bin_name in binary_names {
            let shim_content = generate_windows_cmd_shim(runtime, bin_name, bin_subdir);
            let shim_path = bin_dir.join(format!("{}.cmd", bin_name));
            fs::write(shim_path, shim_content)?;
        }
    }

    Ok(target_version_dir)
}

/// Upgrades any existing direct runtime symlinks in `bin_dir` to smart `synapse` shims on Unix.
pub fn refresh_shims_if_needed(bin_dir: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        if !bin_dir.join("synapse").is_file() || !bin_dir.is_dir() {
            return Ok(());
        }
        for entry in fs::read_dir(bin_dir)? {
            let entry = entry?;
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if runtime_for_binary(&name).is_none() {
                continue;
            }
            let path = entry.path();
            if let Ok(target) = fs::read_link(&path)
                && target == Path::new("synapse")
            {
                continue;
            }
            atomic_symlink(Path::new("synapse"), &path)?;
        }
    }
    #[cfg(not(unix))]
    let _ = bin_dir;
    Ok(())
}

/// Returns the currently active version for a given runtime under `base_dir` by inspecting `current/<runtime>`.
pub fn get_current_version_at(base_dir: &Path, runtime: &str) -> Result<Option<String>> {
    #[cfg(windows)]
    {
        let version_marker = base_dir.join("current").join(format!("{}.version", runtime));
        if version_marker.is_file() {
            let content = fs::read_to_string(&version_marker)?;
            let trimmed = content.trim();
            if !trimmed.is_empty() {
                return Ok(Some(trimmed.to_string()));
            }
        }
    }

    let current_link = base_dir.join("current").join(runtime);
    if !current_link.exists() && !current_link.is_symlink() {
        return Ok(None);
    }

    let target = fs::read_link(&current_link)?;
    // target is typically "../runtimes/node/22.11.0" or an absolute path
    if let Some(file_name) = target.file_name().and_then(|n| n.to_str()) {
        Ok(Some(file_name.to_string()))
    } else {
        Ok(None)
    }
}

/// Returns the currently active version for a given runtime by inspecting `~/.synapse/current/<runtime>`.
pub fn get_current_version(runtime: &str) -> Result<Option<String>> {
    get_current_version_at(&paths::synapse_dir()?, runtime)
}

/// Resolves the target executable and its bin directory for a Synapse-managed shim invocation.
/// Checks project pin files in `cwd` (and ancestors) before falling back to `current/<runtime>`.
pub fn resolve_shim_target_at(
    base_dir: &Path,
    cwd: &Path,
    invoked_bin: &str,
) -> Result<Option<(PathBuf, PathBuf)>> {
    let clean_bin = invoked_bin
        .strip_suffix(".exe")
        .or_else(|| invoked_bin.strip_suffix(".cmd"))
        .unwrap_or(invoked_bin);

    let Some((runtime, bin_subdir)) = runtime_for_binary(clean_bin) else {
        return Ok(None);
    };

    let resolved_version =
        if let Some((pinned_ver, _)) = crate::core::version_file::find_pinned_version(cwd, runtime)? {
            match_installed_version_at(base_dir, runtime, &pinned_ver)?
                .ok_or_else(|| SynapseError::NotInstalled(runtime.to_string(), pinned_ver))?
        } else {
            get_current_version_at(base_dir, runtime)?.ok_or_else(|| {
                SynapseError::NotInstalled(runtime.to_string(), "active".to_string())
            })?
        };

    let version_dir = base_dir.join("runtimes").join(runtime).join(&resolved_version);
    let runtime_bin_dir = if bin_subdir.is_empty() {
        version_dir
    } else {
        version_dir.join(bin_subdir)
    };

    let exe_path = if cfg!(windows) {
        if runtime == "node" && (clean_bin == "npm" || clean_bin == "npx") {
            runtime_bin_dir.join(format!("{}.cmd", clean_bin))
        } else if runtime == "composer" {
            runtime_bin_dir.join("composer")
        } else if runtime == "bun" && clean_bin == "bunx" {
            runtime_bin_dir.join("bun.exe")
        } else {
            runtime_bin_dir.join(format!("{}.exe", clean_bin))
        }
    } else {
        runtime_bin_dir.join(clean_bin)
    };

    if !exe_path.is_file() {
        return Err(SynapseError::NotInstalled(
            runtime.to_string(),
            resolved_version,
        ));
    }

    Ok(Some((exe_path, runtime_bin_dir)))
}

fn build_shim_path(runtime_bin_dir: &Path, synapse_bin_dir: &Path) -> Option<std::ffi::OsString> {
    let mut entries = vec![runtime_bin_dir.to_path_buf()];
    if let Some(existing) = std::env::var_os("PATH") {
        let mut has_synapse_bin = false;
        for p in std::env::split_paths(&existing) {
            if p == synapse_bin_dir {
                has_synapse_bin = true;
            }
            if p != runtime_bin_dir {
                entries.push(p);
            }
        }
        if !has_synapse_bin {
            entries.push(synapse_bin_dir.to_path_buf());
        }
    } else {
        entries.push(synapse_bin_dir.to_path_buf());
    }
    std::env::join_paths(entries).ok()
}

/// Dispatches execution immediately if `synapse` was invoked via a runtime shim symlink (e.g. `node`, `bun`, `php`).
pub fn try_exec_shim() -> Result<bool> {
    let Some(arg0) = std::env::args_os().next() else {
        return Ok(false);
    };
    let Some(invoked_name) = Path::new(&arg0).file_name().and_then(|n| n.to_str()) else {
        return Ok(false);
    };
    if runtime_for_binary(invoked_name).is_none() {
        return Ok(false);
    }

    let base_dir = paths::synapse_dir()?;
    let cwd = std::env::current_dir()?;
    let Some((exe_path, runtime_bin_dir)) = resolve_shim_target_at(&base_dir, &cwd, invoked_name)? else {
        return Ok(false);
    };

    let mut cmd = Command::new(&exe_path);
    cmd.args(std::env::args_os().skip(1));
    if let Some(joined) = build_shim_path(&runtime_bin_dir, &base_dir.join("bin")) {
        cmd.env("PATH", joined);
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = cmd.exec();
        Err(SynapseError::Io(err))
    }

    #[cfg(windows)]
    {
        let status = cmd.status()?;
        std::process::exit(status.code().unwrap_or(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_test_dir(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("synapse_switch_test_{}_{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn test_atomic_symlink() {
        let dir = setup_test_dir("atomic");
        let target1 = dir.join("file1.txt");
        let target2 = dir.join("file2.txt");
        fs::write(&target1, "hello").unwrap();
        fs::write(&target2, "world").unwrap();

        let link = dir.join("active_link");

        atomic_symlink(&target1, &link).unwrap();
        assert_eq!(fs::read_link(&link).unwrap(), target1);
        assert_eq!(fs::read_to_string(&link).unwrap(), "hello");

        // Atomically replace with target2
        atomic_symlink(&target2, &link).unwrap();
        assert_eq!(fs::read_link(&link).unwrap(), target2);
        assert_eq!(fs::read_to_string(&link).unwrap(), "world");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_switch_and_get_current() {
        let dir = setup_test_dir("switch");
        let base = &dir;
        paths::ensure_dirs_at(base).unwrap();

        // Create fake installed version
        let version_dir = base.join("runtimes").join("node").join("22.11.0");
        let bin_folder = version_dir.join("bin");
        fs::create_dir_all(&bin_folder).unwrap();
        fs::write(bin_folder.join("node"), "#!/bin/sh\necho v22.11.0").unwrap();

        // Custom switch implementation using base dir paths
        let current_link = base.join("current").join("node");
        let relative_target = PathBuf::from("../runtimes/node/22.11.0");
        atomic_symlink(&relative_target, &current_link).unwrap();

        let bin_dir = base.join("bin");
        let bin_target = PathBuf::from("../current/node/bin/node");
        atomic_symlink(&bin_target, &bin_dir.join("node")).unwrap();

        assert_eq!(fs::read_link(&current_link).unwrap(), relative_target);
        assert_eq!(
            fs::read_link(bin_dir.join("node")).unwrap(),
            bin_target
        );

        let target_symlink = fs::read_link(&current_link).unwrap();
        assert_eq!(target_symlink.file_name().unwrap(), "22.11.0");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_generate_windows_cmd_shim() {
        assert_eq!(
            generate_windows_cmd_shim("node", "node", ""),
            "@echo off\r\n\"%~dp0..\\current\\node\\node.exe\" %*\r\n"
        );
        assert_eq!(
            generate_windows_cmd_shim("node", "npm", ""),
            "@echo off\r\ncall \"%~dp0..\\current\\node\\npm.cmd\" %*\r\n"
        );
        assert_eq!(
            generate_windows_cmd_shim("bun", "bun", ""),
            "@echo off\r\n\"%~dp0..\\current\\bun\\bun.exe\" %*\r\n"
        );
        assert_eq!(
            generate_windows_cmd_shim("bun", "bunx", ""),
            "@echo off\r\n\"%~dp0..\\current\\bun\\bun.exe\" x %*\r\n"
        );
        assert_eq!(
            generate_windows_cmd_shim("php", "php", ""),
            "@echo off\r\n\"%~dp0..\\current\\php\\php.exe\" %*\r\n"
        );
        assert_eq!(
            generate_windows_cmd_shim("composer", "composer", ""),
            "@echo off\r\nphp \"%~dp0..\\current\\composer\\composer\" %*\r\n"
        );
        assert_eq!(
            generate_windows_cmd_shim("postgres", "psql", "bin"),
            "@echo off\r\n\"%~dp0..\\current\\postgres\\bin\\psql.exe\" %*\r\n"
        );
        assert_eq!(
            generate_windows_cmd_shim("nginx", "nginx", ""),
            "@echo off\r\n\"%~dp0..\\current\\nginx\\nginx.exe\" %*\r\n"
        );
    }

    #[test]
    fn test_resolve_shim_target_prefers_project_pin_over_global_current() {
        let dir = setup_test_dir("shim_resolve");
        let base = dir.join("synapse_home");
        paths::ensure_dirs_at(&base).unwrap();

        let v20_bin_dir = if cfg!(windows) {
            base.join("runtimes").join("node").join("20.18.0")
        } else {
            base.join("runtimes").join("node").join("20.18.0").join("bin")
        };
        let v22_bin_dir = if cfg!(windows) {
            base.join("runtimes").join("node").join("22.11.0")
        } else {
            base.join("runtimes").join("node").join("22.11.0").join("bin")
        };
        fs::create_dir_all(&v20_bin_dir).unwrap();
        fs::create_dir_all(&v22_bin_dir).unwrap();

        let node_bin_name = if cfg!(windows) { "node.exe" } else { "node" };
        let v20_node = v20_bin_dir.join(node_bin_name);
        let v22_node = v22_bin_dir.join(node_bin_name);
        fs::write(&v20_node, "node20").unwrap();
        fs::write(&v22_node, "node22").unwrap();

        // Global current points to 22.11.0
        let current_node = base.join("current").join("node");
        #[cfg(unix)]
        atomic_symlink(Path::new("../runtimes/node/22.11.0"), &current_node).unwrap();
        #[cfg(windows)]
        fs::write(base.join("current").join("node.version"), "22.11.0\n").unwrap();

        // Project directory pins partial "20"
        let project_dir = dir.join("my_project");
        let nested_dir = project_dir.join("packages").join("app");
        fs::create_dir_all(&nested_dir).unwrap();
        fs::write(project_dir.join(".node-version"), "20\n").unwrap();

        // Directory without any pin
        let plain_dir = dir.join("plain");
        fs::create_dir_all(&plain_dir).unwrap();

        // Pinned directory resolves to 20.18.0 without changing global current
        let resolved_pinned = resolve_shim_target_at(&base, &nested_dir, "node")
            .unwrap()
            .unwrap();
        assert_eq!(resolved_pinned, (v20_node, v20_bin_dir));
        assert_eq!(
            get_current_version_at(&base, "node").unwrap(),
            Some("22.11.0".to_string())
        );

        // Unpinned directory falls back to global current (22.11.0)
        let resolved_global = resolve_shim_target_at(&base, &plain_dir, "node")
            .unwrap()
            .unwrap();
        assert_eq!(resolved_global, (v22_node, v22_bin_dir));

        // Non-runtime binary name returns None
        assert_eq!(
            resolve_shim_target_at(&base, &nested_dir, "synapse").unwrap(),
            None
        );

        // Missing pinned version fails with NotInstalled
        let missing_dir = dir.join("missing_pin");
        fs::create_dir_all(&missing_dir).unwrap();
        fs::write(missing_dir.join(".node-version"), "18.0.0\n").unwrap();
        let err = resolve_shim_target_at(&base, &missing_dir, "node").unwrap_err();
        assert!(matches!(err, SynapseError::NotInstalled(ref r, ref v) if r == "node" && v == "18.0.0"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_refresh_shims_if_needed() {
        let dir = setup_test_dir("refresh_shims");
        let bin_dir = dir.join("bin");
        fs::create_dir_all(&bin_dir).unwrap();

        #[cfg(unix)]
        {
            fs::write(bin_dir.join("synapse"), "binary").unwrap();
            atomic_symlink(Path::new("../current/node/bin/node"), &bin_dir.join("node")).unwrap();
            atomic_symlink(Path::new("../current/node/bin/pnpm"), &bin_dir.join("pnpm")).unwrap();

            refresh_shims_if_needed(&bin_dir).unwrap();

            assert_eq!(
                fs::read_link(bin_dir.join("node")).unwrap(),
                PathBuf::from("synapse")
            );
            // Non-runtime global package symlink should stay untouched
            assert_eq!(
                fs::read_link(bin_dir.join("pnpm")).unwrap(),
                PathBuf::from("../current/node/bin/pnpm")
            );
        }

        let _ = fs::remove_dir_all(&dir);
    }
}
