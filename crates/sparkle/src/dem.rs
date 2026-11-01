//! Elevations from `raster-dem` tiles: decoding, the border shared with neighbouring tiles,
//! and the derived data hillshade and color-relief layers draw from.
//!
//! A [`DemData`] keeps a one pixel border around the tile, as MapLibre's `DEMData` does.
//! Anything that looks at a pixel's neighbours (slopes, bilinear sampling) reads into the
//! border at the tile's edges, so once the border holds the neighbouring tiles' edge pixels
//! ([`DemData::backfill_border`]) the result is seamless across tiles. Until then the border
//! repeats the tile's own edge, which shades the edge as flat.

use crate::Error;
use crate::style::RasterDemSource;
use crate::tile::RasterImage;

/// The Earth's circumference at the equator in Web Mercator meters.
const EARTH_CIRCUMFERENCE: f64 = 40_075_016.685_578_5;

/// The elevations of one DEM tile, in meters.
#[derive(Debug, Clone, PartialEq)]
pub struct DemData {
    /// Pixels per side, without the border.
    pub dim: u32,
    /// `(dim + 2)²` values, rows top down, starting with the border's top left corner.
    elevations: Vec<f32>,
}

impl DemData {
    /// Decode a square DEM tile image with `source`'s encoding.
    pub fn from_image(image: &RasterImage, source: &RasterDemSource) -> Result<Self, Error> {
        if image.width != image.height || image.width == 0 {
            return Err(Error::Image(format!(
                "DEM tiles must be square, not {}x{}",
                image.width, image.height
            )));
        }
        let pixels = &image.pixels;
        Ok(Self::from_fn(image.width, |x, y| {
            let i = (y * image.width + x) as usize * 4;
            source.elevation([pixels[i], pixels[i + 1], pixels[i + 2]]) as f32
        }))
    }

    /// A tile of `dim`² pixels with the elevation `f(x, y)` at each, the border repeating
    /// the edge.
    pub fn from_fn(dim: u32, mut f: impl FnMut(u32, u32) -> f32) -> Self {
        let stride = dim as usize + 2;
        let mut dem = Self {
            dim,
            elevations: vec![0.0; stride * stride],
        };
        for y in 0..dim {
            for x in 0..dim {
                let i = dem.index(x as i32, y as i32);
                dem.elevations[i] = f(x, y);
            }
        }
        let d = dim as i32;
        for i in -1..=d {
            let edge = i.clamp(0, d - 1);
            dem.copy(i, -1, edge, 0);
            dem.copy(i, d, edge, d - 1);
            dem.copy(-1, i, 0, edge);
            dem.copy(d, i, d - 1, edge);
        }
        dem
    }

    fn index(&self, x: i32, y: i32) -> usize {
        let stride = self.dim as i32 + 2;
        debug_assert!((-1..=self.dim as i32).contains(&x) && (-1..=self.dim as i32).contains(&y));
        ((y + 1) * stride + x + 1) as usize
    }

    fn copy(&mut self, x: i32, y: i32, from_x: i32, from_y: i32) {
        let value = self.elevations[self.index(from_x, from_y)];
        let i = self.index(x, y);
        self.elevations[i] = value;
    }

    /// The elevation of pixel `(x, y)`, where `-1` and `dim` are the border.
    pub fn get(&self, x: i32, y: i32) -> f32 {
        let d = self.dim as i32;
        self.elevations[self.index(x.clamp(-1, d), y.clamp(-1, d))]
    }

    /// The elevation at a point of the tile, `(0, 0)` being its top left corner and `(1, 1)`
    /// its bottom right, interpolated bilinearly between pixel centers.
    pub fn elevation_at(&self, u: f64, v: f64) -> f32 {
        let dim = f64::from(self.dim);
        let (x, y) = (u * dim - 0.5, v * dim - 0.5);
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = ((x - x0) as f32, (y - y0) as f32);
        let (x0, y0) = (x0 as i32, y0 as i32);
        let row = |y| {
            let (a, b) = (self.get(x0, y), self.get(x0 + 1, y));
            a + (b - a) * fx
        };
        let (top, bottom) = (row(y0), row(y0 + 1));
        top + (bottom - top) * fy
    }

