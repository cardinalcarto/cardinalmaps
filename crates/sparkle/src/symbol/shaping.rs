//! Laying out a label's text: line breaking, bidirectional reordering, shaping, and
//! alignment around the label's anchor, as MapLibre's `shapeText` does.
//!
//! Positions are in pixels at [`ONE_EM`] (renderers scale them by `text-size / ONE_EM`),
//! relative to the anchor, `y` down.

use std::ops::Range;

use unicode_bidi::BidiInfo;

use crate::glyphs::{GlyphKey, GlyphProvider, ONE_EM, ShapedGlyph};
use crate::style::{Color, SymbolAnchor};

/// One run of a label's text with its own style (a `format` section).
#[derive(Debug, Clone, PartialEq)]
pub struct TextSection {
    pub text: String,
    /// Comma separated font names.
    pub fontstack: String,
    /// Multiplies the text size.
    pub scale: f32,
    /// Overrides `text-color`.
    pub color: Option<Color>,
}

/// The layout properties shaping depends on, in pixels at [`ONE_EM`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapeOptions {
    /// Lines are broken to about this width; zero or less keeps lines whole.
    pub max_width: f32,
    pub line_height: f32,
    pub letter_spacing: f32,
    /// Where lines align within the block: 0 left, 0.5 center, 1 right.
    pub justify: f32,
    pub anchor: SymbolAnchor,
    /// Moves the whole block.
    pub offset: [f32; 2],
}

impl Default for ShapeOptions {
    fn default() -> Self {
        Self {
            max_width: 10.0 * ONE_EM,
            line_height: 1.2 * ONE_EM,
            letter_spacing: 0.0,
            justify: 0.5,
            anchor: SymbolAnchor::Center,
            offset: [0.0; 2],
        }
    }
}

/// A glyph placed in a label.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PositionedGlyph {
    pub key: GlyphKey,
    /// The pen position on the baseline, shaping offsets included.
    pub x: f32,
    pub y: f32,
    /// The section's scale.
    pub scale: f32,
    /// Index into the label's sections.
    pub section: u32,
}

/// A label's laid out text.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Shaping {
    pub glyphs: Vec<PositionedGlyph>,
    /// The text block's box, offset included.
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub line_count: usize,
}

/// The horizontal and vertical alignment of a box to its anchor: the fraction of its
/// width that lies left of the anchor, and of its height above it.
pub fn anchor_alignment(anchor: SymbolAnchor) -> [f32; 2] {
    use SymbolAnchor::*;
    let h = match anchor {
        Left | TopLeft | BottomLeft => 0.0,
        Right | TopRight | BottomRight => 1.0,
        _ => 0.5,
    };
    let v = match anchor {
        Top | TopLeft | TopRight => 0.0,
        Bottom | BottomLeft | BottomRight => 1.0,
        _ => 0.5,
    };
    [h, v]
}

/// MapLibre's whitespace, which takes no width when breaking lines.
fn is_whitespace(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ')
}

/// Characters after which a line may break.
fn is_breakable(c: char) -> bool {
    matches!(
        c,
        '\n' | ' '
            | '&'
            | ')'
            | '+'
            | '-'
            | '/'
            | '\u{ad}'
            | '\u{b7}'
            | '\u{200b}'
            | '\u{2010}'
            | '\u{2013}'
    )
}

/// Characters of scripts written without spaces, which may break anywhere.
pub fn allows_ideographic_breaking(c: char) -> bool {
    matches!(c as u32,
        0x2E80..=0x2FDF // CJK Radicals Supplement, Kangxi Radicals
        | 0x2FF0..=0x303F // Ideographic Description Characters, CJK Symbols and Punctuation
        | 0x3040..=0x312F // Hiragana, Katakana, Bopomofo
        | 0x31A0..=0x4DBF // Bopomofo Extended .. CJK Unified Ideographs Extension A
        | 0x4E00..=0x9FFF // CJK Unified Ideographs
        | 0xA000..=0xA4CF // Yi
        | 0xF900..=0xFAFF // CJK Compatibility Ideographs
        | 0xFE10..=0xFE1F // Vertical Forms
        | 0xFE30..=0xFE4F // CJK Compatibility Forms
        | 0xFF00..=0xFFEF // Halfwidth and Fullwidth Forms
    )
}

