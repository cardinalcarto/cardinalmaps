//! Raw access to `.osm.pbf` files: their blobs and the protobuf messages in
//! them, for rewriting blocks without losing anything else.
//!
//! The messages follow `fileformat.proto` and `osmformat.proto` from
//! <https://github.com/openstreetmap/OSM-binary> (MIT, Copyright (c) 2010
//! Scott A. Crosby) and cover every field, so decoding and re-encoding a
//! block keeps all of its data.

use std::io::{self, Read, Write};

use anyhow::{Context, Result, bail};
use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use prost::Message;

#[derive(Clone, PartialEq, Message)]
pub struct BlobHeader {
    #[prost(string, required, tag = "1")]
    pub r#type: String,
    #[prost(bytes = "vec", optional, tag = "2")]
    pub indexdata: Option<Vec<u8>>,
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
    #[prost(bytes = "vec", optional, tag = "4")]
    pub lzma_data: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "5")]
    pub obsolete_bzip2_data: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "6")]
    pub lz4_data: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "7")]
    pub zstd_data: Option<Vec<u8>>,
}

#[derive(Clone, PartialEq, Message)]
pub struct PrimitiveBlock {
    #[prost(message, required, tag = "1")]
    pub stringtable: StringTable,
    #[prost(message, repeated, tag = "2")]
    pub primitivegroup: Vec<PrimitiveGroup>,
    #[prost(int32, optional, tag = "17", default = "100")]
    pub granularity: Option<i32>,
    #[prost(int32, optional, tag = "18", default = "1000")]
    pub date_granularity: Option<i32>,
    #[prost(int64, optional, tag = "19", default = "0")]
    pub lat_offset: Option<i64>,
    #[prost(int64, optional, tag = "20", default = "0")]
    pub lon_offset: Option<i64>,
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
    #[prost(message, repeated, tag = "5")]
    pub changesets: Vec<ChangeSet>,
}

