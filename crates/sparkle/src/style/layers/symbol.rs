//! `symbol`: icons and text labels, placed with collision detection.

use super::{layer, properties};
use crate::paint::PaintProperty;
use crate::style::{
    Alignment, Color, Formatted, IconTextFit, Overlap, Padding, ResolvedImage, SymbolAnchor,
    SymbolPlacement, SymbolZOrder, TextJustify, TextRotationAlignment, TextTransform,
    TranslateAnchor, VariableAnchorOffsetCollection, WritingMode,
};

properties! {
    pub struct SymbolLayout {
        symbol_placement: SymbolPlacement = "symbol-placement",
        /// In pixels, between symbols along a line.
        symbol_spacing: f32 = "symbol-spacing",
        symbol_avoid_edges: bool = "symbol-avoid-edges",
        symbol_sort_key: Option<f32> = "symbol-sort-key",
        symbol_z_order: SymbolZOrder = "symbol-z-order",
        /// Not in the spec: with point placement, place labels whose icon is rarer in the
        /// tile first, after `symbol-sort-key`, so they win collisions.
        sort_by_rarity: bool = "cardinal-sort-by-rarity",

        icon_allow_overlap: bool = "icon-allow-overlap",
        /// Takes precedence over `icon-allow-overlap` when set.
        icon_overlap: Option<Overlap> = "icon-overlap",
        icon_ignore_placement: bool = "icon-ignore-placement",
        icon_optional: bool = "icon-optional",
        icon_rotation_alignment: Alignment = "icon-rotation-alignment",
        icon_size: f32 = "icon-size",
        icon_text_fit: IconTextFit = "icon-text-fit",
        /// `[top, right, bottom, left]` in pixels.
        icon_text_fit_padding: [f32; 4] = "icon-text-fit-padding",
        icon_image: Option<ResolvedImage> = "icon-image",
        /// In degrees, clockwise.
        icon_rotate: f32 = "icon-rotate",
        icon_padding: Padding = "icon-padding",
        icon_keep_upright: bool = "icon-keep-upright",
        /// In pixels, scaled by `icon-size`.
        icon_offset: [f32; 2] = "icon-offset",
        icon_anchor: SymbolAnchor = "icon-anchor",
        icon_pitch_alignment: Alignment = "icon-pitch-alignment",

        text_pitch_alignment: Alignment = "text-pitch-alignment",
        text_rotation_alignment: TextRotationAlignment = "text-rotation-alignment",
        text_field: Formatted = "text-field",
        /// Font names, the first with a glyph for a character wins.
        text_font: Vec<String> = "text-font",
        /// In pixels.
        text_size: f32 = "text-size",
        /// In ems.
        text_max_width: f32 = "text-max-width",
        /// In ems.
        text_line_height: f32 = "text-line-height",
        /// In ems.
        text_letter_spacing: f32 = "text-letter-spacing",
        text_justify: TextJustify = "text-justify",
        /// In ems.
        text_radial_offset: f32 = "text-radial-offset",
        text_variable_anchor: Vec<SymbolAnchor> = "text-variable-anchor",
        text_variable_anchor_offset: Option<VariableAnchorOffsetCollection> = "text-variable-anchor-offset",
        text_anchor: SymbolAnchor = "text-anchor",
        /// In degrees, between adjacent characters of text along a line.
        text_max_angle: f32 = "text-max-angle",
        text_writing_mode: Vec<WritingMode> = "text-writing-mode",
        /// In degrees, clockwise.
        text_rotate: f32 = "text-rotate",
        /// In pixels.
        text_padding: f32 = "text-padding",
        text_keep_upright: bool = "text-keep-upright",
        text_transform: TextTransform = "text-transform",
        /// In ems.
        text_offset: [f32; 2] = "text-offset",
        text_allow_overlap: bool = "text-allow-overlap",
        /// Takes precedence over `text-allow-overlap` when set.
        text_overlap: Option<Overlap> = "text-overlap",
        text_ignore_placement: bool = "text-ignore-placement",
        text_optional: bool = "text-optional",
    }
}

properties! {
    pub struct SymbolPaint {
        icon_opacity: f32 = "icon-opacity",
        /// Only applies to SDF icons.
        icon_color: Color = "icon-color",
        icon_halo_color: Color = "icon-halo-color",
        /// In pixels.
        icon_halo_width: f32 = "icon-halo-width",
        /// In pixels.
        icon_halo_blur: f32 = "icon-halo-blur",
        /// In pixels.
        icon_translate: [f32; 2] = "icon-translate",
        icon_translate_anchor: TranslateAnchor = "icon-translate-anchor",
        text_opacity: f32 = "text-opacity",
        text_color: Color = "text-color",
        text_halo_color: Color = "text-halo-color",
        /// In pixels.
        text_halo_width: f32 = "text-halo-width",
        /// In pixels.
        text_halo_blur: f32 = "text-halo-blur",
        /// In pixels.
        text_translate: [f32; 2] = "text-translate",
        text_translate_anchor: TranslateAnchor = "text-translate-anchor",
    }
}

layer!(SymbolLayer = "symbol", SymbolLayout, SymbolPaint);

impl SymbolLayer {
    /// The properties the text shader reads, by their names in WGSL. The size is a layout
    /// property, but the shader scales glyphs with it like a paint attribute.
    pub fn text_attributes(&self) -> [(&'static str, &dyn PaintProperty); 6] {
        let (l, p) = (&self.layout, &self.paint);
        [
            ("color", &p.text_color),
            ("halo_color", &p.text_halo_color),
            ("halo_width", &p.text_halo_width),
            ("halo_blur", &p.text_halo_blur),
            ("opacity", &p.text_opacity),
            ("size", &l.text_size),
        ]
    }

    /// The properties the icon shader reads. `text_size` scales icons fitted to their
    /// text with `icon-text-fit`.
    pub fn icon_attributes(&self) -> [(&'static str, &dyn PaintProperty); 7] {
        let (l, p) = (&self.layout, &self.paint);
        [
            ("color", &p.icon_color),
            ("halo_color", &p.icon_halo_color),
            ("halo_width", &p.icon_halo_width),
            ("halo_blur", &p.icon_halo_blur),
            ("opacity", &p.icon_opacity),
            ("size", &l.icon_size),
            ("text_size", &l.text_size),
        ]
    }
}
