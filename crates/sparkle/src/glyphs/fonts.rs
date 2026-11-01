//! Finding fonts by the names styles use, and shaping and rasterizing with them.
//!
//! Styles name fonts as glyph servers do: `"Noto Sans Bold"` is the family plus a style.
//! [`FontLibrary`] indexes the font files in some directories by reading only their
//! `name`, `OS/2` and `fvar` tables, so that it can match such names against static fonts
//! (by full name or family plus subfamily) and against the named instances of variable
//! fonts (`NotoSans[wght].ttf` at `wght` 700 is "Noto Sans Bold"). Names that match
//! nothing exactly fall back to the closest weight and slant of the same family. Only the
//! fonts that text actually uses are read in full.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use rustybuzz::ttf_parser::{self, GlyphId, Tag};
use rustybuzz::{Direction, UnicodeBuffer, Variation};
use unicode_properties::{GeneralCategoryGroup, UnicodeGeneralCategory};

use super::{FontMetrics, GlyphKey, GlyphProvider, ONE_EM, SdfGlyph, ShapedGlyph, sdf};

type Face<'a> = rustybuzz::Face<'a>;

self_cell::self_cell!(
    /// A face that owns the font file it reads.
    struct OwnedFace {
        owner: Arc<[u8]>,
        #[covariant]
        dependent: Face,
    }
);

struct LoadedFont {
    face: OwnedFace,
    metrics: FontMetrics,
}

/// Directories fonts are usually installed in, on Linux, Android, macOS and Windows.
fn system_font_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = [
        "/usr/share/fonts",
        "/system/fonts",
        "/usr/local/share/fonts",
        "/System/Library/Fonts",
        "/Library/Fonts",
        "C:\\Windows\\Fonts",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect();
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        dirs.extend([
            home.join(".local/share/fonts"),
            home.join(".fonts"),
            home.join("Library/Fonts"),
        ]);
    }
    dirs
}

/// Local fonts, found by name and loaded when first used. See the [module docs](self).
///
/// Cheap to create: directories are only scanned the first time a font is looked up.
pub struct FontLibrary {
    dirs: Vec<PathBuf>,
    index: OnceLock<Vec<FaceEntry>>,
    state: RwLock<State>,
    glyphs: Mutex<HashMap<GlyphKey, Option<Arc<SdfGlyph>>>>,
}

#[derive(Default)]
struct State {
    /// By font id.
    fonts: Vec<Arc<LoadedFont>>,
    by_key: HashMap<FontKey, u32>,
    files: HashMap<PathBuf, Arc<[u8]>>,
    /// Each fontstack's fonts, in order.
    stacks: HashMap<String, Arc<[u32]>>,
    /// The font that draws a character no font of its stack has, if any does.
    fallback: HashMap<char, Option<u32>>,
}

/// A face of a file at some variation coordinates (`Fixed` 16.16 values).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct FontKey {
    entry: usize,
    coords: Vec<(Tag, i32)>,
}

impl FontLibrary {
    /// Fonts in `dirs` (searched recursively), then in the system's font directories.
    pub fn new(dirs: impl IntoIterator<Item = PathBuf>) -> Self {
        let mut all: Vec<PathBuf> = dirs.into_iter().collect();
        all.extend(system_font_dirs());
        Self::only(all)
    }

    /// Fonts in `dirs` only, without the system's.
    pub fn only(dirs: impl IntoIterator<Item = PathBuf>) -> Self {
        Self {
            dirs: dirs.into_iter().collect(),
            index: OnceLock::new(),
            state: RwLock::new(State::default()),
            glyphs: Mutex::new(HashMap::new()),
        }
    }

    fn index(&self) -> &[FaceEntry] {
        self.index.get_or_init(|| {
            let mut files = Vec::new();
            for (priority, dir) in self.dirs.iter().enumerate() {
                find_font_files(dir, priority, &mut files);
            }
            // Earlier directories first, then a stable order within each.
            files.sort();
            files.dedup_by(|a, b| a.1 == b.1);
            files
                .into_iter()
                .flat_map(|(priority, path)| read_entries(&path, priority).unwrap_or_default())
                .collect()
        })
    }

