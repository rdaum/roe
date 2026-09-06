// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-only

//! Logical presentation to GPU scene. No event loop, session client, or editor access.

use crate::text::TextRenderer;
use crate::{StyledSpan, VelloTheme};
use roe_core::gutter::{GutterConfig, calculate_gutter_width, format_line_number};
use roe_core::session::{PresentationColor, PresentationSnapshot, PresentedView, StyleDefinition};
use vello::Scene;
use vello::kurbo::{Affine, Rect};
use vello::peniko::{Color, Fill};

pub(crate) struct SceneBuilder<'a> {
    pub(crate) scene: &'a mut Scene,
    pub(crate) text_renderer: &'a mut TextRenderer,
    pub(crate) theme: &'a VelloTheme,
}

pub(crate) fn session_vello_color(color: &PresentationColor, default: Color) -> Color {
    match color {
        PresentationColor::Rgb { r, g, b } => Color::from_rgb8(*r, *g, *b),
        PresentationColor::Named(name) => match name.as_str() {
            "black" => Color::BLACK,
            "white" => Color::WHITE,
            "red" => Color::from_rgb8(255, 0, 0),
            "green" => Color::from_rgb8(0, 255, 0),
            "blue" => Color::from_rgb8(0, 0, 255),
            "yellow" => Color::from_rgb8(255, 255, 0),
            "cyan" => Color::from_rgb8(0, 255, 255),
            "magenta" => Color::from_rgb8(255, 0, 255),
            _ => default,
        },
        PresentationColor::Inherit => default,
    }
}

pub(crate) fn session_vello_line_style(
    line: usize,
    view: &PresentedView,
    styles: &[StyleDefinition],
    default_foreground: Color,
    default_background: Color,
) -> (Color, Color) {
    let style = view
        .styled_lines
        .iter()
        .rev()
        .find(|styled| styled.line == line)
        .and_then(|styled| styles.iter().find(|style| style.id == styled.style));
    let Some(style) = style else {
        return (default_foreground, default_background);
    };
    (
        style
            .foreground
            .as_ref()
            .map(|color| session_vello_color(color, default_foreground))
            .unwrap_or(default_foreground),
        style
            .background
            .as_ref()
            .map(|color| session_vello_color(color, default_background))
            .unwrap_or(default_background),
    )
}

/// Scrollbar width in logical pixels
pub(crate) const SCROLLBAR_WIDTH: f64 = 14.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SessionViewMetrics {
    pub(crate) content_width_chars: usize,
    pub(crate) content_rows: usize,
    pub(crate) horizontal_overflow: bool,
}

pub(crate) fn session_view_metrics(view: &PresentedView, char_width: f64) -> SessionViewMetrics {
    let gutter_chars = if view.show_gutter {
        calculate_gutter_width(view.total_lines, &GutterConfig::default())
    } else {
        0
    };
    let width = f64::from(view.geometry.columns) * char_width;
    let gutter_width = gutter_chars as f64 * char_width;
    let renderer_chrome_width = if view.command_view {
        0.0
    } else {
        SCROLLBAR_WIDTH + 4.0
    };
    let content_width =
        (width - (2.0 * char_width) - renderer_chrome_width - gutter_width).max(0.0);
    let content_width_chars = (content_width / char_width).floor() as usize;
    let content_rows = if view.command_view {
        view.geometry.rows.saturating_sub(2) as usize
    } else {
        view.geometry.rows.saturating_sub(3) as usize
    };
    SessionViewMetrics {
        content_width_chars,
        content_rows,
        horizontal_overflow: !view.command_view && view.max_line_chars > content_width_chars,
    }
}

pub(crate) fn typeout_body_capacity(body_top: f64, footer_top: f64, line_height: f64) -> usize {
    if line_height <= 0.0 || footer_top <= body_top {
        return 0;
    }
    // Font metrics are fractional. A half-pixel tolerance keeps an exact
    // logical row from disappearing through harmless floating-point rounding.
    (((footer_top - body_top) + 0.5) / line_height).floor() as usize
}

/// Gutter colors
const GUTTER_FG_COLOR: Color = Color::from_rgba8(0x60, 0x60, 0x60, 0xFF); // Dimmed line numbers

