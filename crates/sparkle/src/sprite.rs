//! Sprites: images for icons and patterns, from local `sprite.json` + `sprite.png` pairs,
//! packed into one atlas texture.
//!
//! A style names its sprites by URL (see [`SpriteSource`](crate::style::SpriteSource));
//! sparkle never fetches, so callers map each sprite id to a local path prefix and
//! [`SpriteSheet::load`] reads `{prefix}.json` and `{prefix}.png` (or the `@2x` pair).
//! [`ImageAtlas::build`] then packs the sheets, naming images of sprites other than
//! `"default"` `"{id}:{name}"` as the spec says.

use std::collections::{BTreeMap, HashMap};
use std::io::Cursor;
use std::path::{Path, PathBuf};

use serde_json::Value as Json;

use crate::Error;
use crate::style::{ResolvedImage, SpriteSource};
use crate::tile::RasterImage;

/// One image of a sprite sheet, cut out of the sheet.
#[derive(Debug, Clone, PartialEq)]
pub struct SpriteImage {
    /// RGBA, 8 bits per channel, premultiplied alpha, rows top down.
    pub pixels: Vec<u8>,
    /// In physical pixels.
    pub width: u32,
    pub height: u32,
    pub meta: ImageMeta,
}

/// What the sprite index says about an image besides where it is.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ImageMeta {
    /// Physical pixels per logical pixel.
    pub pixel_ratio: f32,
    /// A signed distance field, recolored by `icon-color` and `icon-halo-*`.
    pub sdf: bool,
    /// Ranges of x, in image pixels, that stretch for `icon-text-fit`.
    pub stretch_x: Vec<[f32; 2]>,
    pub stretch_y: Vec<[f32; 2]>,
    /// `[left, top, right, bottom]` in image pixels: where text goes for `icon-text-fit`.
    pub content: Option<[f32; 4]>,
}

/// The images of one sprite, by name.
#[derive(Debug, Clone, Default)]
pub struct SpriteSheet {
    pub images: BTreeMap<String, SpriteImage>,
}

impl SpriteSheet {
    /// Load `{prefix}@2x.json`/`.png` if `pixel_ratio` is above 1 and they exist, otherwise
    /// `{prefix}.json`/`.png`.
    pub fn load(prefix: &Path, pixel_ratio: f32) -> Result<Self, Error> {
        let with = |suffix: &str| {
            let mut path = prefix.as_os_str().to_owned();
            path.push(suffix);
            PathBuf::from(path)
        };
        let base = if pixel_ratio > 1.0 && with("@2x.json").exists() && with("@2x.png").exists() {
            "@2x"
        } else {
            ""
        };
        let read = |ext: &str| {
            let path = with(&format!("{base}.{ext}"));
            std::fs::read(&path).map_err(|e| Error::Image(format!("{}: {e}", path.display())))
        };
        Self::from_bytes(&read("json")?, &read("png")?)
    }