    /// The number of font faces found, counting each face of a collection.
    pub fn face_count(&self) -> usize {
        self.index().len()
    }

    /// The fonts of `fontstack`, resolving and loading them the first time.
    fn stack(&self, fontstack: &str) -> Arc<[u32]> {
        if let Some(stack) = self.state.read().unwrap().stacks.get(fontstack) {
            return stack.clone();
        }
        let index = self.index();
        let mut keys: Vec<FontKey> = fontstack
            .split(',')
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .filter_map(|name| resolve(index, name))
            .collect();
        if keys.is_empty()
            && let Some(key) = default_font(index)
        {
            keys.push(key);
        }
        let mut state = self.state.write().unwrap();
        let ids: Vec<u32> = keys
            .into_iter()
            .filter_map(|key| self.load(&mut state, key))
            .collect();
        let ids: Arc<[u32]> = ids.into();
        state.stacks.insert(fontstack.to_owned(), ids.clone());
        ids
    }

    fn load(&self, state: &mut State, key: FontKey) -> Option<u32> {
        if let Some(&id) = state.by_key.get(&key) {
            return Some(id);
        }
        let entry = &self.index()[key.entry];
        let data = match state.files.get(&entry.path) {
            Some(data) => data.clone(),
            None => {
                let data: Arc<[u8]> = std::fs::read(&entry.path).ok()?.into();
                state.files.insert(entry.path.clone(), data.clone());
                data
            }
        };
        let face = OwnedFace::try_new(data, |data| {
            let mut face = Face::from_slice(data, entry.face_index).ok_or(())?;
            let variations: Vec<Variation> = key
                .coords
                .iter()
                .map(|&(tag, value)| Variation {
                    tag,
                    value: value as f32 / 65536.0,
                })
                .collect();
            face.set_variations(&variations);
            Ok::<_, ()>(face)
        })
        .ok()?;
        let metrics = face.with_dependent(|_, face| {
            let scale = ONE_EM / face.units_per_em() as f32;
            FontMetrics {
                ascender: f32::from(face.ascender()) * scale,
                descender: f32::from(face.descender()) * scale,
                cap_height: face
                    .capital_height()
                    .filter(|&h| h > 0)
                    .map_or(0.7 * ONE_EM, |h| f32::from(h) * scale),
            }
        });
        let id = state.fonts.len() as u32;
        state.fonts.push(Arc::new(LoadedFont { face, metrics }));
        state.by_key.insert(key, id);
        Some(id)
    }

    fn font(&self, id: u32) -> Option<Arc<LoadedFont>> {
        self.state.read().unwrap().fonts.get(id as usize).cloned()
    }

    /// Some font with a glyph for `c`, preferring regular styles.
    fn fallback(&self, c: char) -> Option<u32> {
        if let Some(&found) = self.state.read().unwrap().fallback.get(&c) {
            return found;
        }
        let index = self.index();
        let mut order: Vec<usize> = (0..index.len()).collect();
        order.sort_by_key(|&i| {
            let e = &index[i];
            (e.priority, e.italic, (e.weight - 400.0).abs() as u32)
        });
        let entry = order.into_iter().find(|&i| index[i].has_char(c));
        let mut state = self.state.write().unwrap();
        let found = entry.and_then(|entry| {
            let e = &index[entry];
            let coords = match e.wght {
                Some([min, max]) => vec![(WGHT, to_fixed(400f32.clamp(min, max)))],
                None => Vec::new(),
            };
            self.load(&mut state, FontKey { entry, coords })
        });
        state.fallback.insert(c, found);
        found
    }

    /// The font of `stack` that draws `c`.
    fn font_for(&self, stack: &[u32], fonts: &[Arc<LoadedFont>], c: char) -> Option<u32> {
        let found = stack
            .iter()
            .zip(fonts)
            .find(|(_, f)| f.face.borrow_dependent().glyph_index(c).is_some())
            .map(|(&id, _)| id);
        found.or_else(|| self.fallback(c))
    }
}

/// Characters that belong with the one before them and must be shaped in its font.
fn continues_cluster(c: char) -> bool {
    matches!(c, '\u{200c}' | '\u{200d}' | '\u{fe00}'..='\u{fe0f}')
        || c.general_category_group() == GeneralCategoryGroup::Mark
}

