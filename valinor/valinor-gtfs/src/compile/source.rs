//! Where GTFS files come from: a zip archive, a directory, or memory.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Cursor, Read, Seek};
use std::path::{Path, PathBuf};

use zip::ZipArchive;
use zip::result::ZipError;

use crate::error::CompileError;

/// A source of GTFS files, opened one at a time.
pub trait GtfsSource {
    /// Opens the named file for reading, or `None` if the feed doesn't contain it.
    ///
    /// # Errors
    ///
    /// If the file exists but can't be opened.
    fn open(&mut self, name: &str) -> Result<Option<Box<dyn Read + '_>>, CompileError>;
}

/// A feed in a zip archive, the usual form of GTFS.
///
/// Files may be nested in one directory inside the archive, as some publishers do.
#[derive(Debug)]
pub struct ZipSource<R> {
    archive: ZipArchive<R>,
}

impl<R: Read + Seek> ZipSource<R> {
    /// Opens a zip archive over a reader.
    ///
    /// # Errors
    ///
    /// If the archive can't be read.
    pub fn new(reader: R) -> Result<Self, CompileError> {
        Ok(Self {
            archive: ZipArchive::new(reader)?,
        })
    }
}

impl ZipSource<File> {
    /// Opens a zip archive on disk.
    ///
    /// # Errors
    ///
    /// If the file can't be opened or the archive can't be read.
    pub fn from_path(path: &Path) -> Result<Self, CompileError> {
        let file = File::open(path).map_err(|source| CompileError::Io {
            file: path.display().to_string(),
            source,
        })?;
        Self::new(file)
    }
}

impl<R: Read + Seek> GtfsSource for ZipSource<R> {
    fn open(&mut self, name: &str) -> Result<Option<Box<dyn Read + '_>>, CompileError> {
        let suffix = format!("/{name}");
        let mut found = None;
        for path in self.archive.file_names() {
            let path = path?;
            if path == name || path.ends_with(&suffix) {
                found = Some(path.into_owned());
                break;
            }
        }
        let Some(path) = found else {
            return Ok(None);
        };
        match self.archive.by_name(&path) {
            Ok(file) => Ok(Some(Box::new(file))),
            Err(ZipError::FileNotFound) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
}

/// A feed unpacked into a directory.
#[derive(Debug, Clone)]
pub struct DirectorySource {
    directory: PathBuf,
}

impl DirectorySource {
    /// A source reading files from `directory`.
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }
}

impl GtfsSource for DirectorySource {
    fn open(&mut self, name: &str) -> Result<Option<Box<dyn Read + '_>>, CompileError> {
        let path = self.directory.join(name);
        match File::open(&path) {
            Ok(file) => Ok(Some(Box::new(file))),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(CompileError::Io {
                file: path.display().to_string(),
                source,
            }),
        }
    }
}

/// A feed held in memory, mainly for tests.
#[derive(Debug, Clone, Default)]
pub struct InMemorySource {
    files: HashMap<String, Vec<u8>>,
}

impl InMemorySource {
    /// An empty source.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds or replaces a file, returning `self` for chaining.
    #[must_use]
    pub fn with(mut self, name: &str, contents: &str) -> Self {
        self.files
            .insert(name.to_owned(), contents.as_bytes().to_vec());
        self
    }
}

impl GtfsSource for InMemorySource {
    fn open(&mut self, name: &str) -> Result<Option<Box<dyn Read + '_>>, CompileError> {
        Ok(self
            .files
            .get(name)
            .map(|contents| Box::new(Cursor::new(contents.as_slice())) as Box<dyn Read + '_>))
    }
}