#[derive(Clone, PartialEq, Message)]
pub struct StringTable {
    #[prost(bytes = "vec", repeated, tag = "1")]
    pub s: Vec<Vec<u8>>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Info {
    #[prost(int32, optional, tag = "1", default = "-1")]
    pub version: Option<i32>,
    #[prost(int64, optional, tag = "2")]
    pub timestamp: Option<i64>,
    #[prost(int64, optional, tag = "3")]
    pub changeset: Option<i64>,
    #[prost(int32, optional, tag = "4")]
    pub uid: Option<i32>,
    #[prost(uint32, optional, tag = "5")]
    pub user_sid: Option<u32>,
    #[prost(bool, optional, tag = "6")]
    pub visible: Option<bool>,
}

#[derive(Clone, PartialEq, Message)]
pub struct DenseInfo {
    #[prost(int32, repeated, packed = "true", tag = "1")]
    pub version: Vec<i32>,
    #[prost(sint64, repeated, packed = "true", tag = "2")]
    pub timestamp: Vec<i64>,
    #[prost(sint64, repeated, packed = "true", tag = "3")]
    pub changeset: Vec<i64>,
    #[prost(sint32, repeated, packed = "true", tag = "4")]
    pub uid: Vec<i32>,
    #[prost(sint32, repeated, packed = "true", tag = "5")]
    pub user_sid: Vec<i32>,
    #[prost(bool, repeated, packed = "true", tag = "6")]
    pub visible: Vec<bool>,
}

#[derive(Clone, PartialEq, Message)]
pub struct ChangeSet {
    #[prost(int64, required, tag = "1")]
    pub id: i64,
}

#[derive(Clone, PartialEq, Message)]
pub struct Node {
    #[prost(sint64, required, tag = "1")]
    pub id: i64,
    #[prost(uint32, repeated, packed = "true", tag = "2")]
    pub keys: Vec<u32>,
    #[prost(uint32, repeated, packed = "true", tag = "3")]
    pub vals: Vec<u32>,
    #[prost(message, optional, tag = "4")]
    pub info: Option<Info>,
    #[prost(sint64, required, tag = "8")]
    pub lat: i64,
    #[prost(sint64, required, tag = "9")]
    pub lon: i64,
}

#[derive(Clone, PartialEq, Message)]
pub struct DenseNodes {
    #[prost(sint64, repeated, packed = "true", tag = "1")]
    pub id: Vec<i64>,
    #[prost(message, optional, tag = "5")]
    pub denseinfo: Option<DenseInfo>,
    #[prost(sint64, repeated, packed = "true", tag = "8")]
    pub lat: Vec<i64>,
    #[prost(sint64, repeated, packed = "true", tag = "9")]
    pub lon: Vec<i64>,
    #[prost(int32, repeated, packed = "true", tag = "10")]
    pub keys_vals: Vec<i32>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Way {
    #[prost(int64, required, tag = "1")]
    pub id: i64,
    #[prost(uint32, repeated, packed = "true", tag = "2")]
    pub keys: Vec<u32>,
    #[prost(uint32, repeated, packed = "true", tag = "3")]
    pub vals: Vec<u32>,
    #[prost(message, optional, tag = "4")]
    pub info: Option<Info>,
    #[prost(sint64, repeated, packed = "true", tag = "8")]
    pub refs: Vec<i64>,
    #[prost(sint64, repeated, packed = "true", tag = "9")]
    pub lat: Vec<i64>,
    #[prost(sint64, repeated, packed = "true", tag = "10")]
    pub lon: Vec<i64>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Relation {
    #[prost(int64, required, tag = "1")]
    pub id: i64,
    #[prost(uint32, repeated, packed = "true", tag = "2")]
    pub keys: Vec<u32>,
    #[prost(uint32, repeated, packed = "true", tag = "3")]
    pub vals: Vec<u32>,
    #[prost(message, optional, tag = "4")]
    pub info: Option<Info>,
    #[prost(int32, repeated, packed = "true", tag = "8")]
    pub roles_sid: Vec<i32>,
    #[prost(sint64, repeated, packed = "true", tag = "9")]
    pub memids: Vec<i64>,
    /// `MemberType` values: 0 node, 1 way, 2 relation.
    #[prost(int32, repeated, packed = "true", tag = "10")]
    pub types: Vec<i32>,
}

/// One blob of a file, with its encoded header and data kept so it can be
/// written back unchanged.
pub struct RawBlob {
    pub header: BlobHeader,
    pub header_bytes: Vec<u8>,
    pub data: Vec<u8>,
}

impl RawBlob {
    /// Reads the next blob, or `None` at the end of the file.
    pub fn read(input: &mut impl Read) -> Result<Option<RawBlob>> {
        let mut len = [0; 4];
        match input.read_exact(&mut len) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e.into()),
        }
        let mut header_bytes = vec![0; u32::from_be_bytes(len) as usize];
        input.read_exact(&mut header_bytes)?;
        let header = BlobHeader::decode(&header_bytes[..]).context("decoding a blob header")?;
        let mut data = vec![0; usize::try_from(header.datasize)?];
        input.read_exact(&mut data)?;
        Ok(Some(RawBlob { header, header_bytes, data }))
    }

    pub fn write(&self, output: &mut impl Write) -> Result<()> {
        output.write_all(&u32::try_from(self.header_bytes.len())?.to_be_bytes())?;
        output.write_all(&self.header_bytes)?;
        output.write_all(&self.data)?;
        Ok(())
    }

    /// The blob's uncompressed contents.
    pub fn decompress(&self) -> Result<Vec<u8>> {
        let blob = Blob::decode(&self.data[..]).context("decoding a blob")?;
        if let Some(raw) = blob.raw {
            return Ok(raw);
        }
        let Some(zlib) = blob.zlib_data else {
            bail!("unsupported blob compression; only zlib and uncompressed blobs are supported");
        };
        let mut data = Vec::with_capacity(blob.raw_size.unwrap_or(0).max(0) as usize);
        ZlibDecoder::new(&zlib[..]).read_to_end(&mut data)?;
        Ok(data)
    }

    /// A zlib-compressed blob of `contents`, of the same type as `self`.
    pub fn with_contents(&self, contents: &[u8]) -> Result<RawBlob> {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(contents)?;
        let blob = Blob {
            raw_size: Some(i32::try_from(contents.len())?),
            zlib_data: Some(encoder.finish()?),
            ..Blob::default()
        };
        let data = blob.encode_to_vec();
        let header = BlobHeader { datasize: i32::try_from(data.len())?, ..self.header.clone() };
        Ok(RawBlob { header_bytes: header.encode_to_vec(), header, data })
    }
}