impl GlyphProvider for FontLibrary {
    fn shape(&self, fontstack: &str, text: &str, rtl: bool, out: &mut Vec<ShapedGlyph>) {
        let stack = self.stack(fontstack);
        let fonts: Vec<_> = stack.iter().filter_map(|&id| self.font(id)).collect();
        if fonts.len() != stack.len() {
            return;
        }

        // Split the text into runs of one font each.
        let mut runs: Vec<(usize, usize, Option<u32>)> = Vec::new();
        for (i, c) in text.char_indices() {
            let font = match runs.last() {
                Some(&(_, _, prev)) if continues_cluster(c) || c.is_whitespace() => prev,
                _ => self.font_for(&stack, &fonts, c),
            };
            match runs.last_mut() {
                Some(run) if run.2 == font => run.1 = i + c.len_utf8(),
                _ => runs.push((i, i + c.len_utf8(), font)),
            }
        }
        // Right-to-left runs follow each other from right to left.
        if rtl {
            runs.reverse();
        }

        for (start, end, font) in runs {
            let Some(id) = font else { continue };
            let Some(font) = self.font(id) else { continue };
            let face = font.face.borrow_dependent();
            let mut buffer = UnicodeBuffer::new();
            buffer.push_str(&text[start..end]);
            buffer.set_pre_context(&text[..start]);
            buffer.set_post_context(&text[end..]);
            buffer.guess_segment_properties();
            buffer.set_direction(if rtl {
                Direction::RightToLeft
            } else {
                Direction::LeftToRight
            });
            let shaped = rustybuzz::shape(face, &[], buffer);
            let scale = ONE_EM / face.units_per_em() as f32;
            for (info, pos) in shaped.glyph_infos().iter().zip(shaped.glyph_positions()) {
                // Glyph 0 is "missing".
                if info.glyph_id == 0 {
                    continue;
                }
                out.push(ShapedGlyph {
                    key: GlyphKey {
                        font: id,
                        glyph: info.glyph_id as u16,
                    },
                    cluster: start as u32 + info.cluster,
                    advance: pos.x_advance as f32 * scale,
                    offset: [pos.x_offset as f32 * scale, -pos.y_offset as f32 * scale],
                });
            }
        }
    }

    fn glyph(&self, key: GlyphKey) -> Option<Arc<SdfGlyph>> {
        if let Some(found) = self.glyphs.lock().unwrap().get(&key) {
            return found.clone();
        }
        // Rasterize without holding the lock, so other workers aren't held up.
        let glyph = self.font(key.font).and_then(|font| {
            sdf::rasterize(font.face.borrow_dependent(), GlyphId(key.glyph)).map(Arc::new)
        });
        self.glyphs.lock().unwrap().insert(key, glyph.clone());
        glyph
    }

    fn metrics(&self, fontstack: &str) -> FontMetrics {
        self.stack(fontstack)
            .first()
            .and_then(|&id| self.font(id))
            .map_or_else(FontMetrics::default, |f| f.metrics)
    }
}

const WGHT: Tag = Tag::from_bytes(b"wght");

fn to_fixed(v: f32) -> i32 {
    (v * 65536.0).round() as i32
}

/// What the index knows about one face of a font file.
#[derive(Debug)]
struct FaceEntry {
    path: PathBuf,
    face_index: u32,
    /// Lower is searched first: the index of the directory it was found in.
    priority: usize,
    /// Normalized family names (typographic and legacy).
    families: Vec<String>,
    /// Normalized names this face answers to, with the variation coordinates that make it
    /// look like that (empty for static fonts and default instances).
    names: Vec<(String, Vec<(Tag, i32)>)>,
    /// Of the default instance.
    weight: f32,
    italic: bool,
    /// The `wght` axis range of a variable font.
    wght: Option<[f32; 2]>,
    /// Where the `cmap` table is, to read it if this face is a fallback candidate.
    cmap: Option<(u64, u32)>,
    cmap_data: OnceLock<Option<Vec<u8>>>,
}

