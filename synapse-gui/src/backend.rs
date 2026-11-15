use std::collections::BTreeSet;
use std::path::Path;

use synapse::core::{globals, service, switcher, version_file};
use synapse::platform::paths;
use synapse::provider::bun::BunProvider;
use synapse::provider::composer::ComposerProvider;
use synapse::provider::nginx::NginxProvider;
use synapse::provider::node::NodeProvider;
use synapse::provider::php::PhpProvider;
use synapse::provider::postgres::PostgresProvider;
use synapse::provider::RuntimeProvider;

pub const ALL_RUNTIMES: &[&str] = &["node", "bun", "php", "composer", "postgres", "nginx"];
pub const GLOBALS_RUNTIMES: &[&str] = &["node", "bun", "composer"];

#[derive(Debug, Clone)]
pub struct RuntimeOverview {
    pub id: &'static str,
    pub display_name: &'static str,
    pub active_version: Option<String>,
    pub installed_versions: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ServiceOverview {
    pub id: &'static str,
    pub display_name: &'static str,
    pub running: bool,
    pub pid: Option<u32>,
    pub detail: String,
}

#[derive(Debug, Clone)]
pub struct GlobalsOverview {
    pub runtime: &'static str,
    pub active_version: Option<String>,
    pub manifest_path: String,
    pub tracked: Vec<String>,
    pub installed: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ProjectPinOverview {
    pub runtime: String,
    pub version: String,
    pub source_file: String,
}

#[derive(Debug, Clone)]
pub struct DashboardSnapshot {
    pub runtimes: Vec<RuntimeOverview>,
    pub services: Vec<ServiceOverview>,
    pub globals: Vec<GlobalsOverview>,
    pub project_dir: String,
    pub pins: Vec<ProjectPinOverview>,
}

#[derive(Debug, Clone)]
pub struct RemoteVersionItem {
    pub version: String,
    pub lts: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ServiceLogFile {
    pub label: String,
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceAction {
    Start,
    Stop,
    Restart,
}

pub fn runtime_display_name(runtime: &str) -> &'static str {
    match runtime {
        "node" => "Node.js",
        "bun" => "Bun",
        "php" => "PHP",
        "composer" => "Composer",
        "postgres" => "PostgreSQL",
        "nginx" => "Nginx",
        _ => "Runtime",
    }
}

fn service_display_name(svc: &str) -> &'static str {
    match svc {
        "postgres" => "PostgreSQL",
        "nginx" => "Nginx",
        _ => "Service",
    }
}

fn runtime_spec(runtime: &str) -> Result<(&'static [&'static str], &'static str), String> {
    match runtime {
        "node" => {
            let p = NodeProvider::new();
            Ok((p.binary_names(), p.bin_subdir()))
        }
        "bun" => {
            let p = BunProvider::new();
            Ok((p.binary_names(), p.bin_subdir()))
        }
        "php" => {
            let p = PhpProvider::new();
            Ok((p.binary_names(), p.bin_subdir()))
        }
        "composer" => {
            let p = ComposerProvider::new();
            Ok((p.binary_names(), p.bin_subdir()))
        }
        "postgres" => {
            let p = PostgresProvider::new();
            Ok((p.binary_names(), p.bin_subdir()))
        }
        "nginx" => {
            let p = NginxProvider::new();
            Ok((p.binary_names(), p.bin_subdir()))
        }
        _ => Err(format!("Unsupported runtime: {}", runtime)),
    }
}

pub fn load_dashboard_snapshot(project_dir: &Path) -> Result<DashboardSnapshot, String> {
    paths::ensure_dirs().map_err(|e| e.to_string())?;

    let mut runtimes = Vec::with_capacity(ALL_RUNTIMES.len());
    for &rt in ALL_RUNTIMES {
        let active_version = switcher::get_current_version(rt).map_err(|e| e.to_string())?;
        let installed_versions =
            switcher::list_installed_versions(rt).map_err(|e| e.to_string())?;
        runtimes.push(RuntimeOverview {
            id: rt,
            display_name: runtime_display_name(rt),
            active_version,
            installed_versions,
        });
    }

    let mut services = Vec::with_capacity(service::SUPPORTED_SERVICES.len());
    for &svc in service::SUPPORTED_SERVICES {
        let state = service::status_service(svc).map_err(|e| e.to_string())?;
        let (running, pid, detail) = match state {
            service::ServiceState::Running { pid, detail } => (true, pid, detail),
            service::ServiceState::Stopped { detail } => (false, None, detail),
        };
        services.push(ServiceOverview {
            id: svc,
            display_name: service_display_name(svc),
            running,
            pid,
            detail,
        });
    }

    let mut globals_list = Vec::with_capacity(GLOBALS_RUNTIMES.len());
    for &rt in GLOBALS_RUNTIMES {
        let manifest_path = paths::globals_manifest_path(rt).map_err(|e| e.to_string())?;
        let tracked = globals::load_manifest_at(&manifest_path).map_err(|e| e.to_string())?;
        let active_version = switcher::get_current_version(rt).map_err(|e| e.to_string())?;
        let installed = match &active_version {
            Some(ver) => globals::discover_installed_globals(rt, ver).map_err(|e| e.to_string())?,
            None => Vec::new(),
        };
        globals_list.push(GlobalsOverview {
            runtime: rt,
            active_version,
            manifest_path: manifest_path.display().to_string(),
            tracked,
            installed,
        });
    }

    let pins = if project_dir.is_dir() {
        version_file::find_all_pinned_versions(project_dir)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|(runtime, version, path)| ProjectPinOverview {
                runtime,
                version,
                source_file: path.display().to_string(),
            })
            .collect()
    } else {
        Vec::new()
    };

