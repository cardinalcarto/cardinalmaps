//! Assembles the sections of a compiled file.

use zerocopy::IntoBytes;
use zerocopy::little_endian::{U32, U64};

use crate::format::layout::{
    FORMAT_VERSION, FileHeader, MAGIC, SectionEntry, SectionId, checksum_of,
};

/// Accumulates section bytes in any order, and writes the file in the fixed section order.
#[derive(Debug, Default)]
pub(crate) struct FileWriter {
    sections: Vec<(SectionId, Vec<u8>)>,
}

impl FileWriter {
    pub(crate) fn new() -> Self {
        Self {
            sections: Vec::with_capacity(SectionId::ALL.len()),
        }
    }

    /// Sets the contents of a section from a table of records.
    pub(crate) fn table<T: DeriveTable>(&mut self, id: SectionId, rows: &[T]) {
        self.sections.push((id, rows.as_bytes().to_vec()));
    }

    /// Sets the contents of a section from raw bytes.
    pub(crate) fn bytes(&mut self, id: SectionId, bytes: Vec<u8>) {
        self.sections.push((id, bytes));
    }

    /// Writes header, directory, and sections, then fills in the checksum.
    pub(crate) fn finish(mut self, boardable_stops: u32) -> Vec<u8> {
        self.sections.sort_by_key(|(id, _)| *id as u32);
        debug_assert_eq!(self.sections.len(), SectionId::ALL.len());

        let header_len = core::mem::size_of::<FileHeader>();
        let directory_len = SectionId::ALL.len() * core::mem::size_of::<SectionEntry>();
        let payload: usize = self.sections.iter().map(|(_, b)| b.len()).sum();
        let mut out = vec![0u8; header_len + directory_len];
        out.reserve(payload);

        let mut offset = (header_len + directory_len) as u64;
        let mut entries = Vec::with_capacity(SectionId::ALL.len());
        for (id, bytes) in &self.sections {
            entries.push(SectionEntry {
                id: U32::new(*id as u32),
                _reserved: U32::new(0),
                offset: U64::new(offset),
                byte_len: U64::new(bytes.len() as u64),
            });
            offset += bytes.len() as u64;
        }
        for (_, bytes) in &self.sections {
            out.extend_from_slice(bytes);
        }

        let header = FileHeader {
            magic: MAGIC,
            version: U32::new(FORMAT_VERSION),
            checksum: U32::new(0),
            file_len: U64::new(offset),
            section_count: U32::new(u32::try_from(SectionId::ALL.len()).unwrap_or(u32::MAX)),
            boardable_stops: U32::new(boardable_stops),
        };
        out[..header_len].copy_from_slice(header.as_bytes());
        out[header_len..header_len + directory_len].copy_from_slice(entries.as_slice().as_bytes());

        let checksum = checksum_of(&out);
        out[12..16].copy_from_slice(&checksum.to_le_bytes());
        out
    }
}

/// A table record that can be written as bytes.
pub(crate) trait DeriveTable: IntoBytes + zerocopy::Immutable {}
impl<T: IntoBytes + zerocopy::Immutable> DeriveTable for T {}
