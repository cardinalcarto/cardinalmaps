//! The fetch stage's output: `feeds.json` and the GTFS archives beside it.

use core::fmt::Write as _;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use sha2::{Digest, Sha256};

use crate::error::BuildError;

/// The file the fetch stage writes in its output directory.
pub const FEEDS_FILE: &str = "feeds.json";

/// One feed the fetch stage produced.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FeedEntry {
    /// The feed's unique ID.
    pub id: String,
    /// The feed's Transitous region.
    pub region: String,
    /// The feed's name.
    pub name: String,
    /// The archive, relative to the fetch stage's output directory.
    pub file: String,
    /// The SHA-256 of the archive.
    pub sha256: String,
    /// The size of the archive.
    pub bytes: u64,
    /// The feed's license, kept exactly as the fetch stage wrote it.
    pub license: Box<RawValue>,
    /// Where the feed came from, kept exactly as the fetch stage wrote it.
    pub source: Box<RawValue>,
}

/// `feeds.json`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FeedsFile {
    /// The Transitous commit the feeds were fetched from.
    pub transitous_commit: String,
    /// When they were fetched.
    pub fetched_at: String,
    /// The feeds.
    pub feeds: Vec<FeedEntry>,
    /// Feeds the fetch stage skipped, as it wrote them.
    #[serde(default)]
    pub skipped: Vec<Box<RawValue>>,
    /// Realtime endpoints, as the fetch stage wrote them.
    #[serde(default)]
    pub realtime: Vec<Box<RawValue>>,
}

impl FeedsFile {
    /// Reads and checks `feeds.json` in `input`.
    ///
    /// # Errors
    ///
    /// If it can't be read or parsed, two feeds share an ID, or a feed's ID or file isn't a
    /// plain relative path.
    pub fn read(input: &Path) -> Result<Self, BuildError> {
        let path = input.join(FEEDS_FILE);
        let bytes = fs::read(&path).map_err(|source| BuildError::Read {
            path: path.clone(),
            source,
        })?;
        let file: Self =
            serde_json::from_slice(&bytes).map_err(|source| BuildError::Json { path, source })?;
        let mut ids: Vec<&str> = file.feeds.iter().map(|feed| feed.id.as_str()).collect();
        ids.sort_unstable();
        if let Some(duplicate) = ids.windows(2).find(|pair| pair[0] == pair[1]) {
            return Err(BuildError::DuplicateFeedId(duplicate[0].to_owned()));
        }
        for feed in &file.feeds {
            check_feed_id(&feed.id)?;
            archive_path(input, feed)?;
        }
        Ok(file)
    }
}

/// A feed ID must be usable as one path component.
fn check_feed_id(id: &str) -> Result<(), BuildError> {
    let plain = id.chars().all(|c| !matches!(c, '/' | '\\' | '\0'))
        && !id.is_empty()
        && id != "."
        && id != "..";
    if plain {
        Ok(())
    } else {
        Err(BuildError::UnsafeFeedPath(id.to_owned()))
    }
}

/// Where a feed's archive is, refusing a path that would escape `input`.
///
/// # Errors
///
/// If the feed's `file` isn't a plain relative path.
pub fn archive_path(input: &Path, feed: &FeedEntry) -> Result<PathBuf, BuildError> {
    let relative = Path::new(&feed.file);
    if relative
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
        && relative.components().next().is_some()
    {
        Ok(input.join(relative))
    } else {
        Err(BuildError::UnsafeFeedPath(feed.file.clone()))
    }
}

/// The SHA-256 of a file, in lower-case hex, and how many bytes it holds.
///
/// # Errors
///
/// If the file can't be read.
pub fn sha256_of_file(path: &Path) -> Result<(String, u64), BuildError> {
    let read_error = |source| BuildError::Read {
        path: path.to_path_buf(),
        source,
    };
    let mut file = fs::File::open(path).map_err(read_error)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 1 << 20];
    let mut total = 0_u64;
    loop {
        let read = file.read(&mut buffer).map_err(read_error)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        total += read as u64;
    }
    Ok((hex_digest(&hasher.finalize()), total))
}

/// Lower-case hex of `bytes`.
#[must_use]
pub fn hex_digest(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut hex, byte| {
            // Writing to a string can't fail.
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feeds_json(file: &str, id: &str) -> String {
        format!(
            r#"{{"transitous_commit":"abc","fetched_at":"now","feeds":[
              {{"id":"{id}","region":"us-wa","name":"n","file":"{file}",
                "sha256":"x","bytes":1,"license":{{"url":"u","spdx":true}},"source":{{"k":["v"]}}}}],
              "skipped":[{{"id":"s","reason":"r"}}],"realtime":[]}}"#
        )
    }

    #[test]
    fn license_and_source_stay_verbatim() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join(FEEDS_FILE), feeds_json("feeds/a.zip", "a")).expect("write");
        let file = FeedsFile::read(dir.path()).expect("read");
        assert_eq!(file.feeds[0].license.get(), r#"{"url":"u","spdx":true}"#);
        assert_eq!(file.skipped[0].get(), r#"{"id":"s","reason":"r"}"#);
    }

    #[test]
    fn a_feed_escaping_the_input_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        for (file, id) in [
            ("../x.zip", "a"),
            ("/etc/x.zip", "b"),
            ("feeds/x.zip", "../c"),
        ] {
            fs::write(dir.path().join(FEEDS_FILE), feeds_json(file, id)).expect("write");
            assert!(matches!(
                FeedsFile::read(dir.path()),
                Err(BuildError::UnsafeFeedPath(_))
            ));
        }
    }

    #[test]
    fn duplicate_ids_are_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let json = feeds_json("feeds/a.zip", "a").replace(
            "\"feeds\":[",
            r#""feeds":[{"id":"a","region":"r","name":"n","file":"feeds/a.zip","sha256":"x","bytes":1,"license":{},"source":{}},"#,
        );
        fs::write(dir.path().join(FEEDS_FILE), json).expect("write");
        assert!(matches!(
            FeedsFile::read(dir.path()),
            Err(BuildError::DuplicateFeedId(_))
        ));
    }
}
