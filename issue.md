# Synapse — Universal Version Manager

> Version manager untuk **Bun.js** dan **Node.js** (bundled dengan **npm/npx**), fokus platform **Linux**, ditulis dalam **Rust**. *(Dukungan PHP ditunda ke fase berikutnya)*.

---

## 1. Latar Belakang & Motivasi

Developer modern sering bekerja lintas project dengan stack JavaScript/TypeScript yang memerlukan versi Node.js maupun Bun yang berbeda-beda. Version manager yang ada sering kali terfragmentasi (nvm/fnm khusus Node, tooling terpisah untuk Bun), dan sering mengabaikan persistensi global package saat berpindah versi.

**Synapse** menyatukan manajemen versi **Node.js (bersama npm/npx)** dan **Bun.js** dalam satu CLI yang kencang, konsisten, dan ringan di Linux — dibangun dengan Rust untuk performa instan tanpa overhead shell script initialization.

---

## 2. Goals & Scope

| # | Goal | Status / Scope |
|---|------|----------------|
| G1 | **Single CLI** | Satu binary (`synapse`) untuk install, switch, dan manage Node.js dan Bun. |
| G2 | **Fast & Native** | Ditulis dalam Rust, symlink-based switching di Linux tanpa overhead shell slow-init. |
| G3 | **Linux-First** | Fokus penuh ke platform Linux (`x86_64` dan `aarch64`) untuk fase awal. |
| G4 | **Bundled npm** | npm & npx otomatis ikut terkelola langsung satu paket dengan rilis resmi Node.js. |
| G5 | **Global Packages Handling** | Solusi terintegrasi agar CLI tools global (npm/bun) tetap dapat diakses atau dimigrasikan antar versi. |
| G6 | **Per-project Pinning** | Support `.node-version`, `.nvmrc`, `.bun-version`, serta `.synapse-version`. |
| G7 | **Minimal Footprint** | Single static binary tanpa runtime dependensi external. |

*(Catatan: Dukungan PHP dan OS Windows/macOS ditunda ke milestone masa depan).*

---

## 3. Arsitektur Tingkat Tinggi

```
┌─────────────────────────────────────────────┐
│                  CLI Layer                   │
│         (clap — argument parsing)            │
└──────────────┬──────────────────────────────┘
               │
┌──────────────▼──────────────────────────────┐
│              Core Engine                     │
│  ┌─────────┐ ┌─────────┐ ┌────────────────┐ │
│  │ Resolver│ │Installer│ │ Version Switcher│ │
│  └────┬────┘ └────┬────┘ └───────┬────────┘ │
│       │           │              │           │
│       │           │      ┌───────▼────────┐  │
│       │           │      │ Global Package │  │
│       │           │      │    Manager     │  │
│       │           │      └───────┬────────┘  │
│  ┌────▼───────────▼──────────────▼────────┐  │
│  │          Runtime Providers             │  │
│  │  ┌────────────────┐  ┌──────────────┐  │  │
│  │  │ Node.js (+npm) │  │     Bun      │  │  │
│  │  └────────────────┘  └──────────────┘  │  │
│  │  [PHP: Backlog / Ditunda]               │  │
│  └────────────────────────────────────────┘  │
└──────────────┬──────────────────────────────┘
               │
┌──────────────▼──────────────────────────────┐
│            Platform Layer (Linux)            │
│    Filesystem · Unix Symlinks · HTTP · Tar   │
└─────────────────────────────────────────────┘
```

### Komponen Utama

| Komponen | Tanggung Jawab |
|----------|---------------|
| **CLI Layer** | Parsing argumen & subcommand (`clap`), formatting tabel dan warna (`colored`). |
| **Resolver** | Query index rilis Node.js (`dist/index.json`) dan GitHub releases Bun, parsing semver. |
| **Installer** | Download archive (`.tar.xz`, `.zip`), verifikasi SHA256, ekstraksi ke storage runtimes. |
| **Version Switcher** | Manage symlinks aktif `~/.synapse/current/<runtime>` dan delegasi shim `~/.synapse/bin/`. |
| **Global Package Handler** | Manajemen binary global npm & bun agar persist atau mudah dimigrasikan saat ganti versi. |
| **Runtime Providers** | Implementasi detail download, arsip, dan struktur binary untuk Node.js dan Bun. |
| **Platform Layer** | Operasi Linux I/O: Unix symlink atomik, ekstraksi tar/zip, deteksi arsitektur OS. |

---

## 4. Directory Layout (Linux)