    Ok(DashboardSnapshot {
        runtimes,
        services,
        globals: globals_list,
        project_dir: project_dir.display().to_string(),
        pins,
    })
}

pub fn fetch_remote_versions(
    tokio_rt: &tokio::runtime::Runtime,
    runtime: &str,
) -> Result<Vec<RemoteVersionItem>, String> {
    let list = tokio_rt
        .block_on(async {
            match runtime {
                "node" => NodeProvider::new().list_remote().await,
                "bun" => BunProvider::new().list_remote().await,
                "php" => PhpProvider::new().list_remote().await,
                "composer" => ComposerProvider::new().list_remote().await,
                "postgres" => PostgresProvider::new().list_remote().await,
                "nginx" => NginxProvider::new().list_remote().await,
                _ => Err(synapse::error::SynapseError::UnsupportedRuntime(
                    runtime.to_string(),
                )),
            }
        })
        .map_err(|e| e.to_string())?;

    Ok(list
        .into_iter()
        .map(|item| RemoteVersionItem {
            version: item.version,
            lts: item.lts,
        })
        .collect())
}

pub fn install_runtime_version(
    tokio_rt: &tokio::runtime::Runtime,
    runtime: &str,
    version: &str,
    activate: bool,
) -> Result<String, String> {
    let trimmed = version.trim();
    if trimmed.is_empty() {
        return Err("Masukkan versi yang ingin di-install (mis. 22, lts, latest)".to_string());
    }

    let path = tokio_rt
        .block_on(async {
            match runtime {
                "node" => NodeProvider::new().download(trimmed).await,
                "bun" => BunProvider::new().download(trimmed).await,
                "php" => PhpProvider::new().download(trimmed).await,
                "composer" => ComposerProvider::new().download(trimmed).await,
                "postgres" => PostgresProvider::new().download(trimmed).await,
                "nginx" => NginxProvider::new().download(trimmed).await,
                _ => Err(synapse::error::SynapseError::UnsupportedRuntime(
                    runtime.to_string(),
                )),
            }
        })
        .map_err(|e| e.to_string())?;

    let installed_ver = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(trimmed)
        .to_string();

    if activate {
        let _ = switch_runtime_version(runtime, &installed_ver)?;
        Ok(format!(
            "Installed dan mengaktifkan {} v{}",
            runtime_display_name(runtime),
            installed_ver
        ))
    } else {
        Ok(format!(
            "Installed {} v{}",
            runtime_display_name(runtime),
            installed_ver
        ))
    }
}

