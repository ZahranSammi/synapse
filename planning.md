# Phase 1 — Foundation (Linux MVP: Node.js + npm)

> Dokumen ini adalah panduan high-level untuk mengerjakan Phase 1 Synapse.
> Referensi lengkap ada di [`issue.md`](file:///home/hypersex/Dokumen/Ivy/synapse/issue.md).

---

## Tujuan Phase 1

Membangun CLI `synapse` yang bisa **install, switch, dan manage versi Node.js** (beserta npm/npx yang sudah bundled) di **Linux**. Setelah phase ini selesai, user harus bisa:

```bash
synapse install node 22
synapse use node 22.11.0
node -v   # → v22.11.0
npm -v    # → 10.x.x
```

---

## Urutan Pengerjaan

Phase ini dibagi menjadi **6 step** yang dikerjakan berurutan. Setiap step punya kriteria "selesai" yang jelas.

---

### Step 1: Inisialisasi Project

**Apa yang dilakukan:**
- `cargo init` di workspace `synapse/`
- Tambahkan semua crate dependency ke `Cargo.toml` (lihat Section 7 di `issue.md`)
- Buat folder structure sesuai layout yang sudah didefinisikan di `issue.md` Section 9 — minimal:
  - `src/main.rs`
  - `src/cli/mod.rs`
  - `src/provider/mod.rs`
  - `src/core/mod.rs`
  - `src/platform/mod.rs`
  - `src/error.rs`
  - `src/config.rs`

**Selesai kalau:** `cargo build` berhasil tanpa error, binary `synapse` bisa dijalankan (boleh cuma print "hello").

---

### Step 2: Platform Layer — Paths & Directory Setup

**Apa yang dilakukan:**
- Buat module `src/platform/paths.rs` yang menentukan lokasi semua direktori Synapse:
  - `~/.synapse/bin/`
  - `~/.synapse/runtimes/node/`
  - `~/.synapse/current/`
- Buat fungsi `ensure_dirs()` yang membuat semua direktori di atas kalau belum ada (`std::fs::create_dir_all`)
- Deteksi arsitektur mesin (`x86_64` atau `aarch64`) menggunakan `std::env::consts::ARCH`

**Selesai kalau:** Panggil `ensure_dirs()`, folder `~/.synapse/` terbentuk lengkap dengan subdirektorinya.

---

### Step 3: Node.js Provider — Resolver & Downloader

**Apa yang dilakukan:**
- Buat trait `RuntimeProvider` di `src/provider/mod.rs`. Trait ini adalah kontrak yang harus dipenuhi oleh setiap runtime (Node, Bun, dst). Method minimal:
  - `list_remote()` → daftar versi yang tersedia
  - `download(version)` → download & extract ke folder runtimes
  - `binary_names()` → nama-nama executable yang disediakan runtime ini
- Implementasikan trait untuk Node.js di `src/provider/node.rs`:
  - **Resolver**: Fetch JSON dari `https://nodejs.org/dist/index.json`, parse jadi list versi. Support input partial (`22` → resolve ke `22.11.0` terbaru).
  - **Downloader**: Buat URL download berdasarkan versi dan arch (contoh: `https://nodejs.org/dist/v22.11.0/node-v22.11.0-linux-x64.tar.xz`). Download file-nya pakai `reqwest` dengan progress bar (`indicatif`).
  - **Checksum**: Download `SHASUMS256.txt` dari release yang sama, hitung SHA256 file yang diunduh, cocokkan.
  - **Extractor**: Extract `.tar.xz` ke `~/.synapse/runtimes/node/<version>/`

**Selesai kalau:** Bisa download Node.js 22 ke `~/.synapse/runtimes/node/22.11.0/`, file `bin/node` ada dan executable.

---

### Step 4: Version Switcher — Symlink Management

**Apa yang dilakukan:**
- Buat module `src/core/switcher.rs`
- Fungsi `switch(runtime, version)`:
  1. Cek apakah versi yang diminta sudah terinstall di `~/.synapse/runtimes/node/<version>/`
  2. Update symlink `~/.synapse/current/node` → menunjuk ke folder versi tersebut
  3. Update symlink-symlink di `~/.synapse/bin/` untuk setiap binary (`node`, `npm`, `npx`) → menunjuk ke binary di dalam `current/node/bin/`
- Symlink harus **atomik**: hapus symlink lama dulu (`std::fs::remove_file`), baru buat yang baru (`std::os::unix::fs::symlink`)

**Selesai kalau:** Setelah `switch("node", "22.11.0")`, menjalankan `~/.synapse/bin/node -v` menghasilkan `v22.11.0`.

---

### Step 5: CLI Layer — Subcommands

**Apa yang dilakukan:**
- Setup `clap` dengan derive macro di `src/main.rs`
- Implementasi handler untuk setiap subcommand di `src/cli/`:

| Subcommand | Logika |
|------------|--------|
| `install <runtime> <version>` | Panggil provider.download(version). Setelah install, otomatis panggil switcher.switch() kecuali ada flag `--no-use`. |
| `use <runtime> <version>` | Panggil switcher.switch(). Error kalau versi belum terinstall. |
| `list <runtime>` | Baca isi folder `~/.synapse/runtimes/node/`, list semua subfolder. Tandai mana yang aktif (bandingkan dengan symlink `current/`). |
| `list-remote <runtime>` | Panggil provider.list_remote(), tampilkan dalam format tabel. |
| `current [runtime]` | Baca target symlink `~/.synapse/current/node`, extract nama versinya. |
| `uninstall <runtime> <version>` | Hapus folder `~/.synapse/runtimes/node/<version>/`. Kalau itu versi yang aktif, hapus juga symlink `current/`. |
| `env` | Print instruksi `export PATH="$HOME/.synapse/bin:$PATH"` yang bisa di-copy user ke `.bashrc`/`.zshrc`. |

- Gunakan `colored` untuk output berwarna: ✓ hijau untuk sukses, ✗ merah untuk error.

**Selesai kalau:** Semua subcommand di atas berfungsi end-to-end dari terminal.

---

### Step 6: Error Handling & Polish

**Apa yang dilakukan:**
- Definisikan custom error enum di `src/error.rs` pakai `thiserror`. Minimal cover:
  - `NetworkError` — gagal fetch/download
  - `VersionNotFound` — versi tidak ditemukan di remote
  - `NotInstalled` — user coba `use` versi yang belum diinstall
  - `ChecksumMismatch` — file corrupt
  - `IoError` — gagal baca/tulis filesystem
- Pastikan semua error muncul sebagai pesan yang jelas ke user (bukan panic/backtrace)
- Review semua `unwrap()` — ganti dengan proper error propagation (`?` operator)

**Selesai kalau:** Tidak ada panic di happy path maupun error path. Semua error case menampilkan pesan yang actionable.

---

## Kriteria Phase 1 Selesai

Skenario end-to-end ini harus berjalan mulus di terminal Linux:

```bash
# 1. Setup PATH (sekali)
eval "$(synapse env)"

# 2. Lihat versi yang tersedia
synapse list-remote node

# 3. Install Node 20 dan 22
synapse install node 20
synapse install node 22

# 4. Cek versi aktif
synapse current         # → node: v22.x.x

# 5. Switch ke Node 20
synapse use node 20
node -v                 # → v20.x.x
npm -v                  # → 10.x.x

# 6. List versi lokal
synapse list node       # → 20.x.x ✓ (aktif), 22.x.x

# 7. Hapus versi
synapse uninstall node 22
synapse list node       # → 20.x.x ✓ (aktif)
```

---

## Catatan untuk Implementor

> [!TIP]
> **Mulai dari Step 1–3 dulu.** Kalau resolver + downloader sudah jalan, sisanya tinggal wiring.

> [!IMPORTANT]
> - Jangan bikin abstraksi provider yang terlalu generik di phase ini. Trait `RuntimeProvider` cukup punya method yang dibutuhkan Node.js saja. Kalau nanti Bun butuh method berbeda, kita adjust trait-nya di Phase 2.
> - Jangan handle Windows/macOS. Semua symlink pakai `std::os::unix::fs::symlink` langsung.
> - Semua path hardcode ke `~/.synapse/`. Config untuk custom path bisa ditambah nanti kalau ada kebutuhan.

> [!WARNING]
> Hati-hati dengan race condition saat update symlink. Pattern yang aman:
> 1. Buat symlink baru dengan nama temporary (misal `.current_node_tmp`)
> 2. `std::fs::rename()` temporary → target (rename itu atomic di Linux)
