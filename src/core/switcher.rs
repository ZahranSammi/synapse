//! Version switcher: updates active symlinks for runtimes and binary shims.

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

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

/// Switches the active version of a runtime.
///
/// 1. Verifies that the requested version is installed under `~/.synapse/runtimes/<runtime>/<version>/`.
/// 2. Updates `~/.synapse/current/<runtime>` -> `../runtimes/<runtime>/<version>`.
/// 3. Updates `~/.synapse/bin/<bin>` (symlinks on Unix, `.cmd` shims on Windows).
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
        for bin_name in binary_names {
            let bin_target = if bin_subdir.is_empty() {
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

/// Returns the currently active version for a given runtime by inspecting `~/.synapse/current/<runtime>`.
pub fn get_current_version(runtime: &str) -> Result<Option<String>> {
    #[cfg(windows)]
    {
        let version_marker = paths::current_dir()?.join(format!("{}.version", runtime));
        if version_marker.is_file() {
            let content = fs::read_to_string(&version_marker)?;
            let trimmed = content.trim();
            if !trimmed.is_empty() {
                return Ok(Some(trimmed.to_string()));
            }
        }
    }

    let current_link = paths::current_runtime_symlink(runtime)?;
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
}
