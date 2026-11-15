//! Local background service manager for Synapse-managed daemons (`postgres`, `nginx`).
//!
//! Bootstraps user-space data and configuration directories under `~/.synapse/data/`
//! and delegates lifecycle actions (`start`, `stop`, `restart`, `status`) to the
//! currently active runtime binaries (`pg_ctl`/`initdb` for PostgreSQL, `nginx` for Nginx).

use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::core::switcher;
use crate::error::{Result, SynapseError};
use crate::platform::paths;

pub const SUPPORTED_SERVICES: &[&str] = &["postgres", "nginx"];
const DEFAULT_NGINX_PORT: u16 = 8080;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceState {
    Running { pid: Option<u32>, detail: String },
    Stopped { detail: String },
}

/// Returns `(pgdata_dir, log_file)` under `<base_data_dir>/postgres/<version>/`.
pub fn postgres_data_layout(base_data_dir: &Path, version: &str) -> Result<(PathBuf, PathBuf)> {
    paths::validate_version_str(version)?;
    let ver_dir = base_data_dir.join("postgres").join(version);
    Ok((ver_dir.join("data"), ver_dir.join("postgres.log")))
}

/// Bootstraps `<base_data_dir>/nginx/` with `conf/`, `logs/`, `html/`, `temp/` directories
/// and default `nginx.conf`, `mime.types`, and `index.html` if missing. Returns the prefix path.
pub fn ensure_nginx_layout(base_data_dir: &Path, port: u16) -> Result<PathBuf> {
    let prefix_dir = base_data_dir.join("nginx");
    let conf_dir = prefix_dir.join("conf");
    let logs_dir = prefix_dir.join("logs");
    let html_dir = prefix_dir.join("html");
    let temp_dir = prefix_dir.join("temp");

    fs::create_dir_all(&conf_dir)?;
    fs::create_dir_all(&logs_dir)?;
    fs::create_dir_all(&html_dir)?;
    for sub in [
        "client_body_temp",
        "proxy_temp",
        "fastcgi_temp",
        "uwsgi_temp",
        "scgi_temp",
    ] {
        fs::create_dir_all(temp_dir.join(sub))?;
    }

    let mime_path = conf_dir.join("mime.types");
    if !mime_path.exists() {
        let mime_content = r#"types {
    text/html                             html htm shtml;
    text/css                              css;
    text/xml                              xml;
    text/plain                            txt;
    application/javascript                js mjs;
    application/json                      json;
    image/gif                             gif;
    image/jpeg                            jpeg jpg;
    image/png                             png;
    image/svg+xml                         svg svgz;
    image/webp                            webp;
    image/x-icon                          ico;
    application/wasm                      wasm;
    application/octet-stream              bin exe dll;
}
"#;
        fs::write(&mime_path, mime_content)?;
    }

    let conf_path = conf_dir.join("nginx.conf");
    if !conf_path.exists() {
        let conf_content = format!(
            r#"worker_processes 1;
pid logs/nginx.pid;
error_log logs/error.log;

events {{
    worker_connections 1024;
}}

http {{
    include mime.types;
    default_type application/octet-stream;
    access_log logs/access.log;

    client_body_temp_path temp/client_body_temp;
    proxy_temp_path temp/proxy_temp;
    fastcgi_temp_path temp/fastcgi_temp;
    uwsgi_temp_path temp/uwsgi_temp;
    scgi_temp_path temp/scgi_temp;

    sendfile on;
    keepalive_timeout 65;

    server {{
        listen 127.0.0.1:{port};
        server_name localhost;

        location / {{
            root html;
            index index.html index.htm;
        }}
    }}
}}
"#,
            port = port
        );
        fs::write(&conf_path, conf_content)?;
    }

    let index_path = html_dir.join("index.html");
    if !index_path.exists() {
        let index_html = r#"<!DOCTYPE html>
<html>
<head><title>Nginx managed by Synapse</title></head>
<body>
<h1>Nginx is running</h1>
<p>Served from ~/.synapse/data/nginx/html</p>
</body>
</html>
"#;
        fs::write(&index_path, index_html)?;
    }

    Ok(prefix_dir)
}

