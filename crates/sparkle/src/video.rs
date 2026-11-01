//! Video sources: a video stretched over four corners of the map, like an image source.
//!
//! Decoding isn't implemented: no pure-Rust decoder for the formats browsers play (H.264,
//! VP8/VP9, AV1 in MP4 or WebM) is among this crate's dependencies, and binding a system
//! one (FFmpeg) would tie the library to native code that a Flutter build can't assume.
//! [`VideoFrames::open`] resolves the source's local file and then fails with
//! [`Error::Unsupported`], so callers get a clear message instead of a silent blank.

use std::path::{Path, PathBuf};

use crate::Error;
use crate::style::VideoSource;

/// Decoded frames of a video source, for a renderer to show as a texture.
#[derive(Debug)]
pub struct VideoFrames {
    pub path: PathBuf,
    /// `[lng, lat]` of the top left, top right, bottom right and bottom left corners.
    pub coordinates: [[f64; 2]; 4],
}

impl VideoFrames {
    /// Open the first of the source's `urls` that is a local file (relative paths are
    /// relative to `base_dir`).
    pub fn open(source: &VideoSource, base_dir: Option<&Path>) -> Result<Self, Error> {
        let path = source
            .urls
            .iter()
            .map(|url| {
                let path = Path::new(url.strip_prefix("file://").unwrap_or(url));
                match base_dir {
                    Some(base) if path.is_relative() => base.join(path),
                    _ => path.to_owned(),
                }
            })
            .find(|path| path.is_file())
            .ok_or_else(|| {
                Error::Unsupported(format!(
                    "video source: none of {:?} is a local file",
                    source.urls
                ))
            })?;
        Err(Error::Unsupported(format!(
            "video source {}: decoding video is not implemented",
            path.display()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_unsupported() {
        let source = VideoSource {
            urls: vec!["Cargo.toml".into()],
            coordinates: [[0.0; 2]; 4],
        };
        let err =
            VideoFrames::open(&source, Some(Path::new(env!("CARGO_MANIFEST_DIR")))).unwrap_err();
        assert!(err.to_string().contains("not implemented"), "{err}");
        let missing = VideoSource {
            urls: vec!["missing.mp4".into()],
            coordinates: [[0.0; 2]; 4],
        };
        assert!(VideoFrames::open(&missing, None).is_err());
    }
}
