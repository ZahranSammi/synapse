//! CLI layer: argument parsing and subcommand execution.

use std::fs;
use clap::{Parser, Subcommand};
use colored::Colorize;

use crate::core::service::{self, ServiceState};
use crate::error::{Result, SynapseError};
use crate::platform::paths;
use crate::provider::bun::BunProvider;
use crate::provider::composer::ComposerProvider;
use crate::provider::nginx::NginxProvider;
use crate::provider::node::NodeProvider;
use crate::provider::php::PhpProvider;
use crate::provider::postgres::PostgresProvider;
use crate::provider::RuntimeProvider;

#[derive(Parser, Debug)]
#[command(name = "synapse")]
#[command(about = "Universal Version Manager for Node.js, Bun, PHP, Composer, PostgreSQL & Nginx", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Install a runtime version (explicitly or from project version files in current directory)
    Install {
        /// Runtime identifier ("node", "bun", "php", "composer", "postgres", "nginx"); omit to install all pinned runtimes in current directory
        runtime: Option<String>,
        /// Target version (e.g. "22", "lts", "latest" for node; "17", "latest" for postgres; "stable", "latest" for nginx); omit to read from project version file
        version: Option<String>,
        /// Do not switch to this version after installing
        #[arg(long)]
        no_use: bool,
    },
    /// Switch active version (explicitly or from project version files in current directory)
    Use {
        /// Runtime identifier ("node", "bun", "php", "composer", "postgres", "nginx"); omit to use all pinned runtimes in current directory
        runtime: Option<String>,
        /// Target version; omit to read from project version file (.node-version, .nvmrc, .bun-version, .php-version, .composer-version, .postgres-version, .nginx-version, .synapse-version)
        version: Option<String>,
    },
    /// Pin a runtime version in the current directory
    Pin {
        /// Runtime identifier ("node", "bun", "php", "composer", "postgres", "nginx")
        runtime: String,
        /// Version to pin (e.g. "22", "20.18.0", "lts", "latest")
        version: String,
    },
    /// List locally installed versions
    List {
        /// Runtime identifier ("node", "bun", "php", "composer", "postgres", "nginx")
        runtime: String,
    },
    /// List available remote versions
    ListRemote {
        /// Runtime identifier ("node", "bun", "php", "composer", "postgres", "nginx")
        runtime: String,
        /// Maximum number of versions to display (shows all by default)
        #[arg(short, long)]
        limit: Option<usize>,
    },
    /// Show currently active runtime version
    Current {
        /// Runtime identifier ("node", "bun", "php", "composer", "postgres", "nginx", or omit to show all)
        runtime: Option<String>,
    },
    /// Uninstall a specific runtime version
    Uninstall {
        /// Runtime identifier ("node", "bun", "php", "composer", "postgres", "nginx")
        runtime: String,
        /// Version to remove
        version: String,
    },
    /// Manage and sync global packages across runtime versions
    Globals {
        /// Runtime identifier ("node", "bun", "composer")
        runtime: String,
        #[command(subcommand)]
        action: GlobalsCommand,
    },
    /// Start, stop, restart, inspect logs, or check status of managed background services (postgres, nginx)
    Service {
        #[command(subcommand)]
        action: ServiceCommand,
    },
    /// Output export PATH configuration for shell profiles
    Env,
    /// Install the synapse binary into ~/.synapse/bin and configure shell PATH
    Setup,
    /// Generate shell completion script for bash, zsh, fish, or powershell
    Completions {
        /// Target shell ("bash", "zsh", "fish", "powershell")
        shell: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum ServiceCommand {
    /// Start a managed background service ("postgres" or "nginx")
    Start {
        /// Service identifier ("postgres" or "nginx")
        service: String,
    },
    /// Stop a managed background service ("postgres" or "nginx")
    Stop {
        /// Service identifier ("postgres" or "nginx")
        service: String,
    },
    /// Restart a managed background service ("postgres" or "nginx")
    Restart {
        /// Service identifier ("postgres" or "nginx")
        service: String,
    },
    /// Show status of managed background service(s)
    Status {
        /// Service identifier ("postgres", "nginx", or omit to show all)
        service: Option<String>,
    },
    /// View or follow log files for a managed background service ("postgres" or "nginx")
    Logs {
        /// Service identifier ("postgres" or "nginx")
        service: String,
        /// Number of trailing lines to display
        #[arg(short = 'n', long, default_value_t = 50)]
        lines: usize,
        /// Follow log output in real time
        #[arg(short, long)]
        follow: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum GlobalsCommand {
    /// List tracked and installed global packages
    List,
    /// Add package(s) to global manifest and sync into active runtime
    Add {
        /// Package names to add (e.g. "pnpm", "typescript")
        #[arg(required = true)]
        packages: Vec<String>,
    },
    /// Remove package(s) from global manifest
    Remove {
        /// Package names to remove
        #[arg(required = true)]
        packages: Vec<String>,
    },
    /// Sync/reinstall all tracked global packages into the currently active runtime version
    Sync,
    /// Migrate global packages from another installed version into target (or active) version
    Migrate {
        /// Source installed version (e.g. "20" or "20.18.0")
        from: String,
        /// Target installed version (defaults to currently active version)
        to: Option<String>,
    },
}

pub async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Install {
            runtime,
            version,
            no_use,
        } => handle_install(runtime.as_deref(), version.as_deref(), no_use).await,
        Command::Use { runtime, version } => {
            handle_use(runtime.as_deref(), version.as_deref()).await
        }
        Command::Pin { runtime, version } => handle_pin(&runtime, &version).await,
        Command::List { runtime } => handle_list(&runtime),
        Command::ListRemote { runtime, limit } => handle_list_remote(&runtime, limit).await,
        Command::Current { runtime } => handle_current(runtime.as_deref()),
        Command::Uninstall { runtime, version } => handle_uninstall(&runtime, &version),
        Command::Globals { runtime, action } => handle_globals(&runtime, action),
        Command::Service { action } => handle_service(action),
        Command::Env => handle_env(),
        Command::Setup => handle_setup(),
        Command::Completions { shell } => handle_completions(&shell),
    }
}

fn runtime_display_name(runtime: &str) -> &'static str {
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

async fn install_runtime(
    provider: &impl RuntimeProvider,
    runtime: &str,
    version: &str,
    no_use: bool,
) -> Result<()> {
    let display = runtime_display_name(runtime);
    println!("{} Resolving and downloading {} {}...", "•".blue(), display, version);
    let path = provider.download(version).await?;
    let version_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(version);

    println!(
        "{} Successfully installed {} v{} to {}",
        "✓".green().bold(),
        display,
        version_name,
        path.display()
    );

    if !no_use {
        crate::core::switcher::switch(
            runtime,
            version_name,
            provider.binary_names(),
            provider.bin_subdir(),
        )?;
        if runtime == "node"
            && let Ok(vdir) = paths::runtime_version_dir("node", version_name)
            && let Ok(bdir) = paths::bin_dir()
        {
            let _ = crate::core::globals::link_node_global_bins(&vdir, &bdir);
        }
        println!("{} Now using {} v{}", "✓".green().bold(), display, version_name);
        ensure_shell_path();
    }

    Ok(())
}

async fn use_runtime(
    provider: &impl RuntimeProvider,
    runtime: &str,
    version: &str,
) -> Result<()> {
    let display = runtime_display_name(runtime);
    let clean_input = version
        .trim()
        .strip_prefix("bun-v")
        .or_else(|| version.trim().strip_prefix('v'))
        .unwrap_or(version.trim());

    // First check if the exact input folder exists locally
    let mut target_version = clean_input.to_string();
    let version_dir = paths::runtime_version_dir(runtime, &target_version)?;

    if !version_dir.exists() {
        // If not exact match, check installed local versions for partial match (e.g. "22" matches "22.23.2")
        let local_matches = get_installed_versions(runtime)?;
        if let Some(matched) = local_matches.into_iter().find(|v| {
            v == clean_input || v.starts_with(&format!("{}.", clean_input))
        }) {
            target_version = matched;
        } else {
            // Attempt to resolve via remote index if not found locally
            if let Ok(resolved) = provider.resolve_version(clean_input).await {
                if paths::runtime_version_dir(runtime, &resolved)?.exists() {
                    target_version = resolved;
                }
            }
        }
    }

    crate::core::switcher::switch(
        runtime,
        &target_version,
        provider.binary_names(),
        provider.bin_subdir(),
    )?;
    if runtime == "node"
        && let Ok(vdir) = paths::runtime_version_dir("node", &target_version)
        && let Ok(bdir) = paths::bin_dir()
    {
        let _ = crate::core::globals::link_node_global_bins(&vdir, &bdir);
    }
    println!("{} Now using {} v{}", "✓".green().bold(), display, target_version);
    ensure_shell_path();
    Ok(())
}

async fn list_remote_runtime(
    provider: &impl RuntimeProvider,
    runtime: &str,
    limit: Option<usize>,
) -> Result<()> {
    let mut versions = provider.list_remote().await?;
    if let Some(n) = limit {
        versions.truncate(n);
    }
    let installed = get_installed_versions(runtime)?;
    let current = crate::core::switcher::get_current_version(runtime)?;

    let rows: Vec<(String, String, String, bool, bool, bool)> = versions
        .into_iter()
        .map(|item| {
            let version_plain = format!("v{}", item.version);
            let lts_plain = match &item.lts {
                Some(name) if name == "LTS" => "LTS".to_string(),
                Some(name) => format!("LTS ({})", name),
                None => "-".to_string(),
            };
            let is_installed = installed.contains(&item.version);
            let is_active = current.as_deref() == Some(&item.version);
            let status_plain = if is_active {
                "✓ active (installed)".to_string()
            } else if is_installed {
                "installed".to_string()
            } else {
                "-".to_string()
            };
            (
                version_plain,
                lts_plain,
                status_plain,
                item.lts.is_some(),
                is_active,
                is_installed,
            )
        })
        .collect();

    let ver_w = rows
        .iter()
        .map(|r| r.0.chars().count())
        .max()
        .unwrap_or(0)
        .max("VERSION".len());
    let lts_w = rows
        .iter()
        .map(|r| r.1.chars().count())
        .max()
        .unwrap_or(0)
        .max("LTS".len());
    let status_w = rows
        .iter()
        .map(|r| r.2.chars().count())
        .max()
        .unwrap_or(0)
        .max("STATUS".len());

    let border = format!(
        "+-{}-+-{}-+-{}-+",
        "-".repeat(ver_w),
        "-".repeat(lts_w),
        "-".repeat(status_w)
    );

    println!("Available remote versions for {}:", runtime.bold());
    println!("{}", border);
    println!(
        "| {} | {} | {} |",
        format!("{:<ver_w$}", "VERSION").bold(),
        format!("{:<lts_w$}", "LTS").bold(),
        format!("{:<status_w$}", "STATUS").bold()
    );
    println!("{}", border);

    for (version_plain, lts_plain, status_plain, is_lts, is_active, is_installed) in rows {
        let lts_padded = format!("{:<lts_w$}", lts_plain);
        let lts_styled = if is_lts {
            lts_padded.yellow().bold().to_string()
        } else {
            lts_padded.dimmed().to_string()
        };

        let status_padded = format!("{:<status_w$}", status_plain);
        let status_styled = if is_active {
            status_padded.green().bold().to_string()
        } else if is_installed {
            status_padded.dimmed().to_string()
        } else {
            status_padded
        };

        println!(
            "| {:<ver_w$} | {} | {} |",
            version_plain, lts_styled, status_styled
        );
    }

    println!("{}", border);
    Ok(())
}

fn resolve_pinned_targets(
    cwd: &std::path::Path,
    runtime: Option<&str>,
) -> Result<Vec<(String, String, std::path::PathBuf)>> {
    match runtime {
        Some(r) => {
            let (pinned_ver, pin_path) = crate::core::version_file::find_pinned_version(cwd, r)?
                .ok_or_else(|| SynapseError::PinNotFound(r.to_string()))?;
            Ok(vec![(r.to_string(), pinned_ver, pin_path)])
        }
        None => {
            let pins = crate::core::version_file::find_all_pinned_versions(cwd)?;
            if pins.is_empty() {
                return Err(SynapseError::PinNotFound(
                    "any runtime (.node-version, .nvmrc, .bun-version, .php-version, .composer-version, .postgres-version, .nginx-version, .synapse-version)".to_string(),
                ));
            }
            Ok(pins)
        }
    }
}

async fn install_runtime_by_name(runtime: &str, version: &str, no_use: bool) -> Result<()> {
    match runtime {
        "node" => install_runtime(&NodeProvider::new(), runtime, version, no_use).await,
        "bun" => install_runtime(&BunProvider::new(), runtime, version, no_use).await,
        "php" => install_runtime(&PhpProvider::new(), runtime, version, no_use).await,
        "composer" => install_runtime(&ComposerProvider::new(), runtime, version, no_use).await,
        "postgres" => install_runtime(&PostgresProvider::new(), runtime, version, no_use).await,
        "nginx" => install_runtime(&NginxProvider::new(), runtime, version, no_use).await,
        _ => Err(SynapseError::UnsupportedRuntime(runtime.to_string())),
    }
}

async fn handle_install(
    runtime: Option<&str>,
    version: Option<&str>,
    no_use: bool,
) -> Result<()> {
    match (runtime, version) {
        (Some(r), Some(v)) => install_runtime_by_name(r, v, no_use).await,
        (r, None) => {
            let cwd = std::env::current_dir()?;
            for (rt, pinned_ver, pin_path) in resolve_pinned_targets(&cwd, r)? {
                println!(
                    "{} Detected {} v{} in {}",
                    "•".blue(),
                    rt,
                    pinned_ver,
                    pin_path.display()
                );
                install_runtime_by_name(&rt, &pinned_ver, no_use).await?;
            }
            Ok(())
        }
        (None, Some(_)) => Err(SynapseError::UnsupportedRuntime(String::new())),
    }
}

async fn switch_runtime_by_name(runtime: &str, version: &str) -> Result<()> {
    match runtime {
        "node" => use_runtime(&NodeProvider::new(), runtime, version).await,
        "bun" => use_runtime(&BunProvider::new(), runtime, version).await,
        "php" => use_runtime(&PhpProvider::new(), runtime, version).await,
        "composer" => use_runtime(&ComposerProvider::new(), runtime, version).await,
        "postgres" => use_runtime(&PostgresProvider::new(), runtime, version).await,
        "nginx" => use_runtime(&NginxProvider::new(), runtime, version).await,
        _ => Err(SynapseError::UnsupportedRuntime(runtime.to_string())),
    }
}

async fn pin_runtime(provider: &impl RuntimeProvider, runtime: &str, version: &str) -> Result<()> {
    let clean_input = version
        .trim()
        .strip_prefix("bun-v")
        .or_else(|| version.trim().strip_prefix('v'))
        .or_else(|| version.trim().strip_prefix('V'))
        .unwrap_or(version.trim());

    let local_installed = get_installed_versions(runtime)?;
    let resolved_version = if let Some(matched) = local_installed
        .into_iter()
        .find(|v| v == clean_input || v.starts_with(&format!("{}.", clean_input)))
    {
        matched
    } else if semver::Version::parse(clean_input).is_ok() {
        clean_input.to_string()
    } else {
        provider.resolve_version(clean_input).await?
    };

    let cwd = std::env::current_dir()?;
    let pin_path = crate::core::version_file::write_pin_file(&cwd, runtime, &resolved_version)?;
    let display = runtime_display_name(runtime);
    println!(
        "{} Pinned {} v{} in {}",
        "✓".green().bold(),
        display,
        resolved_version,
        pin_path.display()
    );
    Ok(())
}

async fn handle_pin(runtime: &str, version: &str) -> Result<()> {
    match runtime {
        "node" => pin_runtime(&NodeProvider::new(), runtime, version).await,
        "bun" => pin_runtime(&BunProvider::new(), runtime, version).await,
        "php" => pin_runtime(&PhpProvider::new(), runtime, version).await,
        "composer" => pin_runtime(&ComposerProvider::new(), runtime, version).await,
        "postgres" => pin_runtime(&PostgresProvider::new(), runtime, version).await,
        "nginx" => pin_runtime(&NginxProvider::new(), runtime, version).await,
        _ => Err(SynapseError::UnsupportedRuntime(runtime.to_string())),
    }
}

async fn handle_use(runtime: Option<&str>, version: Option<&str>) -> Result<()> {
    match (runtime, version) {
        (Some(r), Some(v)) => switch_runtime_by_name(r, v).await,
        (r, None) => {
            let cwd = std::env::current_dir()?;
            for (rt, pinned_ver, pin_path) in resolve_pinned_targets(&cwd, r)? {
                println!(
                    "{} Detected {} v{} in {}",
                    "•".blue(),
                    rt,
                    pinned_ver,
                    pin_path.display()
                );
                switch_runtime_by_name(&rt, &pinned_ver).await?;
            }
            Ok(())
        }
        (None, Some(_)) => Err(SynapseError::UnsupportedRuntime(String::new())),
    }
}

fn handle_list(runtime: &str) -> Result<()> {
    if !matches!(
        runtime,
        "node" | "bun" | "php" | "composer" | "postgres" | "nginx"
    ) {
        return Err(SynapseError::UnsupportedRuntime(runtime.to_string()));
    }

    paths::ensure_dirs()?;
    let installed = get_installed_versions(runtime)?;

    if installed.is_empty() {
        println!("No versions of '{}' are currently installed.", runtime);
        println!("Hint: run 'synapse install {} <version>' to install one.", runtime);
        return Ok(());
    }

    let current = crate::core::switcher::get_current_version(runtime)?;

    println!("Installed {} versions:", runtime.bold());
    for v in installed {
        if current.as_deref() == Some(&v) {
            println!("  {} v{} {}", "✓".green().bold(), v.green().bold(), "(active)".cyan());
        } else {
            println!("    v{}", v);
        }
    }

    Ok(())
}

async fn handle_list_remote(runtime: &str, limit: Option<usize>) -> Result<()> {
    match runtime {
        "node" => list_remote_runtime(&NodeProvider::new(), runtime, limit).await,
        "bun" => list_remote_runtime(&BunProvider::new(), runtime, limit).await,
        "php" => list_remote_runtime(&PhpProvider::new(), runtime, limit).await,
        "composer" => list_remote_runtime(&ComposerProvider::new(), runtime, limit).await,
        "postgres" => list_remote_runtime(&PostgresProvider::new(), runtime, limit).await,
        "nginx" => list_remote_runtime(&NginxProvider::new(), runtime, limit).await,
        _ => Err(SynapseError::UnsupportedRuntime(runtime.to_string())),
    }
}

fn handle_current(runtime: Option<&str>) -> Result<()> {
    paths::ensure_dirs()?;

    let runtimes = match runtime {
        Some(r) => vec![r],
        None => vec!["node", "bun", "php", "composer", "postgres", "nginx"],
    };

    for r in runtimes {
        match crate::core::switcher::get_current_version(r)? {
            Some(v) => println!("{}: v{} {}", r, v.green().bold(), "(active)".cyan()),
            None => println!("{}: none (no active version set)", r),
        }
    }

    Ok(())
}

fn handle_uninstall(runtime: &str, version: &str) -> Result<()> {
    let display = runtime_display_name(runtime);
    let (clean_version, was_active) = crate::core::switcher::uninstall_version(runtime, version)?;
    if was_active {
        println!(
            "{} Removed active version symlinks for {} v{}",
            "•".yellow(),
            display,
            clean_version
        );
    }
    println!(
        "{} Successfully uninstalled {} v{}",
        "✓".green().bold(),
        display,
        clean_version
    );

    Ok(())
}

fn handle_globals(runtime: &str, action: GlobalsCommand) -> Result<()> {
    if !matches!(runtime, "node" | "bun" | "composer") {
        return Err(SynapseError::UnsupportedRuntime(runtime.to_string()));
    }
    paths::ensure_dirs()?;

    let manifest_path = paths::globals_manifest_path(runtime)?;
    match action {
        GlobalsCommand::List => {
            let tracked = crate::core::globals::load_manifest_at(&manifest_path)?;
            let active = crate::core::switcher::get_current_version(runtime)?;
            let installed = match &active {
                Some(ver) => crate::core::globals::discover_installed_globals(runtime, ver)?,
                None => Vec::new(),
            };

            println!("Tracked global packages for {} ({}):", runtime.bold(), manifest_path.display());
            if tracked.is_empty() {
                println!("  (none)");
            } else {
                for pkg in &tracked {
                    println!("  • {}", pkg);
                }
            }

            if let Some(ver) = active {
                println!("Installed in {} v{}:", runtime.bold(), ver);
                if installed.is_empty() {
                    println!("  (none)");
                } else {
                    for pkg in &installed {
                        println!("  ✓ {}", pkg.green());
                    }
                }
            }
            Ok(())
        }
        GlobalsCommand::Add { packages } => {
            for pkg in &packages {
                if !crate::core::globals::is_valid_package_name(pkg) {
                    return Err(SynapseError::Io(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        format!("Invalid package name: '{}'", pkg),
                    )));
                }
            }
            let mut merged = crate::core::globals::load_manifest_at(&manifest_path)?;
            merged.extend(packages);
            crate::core::globals::save_manifest_at(&manifest_path, &merged)?;

            let synced = crate::core::globals::sync_globals(runtime)?;
            println!(
                "{} Synced {} global package(s) for {}",
                "✓".green().bold(),
                synced.len(),
                runtime_display_name(runtime)
            );
            Ok(())
        }
        GlobalsCommand::Remove { packages } => {
            let existing = crate::core::globals::load_manifest_at(&manifest_path)?;
            let filtered: Vec<String> = existing
                .into_iter()
                .filter(|p| !packages.contains(p))
                .collect();
            crate::core::globals::save_manifest_at(&manifest_path, &filtered)?;
            println!(
                "{} Updated global manifest ({} package(s) remaining)",
                "✓".green().bold(),
                filtered.len()
            );
            Ok(())
        }
        GlobalsCommand::Sync => {
            let synced = crate::core::globals::sync_globals(runtime)?;
            if synced.is_empty() {
                println!("No global packages to sync for '{}'.", runtime);
            } else {
                println!(
                    "{} Synced {} global package(s) for {}: {}",
                    "✓".green().bold(),
                    synced.len(),
                    runtime_display_name(runtime),
                    synced.join(", ")
                );
            }
            Ok(())
        }
        GlobalsCommand::Migrate { from, to } => {
            let (from_ver, to_ver, synced) =
                crate::core::globals::migrate_globals(runtime, &from, to.as_deref())?;
            if synced.is_empty() {
                println!(
                    "No global packages found to migrate from {} v{} to v{}.",
                    runtime, from_ver, to_ver
                );
            } else {
                println!(
                    "{} Migrated {} global package(s) for {} (v{} -> v{}): {}",
                    "✓".green().bold(),
                    synced.len(),
                    runtime_display_name(runtime),
                    from_ver,
                    to_ver,
                    synced.join(", ")
                );
            }
            Ok(())
        }
    }
}

