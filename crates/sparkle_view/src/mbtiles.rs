//! Reading tiles out of an MBTiles (SQLite) file.

use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use sparkle::TileId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileFormat {
    /// MapLibre Tiles, which sparkle processes directly.
    Mlt,
    /// Mapbox Vector Tiles, converted to MLT on load so existing tilesets can be viewed.
    Mvt,
    /// Encoded images (`png`, `jpg`, `webp`), for raster and raster-dem sources.
    Image,
}

impl TileFormat {
    /// The format an MBTiles `format` or a TileJSON `format` names.
    pub fn from_name(name: &str) -> Result<Self> {
        Ok(match name {
            "mlt" | "application/vnd.maplibre-vector-tile" => Self::Mlt,
            "pbf" | "mvt" | "application/x-protobuf" | "application/vnd.mapbox-vector-tile" => {
                Self::Mvt
            }
            "png" | "jpg" | "jpeg" | "webp" => Self::Image,
            image if image.starts_with("image/") => Self::Image,
            other => bail!("unsupported tile format {other:?}"),
        })
    }
}

/// `data`, decompressed if it's gzipped.
pub fn decompress(data: Vec<u8>) -> Result<Vec<u8>> {
    if !data.starts_with(&[0x1f, 0x8b]) {
        return Ok(data);
    }
    let mut out = Vec::with_capacity(data.len() * 4);
    flate2::read::GzDecoder::new(&data[..]).read_to_end(&mut out)?;
    Ok(out)
}

#[derive(Debug, Clone)]
pub struct Metadata {
    pub format: TileFormat,
    pub min_zoom: u8,
    pub max_zoom: u8,
    /// `(longitude, latitude, zoom)`
    pub center: Option<(f64, f64, f64)>,
    /// `[west, south, east, north]`
    pub bounds: Option<[f64; 4]>,
}

pub struct MbTiles {
    conn: Connection,
}

impl MbTiles {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .with_context(|| format!("opening {}", path.display()))?;
        Ok(Self { conn })
    }

    pub fn metadata(&self) -> Result<Metadata> {
        let mut stmt = self.conn.prepare("SELECT name, value FROM metadata")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut format = None;
        let (mut min_zoom, mut max_zoom) = (0, 14);
        let (mut center, mut bounds) = (None, None);
        for row in rows {
            let (name, value) = row?;
            let numbers = || {
                value
                    .split(',')
                    .map(|v| v.trim().parse::<f64>())
                    .collect::<Result<Vec<_>, _>>()
            };
            match name.as_str() {
                "format" => format = Some(TileFormat::from_name(&value)?),
                "minzoom" => min_zoom = value.parse()?,
                "maxzoom" => max_zoom = value.parse()?,
                "center" => {
                    center = numbers()
                        .ok()
                        .and_then(|v| Some((*v.first()?, *v.get(1)?, *v.get(2)?)))
                }
                "bounds" => bounds = numbers().ok().and_then(|v| v.try_into().ok()),
                _ => {}
            }
        }
        Ok(Metadata {
            format: format.context("metadata has no `format`")?,
            min_zoom,
            max_zoom,
            center,
            bounds,
        })
    }

    /// The tile's bytes, decompressed, or `None` if the tileset doesn't have it.
    pub fn tile(&self, id: TileId) -> Result<Option<Vec<u8>>> {
        // MBTiles rows count from the bottom (TMS), XYZ tile ids from the top.
        let row = (1u32 << id.z) - 1 - id.y;
        let data: Option<Vec<u8>> = self
            .conn
            .prepare_cached(
                "SELECT tile_data FROM tiles WHERE zoom_level = ?1 AND tile_column = ?2 AND tile_row = ?3",
            )?
            .query_row((id.z, id.x, row), |r| r.get(0))
            .optional()?;
        data.map(decompress).transpose()
    }
}
