//! Raster and raster-dem tiles, and image sources: decoded once, then handed to each layer
//! drawing them.

use std::io::Cursor;
use std::sync::Arc;

use super::{Bucket, BucketGeometry, TileId, color_relief, hillshade};
use crate::Error;
use crate::dem::DemData;
use crate::style::{ImageSource, LayerKind, Source, Style};

/// The tile units an image source's corners are given in. Only precision depends on it.
pub const IMAGE_EXTENT: u32 = 8192;

/// A decoded tile image: RGBA, 8 bits per channel, rows top down. Alpha is straight unless
/// the owner says otherwise ([`RasterBucket::image`] is premultiplied).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RasterImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl RasterImage {
    /// Decode a PNG, JPEG or WebP image, told apart by their signatures.
    pub fn decode(data: &[u8]) -> Result<Self, Error> {
        match data {
            [0x89, b'P', b'N', b'G', ..] => crate::sprite::decode_png(data),
            [0xff, 0xd8, ..] => decode_jpeg(data),
            [
                b'R',
                b'I',
                b'F',
                b'F',
                _,
                _,
                _,
                _,
                b'W',
                b'E',
                b'B',
                b'P',
                ..,
            ] => decode_webp(data),
            _ => Err(Error::Image("not a PNG, JPEG or WebP image".into())),
        }
    }

    /// Multiply the color channels by alpha, as blending expects.
    pub fn premultiply(&mut self) {
        crate::sprite::premultiply(&mut self.pixels);
    }
}

fn decode_jpeg(data: &[u8]) -> Result<RasterImage, Error> {
    use zune_jpeg::zune_core::colorspace::ColorSpace;
    use zune_jpeg::zune_core::options::DecoderOptions;

    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA);
    let mut decoder = zune_jpeg::JpegDecoder::new_with_options(Cursor::new(data), options);
    let pixels = decoder
        .decode()
        .map_err(|e| Error::Image(format!("JPEG: {e:?}")))?;
    let info = decoder
        .info()
        .ok_or_else(|| Error::Image("JPEG without a header".into()))?;
    let (width, height) = (u32::from(info.width), u32::from(info.height));
    if pixels.len() != (width * height * 4) as usize {
        return Err(Error::Image("JPEG did not decode to RGBA".into()));
    }
    Ok(RasterImage {
        width,
        height,
        pixels,
    })
}

fn decode_webp(data: &[u8]) -> Result<RasterImage, Error> {
    let err = |e: image_webp::DecodingError| Error::Image(format!("WebP: {e}"));
    let mut decoder = image_webp::WebPDecoder::new(Cursor::new(data)).map_err(err)?;
    let (width, height) = decoder.dimensions();
    let size = decoder
        .output_buffer_size()
        .ok_or_else(|| Error::Image("WebP too large".into()))?;
    let mut buf = vec![0; size];
    decoder.read_image(&mut buf).map_err(err)?;
    let pixels = if decoder.has_alpha() {
        buf
    } else {
        buf.chunks(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect()
    };
    Ok(RasterImage {
        width,
        height,
        pixels,
    })
}

/// A corner of the quad an image is drawn on.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct RasterVertex {
    /// In tile units, `0..extent`.
    pub position: [f32; 2],
    /// `(0, 0)` is the image's top left corner, `(1, 1)` its bottom right.
    pub texcoord: [f32; 2],
}

/// What a `raster` layer draws from one tile: an image on a quad.
#[derive(Debug, Clone)]
pub struct RasterBucket {
    /// The tile the bucket was made for, which renderers fading between zoom levels use to
    /// find its ancestors.
    pub tile: TileId,
    /// Premultiplied, to upload as a texture.
    pub image: Arc<RasterImage>,
    /// The quad as a triangle strip: top left, top right, bottom left, bottom right. A tile
    /// image covers its tile; an image source's corners can be anywhere.
    pub vertices: [RasterVertex; 4],
}