```
~/.synapse/
├── bin/                          # Shims directory yang didaftarkan ke $PATH
│   ├── node -> ../current/node/bin/node
│   ├── npm  -> ../current/node/bin/npm
│   ├── npx  -> ../current/node/bin/npx
│   ├── bun  -> ../current/bun/bun
│   └── bunx -> ../current/bun/bunx
├── runtimes/                     # Versi yang terinstal
│   ├── node/
│   │   ├── 20.18.0/
│   │   └── 22.11.0/
│   └── bun/
│       ├── 1.1.30/
│       └── 1.2.0/
├── current/                      # Symlinks ke versi yang sedang aktif secara global
│   ├── node -> ../runtimes/node/22.11.0
│   └── bun  -> ../runtimes/bun/1.2.0
├── globals/                      # Manifest & shared bin untuk global packages
│   ├── npm/
│   │   ├── packages.txt          # Daftar package global yang dipelihara
│   │   └── bin/                  # Shim untuk executable package global
│   └── bun/
│       └── packages.txt
└── config.toml                   # Konfigurasi user
```

---

## 5. CLI Interface Design

```
synapse <COMMAND> [OPTIONS]

COMMANDS:
  install   <runtime> <version>       Install versi runtime tertentu
  use       <runtime> <version>       Switch versi aktif secara global
  list      <runtime>                 Tampilkan versi yang sudah terinstall lokal
  list-remote <runtime>               Tampilkan versi yang tersedia di remote
  uninstall <runtime> <version>       Hapus versi runtime tertentu
  current   [runtime]                 Tampilkan versi yang sedang aktif
  pin       <runtime> <version>       Buat version file (.node-version / .bun-version) di direktori aktif
  globals   <runtime> [subcommand]    Kelola / sync global packages lintas versi
  env                                 Output konfigurasi export PATH untuk .bashrc / .zshrc
  completions <shell>                 Generate shell completions (bash, zsh, fish)

RUNTIMES:
  node, bun

GLOBAL SUBCOMMANDS:
  synapse globals node list           Lihat paket global yang terdaftar
  synapse globals node migrate <from> <to>  Migrasikan instalasi paket global ke versi baru
  synapse globals node sync           Reinstall daftar paket global ke versi aktif saat ini
```

### Contoh Penggunaan

```bash
# Setup PATH di ~/.bashrc atau ~/.zshrc
export PATH="$HOME/.synapse/bin:$PATH"

# Install & gunakan Node 22 (npm & npx otomatis ikut terpasang)
synapse install node 22
synapse use node 22.11.0

# Install Bun
synapse install bun latest
synapse use bun 1.2.0

# Pin versi untuk project tertentu
cd ~/work/frontend-project
synapse pin node 20.18.0

# Pasang package global dan catat agar tidak hilang saat switch
npm i -g pnpm rimraf
synapse globals node sync
```

---

## 6. Runtime Provider Details

### 6.1 Node.js (Bundled dengan npm)

| Item | Detail |
|------|--------|
| **Source** | Official dist: `https://nodejs.org/dist/` — Index: `https://nodejs.org/dist/index.json` |
| **Archive (Linux)** | `.tar.xz` (atau `.tar.gz`) untuk arsitektur `linux-x64` / `linux-arm64` |
| **Checksum** | `SHASUMS256.txt` per release |
| **Bundled Binaries** | `bin/node`, `bin/npm`, `bin/npx` (npm sudah satu bundle resmi dari nodejs.org) |
| **Version File** | `.node-version`, `.nvmrc` |

### 6.2 Bun

| Item | Detail |
|------|--------|
| **Source** | GitHub Releases: `https://github.com/oven-sh/bun/releases` |
| **Archive (Linux)** | `.zip` (misal `bun-linux-x64.zip`, `bun-linux-aarch64.zip`) |
| **Checksum** | `SHASUMS256.txt` di release assets |
| **Binaries** | `bun`, `bunx` (symlink internal bun) |
| **Version File** | `.bun-version` |

### 6.3 PHP (Status: DEFERRED / BACKLOG)

> [!NOTE]
> Sesuai kesepakatan, implementasi provider PHP **ditunda** dan akan dibahas kembali setelah fondasi Node.js dan Bun berjalan stabil di Linux.

---

## 7. Crate Dependencies (Rust)

| Crate | Justifikasi |
|-------|-------------|
| `clap` | CLI argument parser dengan derive macro, standar industri Rust. |
| `reqwest` | HTTP client async untuk download release index dan archive binary. |
| `tokio` | Async runtime pendukung `reqwest` dan parallel downloads. |
| `serde` + `serde_json` | Deserialisasi JSON release index Node.js & GitHub API Bun. |
| `semver` | Parser dan matcher semver (`22`, `^20.0.0`, `lts`, `latest`). |
| `flate2` + `tar` | Ekstraksi arsip tarball `.tar.gz` di Linux. |
| `xz2` | Ekstraksi arsip `.tar.xz` (format standar rilis official Node.js Linux). |
| `zip` | Ekstraksi arsip `.zip` rilis Bun. |
| `sha2` | Verifikasi integritas checksum SHA256 file unduhan. |
| `indicatif` | Visualisasi progress bar saat download archive. |
| `toml` | Parse & serialize file konfigurasi `config.toml`. |
| `dirs` | Resolusi direktori `$HOME` / standar direktori user Linux. |
| `colored` | Pewarnaan teks terminal untuk status sukses/gagal. |
| `thiserror` | Custom error types yang rapi dan maintainable. |

