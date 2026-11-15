use std::fs;
use std::path::{Component, Path};
use crate::error::{Result, SynapseError};

/// Validates that a tar symlink/hardlink target is relative and cannot escape `target_dir`.
fn validate_link_target(link_target: &Path, start_depth: usize) -> Result<()> {
    let mut depth = start_depth;
    for comp in link_target.components() {
        match comp {
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            Component::ParentDir => {
                if depth == 0 {
                    return Err(SynapseError::Extraction(format!(
                        "Unsafe link target escaping extraction directory: {}",
                        link_target.display()
                    )));
                }
                depth -= 1;
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(SynapseError::Extraction(format!(
                    "Absolute link target is not allowed in tar archive: {}",
                    link_target.display()
                )));
            }
        }
    }
    Ok(())
}

/// Unpacks entries from a tar archive into `target_dir`, optionally stripping the first path component.
fn unpack_tar_entries<R: std::io::Read>(
    mut archive: tar::Archive<R>,
    target_dir: &Path,
    strip_root: bool,
) -> Result<()> {
    archive.set_preserve_permissions(true);

    let entries = archive
        .entries()
        .map_err(|e| SynapseError::Extraction(format!("Failed to read tar archive: {}", e)))?;

    for entry_res in entries {
        let mut entry = entry_res
            .map_err(|e| SynapseError::Extraction(format!("Corrupted tar entry: {}", e)))?;
        let path = entry
            .path()
            .map_err(|e| SynapseError::Extraction(format!("Invalid path in tar entry: {}", e)))?
            .into_owned();

        if path.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) {
            return Err(SynapseError::Extraction(format!(
                "Unsafe path in tar archive: {}",
                path.display()
            )));
        }

        let relative_path = if strip_root {
            let mut components = path.components();
            components.next(); // Skip root component
            components.as_path()
        } else {
            path.as_path()
        };

        if relative_path.as_os_str().is_empty() {
            continue;
        }

        if !relative_path
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
        {
            return Err(SynapseError::Extraction(format!(
                "Unsafe relative path in tar archive: {}",
                relative_path.display()
            )));
        }

        let entry_type = entry.header().entry_type();
        if entry_type.is_symlink() || entry_type.is_hard_link() {
            let link_target = entry
                .link_name()
                .map_err(|e| {
                    SynapseError::Extraction(format!("Invalid link target in tar entry: {}", e))
                })?
                .ok_or_else(|| {
                    SynapseError::Extraction(format!(
                        "Missing link target for tar entry: {}",
                        relative_path.display()
                    ))
                })?;

            let start_depth = if entry_type.is_symlink() {
                relative_path
                    .parent()
                    .map(|p| p.components().count())
                    .unwrap_or(0)
            } else {
                0
            };
            validate_link_target(&link_target, start_depth)?;
        }

        let dest = target_dir.join(relative_path);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        entry
            .unpack(&dest)
            .map_err(|e| SynapseError::Extraction(format!("Failed to unpack to {}: {}", dest.display(), e)))?;
    }

    Ok(())
}

/// Extracts a `.tar.xz` archive from a reader into `target_dir`, stripping the first path component.
pub fn extract_tar_xz<R: std::io::Read>(reader: R, target_dir: &Path) -> Result<()> {
    let decompressor = xz2::read::XzDecoder::new(reader);
    let archive = tar::Archive::new(decompressor);
    unpack_tar_entries(archive, target_dir, true)
}

/// Extracts a `.tar.gz` archive from a reader into `target_dir`, optionally stripping the first path component.
pub fn extract_tar_gz<R: std::io::Read>(reader: R, target_dir: &Path, strip_root: bool) -> Result<()> {
    let decompressor = flate2::read::GzDecoder::new(reader);
    let archive = tar::Archive::new(decompressor);
    unpack_tar_entries(archive, target_dir, strip_root)
}

/// Extracts a `.zip` archive from a reader into `target_dir`, stripping the first path component.
pub fn extract_zip<R: std::io::Read + std::io::Seek>(reader: R, target_dir: &Path) -> Result<()> {
    extract_zip_opts(reader, target_dir, true)
}