/// Reads the leading numeric PID from a `.pid` file if present and valid.
pub fn read_pid_file(pid_path: &Path) -> Option<u32> {
    let content = fs::read_to_string(pid_path).ok()?;
    let first_line = content.lines().next()?.trim();
    first_line.parse::<u32>().ok()
}

/// Checks whether a process with `pid` is currently alive.
pub fn is_pid_running(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let proc_dir = Path::new("/proc");
        if proc_dir.is_dir() {
            return proc_dir.join(pid.to_string()).exists();
        }
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    #[cfg(windows)]
    {
        let filter = format!("PID eq {}", pid);
        Command::new("tasklist")
            .args(["/FI", &filter, "/NH"])
            .output()
            .map(|out| {
                out.status.success()
                    && String::from_utf8_lossy(&out.stdout).contains(&pid.to_string())
            })
            .unwrap_or(false)
    }
}

fn require_active_version(runtime: &str) -> Result<String> {
    switcher::get_current_version(runtime)?.ok_or_else(|| {
        SynapseError::NotInstalled(
            runtime.to_string(),
            format!(
                "no active version (run 'synapse use {} <version>')",
                runtime
            ),
        )
    })
}

fn postgres_bin(version: &str, name: &str) -> Result<PathBuf> {
    let exe_name = if cfg!(windows) {
        format!("{}.exe", name)
    } else {
        name.to_string()
    };
    let bin_path = paths::runtime_version_dir("postgres", version)?
        .join("bin")
        .join(exe_name);
    if !bin_path.is_file() {
        return Err(SynapseError::NotInstalled(
            "postgres".to_string(),
            version.to_string(),
        ));
    }
    Ok(bin_path)
}

fn nginx_bin_path(version: &str) -> Result<PathBuf> {
    let exe_name = if cfg!(windows) { "nginx.exe" } else { "nginx" };
    let bin_path = paths::runtime_version_dir("nginx", version)?.join(exe_name);
    if !bin_path.is_file() {
        return Err(SynapseError::NotInstalled(
            "nginx".to_string(),
            version.to_string(),
        ));
    }
    Ok(bin_path)
}

fn nginx_prefix_arg(prefix_dir: &Path) -> String {
    let s = prefix_dir.display().to_string();
    if s.ends_with(std::path::MAIN_SEPARATOR) {
        s
    } else {
        format!("{}{}", s, std::path::MAIN_SEPARATOR)
    }
}

fn command_error(context: &str, out: &std::process::Output) -> SynapseError {
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let detail = if !stderr.is_empty() { stderr } else { stdout };
    SynapseError::Io(std::io::Error::other(format!("{}: {}", context, detail)))
}

fn status_postgres() -> Result<ServiceState> {
    let Some(version) = switcher::get_current_version("postgres")? else {
        return Ok(ServiceState::Stopped {
            detail: "no active postgres version selected".to_string(),
        });
    };

    let (db_dir, _) = postgres_data_layout(&paths::data_dir()?, &version)?;
    if !db_dir.join("PG_VERSION").is_file() {
        return Ok(ServiceState::Stopped {
            detail: format!("v{} (data dir not initialized: {})", version, db_dir.display()),
        });
    }

    let pg_ctl = postgres_bin(&version, "pg_ctl")?;
    let out = Command::new(&pg_ctl)
        .arg("-D")
        .arg(&db_dir)
        .arg("status")
        .output()?;

    if out.status.success() {
        let pid = read_pid_file(&db_dir.join("postmaster.pid"));
        Ok(ServiceState::Running {
            pid,
            detail: format!("v{} (data: {})", version, db_dir.display()),
        })
    } else {
        Ok(ServiceState::Stopped {
            detail: format!("v{} (data: {})", version, db_dir.display()),
        })
    }
}