impl FaceEntry {
    fn has_char(&self, c: char) -> bool {
        let data = self.cmap_data.get_or_init(|| {
            let (offset, len) = self.cmap?;
            let mut file = File::open(&self.path).ok()?;
            read_at(&mut file, offset, len as usize).ok()
        });
        let Some(table) = data.as_deref().and_then(ttf_parser::cmap::Table::parse) else {
            return false;
        };
        table
            .subtables
            .into_iter()
            .filter(|s| s.is_unicode())
            .any(|s| s.glyph_index(c as u32).is_some_and(|g| g.0 != 0))
    }
}

/// Lowercase letters and digits only, so `"Noto Sans-Bold"` matches `"NotoSans Bold"`.
fn normalize(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn find_font_files(dir: &Path, priority: usize, out: &mut Vec<(usize, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            find_font_files(&path, priority, out);
        } else if path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
            matches!(
                e.to_ascii_lowercase().as_str(),
                "ttf" | "otf" | "ttc" | "otc"
            )
        }) {
            out.push((priority, path));
        }
    }
}

fn read_at(file: &mut File, offset: u64, len: usize) -> io::Result<Vec<u8>> {
    file.seek(SeekFrom::Start(offset))?;
    let mut buf = vec![0; len];
    file.read_exact(&mut buf)?;
    Ok(buf)
}

fn be16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// Index every face of a font file, reading only the tables needed to name them.
fn read_entries(path: &Path, priority: usize) -> io::Result<Vec<FaceEntry>> {
    let mut file = File::open(path)?;
    let header = read_at(&mut file, 0, 12)?;
    let offsets = if &header[..4] == b"ttcf" {
        let count = be32(&header, 8).unwrap_or(0).min(256) as usize;
        let table = read_at(&mut file, 12, count * 4)?;
        (0..count)
            .filter_map(|i| be32(&table, i * 4).map(u64::from))
            .collect()
    } else {
        vec![0]
    };
    Ok(offsets
        .into_iter()
        .enumerate()
        .filter_map(|(i, offset)| read_entry(&mut file, path, offset, i as u32, priority).ok()?)
        .collect())
}

