use sha2::{Digest, Sha256};
use crate::error::{Result, SynapseError};

/// Parses `SHASUMS256.txt` content to find the expected SHA256 hash for a specific file.
pub fn parse_checksum(content: &str, target_file: &str) -> Option<String> {
    for line in content.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 && parts[1] == target_file {
            return Some(parts[0].to_lowercase());
        }
    }
    None
}

/// Verifies that the data matches the expected SHA256 checksum.
pub fn verify_sha256(data: &[u8], expected_checksum: &str, file_name: &str) -> Result<()> {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let actual_checksum: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect();

    if !actual_checksum.eq_ignore_ascii_case(expected_checksum) {
        return Err(SynapseError::ChecksumMismatch {
            file: file_name.to_string(),
            expected: expected_checksum.to_string(),
            actual: actual_checksum,
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_checksum() {
        let sample_shasums = r#"
9b9a674488b3992b8d4bb67a840e698889980ee91f24d193309a63aa36eb9d13  node-v22.11.0-headers.tar.xz
8dd374667a489dd3e0d8fa0aa53460830401bcfcbb2ffbaeaec009b0b46ebf24  node-v22.11.0-linux-arm64.tar.xz
d387f3ff0bc46312a03cf65a3964ff05e608f51a89c97b83d8e5781a7d65bce8  node-v22.11.0-linux-x64.tar.xz
"#;
        let x64_hash = parse_checksum(sample_shasums, "node-v22.11.0-linux-x64.tar.xz");
        assert_eq!(
            x64_hash,
            Some("d387f3ff0bc46312a03cf65a3964ff05e608f51a89c97b83d8e5781a7d65bce8".to_string())
        );

        let arm64_hash = parse_checksum(sample_shasums, "node-v22.11.0-linux-arm64.tar.xz");
        assert_eq!(
            arm64_hash,
            Some("8dd374667a489dd3e0d8fa0aa53460830401bcfcbb2ffbaeaec009b0b46ebf24".to_string())
        );

        let not_found = parse_checksum(sample_shasums, "non-existent-file.tar.xz");
        assert_eq!(not_found, None);
    }
    
    #[test]
    fn test_verify_sha256() {
        let data = b"hello world";
        let expected = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
        assert!(verify_sha256(data, expected, "test.txt").is_ok());
        
        let invalid = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde0";
        assert!(verify_sha256(data, invalid, "test.txt").is_err());
    }
}