fn handle_service(action: ServiceCommand) -> Result<()> {
    match action {
        ServiceCommand::Start { service: name } => {
            let msg = service::start_service(&name)?;
            println!("{} {}", "✓".green().bold(), msg);
            Ok(())
        }
        ServiceCommand::Stop { service: name } => {
            let msg = service::stop_service(&name)?;
            println!("{} {}", "✓".green().bold(), msg);
            Ok(())
        }
        ServiceCommand::Restart { service: name } => {
            let msg = service::restart_service(&name)?;
            println!("{} {}", "✓".green().bold(), msg);
            Ok(())
        }
        ServiceCommand::Status { service: target } => {
            let services: Vec<&str> = match target.as_deref() {
                Some(s) => vec![s],
                None => service::SUPPORTED_SERVICES.to_vec(),
            };
            for svc in services {
                match service::status_service(svc)? {
                    ServiceState::Running { pid, detail } => {
                        let pid_str = pid.map(|p| format!(" [PID {}]", p)).unwrap_or_default();
                        println!(
                            "{}: {} {}{}",
                            svc.bold(),
                            "running".green().bold(),
                            detail,
                            pid_str.cyan()
                        );
                    }
                    ServiceState::Stopped { detail } => {
                        println!("{}: {} ({})", svc.bold(), "stopped".dimmed(), detail);
                    }
                }
            }
            Ok(())
        }
        ServiceCommand::Logs {
            service: name,
            lines,
            follow,
        } => {
            let targets = service::service_log_paths(&name)?;
            for (label, path) in &targets {
                println!(
                    "==> {} ({}) <==",
                    label.bold().cyan(),
                    path.display().to_string().dimmed()
                );
                match service::tail_file_lines(path, lines)? {
                    Some(text) if !text.is_empty() => println!("{}", text),
                    Some(_) => println!("{}", "(log file is empty)".dimmed()),
                    None => println!(
                        "{}",
                        "(log file not created yet — start the service first)".dimmed()
                    ),
                }
            }
            if follow {
                println!("{} Following logs (press Ctrl+C to stop)...", "•".blue());
                service::follow_log_files(&targets)?;
            }
            Ok(())
        }
    }
}