/// Lay out `sections` with `options`, or `None` if there is no text.
pub fn shape_text(
    sections: &[TextSection],
    options: &ShapeOptions,
    glyphs: &dyn GlyphProvider,
) -> Option<Shaping> {
    // The text with leading and trailing whitespace trimmed, and each byte's section.
    let full: String = sections.iter().map(|s| s.text.as_str()).collect();
    let trimmed = full.trim_matches(is_whitespace);
    if trimmed.is_empty() {
        return None;
    }
    let skip = full.len() - full.trim_start_matches(is_whitespace).len();
    let text = trimmed;
    let mut section_of = Vec::with_capacity(text.len());
    {
        let mut at = 0;
        for (i, s) in sections.iter().enumerate() {
            for _ in 0..s.text.len() {
                if at >= skip && at < skip + text.len() {
                    section_of.push(i as u32);
                }
                at += 1;
            }
        }
    }
    let runs_of = |range: Range<usize>| section_runs(&section_of, range);

    // Measure in logical order for line breaking.
    let mut advances = vec![0.0f32; text.len()];
    let mut scratch = Vec::new();
    for run in runs_of(0..text.len()) {
        let section = &sections[section_of[run.start] as usize];
        scratch.clear();
        glyphs.shape(&section.fontstack, &text[run.clone()], false, &mut scratch);
        for g in &scratch {
            advances[run.start + g.cluster as usize] += g.advance * section.scale;
        }
    }
    let lines = break_lines(text, &advances, options);

    let bidi = BidiInfo::new(text, None);
    let metrics = glyphs.metrics(&sections[section_of[0] as usize].fontstack);
    let [h_align, v_align] = anchor_alignment(options.anchor);

    let mut shaping = Shaping::default();
    let mut line_extents = Vec::new();
    let mut y = 0.0;
    let mut max_width: f32 = 0.0;
    for line in &lines {
        let line = trim_range(text, line.clone());
        let first = shaping.glyphs.len();
        let max_scale = runs_of(line.clone())
            .map(|r| sections[section_of[r.start] as usize].scale)
            .fold(1.0f32, f32::max);
        let height = options.line_height * max_scale;
        if line.is_empty() {
            y += height;
            continue;
        }
        let baseline = y + 0.5 * height + 0.5 * metrics.cap_height * max_scale;

        // Visual runs of one direction, then sections within them.
        let mut pieces = Vec::new();
        if bidi.has_rtl() {
            // A line may span paragraphs when separators other than newlines (which
            // always break lines) end them; each is reordered on its own.
            for para in &bidi.paragraphs {
                let start = para.range.start.max(line.start);
                let end = para.range.end.min(line.end);
                if start >= end {
                    continue;
                }
                let (levels, runs) = bidi.visual_runs(para, start..end);
                for run in runs {
                    let rtl = levels[run.start].is_rtl();
                    let mut parts: Vec<_> = runs_of(run).collect();
                    if rtl {
                        parts.reverse();
                    }
                    pieces.extend(parts.into_iter().map(|p| (p, rtl)));
                }
            }
        } else {
            pieces.extend(runs_of(line.clone()).map(|p| (p, false)));
        }

        let mut x = 0.0;
        for (piece, rtl) in pieces {
            let section_index = section_of[piece.start];
            let section = &sections[section_index as usize];
            scratch.clear();
            glyphs.shape(&section.fontstack, &text[piece], rtl, &mut scratch);
            for g in &scratch {
                let ShapedGlyph {
                    key,
                    advance,
                    offset,
                    ..
                } = *g;
                shaping.glyphs.push(PositionedGlyph {
                    key,
                    x: x + offset[0] * section.scale,
                    y: baseline + offset[1] * section.scale,
                    scale: section.scale,
                    section: section_index,
                });
                x += advance * section.scale + options.letter_spacing;
            }
        }
        let width = if shaping.glyphs.len() > first {
            x - options.letter_spacing
        } else {
            0.0
        };
        line_extents.push((first..shaping.glyphs.len(), width));
        max_width = max_width.max(width);
        y += height;
        shaping.line_count += 1;
    }
    if shaping.glyphs.is_empty() {
        return None;
    }

    // Justify each line within the block, then align the block to the anchor.
    let block_height = y;
    let shift_y = -v_align * block_height + options.offset[1];
    for (range, width) in line_extents {
        let shift_x =
            (max_width - width) * options.justify - h_align * max_width + options.offset[0];
        for g in &mut shaping.glyphs[range] {
            g.x += shift_x;
            g.y += shift_y;
        }
    }
    shaping.left = -h_align * max_width + options.offset[0];
    shaping.right = shaping.left + max_width;
    shaping.top = shift_y;
    shaping.bottom = shift_y + block_height;
    Some(shaping)
}

/// Ranges of `range` with one section each.
fn section_runs(
    section_of: &[u32],
    range: Range<usize>,
) -> impl Iterator<Item = Range<usize>> + '_ {
    let mut start = range.start;
    std::iter::from_fn(move || {
        if start >= range.end {
            return None;
        }
        let s = section_of[start];
        let end = (start..range.end)
            .find(|&i| section_of[i] != s)
            .unwrap_or(range.end);
        let run = start..end;
        start = end;
        Some(run)
    })
}