fn start_postgres() -> Result<String> {
    let version = require_active_version("postgres")?;
    let (db_dir, log_file) = postgres_data_layout(&paths::data_dir()?, &version)?;
    let initdb = postgres_bin(&version, "initdb")?;
    let pg_ctl = postgres_bin(&version, "pg_ctl")?;

    if let ServiceState::Running { pid, .. } = status_postgres()? {
        let pid_info = pid.map(|p| format!(" (PID {})", p)).unwrap_or_default();
        return Ok(format!(
            "PostgreSQL v{} is already running{} (data: {})",
            version,
            pid_info,
            db_dir.display()
        ));
    }

    if !db_dir.join("PG_VERSION").is_file() {
        if let Some(parent) = db_dir.parent() {
            fs::create_dir_all(parent)?;
        }
        let init_out = Command::new(&initdb)
            .arg("-D")
            .arg(&db_dir)
            .args(["-U", "postgres", "-A", "trust", "--encoding=UTF8", "--no-instructions"])
            .output()?;
        if !init_out.status.success() {
            return Err(command_error("PostgreSQL initdb failed", &init_out));
        }
    }

    let start_out = Command::new(&pg_ctl)
        .arg("-D")
        .arg(&db_dir)
        .arg("-l")
        .arg(&log_file)
        .arg("start")
        .output()?;

    if !start_out.status.success() {
        return Err(command_error("PostgreSQL start failed", &start_out));
    }

    Ok(format!(
        "Started PostgreSQL v{} (user: postgres, port: 5432, data: {}, log: {})",
        version,
        db_dir.display(),
        log_file.display()
    ))
}

fn stop_postgres() -> Result<String> {
    let version = require_active_version("postgres")?;
    let (db_dir, _) = postgres_data_layout(&paths::data_dir()?, &version)?;
    if !db_dir.join("PG_VERSION").is_file() {
        return Ok(format!(
            "PostgreSQL v{} is not running (cluster not initialized)",
            version
        ));
    }

    if matches!(status_postgres()?, ServiceState::Stopped { .. }) {
        return Ok(format!("PostgreSQL v{} is already stopped", version));
    }

    let pg_ctl = postgres_bin(&version, "pg_ctl")?;
    let out = Command::new(&pg_ctl)
        .arg("-D")
        .arg(&db_dir)
        .args(["-m", "fast", "stop"])
        .output()?;

    if !out.status.success() {
        return Err(command_error("PostgreSQL stop failed", &out));
    }

    Ok(format!("Stopped PostgreSQL v{}", version))
}

fn status_nginx() -> Result<ServiceState> {
    let Some(version) = switcher::get_current_version("nginx")? else {
        return Ok(ServiceState::Stopped {
            detail: "no active nginx version selected".to_string(),
        });
    };

    let prefix_dir = paths::service_data_dir("nginx")?;
    let pid_path = prefix_dir.join("logs").join("nginx.pid");

    if let Some(pid) = read_pid_file(&pid_path)
        && is_pid_running(pid)
    {
        return Ok(ServiceState::Running {
            pid: Some(pid),
            detail: format!("v{} (prefix: {})", version, prefix_dir.display()),
        });
    }

    Ok(ServiceState::Stopped {
        detail: format!("v{} (prefix: {})", version, prefix_dir.display()),
    })
}

