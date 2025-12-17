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

//! Theme configuration for Vello renderer.

use vello::peniko::Color;

/// Theme colors and font settings for the Vello renderer
#[derive(Clone)]
pub struct VelloTheme {
    pub bg_color: Color,
    pub fg_color: Color,
    pub selection_color: Color,
    pub mode_line_bg_color: Color,
    pub inactive_mode_line_bg_color: Color,
    pub rune_color: Color,
    pub border_color: Color,
    pub active_border_color: Color,
    pub cursor_color: Color,
    pub font_family: String,
    pub font_size: f32,
}

impl Default for VelloTheme {
    fn default() -> Self {
        Self {
            // Dark theme similar to terminal version
            bg_color: Color::rgb8(0x1e, 0x1e, 0x1e),
            fg_color: Color::rgb8(0xd4, 0xd4, 0xd4),
            selection_color: Color::rgb8(0x26, 0x4f, 0x78),
            mode_line_bg_color: Color::rgb8(0x00, 0x7a, 0xcc),
            inactive_mode_line_bg_color: Color::rgb8(0x3c, 0x3c, 0x3c),
            rune_color: Color::rgb8(0xdc, 0xdc, 0xaa),
            border_color: Color::rgb8(0x3c, 0x3c, 0x3c),
            active_border_color: Color::rgb8(0x00, 0x7a, 0xcc),
            cursor_color: Color::rgb8(0xae, 0xaf, 0xad),
            font_family: String::new(), // Empty means use system monospace
            font_size: 14.0,
        }
    }
}

impl VelloTheme {
    /// Create a theme from hex color strings
    pub fn from_hex(
        bg: &str,
        fg: &str,
        selection: &str,
    ) -> Self {
        let mut theme = Self::default();

        if let Some(color) = parse_hex_color(bg) {
            theme.bg_color = color;
        }
        if let Some(color) = parse_hex_color(fg) {
            theme.fg_color = color;
        }
        if let Some(color) = parse_hex_color(selection) {
            theme.selection_color = color;
        }

        theme
    }

    /// Set a color from a hex string, given a color key name
    pub fn set_color(&mut self, key: &str, hex: &str) {
        if let Some(color) = parse_hex_color(hex) {
            match key {
                "background" | "bg" => self.bg_color = color,
                "foreground" | "fg" => self.fg_color = color,
                "selection" | "sel" => self.selection_color = color,
                "modeline" | "mode_line" | "mode-line" => self.mode_line_bg_color = color,
                "modeline_inactive" | "mode_line_inactive" | "mode-line-inactive" => {
                    self.inactive_mode_line_bg_color = color
                }
                "border" => self.border_color = color,
                "border_active" | "active_border" | "active-border" => {
                    self.active_border_color = color
                }
                "cursor" => self.cursor_color = color,
                "rune" => self.rune_color = color,
                _ => {}
            }
        }
    }

    /// Set font family
    pub fn set_font_family(&mut self, family: &str) {
        self.font_family = family.to_string();
    }

    /// Set font size
    pub fn set_font_size(&mut self, size: f32) {
        if size > 0.0 {
            self.font_size = size;
        }
    }
}

/// Parse a hex color string like "#272822" to a Color
pub fn parse_hex_color(hex: &str) -> Option<Color> {
    let hex = hex.strip_prefix('#').unwrap_or(hex);

    if hex.len() != 6 {
        return None;
    }

    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;

    Some(Color::rgb8(r, g, b))
}