impl RasterBucket {
    /// A bucket covering the whole tile, for an `extent` of the image's width.
    fn tile(tile: TileId, image: Arc<RasterImage>) -> Self {
        let e = image.width as f32;
        let corner = |x: f32, y: f32| RasterVertex {
            position: [x * e, y * e],
            texcoord: [x, y],
        };
        Self {
            tile,
            image,
            vertices: [
                corner(0.0, 0.0),
                corner(1.0, 0.0),
                corner(0.0, 1.0),
                corner(1.0, 1.0),
            ],
        }
    }
}

/// Decode a raster or raster-dem tile and make a bucket for each visible layer drawing it.
/// Returns the elevations when the tile is of the style's terrain source.
pub(crate) fn process(
    style: &Style,
    source_id: &str,
    id: TileId,
    data: &[u8],
    buckets: &mut Vec<Bucket>,
) -> Result<Option<Arc<DemData>>, Error> {
    let zoom = f32::from(id.z);
    let source = style.source(source_id);
    let mut image: Option<RasterImage> = None;
    let mut premultiplied: Option<Arc<RasterImage>> = None;
    let mut dem: Option<Arc<DemData>> = None;
    let mut slopes: Option<Arc<RasterImage>> = None;
    for (index, layer) in style.layers.iter().enumerate() {
        if layer.source.as_deref() != Some(source_id) || !layer.is_visible_at(zoom) {
            continue;
        }
        let image = match &mut image {
            Some(image) => &*image,
            None => image.insert(RasterImage::decode(data)?),
        };
        let mut dem = |dem_source| -> Result<Arc<DemData>, Error> {
            Ok(match &dem {
                Some(dem) => dem.clone(),
                None => dem
                    .insert(Arc::new(DemData::from_image(image, dem_source)?))
                    .clone(),
            })
        };
        let geometry = match (&layer.kind, source) {
            (LayerKind::Raster(_), _) => {
                let image = premultiplied.get_or_insert_with(|| {
                    let mut image = image.clone();
                    image.premultiply();
                    Arc::new(image)
                });
                Some(BucketGeometry::Raster(RasterBucket::tile(
                    id,
                    image.clone(),
                )))
            }
            (LayerKind::Hillshade(_), Some(Source::RasterDem(s))) => {
                let dem = dem(s)?;
                let slopes = slopes
                    .get_or_insert_with(|| Arc::new(dem.hillshade_slopes(id.z)))
                    .clone();
                Some(BucketGeometry::Hillshade(hillshade::bucket(
                    id, dem, slopes,
                )))
            }
            (LayerKind::ColorRelief(_), Some(Source::RasterDem(s))) => Some(
                BucketGeometry::ColorRelief(color_relief::bucket(id, dem(s)?)),
            ),
            _ => None,
        };
        if let Some(geometry) = geometry {
            buckets.push(Bucket {
                style_layer: index,
                extent: image.width,
                geometry,
            });
        }
    }
    let is_terrain = style
        .terrain
        .as_ref()
        .is_some_and(|t| t.source == source_id);
    match (source, is_terrain) {
        (Some(Source::RasterDem(s)), true) => Ok(Some(match dem {
            Some(dem) => dem,
            None => {
                let image = match image {
                    Some(image) => image,
                    None => RasterImage::decode(data)?,
                };
                Arc::new(DemData::from_image(&image, s)?)
            }
        })),
        _ => Ok(None),
    }
}

/// The tile an image source is drawn in: the deepest one containing all its corners, so
/// clipping to it cuts nothing off.
pub fn image_source_tile(source: &ImageSource) -> TileId {
    let corners = source
        .coordinates
        .map(|[lng, lat]| lng_lat_to_world(lng, lat));
    let (mut min, mut max) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for c in corners {
        for k in 0..2 {
            min[k] = min[k].min(c[k]);
            max[k] = max[k].max(c[k]);
        }
    }
    let cell = |v: f64, z: u8| {
        (v * f64::from(1u32 << z))
            .floor()
            .clamp(0.0, f64::from((1u32 << z) - 1)) as u32
    };
    let mut tile = TileId::new(0, 0, 0);
    for z in 1..=24 {
        let (x, y) = (cell(min[0], z), cell(min[1], z));
        if x != cell(max[0], z) || y != cell(max[1], z) {
            break;
        }
        tile = TileId::new(z, x, y);
    }
    tile
}

