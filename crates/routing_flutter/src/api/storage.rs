//! The device's storage, for offline downloads (see `flutter/cardinal/lib/offline/`).

use std::io;
use std::path::Path;

/// Why free space couldn't be read.
pub struct FreeSpaceError {
    /// A description for logs, naming the path.
    pub message: String,
}

/// The bytes the app may still write on the filesystem holding `path`: the blocks free
/// to an unprivileged process (`statvfs`'s `f_bavail`, which leaves out the blocks
/// reserved for root), times the fragment size.
///
/// A `path` that doesn't exist yet, such as the offline folder before the first
/// download, is measured at its nearest existing parent.
pub fn free_bytes(path: String) -> Result<i64, FreeSpaceError> {
    let error = |e: io::Error| FreeSpaceError {
        message: format!("{path}: {e}"),
    };
    let mut at = Path::new(&path);
    loop {
        match available(at) {
            Ok(bytes) => return Ok(i64::try_from(bytes).unwrap_or(i64::MAX)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                at = at.parent().ok_or_else(|| error(e))?;
            }
            Err(e) => return Err(error(e)),
        }
    }
}

#[cfg(unix)]
fn available(path: &Path) -> io::Result<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    if path.as_os_str().is_empty() {
        return Err(io::ErrorKind::NotFound.into());
    }
    let c_path = CString::new(path.as_os_str().as_bytes())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `c_path` is NUL-terminated, and `stat` is written by a successful call.
    if unsafe { libc::statvfs(c_path.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the call succeeded, so it filled `stat` in.
    let stat = unsafe { stat.assume_init() };
    // The fields' types differ by platform (32-bit block counts on Apple's).
    #[allow(clippy::useless_conversion)]
    let (blocks, size) = (u64::from(stat.f_bavail), u64::from(stat.f_frsize));
    Ok(blocks.saturating_mul(size))
}

#[cfg(not(unix))]
fn available(_path: &Path) -> io::Result<u64> {
    Err(io::ErrorKind::Unsupported.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("free-bytes-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn measures_a_folder() {
        let dir = temp_dir("folder");
        let free = free_bytes(dir.to_string_lossy().into_owned()).ok().unwrap();
        assert!(free > 0, "{free}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn measures_a_missing_folder_at_its_parent() {
        let dir = temp_dir("missing");
        let missing = dir.join("offline/routing");
        let free = free_bytes(missing.to_string_lossy().into_owned())
            .ok()
            .unwrap();
        // The same filesystem, give or take what was written in between.
        let parent = free_bytes(dir.to_string_lossy().into_owned()).ok().unwrap();
        assert!((free - parent).abs() < 64 << 20, "{free} vs {parent}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn refuses_an_empty_path() {
        assert!(free_bytes(String::new()).is_err());
    }
}