fn read_entry(
    file: &mut File,
    path: &Path,
    offset: u64,
    face_index: u32,
    priority: usize,
) -> io::Result<Option<FaceEntry>> {
    let header = read_at(file, offset, 12)?;
    let count = be16(&header, 4).unwrap_or(0) as usize;
    let records = read_at(file, offset + 12, count * 16)?;
    let mut tables = HashMap::new();
    for r in records.as_chunks::<16>().0 {
        let tag: [u8; 4] = r[..4].try_into().unwrap();
        let (Some(at), Some(len)) = (be32(r, 8), be32(r, 12)) else {
            continue;
        };
        tables.insert(tag, (u64::from(at), len));
    }
    // Bitmap-only fonts (color emoji) have nothing to make distance fields from.
    if !tables.contains_key(b"glyf")
        && !tables.contains_key(b"CFF ")
        && !tables.contains_key(b"CFF2")
    {
        return Ok(None);
    }
    let mut read = |tag: &[u8; 4]| -> Option<Vec<u8>> {
        let &(at, len) = tables.get(tag)?;
        read_at(file, at, len.min(1 << 20) as usize).ok()
    };
    let Some(name_data) = read(b"name") else {
        return Ok(None);
    };
    let os2 = read(b"OS/2");
    let fvar = read(b"fvar");
    let Some(names) = ttf_parser::name::Table::parse(&name_data) else {
        return Ok(None);
    };
    let name = |id: u16| -> Option<String> {
        let mut found = None;
        for n in names.names.into_iter().filter(|n| n.name_id == id) {
            let Some(s) = n.to_string() else { continue };
            // US English first, then whatever is there.
            if n.language_id == 0x409 || found.is_none() {
                found = Some(s);
            }
        }
        found
    };
    let Some(family) = name(1) else {
        return Ok(None);
    };
    let subfamily = name(2).unwrap_or_else(|| "Regular".into());
    let typo_family = name(16).unwrap_or_else(|| family.clone());
    let typo_subfamily = name(17).unwrap_or_else(|| subfamily.clone());

    let weight = os2
        .as_deref()
        .and_then(|t| be16(t, 4))
        .map_or(400.0, f32::from);
    let fs_selection = os2.as_deref().and_then(|t| be16(t, 62)).unwrap_or(0);
    let italic = fs_selection & 1 != 0 || normalize(&typo_subfamily).contains("italic");

    let mut entry_names = Vec::new();
    let mut add = |name: String, coords: &[(Tag, i32)]| {
        let name = normalize(&name);
        if !entry_names.iter().any(|(n, _)| n == &name) {
            entry_names.push((name, coords.to_vec()));
        }
    };
    let mut wght = None;
    let mut weight = weight;
    if let Some(fvar) = fvar.as_deref() {
        let axes = fvar_axes(fvar);
        if let Some(&(_, min, default, max)) = axes.iter().find(|a| a.0 == WGHT) {
            wght = Some([min, max]);
            weight = default;
        }
        for (subfamily_id, coords) in fvar_instances(fvar, &axes) {
            let Some(instance) = name(subfamily_id) else {
                continue;
            };
            add(format!("{typo_family} {instance}"), &coords);
            if typo_family != family {
                add(format!("{family} {instance}"), &coords);
            }
            if normalize(&instance) == "regular" {
                add(typo_family.clone(), &coords);
            }
        }
    }
    // After the named instances, which take precedence: the default instance of a variable
    // font isn't always the one its names say (Noto Sans CJK's is Thin, named "Regular").
    if let Some(full) = name(4) {
        add(full, &[]);
    }
    if let Some(ps) = name(6) {
        add(ps, &[]);
    }
    add(format!("{family} {subfamily}"), &[]);
    add(format!("{typo_family} {typo_subfamily}"), &[]);
    if normalize(&typo_subfamily) == "regular" {
        add(typo_family.clone(), &[]);
    }

    let mut families = vec![normalize(&typo_family)];
    if normalize(&family) != families[0] {
        families.push(normalize(&family));
    }
    Ok(Some(FaceEntry {
        path: path.to_owned(),
        face_index,
        priority,
        families,
        names: entry_names,
        weight,
        italic,
        wght,
        cmap: tables.get(b"cmap").copied(),
        cmap_data: OnceLock::new(),
    }))
}

fn fixed(b: &[u8], at: usize) -> Option<f32> {
    Some(be32(b, at)? as i32 as f32 / 65536.0)
}

/// `(tag, min, default, max)` of each axis of an `fvar` table.
fn fvar_axes(fvar: &[u8]) -> Vec<(Tag, f32, f32, f32)> {
    let (Some(offset), Some(count), Some(size)) = (be16(fvar, 4), be16(fvar, 8), be16(fvar, 10))
    else {
        return Vec::new();
    };
    (0..count as usize)
        .filter_map(|i| {
            let at = offset as usize + i * size as usize;
            Some((
                Tag(be32(fvar, at)?),
                fixed(fvar, at + 4)?,
                fixed(fvar, at + 8)?,
                fixed(fvar, at + 12)?,
            ))
        })
        .collect()
}

/// The named instances of an `fvar` table: subfamily name id and coordinates.
fn fvar_instances(fvar: &[u8], axes: &[(Tag, f32, f32, f32)]) -> Vec<(u16, Vec<(Tag, i32)>)> {
    let (Some(offset), Some(axis_size), Some(count), Some(size)) = (
        be16(fvar, 4),
        be16(fvar, 10),
        be16(fvar, 12),
        be16(fvar, 14),
    ) else {
        return Vec::new();
    };
    let start = offset as usize + axes.len() * axis_size as usize;
    (0..count as usize)
        .filter_map(|i| {
            let at = start + i * size as usize;
            let name_id = be16(fvar, at)?;
            let coords = axes
                .iter()
                .enumerate()
                .map(|(k, a)| Some((a.0, be32(fvar, at + 4 + 4 * k)? as i32)))
                .collect::<Option<_>>()?;
            Some((name_id, coords))
        })
        .collect()
}

