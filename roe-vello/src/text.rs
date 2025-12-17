// Copyright (C) 2025 Ryan Daum <ryan.daum@gmail.com> This program is free
// software: you can redistribute it and/or modify it under the terms of the GNU
// General Public License as published by the Free Software Foundation, version
// 3.
//
// This program is distributed in the hope that it will be useful, but WITHOUT
// ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS
// FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License along with
// this program. If not, see <https://www.gnu.org/licenses/>.
//

//! Text rendering with Parley.

use parley::layout::{Alignment, Layout};
use parley::style::{FontFamily, FontStack, StyleProperty};
use parley::{FontContext, LayoutContext};
use std::borrow::Cow;
use vello::kurbo::Affine;
use vello::peniko::{Brush, Color, Fill};
use vello::Scene;

/// Font size in pixels
pub const DEFAULT_FONT_SIZE: f32 = 14.0;

/// Line height multiplier
pub const LINE_HEIGHT_FACTOR: f32 = 1.3;

/// Text renderer using Parley for layout
pub struct TextRenderer {
    font_cx: FontContext,
    layout_cx: LayoutContext<Color>,
    font_size: f32,
    line_height: f32,
    char_width: f32,
    font_family: Option<String>,
}

impl Default for TextRenderer {
    fn default() -> Self {
        Self::new(DEFAULT_FONT_SIZE, None)
    }
}

impl TextRenderer {
    pub fn new(font_size: f32, font_family: Option<String>) -> Self {
        let mut font_cx = FontContext::default();
        let mut layout_cx = LayoutContext::new();

        // Measure actual character metrics by laying out a test string
        let (char_width, line_height) =
            Self::measure_metrics(&mut font_cx, &mut layout_cx, font_size, font_family.as_deref());

        Self {
            font_cx,
            layout_cx,
            font_size,
            line_height,
            char_width,
            font_family,
        }
    }

    /// Measure actual character width and line height from the font
    fn measure_metrics(
        font_cx: &mut FontContext,
        layout_cx: &mut LayoutContext<Color>,
        font_size: f32,
        font_family: Option<&str>,
    ) -> (f32, f32) {
        // Use a test string to measure character width - 'M' is typically the widest
        let test_str = "MMMMMMMMMM";
        let mut builder = layout_cx.ranged_builder(font_cx, test_str, 1.0);

        builder.push_default(StyleProperty::FontSize(font_size));

        if let Some(family_name) = font_family {
            builder.push_default(StyleProperty::FontStack(FontStack::List(Cow::Borrowed(&[
                FontFamily::Named(Cow::Owned(family_name.to_string())),
                FontFamily::Generic(parley::style::GenericFamily::Monospace),
            ]))));
        } else {
            builder.push_default(StyleProperty::FontStack(FontStack::Single(
                FontFamily::Generic(parley::style::GenericFamily::Monospace),
            )));
        }

        builder.push_default(StyleProperty::Brush(Color::WHITE));

        let mut layout: Layout<Color> = builder.build(test_str);
        layout.break_all_lines(None);

        // Get metrics from the layout
        let mut char_width = font_size * 0.6; // fallback
        let mut line_height = font_size * LINE_HEIGHT_FACTOR; // fallback

        if let Some(line) = layout.lines().next() {
            // Line height from actual font metrics
            line_height = line.metrics().line_height;

            // Calculate char width from total advance
            for item in line.items() {
                if let parley::layout::PositionedLayoutItem::GlyphRun(glyph_run) = item {
                    let mut total_advance = 0.0f32;
                    let mut glyph_count = 0;
                    for glyph in glyph_run.glyphs() {
                        total_advance += glyph.advance;
                        glyph_count += 1;
                    }
                    if glyph_count > 0 {
                        char_width = total_advance / glyph_count as f32;
                    }
                }
            }
        }

        (char_width, line_height)
    }

    /// Get the line height
    pub fn line_height(&self) -> f32 {
        self.line_height
    }

    /// Get the approximate character width
    pub fn char_width(&self) -> f32 {
        self.char_width
    }

    /// Render a single line of text
    pub fn render_line(
        &mut self,
        scene: &mut Scene,
        text: &str,
        x: f32,
        y: f32,
        color: Color,
        max_width: Option<f32>,
    ) {
        if text.is_empty() {
            return;
        }

        // Build layout
        let mut builder = self.layout_cx.ranged_builder(&mut self.font_cx, text, 1.0);

        // Set styles
        builder.push_default(StyleProperty::FontSize(self.font_size));

        // Use custom font family if specified, otherwise fall back to system monospace
        if let Some(ref family_name) = self.font_family {
            // Create a font stack with the named font, falling back to monospace
            builder.push_default(StyleProperty::FontStack(FontStack::List(Cow::Borrowed(&[
                FontFamily::Named(Cow::Owned(family_name.clone())),
                FontFamily::Generic(parley::style::GenericFamily::Monospace),
            ]))));
        } else {
            builder.push_default(StyleProperty::FontStack(FontStack::Single(
                FontFamily::Generic(parley::style::GenericFamily::Monospace),
            )));
        }

        builder.push_default(StyleProperty::Brush(color));

        let mut layout: Layout<Color> = builder.build(text);

        // Break lines and align
        let width = max_width.unwrap_or(f32::MAX);
        layout.break_all_lines(Some(width));
        layout.align(Some(width), Alignment::Start);

        // Render glyphs
        self.render_layout(scene, &layout, x, y);
    }

    /// Render a pre-built layout
    fn render_layout(&self, scene: &mut Scene, layout: &Layout<Color>, x: f32, y: f32) {
        for line in layout.lines() {
            for item in line.items() {
                let parley::layout::PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                    continue;
                };

                let run = glyph_run.run();
                let font = run.font();
                let font_size = run.font_size();
                let synthesis = run.synthesis();
                let brush = glyph_run.style().brush;

                let run_x = x + glyph_run.offset();
                let run_y = y + glyph_run.baseline();

                // Build transform for the glyph run
                let transform = Affine::translate((run_x as f64, run_y as f64));

                // Separate transform for italic/skew
                let glyph_xform = synthesis
                    .skew()
                    .map(|angle| Affine::skew(angle.to_radians().tan() as f64, 0.0));

                // Get normalized coordinates for variable fonts
                let coords: Vec<_> = run
                    .normalized_coords()
                    .iter()
                    .map(|coord| vello::skrifa::instance::NormalizedCoord::from_bits(*coord))
                    .collect();

                // Track cumulative x position - glyphs need to be advanced manually
                let mut cursor_x = 0.0f32;

                // Collect glyphs with proper positioning
                let glyphs: Vec<vello::Glyph> = glyph_run
                    .glyphs()
                    .map(|glyph| {
                        let gx = cursor_x + glyph.x;
                        cursor_x += glyph.advance;
                        vello::Glyph {
                            id: glyph.id as u32,
                            x: gx,
                            y: glyph.y,
                        }
                    })
                    .collect();

                let solid_brush = Brush::Solid(brush);
                let mut builder = scene
                    .draw_glyphs(font)
                    .font_size(font_size)
                    .transform(transform)
                    .brush(&solid_brush)
                    .hint(true);

                if let Some(xform) = glyph_xform {
                    builder = builder.glyph_transform(Some(xform));
                }

                if !coords.is_empty() {
                    builder = builder.normalized_coords(&coords);
                }

                builder.draw(Fill::NonZero, glyphs.into_iter());
            }
        }
    }
}