---

## 8. Milestones & Phases

### Phase 1 — Foundation (Linux MVP: Node.js + npm)

> **Target: CLI dasar, install dan use Node.js dengan bundled npm di Linux.**

- [ ] Inisialisasi project Rust: `cargo new synapse --bin`
- [ ] Core paths & directory layout generator (`~/.synapse/{bin,runtimes,current}`)
- [ ] Trait `RuntimeProvider`
- [ ] Implementasi Node.js Provider:
  - [ ] Fetch & parse `nodejs.org/dist/index.json`
  - [ ] Download arsip `.tar.xz` sesuai arch (`x86_64` / `aarch64`)
  - [ ] Verifikasi checksum `SHASUMS256.txt`
  - [ ] Ekstrak dan susun binary `node`, `npm`, `npx`
- [ ] Subcommand CLI: `install`, `use`, `list`, `current`, `uninstall`
- [ ] Symlink switching untuk `node`, `npm`, `npx`
- [ ] Command `synapse env` untuk panduan export `PATH` di shell Linux
- [ ] **Verifikasi Phase 1**:
  - `synapse install node 22`
  - `synapse use node 22.x.x`
  - Jalankan `node -v` dan `npm -v` langsung dari terminal Linux.

### Phase 2 — Bun Provider

> **Target: Full support Bun.js di Linux.**

- [ ] Implementasi Bun Provider:
  - [ ] Fetch metadata rilis dari GitHub Releases API (`oven-sh/bun`)
  - [ ] Download `.zip` untuk Linux
  - [ ] Verifikasi checksum
  - [ ] Ekstrak `bun` dan `bunx`
- [ ] Integrasikan `bun` ke commands (`install`, `use`, `list`, `current`, `uninstall`)
- [ ] **Verifikasi Phase 2**:
  - `synapse install bun latest`
  - `synapse use bun 1.x.x`
  - Jalankan `bun --version` dan `bunx --version`.

### Phase 3 — Global Packages Handling

> **Target: Solusi agar global packages (npm & bun) tidak hilang saat switch versi.**

- [ ] Desain mekanisme global packages:
  - Opsi A: Manifest-driven auto-migration (`synapse globals <runtime> migrate <old> <new>`)
  - Opsi B: Persistent shared global directory (mengatur `npm config set prefix` dan `BUN_INSTALL`)
- [ ] Subcommand `synapse globals`:
  - `synapse globals <runtime> list`
  - `synapse globals <runtime> sync` (reinstall packages yang tercatat ke versi aktif)
- [ ] **Verifikasi Phase 3**:
  - Install package global di Node v20 (misal: `npm i -g pnpm`)
  - Pindah ke Node v22 via `synapse use node 22.x.x`
  - Jalankan `synapse globals node sync` atau akses binary `pnpm` tanpa kendala.

### Phase 4 — Per-Project Version Pinning

> **Target: Deteksi versi per direktori secara otomatis.**

- [ ] Parser version files: `.node-version`, `.nvmrc`, `.bun-version`, `.synapse-version`
- [ ] Subcommand `synapse pin <runtime> <version>`
- [ ] Lightweight wrapper/shim binary di `~/.synapse/bin/` yang membaca file pin di direktori aktif sebelum fallback ke versi global
- [ ] **Verifikasi Phase 4**:
  - Buat folder uji coba dengan `.node-version` berisi `20.18.0`
  - Eksekusi `node -v` di dalam folder tersebut mengembalikan `v20.18.0` meskipun versi global adalah `v22.x.x`.

### Phase 5 — Polish & Linux Distribution

> **Target: Siap pakai untuk end-user Linux.**

- [ ] Shell completion generation (`bash`, `zsh`, `fish`)
- [ ] Script one-liner installer untuk Linux (`curl -fsSL ... | bash`)
- [ ] Build binary statis Linux (`x86_64-unknown-linux-gnu` / `musl`)
- [ ] Dokumentasi lengkap di `README.md`

### Backlog / Future Milestones (Post-v1.0)
- **PHP Provider**: Evaluasi implementasi via `static-php-cli` pre-built binary.
- **Cross-Platform**: Dukungan macOS dan Windows (Junctions & PowerShell shims).

---

## 9. Project Structure (Rust Workspace)

