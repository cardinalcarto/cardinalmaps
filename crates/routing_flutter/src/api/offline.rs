//! Checking downloaded files (see `flutter/cardinal/lib/offline/`).

use std::fs::File;
use std::io::Read;

use sha2::{Digest, Sha256};

/// Why a file couldn't be hashed.
pub struct HashError {
    /// A description for logs, naming the file.
    pub message: String,
}

/// How much of a file is read at once.
const BLOCK: usize = 1 << 20;

/// The SHA-256 of a file, in lowercase hexadecimal.
///
/// Uses the CPU's SHA instructions where it has them (ARMv8's on phones, SHA-NI on x86),
/// found at run time. Not `#[frb(sync)]`: it runs on flutter_rust_bridge's worker pool,
/// reading a megabyte at a time, so a large timetable doesn't block the UI.
pub fn sha256_file(path: String) -> Result<String, HashError> {
    let error = |e: std::io::Error| HashError {
        message: format!("{path}: {e}"),
    };
    let mut file = File::open(&path).map_err(error)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; BLOCK];
    loop {
        let read = file.read(&mut buffer).map_err(error)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_files() {
        let dir = std::env::temp_dir().join(format!("routing-flutter-sha-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let empty = dir.join("empty");
        std::fs::write(&empty, b"").unwrap();
        let big = dir.join("big");
        // More than one block, so blocks are joined.
        std::fs::write(&big, vec![b'a'; BLOCK + 3]).unwrap();
        assert_eq!(
            sha256_file(empty.to_string_lossy().into_owned())
                .ok()
                .unwrap(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        let expected: String = Sha256::digest(vec![b'a'; BLOCK + 3])
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(
            sha256_file(big.to_string_lossy().into_owned())
                .ok()
                .unwrap(),
            expected
        );
        let missing = sha256_file(dir.join("missing").to_string_lossy().into_owned());
        assert!(missing.err().unwrap().message.contains("missing"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
