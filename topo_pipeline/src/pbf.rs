//! Minimal `.osm.pbf` encoding, plus the raw blob access needed to splice files.
//!
//! The messages follow `fileformat.proto` and `osmformat.proto` from
//! <https://github.com/openstreetmap/OSM-binary> (MIT, Copyright (c) 2010
//! Scott A. Crosby). Only the fields this crate reads or writes are declared.

use std::io::{Read, Seek, SeekFrom, Write};

use anyhow::{Context, Result, bail};
use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use prost::Message;

#[derive(Clone, PartialEq, Message)]
pub struct BlobHeader {
    #[prost(string, required, tag = "1")]
    pub r#type: String,
    #[prost(int32, required, tag = "3")]
    pub datasize: i32,
}

#[derive(Clone, PartialEq, Message)]
pub struct Blob {
    #[prost(bytes = "vec", optional, tag = "1")]
    pub raw: Option<Vec<u8>>,
    #[prost(int32, optional, tag = "2")]
    pub raw_size: Option<i32>,
    #[prost(bytes = "vec", optional, tag = "3")]
    pub zlib_data: Option<Vec<u8>>,
}

#[derive(Clone, PartialEq, Message)]
pub struct HeaderBBox {
    #[prost(sint64, required, tag = "1")]
    pub left: i64,
    #[prost(sint64, required, tag = "2")]
    pub right: i64,
    #[prost(sint64, required, tag = "3")]
    pub top: i64,
    #[prost(sint64, required, tag = "4")]
    pub bottom: i64,
}

#[derive(Clone, PartialEq, Message)]
pub struct HeaderBlock {
    #[prost(message, optional, tag = "1")]
    pub bbox: Option<HeaderBBox>,
    #[prost(string, repeated, tag = "4")]
    pub required_features: Vec<String>,
    #[prost(string, repeated, tag = "5")]
    pub optional_features: Vec<String>,
    #[prost(string, optional, tag = "16")]
    pub writingprogram: Option<String>,
}

#[derive(Clone, PartialEq, Message)]
pub struct PrimitiveBlock {
    #[prost(message, required, tag = "1")]
    pub stringtable: StringTable,
    #[prost(message, repeated, tag = "2")]
    pub primitivegroup: Vec<PrimitiveGroup>,
}

#[derive(Clone, PartialEq, Message)]
pub struct PrimitiveGroup {
    #[prost(message, repeated, tag = "1")]
    pub nodes: Vec<Node>,
    #[prost(message, optional, tag = "2")]
    pub dense: Option<DenseNodes>,
    #[prost(message, repeated, tag = "3")]
    pub ways: Vec<Way>,
    #[prost(message, repeated, tag = "4")]
    pub relations: Vec<Relation>,
}