    /// Parse a sprite index and cut its images out of the sheet PNG.
    pub fn from_bytes(index: &[u8], png: &[u8]) -> Result<Self, Error> {
        let index: Json = serde_json::from_slice(index)?;
        let index = index
            .as_object()
            .ok_or_else(|| Error::Image("sprite index is not an object".into()))?;
        let sheet = decode_png(png)?;
        let mut images = BTreeMap::new();
        for (name, entry) in index {
            let num = |key| entry.get(key).and_then(Json::as_f64);
            let (Some(x), Some(y), Some(width), Some(height)) =
                (num("x"), num("y"), num("width"), num("height"))
            else {
                continue;
            };
            let [x, y, width, height] = [x, y, width, height].map(|v| v.max(0.0) as u32);
            if width == 0 || height == 0 || x + width > sheet.width || y + height > sheet.height {
                continue;
            }
            let mut pixels = Vec::with_capacity((width * height * 4) as usize);
            for row in y..y + height {
                let start = ((row * sheet.width + x) * 4) as usize;
                pixels.extend_from_slice(&sheet.pixels[start..start + width as usize * 4]);
            }
            premultiply(&mut pixels);
            let stretch = |key| {
                entry
                    .get(key)
                    .and_then(Json::as_array)
                    .map(|ranges| {
                        ranges
                            .iter()
                            .filter_map(|r| {
                                Some([r.get(0)?.as_f64()? as f32, r.get(1)?.as_f64()? as f32])
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            };
            let meta = ImageMeta {
                pixel_ratio: num("pixelRatio").unwrap_or(1.0) as f32,
                sdf: entry.get("sdf").and_then(Json::as_bool).unwrap_or(false),
                stretch_x: stretch("stretchX"),
                stretch_y: stretch("stretchY"),
                content: entry.get("content").and_then(Json::as_array).and_then(|c| {
                    let c: Vec<f32> = c
                        .iter()
                        .map(|v| v.as_f64().map(|v| v as f32))
                        .collect::<Option<_>>()?;
                    c.try_into().ok()
                }),
            };
            images.insert(
                name.clone(),
                SpriteImage {
                    pixels,
                    width,
                    height,
                    meta,
                },
            );
        }
        Ok(Self { images })
    }
}

/// Where an image is in an [`ImageAtlas`].
#[derive(Debug, Clone, PartialEq)]
pub struct AtlasImage {
    /// `[x, y, width, height]` in atlas pixels, excluding the padding around it.
    pub rect: [u32; 4],
    pub meta: ImageMeta,
}

impl AtlasImage {
    /// The size in logical pixels.
    pub fn logical_size(&self) -> [f32; 2] {
        [
            self.rect[2] as f32 / self.meta.pixel_ratio,
            self.rect[3] as f32 / self.meta.pixel_ratio,
        ]
    }
}

/// Every sprite image packed into one texture.
#[derive(Debug, Clone, Default)]
pub struct ImageAtlas {
    pub width: u32,
    pub height: u32,
    /// RGBA, 8 bits per channel, premultiplied alpha, rows top down.
    pub pixels: Vec<u8>,
    images: HashMap<String, AtlasImage>,
}

/// Transparent pixels between images, so linear filtering doesn't bleed neighbours in.
pub(crate) const PADDING: u32 = 1;

impl ImageAtlas {
    /// Pack the sheets of the style's sprites, by sprite id.
    pub fn build<'a>(sheets: impl IntoIterator<Item = (&'a str, &'a SpriteSheet)>) -> Self {
        let mut images: Vec<(String, &SpriteImage)> = sheets
            .into_iter()
            .flat_map(|(id, sheet)| {
                sheet.images.iter().map(move |(name, image)| {
                    let name = if id == SpriteSource::DEFAULT_ID {
                        name.clone()
                    } else {
                        format!("{id}:{name}")
                    };
                    (name, image)
                })
            })
            .collect();
        // Tallest first packs shelves tightly; then by name for a stable layout.
        images.sort_by(|a, b| b.1.height.cmp(&a.1.height).then_with(|| a.0.cmp(&b.0)));

        let sizes: Vec<_> = images
            .iter()
            .map(|(_, i)| [i.width + 2 * PADDING, i.height + 2 * PADDING])
            .collect();
        let ([width, height], placed) = pack(&sizes);

        let mut atlas = Self {
            width,
            height,
            pixels: vec![0; (width * height * 4) as usize],
            images: HashMap::new(),
        };
        for ((name, image), [x, y]) in images.into_iter().zip(placed) {
            let (x, y) = (x + PADDING, y + PADDING);
            for row in 0..image.height {
                let src = (row * image.width * 4) as usize;
                let dst = (((y + row) * width + x) * 4) as usize;
                let len = image.width as usize * 4;
                atlas.pixels[dst..dst + len].copy_from_slice(&image.pixels[src..src + len]);
            }
            atlas.images.insert(
                name,
                AtlasImage {
                    rect: [x, y, image.width, image.height],
                    meta: image.meta.clone(),
                },
            );
        }
        atlas
    }

    /// This atlas with `added` too, on new rows below what's there. What's there stays
    /// where it is, so tiles already laid out against this atlas still draw from the
    /// new one. Images with a name the atlas has already are left out.
    pub fn with_images(&self, added: &[(String, SpriteImage)]) -> Self {
        let mut added: Vec<_> = added
            .iter()
            .filter(|(name, _)| !self.images.contains_key(name))
            .collect();
        if added.is_empty() {
            return self.clone();
        }
        added.sort_by(|a, b| b.1.height.cmp(&a.1.height).then_with(|| a.0.cmp(&b.0)));
        let widest = added.iter().map(|(_, i)| i.width + 2 * PADDING).max();
        let width = self.width.max(widest.unwrap_or(1).next_power_of_two());
        let mut packer = ShelfPacker::new(width);
        let placed: Vec<_> = added
            .iter()
            .map(|(_, i)| packer.add(i.width + 2 * PADDING, i.height + 2 * PADDING))
            .collect();
        let height = self.height + packer.height();
        let mut pixels = vec![0; (width * height * 4) as usize];
        let (old, new) = ((self.width * 4) as usize, (width * 4) as usize);
        for row in 0..self.height as usize {
            pixels[row * new..row * new + old]
                .copy_from_slice(&self.pixels[row * old..(row + 1) * old]);
        }
        let mut images = self.images.clone();
        for ((name, image), [x, y]) in added.into_iter().zip(placed) {
            let (x, y) = (x + PADDING, y + self.height + PADDING);
            for row in 0..image.height {
                let src = (row * image.width * 4) as usize;
                let dst = (((y + row) * width + x) * 4) as usize;
                let len = image.width as usize * 4;
                pixels[dst..dst + len].copy_from_slice(&image.pixels[src..src + len]);
            }
            images.insert(
                name.clone(),
                AtlasImage {
                    rect: [x, y, image.width, image.height],
                    meta: image.meta.clone(),
                },
            );
        }
        Self {
            width,
            height,
            pixels,
            images,
        }
    }

    pub fn get(&self, name: &str) -> Option<&AtlasImage> {
        self.images.get(name)
    }

    /// The image a `resolvedImage` property value names, if the atlas has it.
    pub fn resolve(&self, image: &ResolvedImage) -> Option<&AtlasImage> {
        self.get(&image.0)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.images.keys().map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.images.len()
    }

    pub fn is_empty(&self) -> bool {
        self.images.is_empty()
    }
}

/// Pack rectangles of `sizes` (sorted tallest first for the tightest result) into a
/// texture: its size and the top left corner of each rectangle.
pub(crate) fn pack(sizes: &[[u32; 2]]) -> ([u32; 2], Vec<[u32; 2]>) {
    let area: u64 = sizes.iter().map(|&[w, h]| u64::from(w * h)).sum();
    let widest = sizes.iter().map(|s| s[0]).max().unwrap_or(1);
    let width = ((area as f64).sqrt().ceil() as u32)
        .max(widest)
        .next_power_of_two();
    let mut packer = ShelfPacker::new(width);
    let placed = sizes.iter().map(|&[w, h]| packer.add(w, h)).collect();
    ([width, packer.height().max(1)], placed)
}

/// Packs rectangles into rows ("shelves") of a fixed width, growing downwards.
struct ShelfPacker {
    width: u32,
    /// `(y, height, used width)` of each shelf.
    shelves: Vec<(u32, u32, u32)>,
}

impl ShelfPacker {
    fn new(width: u32) -> Self {
        Self {
            width,
            shelves: Vec::new(),
        }
    }

    fn height(&self) -> u32 {
        self.shelves.last().map_or(0, |&(y, h, _)| y + h)
    }

    /// The top left corner for a `w` by `h` rectangle.
    fn add(&mut self, w: u32, h: u32) -> [u32; 2] {
        // The first shelf it fits on without wasting more than half the shelf's height.
        for shelf in &mut self.shelves {
            let (y, height, used) = *shelf;
            if h <= height && h * 2 > height && used + w <= self.width {
                shelf.2 += w;
                return [used, y];
            }
        }
        let y = self.height();
        self.shelves.push((y, h, w));
        [0, y]
    }
}

/// Decode a PNG to 8-bit RGBA, not premultiplied.
pub(crate) fn decode_png(data: &[u8]) -> Result<RasterImage, Error> {
    let err = |e: png::DecodingError| Error::Image(e.to_string());
    let mut decoder = png::Decoder::new(Cursor::new(data));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(err)?;
    let size = reader
        .output_buffer_size()
        .ok_or_else(|| Error::Image("PNG too large".into()))?;
    let mut buf = vec![0; size];
    let info = reader.next_frame(&mut buf).map_err(err)?;
    buf.truncate(info.buffer_size());
    let pixels = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf
            .chunks(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => buf
            .chunks(2)
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Grayscale => buf.iter().flat_map(|&v| [v, v, v, 255]).collect(),
        png::ColorType::Indexed => return Err(Error::Image("palette PNG was not expanded".into())),
    };
    Ok(RasterImage {
        width: info.width,
        height: info.height,
        pixels,
    })
}

pub(crate) fn premultiply(rgba: &mut [u8]) {
    for p in rgba.chunks_mut(4) {
        let a = u16::from(p[3]);
        for c in &mut p[..3] {
            *c = ((u16::from(*c) * a + 127) / 255) as u8;
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A sprite sheet drawn in code: a red 4x4 square, a 6x2 green bar with stretch and
    /// content fields, and a 3x3 SDF dot, at `ratio` physical pixels per logical one.
    pub fn test_sheet(ratio: u32) -> (Vec<u8>, Vec<u8>) {
        let (w, h) = (16 * ratio, 4 * ratio);
        let mut pixels = vec![0u8; (w * h * 4) as usize];
        let mut fill = |x0: u32, y0: u32, x1: u32, y1: u32, rgba: [u8; 4]| {
            for y in y0 * ratio..y1 * ratio {
                for x in x0 * ratio..x1 * ratio {
                    let i = ((y * w + x) * 4) as usize;
                    pixels[i..i + 4].copy_from_slice(&rgba);
                }
            }
        };
        fill(0, 0, 4, 4, [255, 0, 0, 255]);
        fill(4, 0, 10, 2, [0, 255, 0, 128]);
        fill(10, 0, 13, 3, [255, 255, 255, 255]);
        let mut png_bytes = Vec::new();
        let mut encoder = png::Encoder::new(&mut png_bytes, w, h);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&pixels)
            .unwrap();
        let r = ratio as f64;
        let index = serde_json::json!({
            "square": {"x": 0, "y": 0, "width": 4.0 * r, "height": 4.0 * r, "pixelRatio": ratio},
            "bar": {"x": 4.0 * r, "y": 0, "width": 6.0 * r, "height": 2.0 * r, "pixelRatio": ratio,
                    "stretchX": [[1.0 * r, 5.0 * r]], "content": [1.0 * r, 0, 5.0 * r, 2.0 * r]},
            "dot": {"x": 10.0 * r, "y": 0, "width": 3.0 * r, "height": 3.0 * r, "pixelRatio": ratio,
                    "sdf": true},
        });
        (serde_json::to_vec(&index).unwrap(), png_bytes)
    }

    fn image(width: u32, height: u32, value: u8) -> SpriteImage {
        SpriteImage {
            pixels: vec![value; (width * height * 4) as usize],
            width,
            height,
            meta: ImageMeta {
                pixel_ratio: 2.0,
                ..ImageMeta::default()
            },
        }
    }

    fn pixel(atlas: &ImageAtlas, x: u32, y: u32) -> u8 {
        atlas.pixels[((y * atlas.width + x) * 4) as usize]
    }

    fn sheet(images: &[(&str, SpriteImage)]) -> SpriteSheet {
        SpriteSheet {
            images: images
                .iter()
                .map(|(name, image)| ((*name).to_owned(), image.clone()))
                .collect(),
        }
    }

    #[test]
    fn added_images_leave_the_others_where_they_are() {
        let base = ImageAtlas::build([("default", &sheet(&[("old", image(8, 8, 7))]))]);
        let old = base.get("old").unwrap().clone();
        let more = base.with_images(&[
            ("new".to_owned(), image(6, 4, 9)),
            ("tall".to_owned(), image(4, 6, 5)),
        ]);

        assert_eq!(more.get("old").unwrap(), &old);
        assert_eq!(pixel(&more, old.rect[0], old.rect[1]), 7);
        let new = more.get("new").unwrap();
        assert!(new.rect[1] >= base.height);
        assert_eq!(new.meta.pixel_ratio, 2.0);
        assert_eq!(pixel(&more, new.rect[0], new.rect[1]), 9);
        let tall = more.get("tall").unwrap();
        assert_eq!(pixel(&more, tall.rect[0], tall.rect[1]), 5);
        assert_eq!(more.pixels.len(), (more.width * more.height * 4) as usize);
    }

    #[test]
    fn adding_a_name_it_has_changes_nothing() {
        let base = ImageAtlas::build([("default", &sheet(&[("old", image(8, 8, 7))]))]);
        let same = base.with_images(&[("old".to_owned(), image(8, 8, 1))]);
        assert_eq!((same.width, same.height), (base.width, base.height));
        assert_eq!(same.pixels, base.pixels);
    }

    #[test]
    fn images_can_go_in_an_empty_atlas_or_outgrow_a_narrow_one() {
        let first = ImageAtlas::default().with_images(&[("a".to_owned(), image(30, 30, 3))]);
        let a = first.get("a").unwrap().clone();
        assert_eq!(pixel(&first, a.rect[0], a.rect[1]), 3);

        let wider = first.with_images(&[("b".to_owned(), image(100, 10, 4))]);
        assert_eq!(wider.get("a").unwrap(), &a);
        assert_eq!(pixel(&wider, a.rect[0], a.rect[1]), 3);
        assert!(wider.width >= 102);
    }

    #[test]
    fn cuts_images_out_of_the_sheet() {
        let (index, png) = test_sheet(1);
        let sheet = SpriteSheet::from_bytes(&index, &png).unwrap();
        assert_eq!(sheet.images.len(), 3);
        let square = &sheet.images["square"];
        assert_eq!((square.width, square.height), (4, 4));
        assert_eq!(&square.pixels[..4], [255, 0, 0, 255]);
        let bar = &sheet.images["bar"];
        assert_eq!(&bar.pixels[..4], [0, 128, 0, 128], "premultiplied");
        assert_eq!(bar.meta.stretch_x, [[1.0, 5.0]]);
        assert_eq!(bar.meta.content, Some([1.0, 0.0, 5.0, 2.0]));
        assert!(sheet.images["dot"].meta.sdf);
    }

    #[test]
    fn packs_sheets_with_prefixes() {
        let (index, png) = test_sheet(1);
        let a = SpriteSheet::from_bytes(&index, &png).unwrap();
        let (index, png) = test_sheet(2);
        let b = SpriteSheet::from_bytes(&index, &png).unwrap();
        let atlas = ImageAtlas::build([("default", &a), ("hd", &b)]);
        assert_eq!(atlas.len(), 6);
        let square = atlas.resolve(&ResolvedImage("square".into())).unwrap();
        assert_eq!(square.logical_size(), [4.0, 4.0]);
        let hd = atlas.get("hd:square").unwrap();
        assert_eq!((hd.rect[2], hd.meta.pixel_ratio), (8, 2.0));
        assert_eq!(hd.logical_size(), [4.0, 4.0]);
        assert!(atlas.get("hd:missing").is_none());

        // Every image is where the atlas says, and no two overlap.
        let rects: Vec<_> = atlas.names().map(|n| atlas.get(n).unwrap().rect).collect();
        for (i, a) in rects.iter().enumerate() {
            assert!(a[0] + a[2] <= atlas.width && a[1] + a[3] <= atlas.height);
            for b in &rects[i + 1..] {
                let apart = a[0] + a[2] <= b[0]
                    || b[0] + b[2] <= a[0]
                    || a[1] + a[3] <= b[1]
                    || b[1] + b[3] <= a[1];
                assert!(apart, "{a:?} overlaps {b:?}");
            }
        }
        let [x, y, ..] = square.rect;
        let i = ((y * atlas.width + x) * 4) as usize;
        assert_eq!(&atlas.pixels[i..i + 4], [255, 0, 0, 255]);
    }

    #[test]
    fn loads_fixture_and_prefers_2x() {
        let prefix = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sprite/sprite");
        let one = SpriteSheet::load(&prefix, 1.0).unwrap();
        assert_eq!(one.images["square"].meta.pixel_ratio, 1.0);
        let two = SpriteSheet::load(&prefix, 2.0).unwrap();
        assert_eq!(two.images["square"].meta.pixel_ratio, 2.0);
        assert_eq!(two.images["square"].width, 8);
    }

    /// Regenerates `tests/fixtures/sprite`. Run with `--ignored` after changing
    /// [`test_sheet`].
    #[test]
    #[ignore]
    fn write_fixture() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sprite");
        std::fs::create_dir_all(&dir).unwrap();
        for (ratio, suffix) in [(1, ""), (2, "@2x")] {
            let (index, png) = test_sheet(ratio);
            std::fs::write(dir.join(format!("sprite{suffix}.json")), index).unwrap();
            std::fs::write(dir.join(format!("sprite{suffix}.png")), png).unwrap();
        }
    }
}