/// Style words at the end of a font name, normalized, with the weight they mean (or
/// `None` for slant words).
const STYLE_WORDS: &[(&str, Option<f32>)] = &[
    ("thin", Some(100.0)),
    ("hairline", Some(100.0)),
    ("extralight", Some(200.0)),
    ("ultralight", Some(200.0)),
    ("light", Some(300.0)),
    ("regular", Some(400.0)),
    ("normal", Some(400.0)),
    ("book", Some(400.0)),
    ("roman", Some(400.0)),
    ("medium", Some(500.0)),
    ("semibold", Some(600.0)),
    ("demibold", Some(600.0)),
    ("bold", Some(700.0)),
    ("extrabold", Some(800.0)),
    ("ultrabold", Some(800.0)),
    ("black", Some(900.0)),
    ("heavy", Some(900.0)),
    ("italic", None),
    ("oblique", None),
];

/// A font name split into its normalized family, weight and slant.
fn parse_style(name: &str) -> (String, f32, bool) {
    let mut words: Vec<&str> = name.split_whitespace().collect();
    let (mut weight, mut italic) = (400.0, false);
    loop {
        // One word ("SemiBold") or two ("Semi Bold").
        let found = (1..=2).rev().find_map(|n| {
            let tail = words.len().checked_sub(n)?;
            let word = normalize(&words[tail..].concat());
            STYLE_WORDS
                .iter()
                .find(|(w, _)| *w == word)
                .map(|&(_, w)| (tail, w))
        });
        match found {
            Some((tail, w)) if tail > 0 => {
                match w {
                    Some(w) => weight = w,
                    None => italic = true,
                }
                words.truncate(tail);
            }
            _ => break,
        }
    }
    (normalize(&words.concat()), weight, italic)
}

/// The face and coordinates best matching the font `name`.
fn resolve(index: &[FaceEntry], name: &str) -> Option<FontKey> {
    let wanted = normalize(name);
    for (entry, e) in index.iter().enumerate() {
        if let Some((_, coords)) = e.names.iter().find(|(n, _)| *n == wanted) {
            return Some(FontKey {
                entry,
                coords: coords.clone(),
            });
        }
    }
    // The closest style of the family.
    let (family, weight, italic) = parse_style(name);
    let entry = index
        .iter()
        .enumerate()
        .filter(|(_, e)| e.families.contains(&family))
        .min_by_key(|(_, e)| {
            let distance = match e.wght {
                Some([min, max]) => (weight.clamp(min, max) - weight).abs(),
                None => (e.weight - weight).abs(),
            };
            (e.italic != italic, distance as u32, e.priority)
        })?
        .0;
    let coords = match index[entry].wght {
        Some([min, max]) => vec![(WGHT, to_fixed(weight.clamp(min, max)))],
        None => Vec::new(),
    };
    Some(FontKey { entry, coords })
}