fn handle_env() -> Result<()> {
    let bin_path = paths::bin_dir()?;
    if cfg!(windows) {
        println!(r#"$env:Path = "{};$env:Path""#, bin_path.display());
    } else {
        let shell = std::env::var("SHELL").unwrap_or_default();
        if shell.ends_with("/fish") || shell == "fish" {
            println!(r#"fish_add_path -p "{}""#, bin_path.display());
        } else {
            println!(r#"export PATH="{}:$PATH""#, bin_path.display());
        }
    }
    Ok(())
}

/// Generates a shell completion script for bash, zsh, fish, or powershell.
pub fn generate_completions(shell: &str) -> Result<&'static str> {
    match shell.trim().to_ascii_lowercase().as_str() {
        "bash" => Ok(BASH_COMPLETIONS),
        "zsh" => Ok(ZSH_COMPLETIONS),
        "fish" => Ok(FISH_COMPLETIONS),
        "powershell" | "pwsh" => Ok(POWERSHELL_COMPLETIONS),
        _ => Err(SynapseError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "Unsupported shell '{}'. Supported shells: bash, zsh, fish, powershell",
                shell
            ),
        ))),
    }
}

fn handle_completions(shell: &str) -> Result<()> {
    let script = generate_completions(shell)?;
    print!("{}", script);
    Ok(())
}