/// Decode an image source's image and make a bucket for each visible `raster` layer drawing
/// it, placed in tile `id` (normally [`image_source_tile`]).
pub(crate) fn process_image(
    style: &Style,
    source_id: &str,
    source: &ImageSource,
    id: TileId,
    data: &[u8],
    buckets: &mut Vec<Bucket>,
) -> Result<(), Error> {
    let zoom = f32::from(id.z);
    let mut image: Option<Arc<RasterImage>> = None;
    let tiles = f64::from(1u32 << id.z);
    let extent = f64::from(IMAGE_EXTENT);
    let [tl, tr, br, bl] = source.coordinates.map(|[lng, lat]| {
        let [x, y] = lng_lat_to_world(lng, lat);
        [
            ((x * tiles - f64::from(id.x)) * extent) as f32,
            ((y * tiles - f64::from(id.y)) * extent) as f32,
        ]
    });
    let vertex = |position, texcoord| RasterVertex { position, texcoord };
    let vertices = [
        vertex(tl, [0.0, 0.0]),
        vertex(tr, [1.0, 0.0]),
        vertex(bl, [0.0, 1.0]),
        vertex(br, [1.0, 1.0]),
    ];
    for (index, layer) in style.layers.iter().enumerate() {
        if layer.source.as_deref() != Some(source_id)
            || !layer.is_visible_at(zoom)
            || !matches!(layer.kind, LayerKind::Raster(_))
        {
            continue;
        }
        let image = match &image {
            Some(image) => image.clone(),
            None => {
                let mut decoded = RasterImage::decode(data)?;
                decoded.premultiply();
                image.insert(Arc::new(decoded)).clone()
            }
        };
        buckets.push(Bucket {
            style_layer: index,
            extent: IMAGE_EXTENT,
            geometry: BucketGeometry::Raster(RasterBucket {
                tile: id,
                image,
                vertices,
            }),
        });
    }
    Ok(())
}

/// Web Mercator: `(0, 0)` is the world's top left, `(1, 1)` its bottom right.
fn lng_lat_to_world(lng: f64, lat: f64) -> [f64; 2] {
    let lat = lat.clamp(-85.051_128_78, 85.051_128_78).to_radians();
    [
        lng / 360.0 + 0.5,
        0.5 - (std::f64::consts::FRAC_PI_4 + lat / 2.0).tan().ln() / (2.0 * std::f64::consts::PI),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_tile_contains_every_corner() {
        // A small rectangle in Seattle, which straddles a zoom 8 tile boundary.
        let source = ImageSource {
            url: String::new(),
            coordinates: [
                [-122.36, 47.62],
                [-122.32, 47.62],
                [-122.32, 47.60],
                [-122.36, 47.60],
            ],
        };
        let tile = image_source_tile(&source);
        assert_eq!(tile, TileId::new(7, 20, 44));
        let n = f64::from(1u32 << tile.z);
        for [lng, lat] in source.coordinates {
            let [x, y] = lng_lat_to_world(lng, lat);
            assert_eq!((x * n) as u32, tile.x);
            assert_eq!((y * n) as u32, tile.y);
        }
        // One level deeper splits it.
        let deeper = |v: f64| (v * n * 2.0) as u32;
        let [a, b] = [
            lng_lat_to_world(-122.36, 47.62),
            lng_lat_to_world(-122.32, 47.60),
        ];
        assert!(deeper(a[0]) != deeper(b[0]) || deeper(a[1]) != deeper(b[1]));
    }

    #[test]
    fn rejects_unknown_formats() {
        assert!(RasterImage::decode(b"GIF89a").is_err());
    }
}
