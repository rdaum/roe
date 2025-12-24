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

/// A styled span for rendering text with syntax highlighting
#[derive(Clone, Debug)]
pub struct StyledSpan {
    /// Character offset start (relative to line text)
    pub start: usize,
    /// Character offset end (relative to line text)
    pub end: usize,
    /// Foreground color
    pub color: Color,
    /// Bold
    pub bold: bool,
    /// Italic
    pub italic: bool,
}

impl StyledSpan {
    pub fn new(start: usize, end: usize, color: Color) -> Self {
        Self {
            start,
            end,
            color,
            bold: false,
            italic: false,
        }
    }

    pub fn with_bold(mut self, bold: bool) -> Self {
        self.bold = bold;
        self
    }

    pub fn with_italic(mut self, italic: bool) -> Self {
        self.italic = italic;
        self
    }
}

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
        let (char_width, line_height) = Self::measure_metrics(
            &mut font_cx,
            &mut layout_cx,
            font_size,
            font_family.as_deref(),
        );

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
        _max_width: Option<f32>, // Reserved for future wrapping support
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

        // Don't wrap lines - let clipping handle overflow
        // For proper wrapping we'd need to pre-calculate visual line counts
        layout.break_all_lines(None);
        layout.align(None, Alignment::Start);

        // Render glyphs
        self.render_layout(scene, &layout, x, y);
    }

    /// Render a single line of text with multiple styled spans
    pub fn render_line_with_styles(
        &mut self,
        scene: &mut Scene,
        text: &str,
        x: f32,
        y: f32,
        default_color: Color,
        spans: &[StyledSpan],
        _max_width: Option<f32>, // Reserved for future wrapping support
    ) {
        if text.is_empty() {
            return;
        }

        // Build layout with ranged styles
        let mut builder = self.layout_cx.ranged_builder(&mut self.font_cx, text, 1.0);

        // Set default styles
        builder.push_default(StyleProperty::FontSize(self.font_size));

        // Use custom font family if specified, otherwise fall back to system monospace
        if let Some(ref family_name) = self.font_family {
            builder.push_default(StyleProperty::FontStack(FontStack::List(Cow::Borrowed(&[
                FontFamily::Named(Cow::Owned(family_name.clone())),
                FontFamily::Generic(parley::style::GenericFamily::Monospace),
            ]))));
        } else {
            builder.push_default(StyleProperty::FontStack(FontStack::Single(
                FontFamily::Generic(parley::style::GenericFamily::Monospace),
            )));
        }

        builder.push_default(StyleProperty::Brush(default_color));

        // Apply styled spans
        // Note: span.start and span.end are character positions, convert to byte positions
        let char_count = text.chars().count();
        for span in spans {
            // Ensure span is within text bounds (character positions)
            let start_char = span.start.min(char_count);
            let end_char = span.end.min(char_count);
            if start_char >= end_char {
                continue;
            }

            // Convert character positions to byte positions
            let start_idx = text
                .char_indices()
                .nth(start_char)
                .map(|(i, _)| i)
                .unwrap_or(text.len());
            let end_idx = text
                .char_indices()
                .nth(end_char)
                .map(|(i, _)| i)
                .unwrap_or(text.len());

            // Apply color for this span
            builder.push(StyleProperty::Brush(span.color), start_idx..end_idx);

            // Apply bold if set
            if span.bold {
                builder.push(
                    StyleProperty::FontWeight(parley::style::FontWeight::BOLD),
                    start_idx..end_idx,
                );
            }

            // Apply italic if set
            if span.italic {
                builder.push(
                    StyleProperty::FontStyle(parley::style::FontStyle::Italic),
                    start_idx..end_idx,
                );
            }
        }

        let mut layout: Layout<Color> = builder.build(text);

        // Don't wrap lines - let clipping handle overflow
        layout.break_all_lines(None);
        layout.align(None, Alignment::Start);

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