const BASH_COMPLETIONS: &str = r#"_synapse_completions() {
    local cur prev cmd
    COMPREPLY=()
    cur="${COMP_WORDS[COMP_CWORD]}"
    prev="${COMP_WORDS[COMP_CWORD-1]}"

    local commands="install use pin list list-remote current uninstall globals service env setup completions"
    local runtimes="node bun php composer postgres nginx"
    local globals_runtimes="node bun composer"
    local globals_actions="list add remove sync migrate"
    local service_actions="start stop restart status logs"
    local services="postgres nginx"
    local shells="bash zsh fish powershell"

    if [[ ${COMP_CWORD} -eq 1 ]]; then
        COMPREPLY=( $(compgen -W "${commands}" -- "${cur}") )
        return 0
    fi

    cmd="${COMP_WORDS[1]}"
    case "${cmd}" in
        install)
            if [[ ${cur} == -* ]]; then
                COMPREPLY=( $(compgen -W "--no-use --help" -- "${cur}") )
            elif [[ ${COMP_CWORD} -eq 2 ]]; then
                COMPREPLY=( $(compgen -W "${runtimes}" -- "${cur}") )
            fi
            ;;
        use|pin|list|current|uninstall)
            if [[ ${COMP_CWORD} -eq 2 ]]; then
                COMPREPLY=( $(compgen -W "${runtimes}" -- "${cur}") )
            fi
            ;;
        list-remote)
            if [[ ${cur} == -* ]]; then
                COMPREPLY=( $(compgen -W "--limit -l --help" -- "${cur}") )
            elif [[ ${COMP_CWORD} -eq 2 ]]; then
                COMPREPLY=( $(compgen -W "${runtimes}" -- "${cur}") )
            fi
            ;;
        globals)
            if [[ ${COMP_CWORD} -eq 2 ]]; then
                COMPREPLY=( $(compgen -W "${globals_runtimes}" -- "${cur}") )
            elif [[ ${COMP_CWORD} -eq 3 ]]; then
                COMPREPLY=( $(compgen -W "${globals_actions}" -- "${cur}") )
            fi
            ;;
        service)
            if [[ ${COMP_CWORD} -eq 2 ]]; then
                COMPREPLY=( $(compgen -W "${service_actions}" -- "${cur}") )
            elif [[ ${COMP_CWORD} -eq 3 ]]; then
                if [[ "${COMP_WORDS[2]}" == "logs" && ${cur} == -* ]]; then
                    COMPREPLY=( $(compgen -W "--lines -n --follow -f --help" -- "${cur}") )
                else
                    COMPREPLY=( $(compgen -W "${services}" -- "${cur}") )
                fi
            elif [[ "${COMP_WORDS[2]}" == "logs" && ${cur} == -* ]]; then
                COMPREPLY=( $(compgen -W "--lines -n --follow -f --help" -- "${cur}") )
            fi
            ;;
        completions)
            if [[ ${COMP_CWORD} -eq 2 ]]; then
                COMPREPLY=( $(compgen -W "${shells}" -- "${cur}") )
            fi
            ;;
    esac
}
complete -F _synapse_completions synapse
"#;