#[derive(Clone, PartialEq, Message)]
pub struct StringTable {
    #[prost(bytes = "vec", repeated, tag = "1")]
    pub s: Vec<Vec<u8>>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Node {
    #[prost(sint64, required, tag = "1")]
    pub id: i64,
}

#[derive(Clone, PartialEq, Message)]
pub struct DenseNodes {
    #[prost(sint64, repeated, packed = "true", tag = "1")]
    pub id: Vec<i64>,
    #[prost(sint64, repeated, packed = "true", tag = "8")]
    pub lat: Vec<i64>,
    #[prost(sint64, repeated, packed = "true", tag = "9")]
    pub lon: Vec<i64>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Way {
    #[prost(int64, required, tag = "1")]
    pub id: i64,
    #[prost(uint32, repeated, packed = "true", tag = "2")]
    pub keys: Vec<u32>,
    #[prost(uint32, repeated, packed = "true", tag = "3")]
    pub vals: Vec<u32>,
    #[prost(sint64, repeated, packed = "true", tag = "8")]
    pub refs: Vec<i64>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Relation {
    #[prost(int64, required, tag = "1")]
    pub id: i64,
}

/// What a data block holds, and the largest ID in it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BlockKind {
    Nodes(i64),
    Ways(i64),
    Relations(i64),
    Empty,
}

/// Where a blob sits in a file.
#[derive(Clone, Copy, Debug)]
pub struct BlobRef {
    /// Of the 4-byte header length that starts the blob.
    pub offset: u64,
    /// Including the length, the header and the data.
    pub len: u64,
    pub data_offset: u64,
    pub data_len: u64,
    pub is_header: bool,
}

/// Lists a file's blobs without reading their data.
pub fn scan_blobs(file: &mut (impl Read + Seek)) -> Result<Vec<BlobRef>> {
    let end = file.seek(SeekFrom::End(0))?;
    let mut offset = file.seek(SeekFrom::Start(0))?;
    let mut blobs = Vec::new();
    while offset < end {
        let mut len = [0; 4];
        file.read_exact(&mut len)?;
        let header_len = u32::from_be_bytes(len) as usize;
        let mut buf = vec![0; header_len];
        file.read_exact(&mut buf)?;
        let header = BlobHeader::decode(&buf[..]).context("decoding a blob header")?;
        let data_len = u64::try_from(header.datasize)?;
        let data_offset = offset + 4 + header_len as u64;
        blobs.push(BlobRef {
            offset,
            len: 4 + header_len as u64 + data_len,
            data_offset,
            data_len,
            is_header: header.r#type == "OSMHeader",
        });
        offset = data_offset + data_len;
        file.seek(SeekFrom::Start(offset))?;
    }
    Ok(blobs)
}

/// A blob's uncompressed contents.
pub fn read_blob(file: &mut (impl Read + Seek), blob: &BlobRef) -> Result<Vec<u8>> {
    file.seek(SeekFrom::Start(blob.data_offset))?;
    let mut data = vec![0; blob.data_len as usize];
    file.read_exact(&mut data)?;
    let blob = Blob::decode(&data[..]).context("decoding a blob")?;
    if let Some(raw) = blob.raw {
        return Ok(raw);
    }
    let Some(zlib) = blob.zlib_data else {
        bail!("unsupported blob compression: only zlib and uncompressed blobs are supported");
    };
    let mut out = Vec::with_capacity(blob.raw_size.unwrap_or(0).max(0) as usize);
    ZlibDecoder::new(&zlib[..]).read_to_end(&mut out)?;
    Ok(out)
}

/// Classifies a data block from its uncompressed contents.
pub fn block_kind(contents: &[u8]) -> Result<BlockKind> {
    let block = PrimitiveBlock::decode(contents).context("decoding a data block")?;
    let mut kind = BlockKind::Empty;
    for group in &block.primitivegroup {
        if let Some(dense) = &group.dense {
            if !dense.id.is_empty() {
                let max = dense.id.iter().sum();
                kind = BlockKind::Nodes(max);
            }
        } else if let Some(node) = group.nodes.last() {
            kind = BlockKind::Nodes(node.id);
        } else if let Some(way) = group.ways.last() {
            kind = BlockKind::Ways(way.id);
        } else if let Some(relation) = group.relations.last() {
            kind = BlockKind::Relations(relation.id);
        }
    }
    Ok(kind)
}

/// Writes one blob in the file format.
pub fn write_blob(out: &mut impl Write, kind: &str, contents: &[u8]) -> Result<()> {
    out.write_all(&encode_blob(kind, contents)?)?;
    Ok(())
}

/// One blob in the file format: the length, the header and the zlib data.
pub fn encode_blob(kind: &str, contents: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(contents)?;
    let blob = Blob {
        raw: None,
        raw_size: Some(i32::try_from(contents.len())?),
        zlib_data: Some(encoder.finish()?),
    };
    let data = blob.encode_to_vec();
    let header = BlobHeader {
        r#type: kind.to_owned(),
        datasize: i32::try_from(data.len())?,
    }
    .encode_to_vec();
    let mut out = Vec::with_capacity(4 + header.len() + data.len());
    out.extend_from_slice(&u32::try_from(header.len())?.to_be_bytes());
    out.extend_from_slice(&header);
    out.extend_from_slice(&data);
    Ok(out)
}