    /// The lowest and highest elevation inside the tile (not the border).
    pub fn min_max(&self) -> (f32, f32) {
        let d = self.dim as i32;
        (0..d)
            .flat_map(|y| (0..d).map(move |x| (x, y)))
            .map(|(x, y)| self.get(x, y))
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), e| {
                (lo.min(e), hi.max(e))
            })
    }

    /// Copy the edge of `neighbor`, the tile `dx, dy` (each `-1..=1`) tiles away at the same
    /// zoom, into the part of the border it touches.
    ///
    /// # Panics
    ///
    /// If the tiles' dimensions differ.
    pub fn backfill_border(&mut self, neighbor: &DemData, dx: i32, dy: i32) {
        assert_eq!(self.dim, neighbor.dim, "DEM tiles of different sizes");
        let d = self.dim as i32;
        let range = |delta: i32| match delta {
            -1 => -1..0,
            1 => d..d + 1,
            _ => 0..d,
        };
        for y in range(dy) {
            for x in range(dx) {
                let value = neighbor.get(x - dx * d, y - dy * d);
                let i = self.index(x, y);
                self.elevations[i] = value;
            }
        }
    }

    /// Every pixel including the border, `(dim + 2)²` of them, as RGBA with the elevation in
    /// the `mapbox` encoding: `-10000 + (r * 65536 + g * 256 + b) * 0.1` meters, alpha 255.
    /// This is how elevations reach a GPU that can't sample float textures.
    pub fn to_rgba(&self) -> Vec<u8> {
        self.elevations
            .iter()
            .flat_map(|&e| {
                let v = ((f64::from(e) + 10000.0) * 10.0)
                    .round()
                    .clamp(0.0, 16_777_215.0) as u32;
                [(v >> 16) as u8, (v >> 8) as u8, v as u8, 255]
            })
            .collect()
    }

    /// The slopes hillshading shades by, as MapLibre's hillshade "prepare" pass computes
    /// them for a tile at zoom `z`: rise over run to the east and to the south, at the
    /// equator's meters per pixel, exaggerated at low zooms so relief stays visible.
    ///
    /// The result is `dim`² RGBA pixels, each holding both slopes in 16 bits (see
    /// [`decode_slopes`]); MapLibre's 8 bits band visibly on smooth terrain. A renderer
    /// reading them has to interpolate between pixels itself.
    pub fn hillshade_slopes(&self, z: u8) -> RasterImage {
        let zoom = f64::from(z);
        let factor = if zoom < 2.0 {
            0.4
        } else if zoom < 4.5 {
            0.35
        } else {
            0.3
        };
        let exaggeration = if zoom < 15.0 {
            (zoom - 15.0) * factor
        } else {
            0.0
        };
        // Horn's 3x3 differences sum 8 pixel steps; dividing by 8 * meters per pixel gives
        // rise over run.
        let scale = (f64::from(self.dim)
            / (exaggeration + 3.0 + EARTH_CIRCUMFERENCE.log2() - zoom).exp2())
            as f32;
        let d = self.dim as i32;
        let mut pixels = Vec::with_capacity((self.dim * self.dim * 4) as usize);
        for y in 0..d {
            for x in 0..d {
                let e = |dx, dy| self.get(x + dx, y + dy);
                let (a, b, c) = (e(-1, -1), e(0, -1), e(1, -1));
                let (dl, f) = (e(-1, 0), e(1, 0));
                let (g, h, i) = (e(-1, 1), e(0, 1), e(1, 1));
                let deriv = [
                    ((c + f + f + i) - (a + dl + dl + g)) * scale,
                    ((g + h + h + i) - (a + b + b + c)) * scale,
                ];
                let [x, y] =
                    deriv.map(|s| ((s / 8.0 + 0.5).clamp(0.0, 1.0) * 65535.0).round() as u16);
                pixels.extend([(x >> 8) as u8, x as u8, (y >> 8) as u8, y as u8]);
            }
        }
        RasterImage {
            width: self.dim,
            height: self.dim,
            pixels,
        }
    }
}