const ZSH_COMPLETIONS: &str = r#"#compdef synapse

_synapse() {
    local -a commands runtimes globals_runtimes globals_actions service_actions services shells
    commands=(
        'install:Install a runtime version'
        'use:Switch active version'
        'pin:Pin a runtime version in the current directory'
        'list:List locally installed versions'
        'list-remote:List available remote versions'
        'current:Show currently active runtime version'
        'uninstall:Uninstall a specific runtime version'
        'globals:Manage and sync global packages'
        'service:Manage background services'
        'env:Output export PATH configuration'
        'setup:Install synapse binary and configure PATH'
        'completions:Generate shell completion script'
    )
    runtimes=(node bun php composer postgres nginx)
    globals_runtimes=(node bun composer)
    globals_actions=(list add remove sync migrate)
    service_actions=(start stop restart status logs)
    services=(postgres nginx)
    shells=(bash zsh fish powershell)

    if (( CURRENT == 2 )); then
        _describe 'command' commands
        return
    fi

    case "${words[2]}" in
        install)
            if (( CURRENT == 3 )); then
                _describe 'runtime' runtimes
            else
                _arguments '--no-use[Do not switch to this version after installing]'
            fi
            ;;
        use|pin|list|current|uninstall)
            if (( CURRENT == 3 )); then
                _describe 'runtime' runtimes
            fi
            ;;
        list-remote)
            if (( CURRENT == 3 )); then
                _describe 'runtime' runtimes
            else
                _arguments '(-l --limit)'{-l,--limit}'[Maximum number of versions to display]:limit:'
            fi
            ;;
        globals)
            if (( CURRENT == 3 )); then
                _describe 'runtime' globals_runtimes
            elif (( CURRENT == 4 )); then
                _describe 'action' globals_actions
            fi
            ;;
        service)
            if (( CURRENT == 3 )); then
                _describe 'action' service_actions
            elif (( CURRENT == 4 )); then
                _describe 'service' services
            elif [[ "${words[3]}" == "logs" ]]; then
                _arguments \
                    '(-n --lines)'{-n,--lines}'[Number of trailing lines to display]:lines:' \
                    '(-f --follow)'{-f,--follow}'[Follow log output in real time]'
            fi
            ;;
        completions)
            if (( CURRENT == 3 )); then
                _describe 'shell' shells
            fi
            ;;
    esac
}

