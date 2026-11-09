//! Reads one GTFS CSV file at a time, by column name.

use std::collections::HashMap;
use std::io::Read;

use csv::{ByteRecord, ReaderBuilder, Trim};

use crate::compile::source::GtfsSource;
use crate::error::CompileError;

/// An open GTFS file with its header understood.
///
/// Rows may be shorter than the header, as feeds in the wild have rows that omit trailing empty
/// fields; a missing field reads as empty. Columns we don't need are ignored.
pub(super) struct Table<'r> {
    reader: csv::Reader<Box<dyn Read + 'r>>,
    file: &'static str,
    columns: HashMap<String, usize>,
}

impl<'r> Table<'r> {
    /// Opens `file`, which may be absent.
    pub(super) fn open(
        source: &'r mut dyn GtfsSource,
        file: &'static str,
    ) -> Result<Option<Self>, CompileError> {
        let Some(input) = source.open(file)? else {
            return Ok(None);
        };
        let mut reader = ReaderBuilder::new()
            .flexible(true)
            .trim(Trim::All)
            .from_reader(input);
        let headers = reader
            .byte_headers()
            .map_err(|source| CompileError::Csv { file, source })?;
        let columns = headers
            .iter()
            .enumerate()
            .map(|(index, name)| (String::from_utf8_lossy(name).into_owned(), index))
            .collect();
        Ok(Some(Self {
            reader,
            file,
            columns,
        }))
    }

    /// Opens `file`, which must be present.
    pub(super) fn require(
        source: &'r mut dyn GtfsSource,
        file: &'static str,
    ) -> Result<Self, CompileError> {
        Self::open(source, file)?.ok_or(CompileError::MissingFile(file))
    }

    /// The position of a column the file must have.
    pub(super) fn column(&self, name: &'static str) -> Result<usize, CompileError> {
        self.columns
            .get(name)
            .copied()
            .ok_or(CompileError::MissingColumn {
                file: self.file,
                column: name,
            })
    }

    /// The position of a column the file may have.
    pub(super) fn optional(&self, name: &str) -> Option<usize> {
        self.columns.get(name).copied()
    }

    /// Reads the next row into `record`, returning `false` at the end of the file.
    pub(super) fn read(&mut self, record: &mut ByteRecord) -> Result<bool, CompileError> {
        self.reader
            .read_byte_record(record)
            .map_err(|source| CompileError::Csv {
                file: self.file,
                source,
            })
    }
}

/// A field of a row; empty when the column or the field is absent, or not UTF-8.
pub(super) fn field(record: &ByteRecord, column: Option<usize>) -> &str {
    column
        .and_then(|column| record.get(column))
        .and_then(|bytes| core::str::from_utf8(bytes).ok())
        .unwrap_or("")
}