fn trim_range(text: &str, range: Range<usize>) -> Range<usize> {
    let s = &text[range.clone()];
    let start = range.start + (s.len() - s.trim_start_matches(is_whitespace).len());
    let end = range.end - (s.len() - s.trim_end_matches(is_whitespace).len());
    start..end.max(start)
}

/// A possible line break: the line before it ends at byte `index`, `x` pixels in.
struct Break {
    index: usize,
    x: f32,
    prior: Option<usize>,
    badness: f32,
}

/// Where to break `text` into lines: byte ranges. MapLibre's balanced line breaking:
/// lines aim for an equal width under `max_width`, breaking at spaces, some punctuation,
/// and between ideographs, and always at newlines.
fn break_lines(text: &str, advances: &[f32], options: &ShapeOptions) -> Vec<Range<usize>> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let spacing = options.letter_spacing;
    let width_of = |(i, c): (usize, char)| {
        if is_whitespace(c) {
            0.0
        } else {
            advances[i] + spacing
        }
    };
    let total: f32 = chars.iter().map(|&ic| width_of(ic)).sum();
    let target = if options.max_width > 0.0 {
        total / (total / options.max_width).ceil().max(1.0)
    } else {
        f32::INFINITY
    };
    let has_zero_width_spaces = text.contains('\u{200b}');

    let mut breaks: Vec<Break> = Vec::new();
    let mut x = 0.0;
    for (k, &(i, c)) in chars.iter().enumerate() {
        x += width_of((i, c));
        let Some(&(next_index, next)) = chars.get(k + 1) else {
            break;
        };
        let ideographic = allows_ideographic_breaking(c);
        if is_breakable(c) || ideographic {
            let penalty = break_penalty(c, next, ideographic && has_zero_width_spaces);
            if c == '\n' || target.is_finite() {
                let b = evaluate_break(next_index, x, target, &breaks, penalty, false);
                breaks.push(b);
            }
        }
    }
    let last = evaluate_break(text.len(), x, target, &breaks, 0.0, true);

    let mut ends = vec![last.index];
    let mut prior = last.prior;
    while let Some(p) = prior {
        ends.push(breaks[p].index);
        prior = breaks[p].prior;
    }
    ends.reverse();
    let mut start = 0;
    ends.into_iter()
        .map(|end| {
            let line = start..end;
            start = end;
            line
        })
        .collect()
}

fn break_penalty(c: char, next: char, penalizable_ideographic: bool) -> f32 {
    let mut penalty = 0.0;
    if c == '\n' {
        penalty -= 10_000.0;
    }
    if penalizable_ideographic {
        penalty += 150.0;
    }
    if c == '(' || c == '\u{ff08}' {
        penalty += 50.0;
    }
    if next == ')' || next == '\u{ff09}' {
        penalty += 50.0;
    }
    penalty
}

fn badness(width: f32, target: f32, penalty: f32, last: bool) -> f32 {
    if !target.is_finite() {
        // Without a width to aim for only the penalties (newlines) count.
        return if last { 0.0 } else { penalty.abs() * penalty };
    }
    let raggedness = (width - target).powi(2);
    if last {
        // Prefer a short last line to a long one.
        return if width < target {
            raggedness / 2.0
        } else {
            raggedness * 2.0
        };
    }
    raggedness + penalty.abs() * penalty
}