/// A font for fontstacks none of whose fonts exist.
fn default_font(index: &[FaceEntry]) -> Option<FontKey> {
    [
        "Noto Sans Regular",
        "DejaVu Sans",
        "Liberation Sans Regular",
        "Arial",
    ]
    .into_iter()
    .find_map(|name| resolve(index, name))
    .or_else(|| {
        let entry = index
            .iter()
            .enumerate()
            .min_by_key(|(_, e)| (e.priority, e.italic, (e.weight - 400.0).abs() as u32))?
            .0;
        Some(FontKey {
            entry,
            coords: Vec::new(),
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOTO_DIR: &str = "/usr/share/fonts/google-noto-vf";
    const DEJAVU_DIR: &str = "/usr/share/fonts/dejavu-sans-fonts";

    /// A library of the given system directories, or `None` (skipping the test) where
    /// they aren't installed.
    fn library(dirs: &[&str]) -> Option<FontLibrary> {
        if dirs.iter().any(|d| !Path::new(d).is_dir()) {
            eprintln!("skipped: {dirs:?} not installed");
            return None;
        }
        Some(FontLibrary::only(dirs.iter().map(PathBuf::from)))
    }

    #[test]
    fn parses_style_words() {
        assert_eq!(
            parse_style("Noto Sans Bold"),
            ("notosans".into(), 700.0, false)
        );
        assert_eq!(
            parse_style("Open Sans Semi Bold Italic"),
            ("opensans".into(), 600.0, true)
        );
        assert_eq!(parse_style("Roboto"), ("roboto".into(), 400.0, false));
        // A family that is only a style word stays a family.
        assert_eq!(parse_style("Black"), ("black".into(), 400.0, false));
    }

    #[test]
    fn resolves_variable_named_instances() {
        let Some(lib) = library(&[NOTO_DIR]) else {
            return;
        };
        let index = lib.index();
        let regular = resolve(index, "Noto Sans Regular").unwrap();
        let bold = resolve(index, "Noto Sans Bold").unwrap();
        assert_eq!(regular.entry, bold.entry);
        assert!(index[bold.entry].path.ends_with("NotoSans[wght].ttf"));
        assert_eq!(bold.coords, [(WGHT, to_fixed(700.0))]);
        let italic = resolve(index, "Noto Sans Italic").unwrap();
        assert!(index[italic.entry].italic);
        // Not a named instance, but the axis covers it.
        let semi = resolve(index, "Noto Sans Demi Bold").unwrap();
        assert_eq!(semi.coords, [(WGHT, to_fixed(600.0))]);
        assert!(resolve(index, "No Such Font").is_none());
    }

    #[test]
    fn named_instances_win_over_a_thin_default_instance() {
        let Some(lib) = library(&["/usr/share/fonts/google-noto-sans-cjk-vf-fonts"]) else {
            return;
        };
        // The variable CJK font's default instance is Thin, yet its name table calls it
        // "Regular"; the fvar instance named Regular is wght 400.
        let regular = resolve(lib.index(), "Noto Sans CJK SC Regular").unwrap();
        assert_eq!(regular.coords, [(WGHT, to_fixed(400.0))]);
        let mut out = Vec::new();
        lib.shape("Noto Sans CJK SC Regular", "中", false, &mut out);
        let glyph = lib.glyph(out[0].key).unwrap();
        assert!(glyph.pixels.iter().any(|&v| v > 192), "strokes are drawn");
    }

    #[test]
    fn bold_is_wider_than_regular() {
        let Some(lib) = library(&[NOTO_DIR]) else {
            return;
        };
        let width = |stack: &str| {
            let mut out = Vec::new();
            lib.shape(stack, "Seattle", false, &mut out);
            assert_eq!(out.len(), 7);
            out.iter().map(|g| g.advance).sum::<f32>()
        };
        let (regular, bold) = (width("Noto Sans Regular"), width("Noto Sans Bold"));
        assert!(bold > regular * 1.03, "{bold} vs {regular}");
        let m = lib.metrics("Noto Sans Regular");
        assert!(m.cap_height > 15.0 && m.cap_height < 19.0, "{m:?}");
    }

    #[test]
    fn falls_back_through_the_stack_and_beyond() {
        let Some(lib) = library(&[NOTO_DIR, DEJAVU_DIR]) else {
            return;
        };
        let mut out = Vec::new();
        // Noto Sans has no Hebrew; Noto Sans Hebrew does.
        lib.shape("Missing Font, Noto Sans Regular", "aשb", false, &mut out);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].key.font, out[2].key.font);
        assert_ne!(out[0].key.font, out[1].key.font);
        assert_eq!(out.iter().map(|g| g.cluster).collect::<Vec<_>>(), [0, 1, 3]);
        // A stack of unknown fonts still draws with some default font.
        out.clear();
        lib.shape("Nothing, Nada", "ab", false, &mut out);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn shapes_right_to_left_and_rasterizes() {
        let Some(lib) = library(&[NOTO_DIR]) else {
            return;
        };
        let mut out = Vec::new();
        lib.shape("Noto Sans Arabic Regular", "سلام", true, &mut out);
        assert!(!out.is_empty());
        // Visual order: the first character is drawn last.
        assert_eq!(out.last().unwrap().cluster, 0);
        let glyph = lib.glyph(out[0].key).unwrap();
        assert!(glyph.width > 6 && glyph.height > 6);
        assert!(glyph.pixels.iter().any(|&v| v > 200));

        out.clear();
        lib.shape("Noto Sans Regular", " ", false, &mut out);
        assert_eq!(out.len(), 1);
        assert!(out[0].advance > 3.0);
        assert!(lib.glyph(out[0].key).is_none(), "spaces have no outline");
    }
}
