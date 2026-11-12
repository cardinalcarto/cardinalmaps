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

/// Why a downloaded transfer tile isn't the one asked for.
pub struct TileCheckError {
    /// A description for logs, naming the file.
    pub message: String,
}

/// The most a transfer tile may decompress to on a phone; real tiles are a few megabytes.
const MAX_TILE_BYTES: usize = 64 * 1024 * 1024;

/// Checks a downloaded transfer tile without a published hash: its zstd frame must decode
/// whole with nothing after it, the CRC-32 in its header must match the whole decompressed
/// tile (`valinor-walk`'s `transfer_tile` format: it covers every byte but itself), and the
/// header must name `mode`, tile `x`, `y` at zoom 12, and the build `manifest_id` (16
/// hexadecimal digits, as the manifest gives it).
pub fn check_transfer_tile(
    path: String,
    mode: String,
    x: u32,
    y: u32,
    manifest_id: String,
) -> Result<(), TileCheckError> {
    use valinor_walk::transfer_tile::{DecodeLimits, TransferMode, decode_tile};
    let error = |message: String| TileCheckError {
        message: format!("{path}: {message}"),
    };
    let bytes = std::fs::read(&path).map_err(|e| error(e.to_string()))?;
    let tile = decode_tile(
        &bytes,
        DecodeLimits {
            max_decompressed_bytes: MAX_TILE_BYTES,
        },
    )
    .map_err(|e| error(e.to_string()))?;
    let raw = tile.as_bytes();
    // The header's mode byte is the mode's position in `TransferMode::ALL`.
    let mode_byte = TransferMode::ALL
        .iter()
        .position(|m| m.as_str() == mode)
        .ok_or_else(|| error(format!("{mode} isn't a transfer mode")))?;
    let u32_at = |at: usize| u32::from_le_bytes([raw[at], raw[at + 1], raw[at + 2], raw[at + 3]]);
    let mut id = [0u8; 8];
    id.copy_from_slice(&raw[16..24]);
    let expected_id = u64::from_str_radix(&manifest_id, 16)
        .map_err(|_| error(format!("{manifest_id} isn't a manifest ID")))?;
    let found = (
        usize::from(raw[6]),
        u32_at(8),
        u32_at(12),
        u64::from_le_bytes(id),
    );
    if found != (mode_byte, x, y, expected_id) {
        return Err(error(format!(
            "it is mode {} tile {}/{} of build {:016x}, not {mode} {x}/{y} of {manifest_id}",
            found.0, found.1, found.2, found.3
        )));
    }
    Ok(())
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

    /// A transfer tile, compressed as published.
    #[test]
    fn checks_transfer_tiles() {
        use valinor_walk::transfer_tile::{TransferMode, TransferTile, TransferTileRow};
        let walks = [valinor_walk::transfer_tile::TileWalk {
            feed: 0,
            stop: 2,
            seconds: 60,
        }];
        let tile = TransferTile::from_rows(
            TransferMode::Cycling,
            (2066, 1496),
            0x00ff_1234_5678_9abc,
            900,
            &[TransferTileRow {
                feed: 0,
                stop: 1,
                walks: &walks,
            }],
        )
        .ok()
        .unwrap();
        let dir = std::env::temp_dir().join(format!("routing-flutter-tile-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("1496.bin.zst");
        let frame = zstd::bulk::compress(tile.as_bytes(), 3).unwrap();
        std::fs::write(&path, &frame).unwrap();
        let p = || path.to_string_lossy().into_owned();
        let id = "00ff123456789abc".to_owned();
        assert!(check_transfer_tile(p(), "cycling".into(), 2066, 1496, id.clone()).is_ok());
        for (mode, x, y, id) in [
            ("walking", 2066, 1496, id.clone()),
            ("cycling", 2067, 1496, id.clone()),
            ("cycling", 2066, 1496, "00ff123456789abd".to_owned()),
        ] {
            assert!(check_transfer_tile(p(), mode.into(), x, y, id).is_err());
        }
        // Any changed byte, or a short file, fails.
        for at in [0, frame.len() / 2, frame.len() - 1] {
            let mut bad = frame.clone();
            bad[at] ^= 0x10;
            std::fs::write(&path, &bad).unwrap();
            assert!(
                check_transfer_tile(p(), "cycling".into(), 2066, 1496, id.clone()).is_err(),
                "{at}"
            );
        }
        std::fs::write(&path, &frame[..frame.len() - 3]).unwrap();
        assert!(check_transfer_tile(p(), "cycling".into(), 2066, 1496, id).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