pub fn switch_runtime_version(runtime: &str, version: &str) -> Result<String, String> {
    let (bins, bin_subdir) = runtime_spec(runtime)?;
    let clean_input = version
        .trim()
        .strip_prefix("bun-v")
        .or_else(|| version.trim().strip_prefix('v'))
        .or_else(|| version.trim().strip_prefix('V'))
        .unwrap_or(version.trim());

    let synapse_dir = paths::synapse_dir().map_err(|e| e.to_string())?;
    let resolved = switcher::match_installed_version_at(&synapse_dir, runtime, clean_input)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("{} v{} belum terinstall", runtime, clean_input))?;

    switcher::switch(runtime, &resolved, bins, bin_subdir).map_err(|e| e.to_string())?;
    if runtime == "node"
        && let Ok(vdir) = paths::runtime_version_dir("node", &resolved)
        && let Ok(bdir) = paths::bin_dir()
    {
        let _ = globals::link_node_global_bins(&vdir, &bdir);
    }

    Ok(format!(
        "Sekarang aktif: {} v{}",
        runtime_display_name(runtime),
        resolved
    ))
}

pub fn uninstall_runtime_version(runtime: &str, version: &str) -> Result<String, String> {
    let (resolved, _) = switcher::uninstall_version(runtime, version).map_err(|e| e.to_string())?;
    Ok(format!(
        "Berhasil uninstall {} v{}",
        runtime_display_name(runtime),
        resolved
    ))
}

pub fn run_service_action(svc: &str, action: ServiceAction) -> Result<String, String> {
    match action {
        ServiceAction::Start => service::start_service(svc),
        ServiceAction::Stop => service::stop_service(svc),
        ServiceAction::Restart => service::restart_service(svc),
    }
    .map_err(|e| e.to_string())
}

pub fn load_service_logs(svc: &str, lines: usize) -> Result<Vec<ServiceLogFile>, String> {
    let targets = service::service_log_paths(svc).map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(targets.len());
    for (label, path) in targets {
        let content = match service::tail_file_lines(&path, lines).map_err(|e| e.to_string())? {
            Some(text) if !text.is_empty() => text,
            Some(_) => "(file log kosong)".to_string(),
            None => "(file log belum dibuat — jalankan service terlebih dahulu)".to_string(),
        };
        out.push(ServiceLogFile {
            label,
            path: path.display().to_string(),
            content,
        });
    }
    Ok(out)
}

pub fn globals_add_and_sync(runtime: &str, raw_packages: &str) -> Result<String, String> {
    let packages: Vec<String> = raw_packages
        .split(|c: char| c.is_whitespace() || c == ',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    if packages.is_empty() {
        return Err("Masukkan minimal satu nama package".to_string());
    }
    for pkg in &packages {
        if !globals::is_valid_package_name(pkg) {
            return Err(format!("Nama package tidak valid: '{}'", pkg));
        }
    }

    let manifest_path = paths::globals_manifest_path(runtime).map_err(|e| e.to_string())?;
    let mut merged: BTreeSet<String> = globals::load_manifest_at(&manifest_path)
        .map_err(|e| e.to_string())?
        .into_iter()
        .collect();
    for pkg in packages {
        merged.insert(pkg);
    }
    let list: Vec<String> = merged.into_iter().collect();
    globals::save_manifest_at(&manifest_path, &list).map_err(|e| e.to_string())?;

    let synced = globals::sync_globals(runtime).map_err(|e| e.to_string())?;
    Ok(format!(
        "Berhasil sync {} global package untuk {}",
        synced.len(),
        runtime_display_name(runtime)
    ))
}

pub fn globals_remove(runtime: &str, package: &str) -> Result<String, String> {
    let manifest_path = paths::globals_manifest_path(runtime).map_err(|e| e.to_string())?;
    let existing = globals::load_manifest_at(&manifest_path).map_err(|e| e.to_string())?;
    let filtered: Vec<String> = existing.into_iter().filter(|p| p != package).collect();
    globals::save_manifest_at(&manifest_path, &filtered).map_err(|e| e.to_string())?;
    Ok(format!(
        "Menghapus '{}' dari manifest {} ({} tersisa)",
        package,
        runtime_display_name(runtime),
        filtered.len()
    ))
}

