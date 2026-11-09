//! Reads one GTFS CSV file at a time, by column name.

use std::collections::HashMap;
use std::io::Read;

use csv::{ByteRecord, ReaderBuilder, Trim};

use crate::compile::report::FeedReport;
use crate::compile::source::GtfsSource;
use crate::error::CompileError;

/// An open GTFS file with its header understood.
///
/// Rows may be shorter than the header, as feeds in the wild have rows that omit trailing empty
/// fields; a missing field reads as empty. Columns we don't need are ignored.
///
/// A field that isn't UTF-8 is a problem with its row. If it's an identifier, the row is dropped,
/// because two such identifiers would otherwise read as the same one. Other fields read as empty.
/// Either way the feed's report gets a warning.
pub(super) struct Table<'r> {
    reader: csv::Reader<Box<dyn Read + 'r>>,
    file: &'static str,
    columns: HashMap<String, usize>,
    identifiers: Vec<Identifier>,
}

/// A column naming an entity, which a row can't do without.
#[derive(Clone, Copy)]
struct Identifier {
    column: usize,
    may_be_empty: bool,
}

impl<'r> Table<'r> {
    /// Opens `file`, which may be absent. A file with no header counts as absent.
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
        if headers.iter().all(<[u8]>::is_empty) {
            return Ok(None);
        }
        let columns = headers
            .iter()
            .enumerate()
            .map(|(index, name)| (String::from_utf8_lossy(name).into_owned(), index))
            .collect();
        Ok(Some(Self {
            reader,
            file,
            columns,
            identifiers: Vec::new(),
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

    /// Declares a column that identifies each row, so a row without a usable value is dropped.
    pub(super) fn identifier(&mut self, column: usize) {
        self.identifiers.push(Identifier {
            column,
            may_be_empty: false,
        });
    }

    /// Declares an identifier column that GTFS lets rows leave empty.
    pub(super) fn optional_identifier(&mut self, column: Option<usize>) {
        if let Some(column) = column {
            self.identifiers.push(Identifier {
                column,
                may_be_empty: true,
            });
        }
    }

    /// Reads the next usable row into `record`, returning `false` at the end of the file.
    pub(super) fn read(
        &mut self,
        record: &mut ByteRecord,
        report: &mut FeedReport,
    ) -> Result<bool, CompileError> {
        while self
            .reader
            .read_byte_record(record)
            .map_err(|source| CompileError::Csv {
                file: self.file,
                source,
            })?
        {
            let line = record.position().map_or(0, csv::Position::line);
            let file = self.file;
            let dropped = self.identifiers.iter().any(|identifier| {
                record
                    .get(identifier.column)
                    .map_or(!identifier.may_be_empty, |bytes| {
                        core::str::from_utf8(bytes).is_err()
                            || (bytes.is_empty() && !identifier.may_be_empty)
                    })
            });
            if dropped {
                report.warn(|| format!("{file} line {line} has an unusable identifier; dropped"));
                continue;
            }
            if record
                .iter()
                .any(|bytes| core::str::from_utf8(bytes).is_err())
            {
                report.warn(|| format!("{file} line {line} has a field that is not UTF-8"));
            }
            return Ok(true);
        }
        Ok(false)
    }
}

/// A field of a row; empty when the column or the field is absent, or not UTF-8.
pub(super) fn field(record: &ByteRecord, column: Option<usize>) -> &str {
    column
        .and_then(|column| record.get(column))
        .and_then(|bytes| core::str::from_utf8(bytes).ok())
        .unwrap_or("")
}