compdef _synapse synapse
"#;

const FISH_COMPLETIONS: &str = r#"complete -c synapse -f

set -l __synapse_commands install use pin list list-remote current uninstall globals service env setup completions
set -l __synapse_runtimes node bun php composer postgres nginx
set -l __synapse_globals_runtimes node bun composer
set -l __synapse_globals_actions list add remove sync migrate
set -l __synapse_service_actions start stop restart status logs
set -l __synapse_services postgres nginx
set -l __synapse_shells bash zsh fish powershell

complete -c synapse -n "not __fish_seen_subcommand_from $__synapse_commands" -a install -d "Install a runtime version"
complete -c synapse -n "not __fish_seen_subcommand_from $__synapse_commands" -a use -d "Switch active version"
complete -c synapse -n "not __fish_seen_subcommand_from $__synapse_commands" -a pin -d "Pin a runtime version"
complete -c synapse -n "not __fish_seen_subcommand_from $__synapse_commands" -a list -d "List locally installed versions"
complete -c synapse -n "not __fish_seen_subcommand_from $__synapse_commands" -a list-remote -d "List available remote versions"
complete -c synapse -n "not __fish_seen_subcommand_from $__synapse_commands" -a current -d "Show currently active runtime version"
complete -c synapse -n "not __fish_seen_subcommand_from $__synapse_commands" -a uninstall -d "Uninstall a runtime version"
complete -c synapse -n "not __fish_seen_subcommand_from $__synapse_commands" -a globals -d "Manage global packages"
complete -c synapse -n "not __fish_seen_subcommand_from $__synapse_commands" -a service -d "Manage background services"
complete -c synapse -n "not __fish_seen_subcommand_from $__synapse_commands" -a env -d "Output PATH setup command"
complete -c synapse -n "not __fish_seen_subcommand_from $__synapse_commands" -a setup -d "Install synapse binary and configure PATH"
complete -c synapse -n "not __fish_seen_subcommand_from $__synapse_commands" -a completions -d "Generate shell completions"

complete -c synapse -n "__fish_seen_subcommand_from install use pin list list-remote current uninstall; and not __fish_seen_subcommand_from $__synapse_runtimes" -a "$__synapse_runtimes"
complete -c synapse -n "__fish_seen_subcommand_from install" -l no-use -d "Do not switch to this version after installing"
complete -c synapse -n "__fish_seen_subcommand_from list-remote" -s l -l limit -d "Maximum number of versions to display"

complete -c synapse -n "__fish_seen_subcommand_from globals; and not __fish_seen_subcommand_from $__synapse_globals_runtimes" -a "$__synapse_globals_runtimes"
complete -c synapse -n "__fish_seen_subcommand_from globals; and __fish_seen_subcommand_from $__synapse_globals_runtimes; and not __fish_seen_subcommand_from $__synapse_globals_actions" -a "$__synapse_globals_actions"