pub fn globals_sync(runtime: &str) -> Result<String, String> {
    let synced = globals::sync_globals(runtime).map_err(|e| e.to_string())?;
    if synced.is_empty() {
        Ok(format!(
            "Tidak ada global package untuk di-sync pada {}",
            runtime_display_name(runtime)
        ))
    } else {
        Ok(format!(
            "Synced {} package untuk {}: {}",
            synced.len(),
            runtime_display_name(runtime),
            synced.join(", ")
        ))
    }
}

pub fn globals_migrate(runtime: &str, from_version: &str) -> Result<String, String> {
    let trimmed = from_version.trim();
    if trimmed.is_empty() {
        return Err("Masukkan versi sumber untuk migrasi (mis. 20)".to_string());
    }
    let (from_ver, to_ver, synced) =
        globals::migrate_globals(runtime, trimmed, None).map_err(|e| e.to_string())?;
    Ok(format!(
        "Migrasi {} package {} (v{} -> v{}) selesai",
        synced.len(),
        runtime_display_name(runtime),
        from_ver,
        to_ver
    ))
}

pub fn pin_runtime_in_dir(
    tokio_rt: &tokio::runtime::Runtime,
    dir: &Path,
    runtime: &str,
    version: &str,
) -> Result<String, String> {
    if !dir.is_dir() {
        return Err(format!("Direktori tidak ditemukan: {}", dir.display()));
    }
    let clean = version
        .trim()
        .strip_prefix("bun-v")
        .or_else(|| version.trim().strip_prefix('v'))
        .or_else(|| version.trim().strip_prefix('V'))
        .unwrap_or(version.trim());
    if clean.is_empty() {
        return Err("Masukkan versi yang ingin di-pin".to_string());
    }

    let local = switcher::list_installed_versions(runtime).map_err(|e| e.to_string())?;
    let resolved = if let Some(matched) = local
        .into_iter()
        .find(|v| v == clean || v.starts_with(&format!("{}.", clean)))
    {
        matched
    } else if synapse::core::version_file::parse_single_version_content(clean).is_some()
        && clean.chars().all(|c| c.is_ascii_digit() || c == '.')
        && clean.split('.').count() == 3
    {
        clean.to_string()
    } else {
        tokio_rt
            .block_on(async {
                match runtime {
                    "node" => NodeProvider::new().resolve_version(clean).await,
                    "bun" => BunProvider::new().resolve_version(clean).await,
                    "php" => PhpProvider::new().resolve_version(clean).await,
                    "composer" => ComposerProvider::new().resolve_version(clean).await,
                    "postgres" => PostgresProvider::new().resolve_version(clean).await,
                    "nginx" => NginxProvider::new().resolve_version(clean).await,
                    _ => Err(synapse::error::SynapseError::UnsupportedRuntime(
                        runtime.to_string(),
                    )),
                }
            })
            .map_err(|e| e.to_string())?
    };

    let pin_path = version_file::write_pin_file(dir, runtime, &resolved).map_err(|e| e.to_string())?;
    Ok(format!(
        "Pinned {} v{} di {}",
        runtime_display_name(runtime),
        resolved,
        pin_path.display()
    ))
}

pub fn use_all_pinned_in_dir(dir: &Path) -> Result<String, String> {
    if !dir.is_dir() {
        return Err(format!("Direktori tidak ditemukan: {}", dir.display()));
    }
    let pins = version_file::find_all_pinned_versions(dir).map_err(|e| e.to_string())?;
    if pins.is_empty() {
        return Err("Tidak ditemukan pin file di direktori ini atau parent-nya".to_string());
    }
    let mut switched = Vec::new();
    for (rt, ver, _) in pins {
        switch_runtime_version(&rt, &ver)?;
        switched.push(format!("{} v{}", rt, ver));
    }
    Ok(format!("Mengaktifkan: {}", switched.join(", ")))
}