fn start_nginx() -> Result<String> {
    let version = require_active_version("nginx")?;
    let nginx_bin = nginx_bin_path(&version)?;
    let prefix_dir = ensure_nginx_layout(&paths::data_dir()?, DEFAULT_NGINX_PORT)?;
    let pid_path = prefix_dir.join("logs").join("nginx.pid");

    if let Some(pid) = read_pid_file(&pid_path)
        && is_pid_running(pid)
    {
        return Ok(format!(
            "Nginx v{} is already running (PID {}, prefix: {})",
            version,
            pid,
            prefix_dir.display()
        ));
    }

    let prefix_arg = nginx_prefix_arg(&prefix_dir);

    #[cfg(unix)]
    {
        let out = Command::new(&nginx_bin)
            .args(["-p", &prefix_arg, "-c", "conf/nginx.conf"])
            .output()?;
        if !out.status.success() {
            return Err(command_error("Nginx start failed", &out));
        }
    }

    #[cfg(windows)]
    {
        let test_out = Command::new(&nginx_bin)
            .args(["-t", "-p", &prefix_arg, "-c", "conf/nginx.conf"])
            .output()?;
        if !test_out.status.success() {
            return Err(command_error("Nginx configuration test failed", &test_out));
        }

        Command::new(&nginx_bin)
            .args(["-p", &prefix_arg, "-c", "conf/nginx.conf"])
            .spawn()?;

        for _ in 0..10 {
            if let Some(pid) = read_pid_file(&pid_path)
                && is_pid_running(pid)
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }

    Ok(format!(
        "Started Nginx v{} (http://127.0.0.1:{}, prefix: {})",
        version,
        DEFAULT_NGINX_PORT,
        prefix_dir.display()
    ))
}

fn stop_nginx() -> Result<String> {
    let version = require_active_version("nginx")?;
    let nginx_bin = nginx_bin_path(&version)?;
    let prefix_dir = paths::service_data_dir("nginx")?;
    let pid_path = prefix_dir.join("logs").join("nginx.pid");

    let running_pid = read_pid_file(&pid_path).filter(|pid| is_pid_running(*pid));
    if running_pid.is_none() {
        let _ = fs::remove_file(&pid_path);
        return Ok(format!("Nginx v{} is already stopped", version));
    }

    let prefix_arg = nginx_prefix_arg(&prefix_dir);
    let out = Command::new(&nginx_bin)
        .args(["-p", &prefix_arg, "-c", "conf/nginx.conf", "-s", "stop"])
        .output()?;

    if !out.status.success() {
        return Err(command_error("Nginx stop failed", &out));
    }

    if pid_path.exists() {
        let _ = fs::remove_file(&pid_path);
    }

    Ok(format!("Stopped Nginx v{}", version))
}

/// Validates service name and starts the requested service (`"postgres"` or `"nginx"`).
pub fn start_service(service: &str) -> Result<String> {
    paths::ensure_dirs()?;
    match service {
        "postgres" => start_postgres(),
        "nginx" => start_nginx(),
        _ => Err(SynapseError::UnsupportedRuntime(service.to_string())),
    }
}

/// Validates service name and stops the requested service (`"postgres"` or `"nginx"`).
pub fn stop_service(service: &str) -> Result<String> {
    paths::ensure_dirs()?;
    match service {
        "postgres" => stop_postgres(),
        "nginx" => stop_nginx(),
        _ => Err(SynapseError::UnsupportedRuntime(service.to_string())),
    }
}

/// Stops (if running) and starts the requested service (`"postgres"` or `"nginx"`).
pub fn restart_service(service: &str) -> Result<String> {
    let _ = stop_service(service)?;
    start_service(service)
}

/// Returns current running/stopped state for the requested service (`"postgres"` or `"nginx"`).
pub fn status_service(service: &str) -> Result<ServiceState> {
    paths::ensure_dirs()?;
    match service {
        "postgres" => status_postgres(),
        "nginx" => status_nginx(),
        _ => Err(SynapseError::UnsupportedRuntime(service.to_string())),
    }
}

/// Reads the trailing `max_lines` from a log file, returning `Ok(None)` if the file does not exist.
pub fn tail_file_lines(path: &Path, max_lines: usize) -> Result<Option<String>> {
    if !path.is_file() {
        return Ok(None);
    }
    let bytes = fs::read(path)?;
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();
    if max_lines == 0 || lines.is_empty() {
        return Ok(Some(String::new()));
    }
    let start = lines.len().saturating_sub(max_lines);
    Ok(Some(lines[start..].join("\n")))
}

/// Resolves `(label, log_file_path)` pairs for the requested service (`"postgres"` or `"nginx"`).
pub fn service_log_paths(service: &str) -> Result<Vec<(String, PathBuf)>> {
    paths::ensure_dirs()?;
    match service {
        "postgres" => {
            let version = require_active_version("postgres")?;
            let (_, log_file) = postgres_data_layout(&paths::data_dir()?, &version)?;
            Ok(vec![(
                format!("postgres v{} (postgres.log)", version),
                log_file,
            )])
        }
        "nginx" => {
            let version = require_active_version("nginx")?;
            let logs_dir = paths::service_data_dir("nginx")?.join("logs");
            Ok(vec![
                (
                    format!("nginx v{} (error.log)", version),
                    logs_dir.join("error.log"),
                ),
                (
                    format!("nginx v{} (access.log)", version),
                    logs_dir.join("access.log"),
                ),
            ])
        }
        _ => Err(SynapseError::UnsupportedRuntime(service.to_string())),
    }
}

/// Continuously follows log files in `targets`, printing appended content until interrupted.
pub fn follow_log_files(targets: &[(String, PathBuf)]) -> Result<()> {
    let mut offsets: Vec<u64> = targets
        .iter()
        .map(|(_, path)| fs::metadata(path).map(|m| m.len()).unwrap_or(0))
        .collect();
    let multi = targets.len() > 1;

    loop {
        std::thread::sleep(std::time::Duration::from_millis(250));
        for ((label, path), offset) in targets.iter().zip(offsets.iter_mut()) {
            if !path.is_file() {
                continue;
            }
            let len = fs::metadata(path)?.len();
            if len < *offset {
                *offset = 0;
            }
            if len > *offset {
                let mut file = fs::File::open(path)?;
                file.seek(SeekFrom::Start(*offset))?;
                let mut buf = Vec::new();
                file.read_to_end(&mut buf)?;
                *offset = len;

                let chunk = String::from_utf8_lossy(&buf);
                if multi {
                    for line in chunk.lines() {
                        println!("[{}] {}", label, line);
                    }
                } else {
                    print!("{}", chunk);
                }
                let _ = std::io::stdout().flush();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ensure_nginx_layout_creates_config_and_dirs() {
        let base = std::env::temp_dir().join(format!("synapse_nginx_layout_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();

        let prefix = ensure_nginx_layout(&base, 8080).unwrap();
        assert_eq!(prefix, base.join("nginx"));
        assert!(prefix.join("conf").join("nginx.conf").is_file());
        assert!(prefix.join("conf").join("mime.types").is_file());
        assert!(prefix.join("html").join("index.html").is_file());
        assert!(prefix.join("logs").is_dir());
        assert!(prefix.join("temp").join("client_body_temp").is_dir());
        assert!(prefix.join("temp").join("proxy_temp").is_dir());
        assert!(prefix.join("temp").join("fastcgi_temp").is_dir());
        assert!(prefix.join("temp").join("uwsgi_temp").is_dir());
        assert!(prefix.join("temp").join("scgi_temp").is_dir());

        let conf = fs::read_to_string(prefix.join("conf").join("nginx.conf")).unwrap();
        assert!(conf.contains("listen 127.0.0.1:8080;"));
        assert!(conf.contains("pid logs/nginx.pid;"));

        // Existing custom nginx.conf is preserved on subsequent calls
        fs::write(prefix.join("conf").join("nginx.conf"), "# custom config\n").unwrap();
        ensure_nginx_layout(&base, 9090).unwrap();
        assert_eq!(
            fs::read_to_string(prefix.join("conf").join("nginx.conf")).unwrap(),
            "# custom config\n"
        );

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn test_postgres_data_layout_validation() {
        let base = Path::new("/tmp/synapse_pg_test");
        let (data_dir, log_file) = postgres_data_layout(base, "17.2.0").unwrap();
        assert_eq!(data_dir, base.join("postgres").join("17.2.0").join("data"));
        assert_eq!(
            log_file,
            base.join("postgres").join("17.2.0").join("postgres.log")
        );

        assert!(matches!(
            postgres_data_layout(base, "../evil"),
            Err(SynapseError::InvalidVersion(_))
        ));
    }

    #[test]
    fn test_read_pid_file() {
        let base = std::env::temp_dir().join(format!("synapse_pid_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();

        let pid_file = base.join("service.pid");
        fs::write(&pid_file, "42190\n/tmp/data\n").unwrap();
        assert_eq!(read_pid_file(&pid_file), Some(42190));

        fs::write(&pid_file, "not-a-pid\n").unwrap();
        assert_eq!(read_pid_file(&pid_file), None);

        assert_eq!(read_pid_file(&base.join("missing.pid")), None);

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn test_tail_file_lines() {
        let base = std::env::temp_dir().join(format!("synapse_tail_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();

        let missing = base.join("missing.log");
        assert_eq!(tail_file_lines(&missing, 50).unwrap(), None);

        let log_file = base.join("service.log");
        fs::write(&log_file, "line 1\nline 2\nline 3\nline 4\nline 5\nline 6\n").unwrap();
        assert_eq!(
            tail_file_lines(&log_file, 3).unwrap(),
            Some("line 4\nline 5\nline 6".to_string())
        );

        let short_log = base.join("short.log");
        fs::write(&short_log, "alpha\nbeta\n").unwrap();
        assert_eq!(
            tail_file_lines(&short_log, 10).unwrap(),
            Some("alpha\nbeta".to_string())
        );

        let _ = fs::remove_dir_all(&base);
    }
}