/// Extracts a `.zip` archive from a reader into `target_dir`, optionally stripping the first path component.
pub fn extract_zip_opts<R: std::io::Read + std::io::Seek>(
    reader: R,
    target_dir: &Path,
    strip_root: bool,
) -> Result<()> {
    let mut archive = zip::ZipArchive::new(reader)
        .map_err(|e| SynapseError::Extraction(format!("Failed to read zip archive: {}", e)))?;

    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .map_err(|e| SynapseError::Extraction(format!("Corrupted zip entry at index {}: {}", i, e)))?;

        let enclosed = match file.enclosed_name() {
            Some(p) => p.to_owned(),
            None => continue,
        };

        if enclosed.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) {
            return Err(SynapseError::Extraction(format!(
                "Unsafe path in zip archive: {}",
                enclosed.display()
            )));
        }

        let relative_path = if strip_root {
            let mut components = enclosed.components();
            components.next(); // Skip root component
            components.as_path()
        } else {
            enclosed.as_path()
        };
        if relative_path.as_os_str().is_empty() {
            continue;
        }

        if !relative_path
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
        {
            return Err(SynapseError::Extraction(format!(
                "Unsafe relative path in zip archive: {}",
                relative_path.display()
            )));
        }

        let dest = target_dir.join(relative_path);
        if file.is_dir() {
            fs::create_dir_all(&dest)?;
        } else {
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut outfile = fs::File::create(&dest)
                .map_err(|e| SynapseError::Extraction(format!("Failed to create {}: {}", dest.display(), e)))?;
            std::io::copy(&mut file, &mut outfile)
                .map_err(|e| SynapseError::Extraction(format!("Failed to write {}: {}", dest.display(), e)))?;

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(&dest, fs::Permissions::from_mode(0o755));
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_extract_tar_xz() {
        let mut tar_builder = tar::Builder::new(Vec::new());
        let file_content = b"console.log('hello');";
        let mut header = tar::Header::new_gnu();
        header.set_size(file_content.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();

        tar_builder
            .append_data(
                &mut header,
                "node-v22.0.0-linux-x64/bin/node",
                &file_content[..],
            )
            .unwrap();

        let tar_bytes = tar_builder.into_inner().unwrap();

        let mut compressor = xz2::write::XzEncoder::new(Vec::new(), 6);
        compressor.write_all(&tar_bytes).unwrap();
        let xz_bytes = compressor.finish().unwrap();

        let temp_dir = std::env::temp_dir().join(format!("synapse_extract_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        extract_tar_xz(std::io::Cursor::new(xz_bytes), &temp_dir).unwrap();

        let extracted_file = temp_dir.join("bin").join("node");
        assert!(extracted_file.is_file());
        let content = fs::read_to_string(extracted_file).unwrap();
        assert_eq!(content, "console.log('hello');");

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_extract_zip() {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buf);
            let options = zip::write::SimpleFileOptions::default();
            zip.start_file("bun-linux-x64/bun", options).unwrap();
            zip.write_all(b"bun binary content").unwrap();
            zip.finish().unwrap();
        }

        let temp_dir = std::env::temp_dir().join(format!("synapse_zip_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        buf.set_position(0);
        extract_zip(buf, &temp_dir).unwrap();

        let extracted_file = temp_dir.join("bun");
        assert!(extracted_file.is_file());
        let content = fs::read_to_string(&extracted_file).unwrap();
        assert_eq!(content, "bun binary content");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::metadata(&extracted_file).unwrap().permissions();
            assert_eq!(perms.mode() & 0o111, 0o111);
        }

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_extract_tar_gz() {
        let mut tar_builder = tar::Builder::new(Vec::new());
        let file_content = b"php binary content";
        let mut header = tar::Header::new_gnu();
        header.set_size(file_content.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();

        tar_builder
            .append_data(&mut header, "php", &file_content[..])
            .unwrap();

        let tar_bytes = tar_builder.into_inner().unwrap();

        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&tar_bytes).unwrap();
        let gz_bytes = encoder.finish().unwrap();

        let temp_dir = std::env::temp_dir().join(format!("synapse_targz_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        extract_tar_gz(std::io::Cursor::new(gz_bytes), &temp_dir, false).unwrap();

        let extracted_file = temp_dir.join("php");
        assert!(extracted_file.is_file());
        assert_eq!(fs::read_to_string(extracted_file).unwrap(), "php binary content");

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_extract_tar_rejects_traversal() {
        // 1. Rejects path traversal via `..` in entry path
        let mut bad_path_tar = tar::Builder::new(Vec::new());
        let data = b"evil";
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        {
            let gnu = header.as_gnu_mut().unwrap();
            let raw_name = b"../evil";
            gnu.name[..raw_name.len()].copy_from_slice(raw_name);
        }
        header.set_cksum();
        bad_path_tar.append(&header, &data[..]).unwrap();
        let tar_bytes = bad_path_tar.into_inner().unwrap();

        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&tar_bytes).unwrap();
        let gz_bytes = encoder.finish().unwrap();

        let temp_dir =
            std::env::temp_dir().join(format!("synapse_traversal_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let err = extract_tar_gz(std::io::Cursor::new(gz_bytes), &temp_dir, false).unwrap_err();
        assert!(matches!(err, SynapseError::Extraction(_)));

        // 2. Rejects symlink pointing outside extraction directory (/etc/passwd)
        let mut bad_link_tar = tar::Builder::new(Vec::new());
        let mut link_header = tar::Header::new_gnu();
        link_header.set_entry_type(tar::EntryType::Symlink);
        link_header.set_size(0);
        link_header.set_mode(0o777);
        link_header.set_link_name("/etc/passwd").unwrap();
        link_header.set_cksum();
        bad_link_tar
            .append_data(&mut link_header, "bin/leak", std::io::empty())
            .unwrap();
        let link_tar_bytes = bad_link_tar.into_inner().unwrap();

        let mut link_encoder =
            flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        link_encoder.write_all(&link_tar_bytes).unwrap();
        let link_gz_bytes = link_encoder.finish().unwrap();

        let err_link =
            extract_tar_gz(std::io::Cursor::new(link_gz_bytes), &temp_dir, false).unwrap_err();
        assert!(matches!(err_link, SynapseError::Extraction(_)));

        // 3. Rejects relative symlink escaping via `..` beyond entry depth
        let mut rel_link_tar = tar::Builder::new(Vec::new());
        let mut rel_header = tar::Header::new_gnu();
        rel_header.set_entry_type(tar::EntryType::Symlink);
        rel_header.set_size(0);
        rel_header.set_mode(0o777);
        rel_header.set_link_name("../../outside").unwrap();
        rel_header.set_cksum();
        rel_link_tar
            .append_data(&mut rel_header, "bin/escape", std::io::empty())
            .unwrap();
        let rel_tar_bytes = rel_link_tar.into_inner().unwrap();

        let mut rel_encoder =
            flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        rel_encoder.write_all(&rel_tar_bytes).unwrap();
        let rel_gz_bytes = rel_encoder.finish().unwrap();

        let err_rel =
            extract_tar_gz(std::io::Cursor::new(rel_gz_bytes), &temp_dir, false).unwrap_err();
        assert!(matches!(err_rel, SynapseError::Extraction(_)));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_extract_zip_without_strip_root() {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buf);
            let options = zip::write::SimpleFileOptions::default();
            zip.start_file("php.exe", options).unwrap();
            zip.write_all(b"php windows binary").unwrap();
            zip.start_file("ext/php_openssl.dll", options).unwrap();
            zip.write_all(b"dll content").unwrap();
            zip.finish().unwrap();
        }

        let temp_dir =
            std::env::temp_dir().join(format!("synapse_zip_flat_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        buf.set_position(0);
        extract_zip_opts(buf, &temp_dir, false).unwrap();

        assert_eq!(
            fs::read_to_string(temp_dir.join("php.exe")).unwrap(),
            "php windows binary"
        );
        assert_eq!(
            fs::read_to_string(temp_dir.join("ext").join("php_openssl.dll")).unwrap(),
            "dll content"
        );

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