complete -c synapse -n "__fish_seen_subcommand_from service; and not __fish_seen_subcommand_from $__synapse_service_actions" -a "$__synapse_service_actions"
complete -c synapse -n "__fish_seen_subcommand_from service; and __fish_seen_subcommand_from $__synapse_service_actions; and not __fish_seen_subcommand_from $__synapse_services" -a "$__synapse_services"
complete -c synapse -n "__fish_seen_subcommand_from service; and __fish_seen_subcommand_from logs" -s n -l lines -d "Number of trailing lines to display"
complete -c synapse -n "__fish_seen_subcommand_from service; and __fish_seen_subcommand_from logs" -s f -l follow -d "Follow log output in real time"

complete -c synapse -n "__fish_seen_subcommand_from completions; and not __fish_seen_subcommand_from $__synapse_shells" -a "$__synapse_shells"
"#;

const POWERSHELL_COMPLETIONS: &str = r#"Register-ArgumentCompleter -Native -CommandName 'synapse' -ScriptBlock {
    param($wordToComplete, $commandAst, $cursorPosition)

    $tokens = @($commandAst.CommandElements | ForEach-Object { $_.Extent.Text })
    $commands = @('install', 'use', 'pin', 'list', 'list-remote', 'current', 'uninstall', 'globals', 'service', 'env', 'setup', 'completions')
    $runtimes = @('node', 'bun', 'php', 'composer', 'postgres', 'nginx')
    $globalsRuntimes = @('node', 'bun', 'composer')
    $globalsActions = @('list', 'add', 'remove', 'sync', 'migrate')
    $serviceActions = @('start', 'stop', 'restart', 'status', 'logs')
    $services = @('postgres', 'nginx')
    $shells = @('bash', 'zsh', 'fish', 'powershell')

    $isCompletingNew = [string]::IsNullOrEmpty($wordToComplete)
    $pos = if ($isCompletingNew) { $tokens.Count } else { $tokens.Count - 1 }

    $candidates = @()
    if ($pos -eq 1) {
        $candidates = $commands
    } elseif ($tokens.Count -ge 2) {
        switch ($tokens[1]) {
            'install' {
                if ($wordToComplete -like '-*') { $candidates = @('--no-use', '--help') }
                elseif ($pos -eq 2) { $candidates = $runtimes }
            }
            { $_ -in @('use', 'pin', 'list', 'current', 'uninstall') } {
                if ($pos -eq 2) { $candidates = $runtimes }
            }
            'list-remote' {
                if ($wordToComplete -like '-*') { $candidates = @('--limit', '-l', '--help') }
                elseif ($pos -eq 2) { $candidates = $runtimes }
            }
            'globals' {
                if ($pos -eq 2) { $candidates = $globalsRuntimes }
                elseif ($pos -eq 3) { $candidates = $globalsActions }
            }
            'service' {
                if ($pos -eq 2) { $candidates = $serviceActions }
                elseif ($tokens.Count -ge 3 -and $tokens[2] -eq 'logs' -and $wordToComplete -like '-*') {
                    $candidates = @('--lines', '-n', '--follow', '-f', '--help')
                } elseif ($pos -eq 3) {
                    $candidates = $services
                }
            }
            'completions' {
                if ($pos -eq 2) { $candidates = $shells }
            }
        }
    }

    $candidates | Where-Object { $_ -like "$wordToComplete*" } | ForEach-Object {
        [System.Management.Automation.CompletionResult]::new($_, $_, 'ParameterValue', $_)
    }
}
"#;

fn is_in_path(bin_dir: &std::path::Path) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|entry| entry == bin_dir))
        .unwrap_or(false)
}

/// Atomically copies `src_exe` into `bin_dir` as `synapse` (or `synapse.exe` on Windows).
pub fn install_self_to_dir(
    src_exe: &std::path::Path,
    bin_dir: &std::path::Path,
) -> Result<std::path::PathBuf> {
    fs::create_dir_all(bin_dir)?;

    let bin_name = if cfg!(windows) {
        "synapse.exe"
    } else {
        "synapse"
    };
    let dest = bin_dir.join(bin_name);

    if let (Ok(src_canon), Ok(dest_canon)) = (src_exe.canonicalize(), dest.canonicalize())
        && src_canon == dest_canon
    {
        return Ok(dest);
    }

    let temp_path = bin_dir.join(format!(".tmp_{}_{}", bin_name, std::process::id()));
    if temp_path.exists() {
        let _ = fs::remove_file(&temp_path);
    }

    fs::copy(src_exe, &temp_path)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(err) = fs::set_permissions(&temp_path, fs::Permissions::from_mode(0o755)) {
            let _ = fs::remove_file(&temp_path);
            return Err(SynapseError::Io(err));
        }
    }

    #[cfg(windows)]
    if dest.exists() {
        let _ = fs::remove_file(&dest);
    }

    if let Err(err) = fs::rename(&temp_path, &dest) {
        let _ = fs::remove_file(&temp_path);
        return Err(SynapseError::Io(err));
    }

    Ok(dest)
}

fn handle_setup() -> Result<()> {
    paths::ensure_dirs()?;
    let current_exe = std::env::current_exe()?;
    let bin_dir = paths::bin_dir()?;
    let dest = install_self_to_dir(&current_exe, &bin_dir)?;
    crate::core::switcher::refresh_shims_if_needed(&bin_dir)?;
    println!(
        "{} Installed synapse binary to {}",
        "✓".green().bold(),
        dest.display()
    );

    if is_in_path(&bin_dir) {
        println!(
            "{} {} is already in your PATH",
            "✓".green().bold(),
            bin_dir.display()
        );
    } else {
        ensure_shell_path();
    }

    Ok(())
}