fn evaluate_break(
    index: usize,
    x: f32,
    target: f32,
    breaks: &[Break],
    penalty: f32,
    last: bool,
) -> Break {
    let mut best = Break {
        index,
        x,
        prior: None,
        badness: badness(x, target, penalty, last),
    };
    for (i, b) in breaks.iter().enumerate() {
        let candidate = badness(x - b.x, target, penalty, last) + b.badness;
        if candidate <= best.badness {
            best.prior = Some(i);
            best.badness = candidate;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::glyphs::{FontMetrics, SdfGlyph};

    /// Every character is one glyph 10 pixels wide (spaces 5), keyed by its code point.
    struct Monospace;

    impl GlyphProvider for Monospace {
        fn shape(&self, _: &str, text: &str, rtl: bool, out: &mut Vec<ShapedGlyph>) {
            let start = out.len();
            for (i, c) in text.char_indices() {
                out.push(ShapedGlyph {
                    key: GlyphKey {
                        font: 0,
                        glyph: c as u16,
                    },
                    cluster: i as u32,
                    advance: if c == ' ' { 5.0 } else { 10.0 },
                    offset: [0.0; 2],
                });
            }
            if rtl {
                out[start..].reverse();
            }
        }

        fn glyph(&self, _: GlyphKey) -> Option<Arc<SdfGlyph>> {
            None
        }

        fn metrics(&self, _: &str) -> FontMetrics {
            FontMetrics {
                ascender: 20.0,
                descender: -5.0,
                cap_height: 16.0,
            }
        }
    }

    fn section(text: &str) -> TextSection {
        TextSection {
            text: text.into(),
            fontstack: "f".into(),
            scale: 1.0,
            color: None,
        }
    }

    fn lines(text: &str, max_width: f32) -> Vec<String> {
        let advances: Vec<f32> = text
            .char_indices()
            .flat_map(|(_, c)| {
                let mut v = vec![0.0; c.len_utf8()];
                v[0] = if c == ' ' { 5.0 } else { 10.0 };
                v
            })
            .collect();
        let options = ShapeOptions {
            max_width,
            ..Default::default()
        };
        break_lines(text, &advances, &options)
            .into_iter()
            .map(|r| text[trim_range(text, r)].to_owned())
            .collect()
    }

    #[test]
    fn breaks_lines_evenly() {
        assert_eq!(lines("aaaa bbbb cccc", 1000.0), ["aaaa bbbb cccc"]);
        // 120 wide in total: two lines of 60 rather than 80 and 40.
        assert_eq!(lines("aaaa bbbb cccc", 80.0), ["aaaa bbbb", "cccc"]);
        assert_eq!(lines("aa bb cc dd ee ff", 70.0), ["aa bb cc", "dd ee ff"]);
        assert_eq!(lines("one\ntwo", 1000.0), ["one", "two"]);
        assert_eq!(lines("one\ntwo", 0.0), ["one", "two"]);
        assert_eq!(lines("unbreakable", 20.0), ["unbreakable"]);
        // Ideographs break anywhere.
        assert_eq!(lines("東京都千代田区", 40.0).len(), 2);
    }

    #[test]
    fn aligns_to_anchor_and_justifies() {
        let options = ShapeOptions {
            max_width: 0.0,
            ..Default::default()
        };
        let s = shape_text(&[section("ab\nc")], &options, &Monospace).unwrap();
        assert_eq!(s.line_count, 2);
        assert_eq!((s.left, s.right), (-10.0, 10.0));
        assert!((s.top + 28.8).abs() < 1e-4 && (s.bottom - 28.8).abs() < 1e-4);
        // Centered: "c" is in the middle of the 20 pixel block.
        assert_eq!(s.glyphs[2].x, -5.0);
        // Baselines center capitals in each line.
        assert!((s.glyphs[0].y - (-28.8 + 14.4 + 8.0)).abs() < 1e-4);

        let options = ShapeOptions {
            max_width: 0.0,
            anchor: SymbolAnchor::TopLeft,
            justify: 1.0,
            offset: [2.0, 3.0],
            ..Default::default()
        };
        let s = shape_text(&[section("  ab\nc ")], &options, &Monospace).unwrap();
        assert_eq!((s.left, s.top), (2.0, 3.0));
        assert_eq!(s.glyphs[0].x, 2.0);
        assert_eq!(s.glyphs[2].x, 12.0, "right justified");
        assert!(shape_text(&[section(" \n ")], &options, &Monospace).is_none());
    }

    #[test]
    fn letter_spacing_and_sections() {
        let options = ShapeOptions {
            letter_spacing: 2.0,
            anchor: SymbolAnchor::Left,
            ..Default::default()
        };
        let mut big = section("B");
        big.scale = 2.0;
        let s = shape_text(&[section("a"), big], &options, &Monospace).unwrap();
        assert_eq!(s.glyphs[1].x, 12.0);
        assert_eq!(s.glyphs[1].section, 1);
        assert_eq!(s.right, 32.0);
        // The line is as tall as its largest section.
        assert!((s.bottom - s.top - 2.0 * 28.8).abs() < 1e-4);
    }

    #[test]
    fn reorders_right_to_left_text() {
        let options = ShapeOptions {
            anchor: SymbolAnchor::Left,
            ..Default::default()
        };
        // Hebrew letters are drawn right to left; the digits and Latin stay in order.
        let s = shape_text(&[section("ab אב")], &options, &Monospace).unwrap();
        let order: Vec<char> = s
            .glyphs
            .iter()
            .map(|g| char::from_u32(u32::from(g.key.glyph)).unwrap())
            .collect();
        assert_eq!(order, ['a', 'b', ' ', 'ב', 'א']);
        assert!(s.glyphs.windows(2).all(|w| w[0].x < w[1].x));

        // A paragraph separator that doesn't break the line.
        let s = shape_text(&[section("א\u{2029}b")], &options, &Monospace).unwrap();
        assert_eq!(s.line_count, 1);
        assert_eq!(s.glyphs.len(), 3);
    }
}
