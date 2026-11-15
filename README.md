# Synapse

Universal Runtime Version Manager written in Rust for **Node.js** (with bundled `npm`/`npx`), **Bun** (`bun`/`bunx`), **PHP**, **Composer**, **PostgreSQL**, and **Nginx**, targeting **Linux** and **Windows** (`x86_64` and `aarch64`).

## Features

- **Unified CLI** for managing `node`, `bun`, `php`, `composer`, `postgres`, and `nginx` from a single binary.
- **Managed Local Services** (`synapse service`) with automatic `PGDATA` cluster initialization (`initdb` + `pg_ctl`) for PostgreSQL and standalone user-space configuration for Nginx (`nginx.conf` on port `8080`).
- **Native Switching** via atomic POSIX symlinks on Linux and directory junctions + `.cmd` shims on Windows.
- **Per-Project Pinning** supporting `.node-version`, `.nvmrc`, `.bun-version`, `.php-version`, `.composer-version`, `.postgres-version`, `.nginx-version`, and multi-runtime `.synapse-version`.
- **Global Packages Management** (`synapse globals`) with manifest tracking (`packages.txt`), automatic discovery, cross-version migration, and automatic Node global binary shimming.
- **Security Hardening** with fail-closed SHA-256 checksum verification (`node`, `bun`, `composer`, `postgres`), path traversal validation, zip-slip / tar symlink escape protection, and bounded download buffers.

## Build & Setup

```bash
cargo build --release
./target/release/synapse setup
```

`synapse setup` installs the `synapse` binary into `~/.synapse/bin` (`%USERPROFILE%\.synapse\bin` on Windows) and configures your shell `PATH`:

- **Fish**: automatically updates universal `fish_user_paths` via `fish_add_path -U -p`.
- **Bash / Zsh**: appends `export PATH="$HOME/.synapse/bin:$PATH"` to `~/.bashrc` or `~/.zshrc`.
- **Windows PowerShell**: prepends `%USERPROFILE%\.synapse\bin` to the User `Path` environment variable.

You can also print the shell `PATH` command manually at any time:

```bash
synapse env
```

## CLI Reference

### Browse & Install Runtimes

```bash
# List all available remote versions (including Node.js LTS & Nginx Stable channels)
synapse list-remote node
synapse list-remote bun --limit 15
synapse list-remote php
synapse list-remote composer
synapse list-remote postgres
synapse list-remote nginx

# Install and activate a runtime version
synapse install node 22
synapse install node lts
synapse install bun latest
synapse install php 8.4
synapse install composer latest
synapse install postgres 17
synapse install nginx stable

# Install without changing the active global version
synapse install node 20 --no-use
```

### Switch, List & Uninstall

```bash
# Switch active version explicitly
synapse use node 22.11.0
synapse use bun 1.4.2

# List locally installed versions
synapse list node

# Show currently active version(s)
synapse current
synapse current node

# Remove an installed version
synapse uninstall node 20.18.0
```

### Per-Project Version Pinning

```bash
# Create a version pin file in the current directory
synapse pin node 22
synapse pin bun latest

# Switch to the pinned version detected in current or parent directories
synapse use node
synapse use
```

Supported pin files: `.synapse-version` (TOML or `runtime = version` lines), `.node-version`, `.nvmrc`, `.bun-version`, `.php-version`, `.composer-version`, `.postgres-version`, and `.nginx-version`.

### Managed Services (PostgreSQL & Nginx)

```bash
# Start PostgreSQL (auto-runs initdb on first start for active version) or Nginx (port 8080)
synapse service start postgres
synapse service start nginx

# Check status, restart, view logs, or stop services
synapse service status
synapse service logs postgres
synapse service logs nginx -n 100 -f
synapse service restart nginx
synapse service stop postgres
```

- PostgreSQL cluster data & logs are stored per version at `~/.synapse/data/postgres/<version>/data` and `postgres.log` (default superuser: `postgres`, port `5432`).
- Nginx runtime config & web root are bootstrapped at `~/.synapse/data/nginx/` (`conf/nginx.conf`, `html/index.html`, `logs/`).

### Global Packages

```bash
# List tracked and installed global packages
synapse globals node list

# Add package(s) to manifest and install into active runtime
synapse globals node add pnpm typescript

# Remove package(s) from manifest
synapse globals node remove typescript

# Sync/reinstall tracked packages after switching versions
synapse globals node sync

# Migrate global packages from an older installed version
synapse globals node migrate 20 22
```

Supported runtimes for `globals`: `node`, `bun`, and `composer`.

## Directory Layout

```text
~/.synapse/
├── bin/          # Active binary symlinks (Linux) / .cmd shims (Windows) + synapse
├── current/      # Links to the currently active runtime version directories
├── runtimes/     # Installed runtime versions (<runtime>/<version>/)
├── globals/      # Tracked global package manifests (<runtime>/packages.txt)
└── data/         # Managed service data/configs (postgres/<version>/data, nginx/)
```

## Security & Integrity

- **Version Validation**: Version identifiers used in paths and pin files are restricted to `[a-zA-Z0-9._-]` (rejecting `.`, `..`, and path separators).
- **Archive Extraction**: `.tar.xz`, `.tar.gz`, and `.zip` extractors reject traversal paths and symlink/hardlink targets that escape the target directory.
- **Checksums**: Node.js (`SHASUMS256.txt`), Bun (GitHub asset digest / `SHASUMS256.txt`), Composer (asset digest / `composer.phar.sha256sum`), and PostgreSQL (`theseus-rs/postgresql-binaries` asset digest / `.sha256`) fail closed if SHA-256 verification fails or is missing. *(PHP and Nginx downloads rely on HTTPS TLS from `dl.static-php.dev`, `windows.php.net`, `jirutka.github.io`, and `nginx.org`.)*
- **Shell Safety**: `PATH` updates pass paths via positional arguments (`$argv[1]` / `$args[0]`) rather than interpolating `$HOME` into shell strings.