/// Automatically ensures `~/.synapse/bin` is on the user's shell PATH.
fn ensure_shell_path() {
    let Ok(bin_dir) = paths::bin_dir() else {
        return;
    };

    let self_name = if cfg!(windows) {
        "synapse.exe"
    } else {
        "synapse"
    };
    if !bin_dir.join(self_name).exists()
        && let Ok(exe) = std::env::current_exe()
        && exe.file_stem() == Some(std::ffi::OsStr::new("synapse"))
        && install_self_to_dir(&exe, &bin_dir).is_ok()
    {
        let _ = crate::core::switcher::refresh_shims_if_needed(&bin_dir);
    }

    if is_in_path(&bin_dir) {
        return;
    }

    if cfg!(windows) {
        let status = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                "$p = [Environment]::GetEnvironmentVariable('Path', 'User'); if (-not ($p -split ';' | Where-Object { $_ -eq $args[0] })) { $newP = if ($p) { \"$($args[0]);$p\" } else { $args[0] }; [Environment]::SetEnvironmentVariable('Path', $newP, 'User') }",
            ])
            .arg(&bin_dir)
            .status();
        if matches!(status, Ok(s) if s.success()) {
            println!(
                "{} Added {} to Windows User PATH (restart terminal to take effect)",
                "✓".green().bold(),
                bin_dir.display()
            );
            return;
        }
    }

    let shell = std::env::var("SHELL").unwrap_or_default();
    if shell.ends_with("/fish") || shell == "fish" {
        let status = std::process::Command::new("fish")
            .args(["-c", "fish_add_path -U -p -- $argv[1]", "--"])
            .arg(&bin_dir)
            .status();
        if matches!(status, Ok(s) if s.success()) {
            println!(
                "{} Added {} to Fish universal PATH",
                "✓".green().bold(),
                bin_dir.display()
            );
            return;
        }
    }

    let rc_name = if shell.ends_with("/zsh") || shell == "zsh" {
        Some(".zshrc")
    } else if shell.ends_with("/bash") || shell == "bash" {
        Some(".bashrc")
    } else {
        None
    };

    if let Some(rc) = rc_name
        && let Some(home) = dirs::home_dir()
    {
        let rc_path = home.join(rc);
        let export_line = r#"export PATH="$HOME/.synapse/bin:$PATH""#;
        let existing = fs::read_to_string(&rc_path).unwrap_or_default();
        if !existing.contains(".synapse/bin") {
            use std::io::Write;
            if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&rc_path)
                && writeln!(f, "\n# Synapse\n{}", export_line).is_ok()
            {
                println!(
                    "{} Added {} to ~/{} (run 'source ~/{}' for current terminal)",
                    "✓".green().bold(),
                    bin_dir.display(),
                    rc,
                    rc
                );
                return;
            }
        }
    }

    println!(
        "{} Add {} to your PATH to use installed runtimes directly",
        "!".yellow().bold(),
        bin_dir.display()
    );
}

fn get_installed_versions(runtime: &str) -> Result<Vec<String>> {
    crate::core::switcher::list_installed_versions(runtime)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_install_self_to_dir() {
        let temp_root =
            std::env::temp_dir().join(format!("synapse_self_install_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_root);
        fs::create_dir_all(&temp_root).unwrap();

        let src_file = temp_root.join("synapse-src");
        let bin_dir = temp_root.join("bin");
        fs::write(&src_file, b"fake synapse binary").unwrap();

        let dest = install_self_to_dir(&src_file, &bin_dir).unwrap();
        let expected_name = if cfg!(windows) { "synapse.exe" } else { "synapse" };
        assert_eq!(dest, bin_dir.join(expected_name));
        assert!(dest.is_file());
        assert_eq!(fs::read(&dest).unwrap(), b"fake synapse binary");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&dest).unwrap().permissions().mode();
            assert_ne!(mode & 0o111, 0);
        }

        // Idempotent when called with dest itself as source
        let dest_again = install_self_to_dir(&dest, &bin_dir).unwrap();
        assert_eq!(dest_again, dest);
        assert_eq!(fs::read(&dest_again).unwrap(), b"fake synapse binary");

        let _ = fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn test_generate_completions_for_supported_shells() {
        let bash = generate_completions("bash").unwrap();
        assert!(bash.contains("complete -F _synapse_completions synapse"));
        assert!(bash.contains("completions"));

        let zsh = generate_completions("ZSH").unwrap();
        assert!(zsh.contains("#compdef synapse"));
        assert!(zsh.contains("_synapse"));

        let fish = generate_completions("fish").unwrap();
        assert!(fish.contains("complete -c synapse"));

        let ps = generate_completions("powershell").unwrap();
        assert!(ps.contains("Register-ArgumentCompleter"));
        assert_eq!(generate_completions("pwsh").unwrap(), ps);

        let err = generate_completions("tcsh").unwrap_err();
        assert!(matches!(err, SynapseError::Io(_)));
    }

    #[test]
    fn test_resolve_pinned_targets() {
        let temp_root =
            std::env::temp_dir().join(format!("synapse_pinned_targets_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_root);
        fs::create_dir_all(&temp_root).unwrap();

        // Empty dir -> PinNotFound
        let err = resolve_pinned_targets(&temp_root, None).unwrap_err();
        assert!(matches!(err, SynapseError::PinNotFound(_)));

        // Unsupported runtime -> UnsupportedRuntime
        let rt_err = resolve_pinned_targets(&temp_root, Some("ruby")).unwrap_err();
        assert!(matches!(rt_err, SynapseError::UnsupportedRuntime(_)));

        // .synapse-version with multiple runtimes
        let synapse_file = temp_root.join(".synapse-version");
        fs::write(&synapse_file, "node = \"22.11.0\"\nbun = \"1.4.2\"\n").unwrap();

        let single = resolve_pinned_targets(&temp_root, Some("node")).unwrap();
        assert_eq!(
            single,
            vec![("node".to_string(), "22.11.0".to_string(), synapse_file.clone())]
        );

        let all = resolve_pinned_targets(&temp_root, None).unwrap();
        assert_eq!(
            all,
            vec![
                ("node".to_string(), "22.11.0".to_string(), synapse_file.clone()),
                ("bun".to_string(), "1.4.2".to_string(), synapse_file),
            ]
        );

        let _ = fs::remove_dir_all(&temp_root);
    }
}