impl SceneBuilder<'_> {
    pub(crate) fn build(&mut self, snapshot: &PresentationSnapshot, width: u32, height: u32) {
        let background = Rect::new(0.0, 0.0, width as f64, height as f64);
        self.scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            self.theme.bg_color,
            None,
            &background,
        );
        for view in &snapshot.views {
            self.draw_session_view(view, &snapshot.styles);
        }

        let line_height = f64::from(self.text_renderer.line_height());
        let echo_y = height as f64 - line_height;
        let echo_rect = Rect::new(0.0, echo_y, width as f64, height as f64);
        self.scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            self.theme.bg_color,
            None,
            &echo_rect,
        );
        if !snapshot.echo_area.is_empty() {
            self.text_renderer.render_line(
                self.scene,
                &snapshot.echo_area,
                4.0,
                echo_y as f32,
                self.theme.fg_color,
                Some(width as f32 - 8.0),
            );
        }
    }

    fn draw_session_view(&mut self, view: &PresentedView, styles: &[StyleDefinition]) {
        let char_width = f64::from(self.text_renderer.char_width());
        let line_height = f64::from(self.text_renderer.line_height());
        let x = f64::from(view.geometry.x) * char_width;
        let y = f64::from(view.geometry.y) * line_height;
        let width = f64::from(view.geometry.columns) * char_width;
        let height = f64::from(view.geometry.rows) * line_height;
        let border = if view.active {
            self.theme.active_border_color
        } else {
            self.theme.border_color
        };
        self.scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            self.theme.bg_color,
            None,
            &Rect::new(x, y, x + width, y + height),
        );
        for rect in [
            Rect::new(x, y, x + width, y + 2.0),
            Rect::new(x, y, x + 2.0, y + height),
            Rect::new(x + width - 2.0, y, x + width, y + height),
        ] {
            self.scene
                .fill(Fill::NonZero, Affine::IDENTITY, border, None, &rect);
        }
        let modeline_y = y + height - line_height;
        let modeline_color = if view.active {
            self.theme.mode_line_bg_color
        } else {
            self.theme.inactive_mode_line_bg_color
        };
        self.scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            modeline_color,
            None,
            &Rect::new(x, modeline_y, x + width, modeline_y + line_height),
        );
        self.text_renderer.render_line(
            self.scene,
            &view.modeline,
            (x + 4.0) as f32,
            modeline_y as f32,
            self.theme.fg_color,
            Some((width - 8.0) as f32),
        );

        let gutter_chars = if view.show_gutter {
            calculate_gutter_width(view.total_lines, &GutterConfig::default())
        } else {
            0
        };
        let content_x = x + char_width * (1 + gutter_chars) as f64;
        // Normal views reserve renderer-owned scrollbar lanes. Command views
        // present an already-windowed candidate list and use the full shared
        // text area, matching terminal row geometry.
        let metrics = session_view_metrics(view, char_width);
        let content_width_chars = metrics.content_width_chars;
        let content_rows = metrics.content_rows;
        let mut absolute = view.visible_start_char;
        let mut lines: Vec<&str> = view.visible_text.split_inclusive('\n').collect();
        if lines.is_empty() {
            lines.push("");
        }
        for (row, raw_line) in lines.into_iter().take(content_rows).enumerate() {
            let line = raw_line.trim_end_matches('\n');
            let displayed: String = line
                .chars()
                .skip(view.scroll.start_column)
                .take(content_width_chars)
                .collect();
            let line_y = y + line_height * (row + 1) as f64;
            let logical_line = view.scroll.start_line + row;
            let (line_foreground, line_background) = session_vello_line_style(
                logical_line,
                view,
                styles,
                self.theme.fg_color,
                self.theme.bg_color,
            );
            if line_background != self.theme.bg_color {
                self.scene.fill(
                    Fill::NonZero,
                    Affine::IDENTITY,
                    line_background,
                    None,
                    &Rect::new(
                        x + char_width,
                        line_y,
                        x + width - char_width,
                        line_y + line_height,
                    ),
                );
            }
            if view.show_gutter {
                let line_number = view.scroll.start_line + row + 1;
                let label = format_line_number(line_number, gutter_chars.saturating_sub(2));
                self.text_renderer.render_line(
                    self.scene,
                    &format!(" {label}│"),
                    (x + char_width) as f32,
                    line_y as f32,
                    GUTTER_FG_COLOR,
                    None,
                );
            }
            let visible_column = view.scroll.start_column;
            let display_start = absolute + visible_column;
            let display_end = display_start + displayed.chars().count();
            if let Some(selection) = view.selection {
                let start = selection.anchor.min(selection.active).max(display_start);
                let end = selection.anchor.max(selection.active).min(display_end);
                if start < end {
                    let left = content_x + (start - display_start) as f64 * char_width;
                    let right = content_x + (end - display_start) as f64 * char_width;
                    self.scene.fill(
                        Fill::NonZero,
                        Affine::IDENTITY,
                        self.theme.selection_color,
                        None,
                        &Rect::new(left, line_y, right, line_y + line_height),
                    );
                }
            }
            if view.active
                && view.typeout.is_none()
                && view.cursor >= display_start
                && view.cursor <= display_end
            {
                let cursor_x = content_x + (view.cursor - display_start) as f64 * char_width;
                self.scene.fill(
                    Fill::NonZero,
                    Affine::IDENTITY,
                    self.theme.cursor_color,
                    None,
                    &Rect::new(cursor_x, line_y, cursor_x + 2.0, line_y + line_height),
                );
            }
            let spans: Vec<StyledSpan> = view
                .styled_ranges
                .iter()
                .filter_map(|range| {
                    let start = range.start.max(absolute + visible_column);
                    let end = range
                        .end
                        .min(absolute + visible_column + displayed.chars().count());
                    if start >= end {
                        return None;
                    }
                    let style = styles.iter().find(|style| style.id == range.style)?;
                    let color = style
                        .foreground
                        .as_ref()
                        .map(|color| session_vello_color(color, line_foreground))
                        .unwrap_or(line_foreground);
                    Some(
                        StyledSpan::new(
                            start - absolute - visible_column,
                            end - absolute - visible_column,
                            color,
                        )
                        .with_bold(style.bold)
                        .with_italic(style.italic),
                    )
                })
                .collect();
            self.text_renderer.render_line_with_styles(
                self.scene,
                &displayed,
                content_x as f32,
                line_y as f32,
                line_foreground,
                &spans,
            );
            absolute += line.chars().count() + usize::from(raw_line.ends_with('\n'));
        }

        if !view.command_view {
            let scrollbar_top = y + 2.0;
            let scrollbar_extent = (height - line_height - 4.0).max(1.0);
            let scrollbar_x = x + width - SCROLLBAR_WIDTH - 2.0;
            self.scene.fill(
                Fill::NonZero,
                Affine::IDENTITY,
                Color::from_rgba8(0x40, 0x40, 0x40, 0x80),
                None,
                &Rect::new(
                    scrollbar_x,
                    scrollbar_top,
                    scrollbar_x + SCROLLBAR_WIDTH,
                    scrollbar_top + scrollbar_extent,
                ),
            );
            let visible_lines = content_rows.max(1);
            let vertical_fraction =
                (visible_lines as f64 / view.total_lines.max(1) as f64).min(1.0);
            let thumb_height = (scrollbar_extent * vertical_fraction)
                .max(20.0)
                .min(scrollbar_extent);
            let max_line = view.total_lines.saturating_sub(visible_lines);
            let vertical_position = if max_line == 0 {
                0.0
            } else {
                view.scroll.start_line as f64 / max_line as f64
            };
            let thumb_y = scrollbar_top + vertical_position * (scrollbar_extent - thumb_height);
            self.scene.fill(
                Fill::NonZero,
                Affine::IDENTITY,
                if view.active {
                    self.theme.active_border_color
                } else {
                    self.theme.border_color
                },
                None,
                &Rect::new(
                    scrollbar_x + 2.0,
                    thumb_y,
                    scrollbar_x + SCROLLBAR_WIDTH - 2.0,
                    thumb_y + thumb_height,
                ),
            );
        }

        if metrics.horizontal_overflow {
            let horizontal_x = x + 2.0;
            let horizontal_y = y + height - line_height - SCROLLBAR_WIDTH - 2.0;
            let horizontal_extent = (width - SCROLLBAR_WIDTH - 6.0).max(1.0);
            self.scene.fill(
                Fill::NonZero,
                Affine::IDENTITY,
                Color::from_rgba8(0x40, 0x40, 0x40, 0x80),
                None,
                &Rect::new(
                    horizontal_x,
                    horizontal_y,
                    horizontal_x + horizontal_extent,
                    horizontal_y + SCROLLBAR_WIDTH,
                ),
            );
            let visible_columns = content_width_chars.max(1);
            let horizontal_fraction =
                (visible_columns as f64 / view.max_line_chars.max(1) as f64).min(1.0);
            let thumb_width = (horizontal_extent * horizontal_fraction)
                .max(20.0)
                .min(horizontal_extent);
            let max_column = view.max_line_chars.saturating_sub(visible_columns);
            let horizontal_position = if max_column == 0 {
                0.0
            } else {
                view.scroll.start_column as f64 / max_column as f64
            };
            let thumb_x = horizontal_x + horizontal_position * (horizontal_extent - thumb_width);
            self.scene.fill(
                Fill::NonZero,
                Affine::IDENTITY,
                if view.active {
                    self.theme.active_border_color
                } else {
                    self.theme.border_color
                },
                None,
                &Rect::new(
                    thumb_x,
                    horizontal_y + 2.0,
                    thumb_x + thumb_width,
                    horizontal_y + SCROLLBAR_WIDTH - 2.0,
                ),
            );
        }

        self.draw_session_typeout(view, x, y, width, height, line_height);
    }

    fn draw_session_typeout(
        &mut self,
        view: &PresentedView,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        line_height: f64,
    ) {
        let Some(typeout) = view.typeout.as_ref() else {
            return;
        };
        let inset = (width * 0.025)
            .clamp(16.0, 32.0)
            .min((width / 4.0).max(2.0));
        let body: Vec<_> = if typeout.visible_text.is_empty() {
            vec![""]
        } else {
            typeout.visible_text.lines().collect()
        };
        let vertical_padding = 6.0;
        let desired_height =
            (body.len().saturating_add(2) as f64) * line_height + vertical_padding * 4.0;
        let max_height =
            ((height - line_height).max(line_height) * 2.0 / 3.0).max(line_height * 3.0);
        let overlay_height = desired_height.min(max_height).min(height - line_height);
        let left = x + inset;
        let top = y + line_height + 4.0;
        let right = (x + width - inset).max(left + 1.0);
        let available_bottom = y + height - line_height - 4.0;
        if available_bottom <= top + line_height {
            return;
        }
        let bottom = (top + overlay_height).min(available_bottom);
        let shadow = Rect::new(left + 4.0, top + 6.0, right + 4.0, bottom + 6.0);
        self.scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            Color::from_rgba8(0x00, 0x00, 0x00, 0x70),
            None,
            &shadow,
        );
        let panel = Rect::new(left, top, right, bottom);
        self.scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            Color::from_rgba8(0x18, 0x1d, 0x21, 0xf2),
            None,
            &panel,
        );
        let border = if typeout.kind == "diagnostic" {
            Color::from_rgba8(0xc7, 0x91, 0x4f, 0xe0)
        } else {
            Color::from_rgba8(0x55, 0x7d, 0x8d, 0xd0)
        };
        for rect in [
            Rect::new(left, top, right, top + 1.0),
            Rect::new(left, bottom - 1.0, right, bottom),
            Rect::new(left, top, left + 1.0, bottom),
            Rect::new(right - 1.0, top, right, bottom),
        ] {
            self.scene
                .fill(Fill::NonZero, Affine::IDENTITY, border, None, &rect);
        }
        let header_bottom = top + line_height + vertical_padding * 2.0;
        self.scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            Color::from_rgba8(0x24, 0x2c, 0x31, 0xc0),
            None,
            &Rect::new(left + 1.0, top + 1.0, right - 1.0, header_bottom),
        );
        let text_x = (left + 12.0) as f32;
        let max_text_width = ((right - left) - 24.0).max(1.0) as f32;
        self.text_renderer.render_line(
            self.scene,
            &typeout.title,
            text_x,
            (top + vertical_padding) as f32,
            border,
            Some(max_text_width),
        );
        let body_top = header_bottom;
        let footer_top = bottom - line_height - vertical_padding * 2.0;
        let available_body_rows = typeout_body_capacity(body_top, footer_top, line_height);
        for (row, line) in body.into_iter().take(available_body_rows).enumerate() {
            self.text_renderer.render_line(
                self.scene,
                line,
                text_x,
                (body_top + line_height * row as f64) as f32,
                self.theme.fg_color,
                Some(max_text_width),
            );
        }
        let status = if typeout.more_after {
            "More — Space"
        } else if typeout.more_before {
            "End — Backspace"
        } else {
            "End"
        };
        self.text_renderer.render_line(
            self.scene,
            status,
            text_x,
            (footer_top + vertical_padding) as f32,
            Color::from_rgb8(0xa9, 0xb2, 0xb7),
            Some(max_text_width),
        );
    }
}