```
synapse/
├── Cargo.toml
├── src/
│   ├── main.rs                 # Entrypoint & routing CLI clap
│   ├── cli/
│   │   ├── mod.rs
│   │   ├── install.rs          # Subcommand: install
│   │   ├── use_cmd.rs          # Subcommand: use
│   │   ├── list.rs             # Subcommand: list & list-remote
│   │   ├── current.rs          # Subcommand: current
│   │   ├── uninstall.rs        # Subcommand: uninstall
│   │   ├── pin.rs              # Subcommand: pin
│   │   ├── globals.rs          # Subcommand: globals
│   │   └── env.rs              # Subcommand: env
│   ├── provider/
│   │   ├── mod.rs              # Trait RuntimeProvider
│   │   ├── node.rs             # Implementasi Node.js (+ npm bundled)
│   │   └── bun.rs              # Implementasi Bun
│   ├── core/
│   │   ├── mod.rs
│   │   ├── resolver.rs         # Semver & remote version lookup
│   │   ├── installer.rs        # Download, verifikasi SHA256, ekstraksi
│   │   ├── switcher.rs         # Manajemen symlink aktif
│   │   ├── globals.rs          # Logic sinkronisasi / migrasi global packages
│   │   └── version_file.rs     # Deteksi .node-version / .bun-version
│   ├── platform/
│   │   ├── mod.rs
│   │   ├── archive.rs          # Ekstraksi .tar.xz, .tar.gz, .zip
│   │   ├── checksum.rs         # Verifikasi SHA256
│   │   └── paths.rs            # Manajemen path Linux ~/.synapse
│   ├── config.rs               # Parser config.toml
│   └── error.rs                # Definisi error kustom (thiserror)
├── tests/
│   ├── unit/
│   │   ├── resolver_test.rs
│   │   └── version_file_test.rs
│   └── integration/
│       ├── node_test.rs
│       └── bun_test.rs
└── README.md
```

---

## 10. Key Design Decisions

### 10.1 Linux-First Architecture
Fokus pada Linux membuat manipulasi symlink dan filesystem menjadi sangat cepat dan sederhana menggunakan API POSIX standar (`std::os::unix::fs::symlink`). Tidak perlu layer kompleks abstraksi Windows permissions atau registry.

### 10.2 Bundled npm Strategy
Node.js rilis resmi selalu membawa direktori `npm` dan `npx` di dalam folder `bin/` dan `lib/node_modules/npm`. Synapse akan langsung menghubungkan symlink `npm` dan `npx` ke instalasi Node yang sedang aktif. Tidak perlu instalasi npm terpisah.

### 10.3 Global Packages Strategy (Hybrid Approach)
1. **Manifest Tracker**: Synapse menyimpan daftar package global yang sengaja di-install user di `~/.synapse/globals/<runtime>/packages.txt`.
2. **Sync Command**: Saat user berpindah versi (misal Node 20 ke 22), command `synapse globals node sync` dapat secara otomatis menginstal ulang package-package penting tersebut ke versi baru.
3. **Optional Shared Prefix**: Opsi konfigurasi untuk mengarahkan global npm prefix ke direktori persisten di luar folder runtime, sehingga binary global tetap utuh.

### 10.4 Symlink vs Shim
- **Global default**: Symlink langsung di `~/.synapse/bin/` menuju `~/.synapse/current/<runtime>/bin/` untuk eksekusi tercepat tanpa proses overhead.
- **Pinning**: Shim cerdas yang membaca direktori saat ini sebelum mengeksekusi target binary.

---

## 11. Verification Plan

| Fase | Rencana Pengujian |
|------|-------------------|
| **Phase 1** | Unit test parser semver & checksum SHA256. Integration test: instalasi Node 20 & 22 di Linux, uji eksekusi `node -v` dan `npm -v`. |
| **Phase 2** | Integration test: instalasi Bun di Linux, uji eksekusi `bun -v` dan `bunx`. |
| **Phase 3** | Uji coba instalasi package global (misal `pnpm`), switch versi Node, jalankan sync, pastikan `pnpm` tetap dapat dieksekusi. |
| **Phase 4** | Uji coba direktori dengan file `.node-version` dan `.bun-version`, verifikasi deteksi otomatis. |
| **Phase 5** | Validasi instalasi end-to-end pada container / clean environment Ubuntu/Debian/Arch Linux. |

---

## 12. Rekap Keputusan & Catatan

> [!NOTE]
> Keputusan yang sudah difinalisasi:
> 1. **Nama Project**: `synapse` (sudah fix).
> 2. **Dukungan PHP**: Ditunda ke backlog (akan dibahas kembali setelah Node & Bun matang).
> 3. **Target Platform**: Fokus Linux terlebih dahulu (arsitektur x86_64 dan aarch64).
> 4. **npm**: Dibundle langsung bersama Node.js dari rilis resminya.
> 5. **Global Packages**: Ditangani secara eksplisit melalui tracking manifest & migrasi/sync (Phase 3).