/// The east and south slopes in a pixel of [`DemData::hillshade_slopes`]: red and green
/// hold `slope / 8 + 0.5` as a 16-bit fraction for the east, blue and alpha for the south.
pub fn decode_slopes(pixel: [u8; 4]) -> [f32; 2] {
    let [r, g, b, a] = pixel.map(f32::from);
    [r * 256.0 + g, b * 256.0 + a].map(|v| (v / 65535.0 - 0.5) * 8.0)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::style::Source;

    fn ramp(dim: u32, offset: f32) -> DemData {
        DemData::from_fn(dim, |x, y| offset + x as f32 * 10.0 + y as f32)
    }

    fn dem_source(json: serde_json::Value) -> RasterDemSource {
        let style = crate::Style::from_json(&json!({
            "version": 8, "sources": {"dem": json}, "layers": []
        }))
        .unwrap();
        match style.source("dem") {
            Some(Source::RasterDem(s)) => s.clone(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn decodes_every_encoding() {
        let pixel = |rgb: [u8; 3]| RasterImage {
            width: 1,
            height: 1,
            pixels: vec![rgb[0], rgb[1], rgb[2], 255],
        };
        let terrarium = dem_source(json!({"type": "raster-dem", "encoding": "terrarium"}));
        // 32768 + 100.5 = 128 * 256 + 100 + 128 / 256
        let dem = DemData::from_image(&pixel([128, 100, 128]), &terrarium).unwrap();
        assert_eq!(dem.get(0, 0), 100.5);
        assert_eq!(dem.get(-1, 1), 100.5);

        let mapbox = dem_source(json!({"type": "raster-dem"}));
        // (10000 + 100.5) * 10 = 101005 = 1 * 65536 + 138 * 256 + 141
        let dem = DemData::from_image(&pixel([1, 138, 141]), &mapbox).unwrap();
        assert!((dem.get(0, 0) - 100.5).abs() < 1e-3);

        let custom = dem_source(json!({"type": "raster-dem", "encoding": "custom",
            "redFactor": 2.0, "greenFactor": 0.5, "blueFactor": 0.0, "baseShift": 10.0}));
        let dem = DemData::from_image(&pixel([10, 20, 30]), &custom).unwrap();
        assert_eq!(dem.get(0, 0), 20.0);
    }

    #[test]
    fn rejects_non_square_tiles() {
        let image = RasterImage {
            width: 2,
            height: 1,
            pixels: vec![0; 8],
        };
        let source = dem_source(json!({"type": "raster-dem"}));
        assert!(DemData::from_image(&image, &source).is_err());
    }

    #[test]
    fn border_repeats_the_edge_until_backfilled() {
        let mut dem = ramp(4, 0.0);
        assert_eq!(dem.get(-1, -1), dem.get(0, 0));
        assert_eq!(dem.get(4, 2), dem.get(3, 2));
        assert_eq!(dem.get(1, 4), dem.get(1, 3));

        let right = ramp(4, 1000.0);
        dem.backfill_border(&right, 1, 0);
        for y in 0..4 {
            assert_eq!(dem.get(4, y), right.get(0, y));
        }
        // The corners come from diagonal neighbours only.
        assert_eq!(dem.get(4, -1), dem.get(3, 0));
        let below_right = ramp(4, 2000.0);
        dem.backfill_border(&below_right, 1, 1);
        assert_eq!(dem.get(4, 4), below_right.get(0, 0));

        let above = ramp(4, 3000.0);
        dem.backfill_border(&above, 0, -1);
        for x in 0..4 {
            assert_eq!(dem.get(x, -1), above.get(x, 3));
        }
        // The interior is untouched.
        assert_eq!(dem.get(3, 3), 33.0);
    }

    #[test]
    fn samples_bilinearly_between_pixel_centers() {
        let dem = ramp(4, 0.0);
        // The center of pixel (1, 2).
        assert_eq!(dem.elevation_at(1.5 / 4.0, 2.5 / 4.0), 12.0);
        // Halfway between pixels (1, 2) and (2, 3).
        assert_eq!(dem.elevation_at(0.5, 0.75), 17.5);
        assert_eq!(dem.min_max(), (0.0, 33.0));
    }

    #[test]
    fn rgba_round_trips_through_the_mapbox_encoding() {
        let dem = ramp(2, -50.25);
        let rgba = dem.to_rgba();
        assert_eq!(rgba.len(), 4 * 4 * 4);
        let mapbox = dem_source(json!({"type": "raster-dem"}));
        // Pixel (1, 1) is at row 2, column 2 of the bordered grid.
        let i = (2 * 4 + 2) * 4;
        let e = mapbox.elevation([rgba[i], rgba[i + 1], rgba[i + 2]]);
        assert!((e - f64::from(dem.get(1, 1))).abs() < 0.051, "{e}");
    }

    #[test]
    fn slopes_are_flat_on_flat_ground_and_signed_on_slopes() {
        let slopes = |image: &RasterImage, x: usize, y: usize| {
            let i = (y * image.width as usize + x) * 4;
            decode_slopes(image.pixels[i..i + 4].try_into().unwrap())
        };
        let flat = DemData::from_fn(8, |_, _| 500.0).hillshade_slopes(12);
        for p in flat.pixels.chunks(4) {
            let [x, y] = decode_slopes(p.try_into().unwrap());
            assert!(x.abs() < 1e-3 && y.abs() < 1e-3, "{p:?}");
        }

        // Rising to the east.
        let east = DemData::from_fn(8, |x, _| x as f32 * 100.0).hillshade_slopes(12);
        let [x, y] = slopes(&east, 3, 3);
        assert!(x > 0.1 && y.abs() < 1e-3, "{x} {y}");
        // At the edges the border repeats the edge pixel, so the slope halves.
        let [edge, _] = slopes(&east, 0, 3);
        assert!((edge - x / 2.0).abs() < 1e-3, "{edge} {x}");

        // Rising to the south.
        let south = DemData::from_fn(8, |_, y| y as f32 * 100.0).hillshade_slopes(12);
        let [x, y] = slopes(&south, 3, 3);
        assert!(x.abs() < 1e-3 && y > 0.1, "{x} {y}");
    }

    #[test]
    fn slopes_match_rise_over_run() {
        // At zoom 15 there's no exaggeration: a 256 pixel tile spans 40075016 / 2^15 meters,
        // so ~4.78 m per pixel. Rising 0.478 m per pixel is a slope of 0.1.
        let per_pixel = (EARTH_CIRCUMFERENCE / 32768.0 / 256.0) as f32;
        let dem = DemData::from_fn(256, |x, _| x as f32 * per_pixel * 0.1);
        let p = &dem.hillshade_slopes(15).pixels[(100 * 256 + 100) * 4..][..4];
        let [slope, _] = decode_slopes(p.try_into().unwrap());
        assert!((slope - 0.1).abs() < 1e-3, "{slope}");
    }
}
