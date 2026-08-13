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

use compio::time::interval;
use crossterm::event::{
    Event, KeyCode, KeyModifiers, ModifierKeyCode, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::style::{Color, Print, Stylize};
use crossterm::terminal::{Clear, ClearType};
use crossterm::{cursor, queue};
use roe_core::editor::{Frame, Window};
use roe_core::gutter::{
    GutterConfig, LineStatus, calculate_gutter_width, format_line_number, get_line_status,
};
use roe_core::keys::{KeyModifier, LogicalKey, Side};
use roe_core::renderer::{
    DirtyRegion, DirtyTracker, ModelineComponent, PresentationSnapshot, PresentationStreamState,
    Renderer,
};
use roe_core::session::{
    HostSession, InputEvent, LifecycleEvent, PointerButton, PointerEvent, PointerKind,
    PresentationColor, PresentationUpdate, PresentedView, SessionOutput, StyleDefinition,
};
use roe_core::syntax::Color as SyntaxColor;
use roe_core::syntax::face_registry;
use roe_core::{Editor, HighlightSpan, WindowId};
use std::borrow::Cow;
use std::collections::HashSet;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub const ECHO_AREA_HEIGHT: u16 = 1;
pub const BG_COLOR: Color = Color::Black;
pub const FG_COLOR: Color = Color::White;
pub const MODE_LINE_BG_COLOR: Color = Color::Blue;
pub const INACTIVE_MODE_LINE_BG_COLOR: Color = Color::DarkGrey;
pub const RUNE_COLOR: Color = Color::Yellow;
pub const BORDER_COLOR: Color = Color::DarkGrey;
pub const ACTIVE_BORDER_COLOR: Color = Color::Cyan;
// Unicode box drawing characters
pub const BORDER_HORIZONTAL: &str = "─";
pub const BORDER_VERTICAL: &str = "│";
pub const BORDER_TOP_LEFT: &str = "┌";
pub const BORDER_TOP_RIGHT: &str = "┐";
pub const BORDER_BOTTOM_LEFT: &str = "└";
pub const BORDER_BOTTOM_RIGHT: &str = "┘";
pub const _BORDER_CROSS: &str = "┼";
pub const _BORDER_T_DOWN: &str = "┬";
pub const _BORDER_T_UP: &str = "┴";
pub const BORDER_T_RIGHT: &str = "├";
pub const BORDER_T_LEFT: &str = "┤";

fn truncate_echo(message: &str, available_width: usize) -> Cow<'_, str> {
    if message.chars().count() <= available_width {
        return Cow::Borrowed(message);
    }

    if available_width <= 3 {
        return Cow::Owned(".".repeat(available_width));
    }

    let mut truncated: String = message.chars().take(available_width - 3).collect();
    truncated.push_str("...");
    Cow::Owned(truncated)
}

fn cursor_in_visible_slice(view: &PresentedView) -> (u16, u16) {
    let target = view
        .cursor
        .saturating_sub(view.visible_start_char)
        .min(view.visible_text.chars().count());
    let mut column = 0u16;
    let mut line = 0u16;
    for character in view.visible_text.chars().take(target) {
        if character == '\n' {
            line = line.saturating_add(1);
            column = 0;
        } else {
            column = column.saturating_add(1);
        }
    }
    (column, line)
}

fn session_style(
    position: usize,
    ranges: &[roe_core::session::StyledRange],
    styles: &[StyleDefinition],
    theme: &CachedTheme,
) -> (Color, Color) {
    let style_id = ranges
        .iter()
        .rev()
        .find(|range| position >= range.start && position < range.end)
        .map(|range| range.style);
    let Some(style) = style_id.and_then(|id| styles.iter().find(|style| style.id == id)) else {
        return (theme.fg_color, theme.bg_color);
    };
    (
        style
            .foreground
            .as_ref()
            .map(|color| session_color(color, theme.fg_color))
            .unwrap_or(theme.fg_color),
        style
            .background
            .as_ref()
            .map(|color| session_color(color, theme.bg_color))
            .unwrap_or(theme.bg_color),
    )
}

fn session_color(color: &PresentationColor, default: Color) -> Color {
    match color {
        PresentationColor::Rgb { r, g, b } => Color::Rgb {
            r: *r,
            g: *g,
            b: *b,
        },
        PresentationColor::Named(name) => match name.as_str() {
            "black" => Color::Black,
            "white" => Color::White,
            "red" => Color::Red,
            "green" => Color::Green,
            "blue" => Color::Blue,
            "yellow" => Color::Yellow,
            "cyan" => Color::Cyan,
            "magenta" => Color::Magenta,
            _ => default,
        },
        PresentationColor::Inherit => default,
    }
}

// Gutter colors
pub const GUTTER_BG_COLOR: Color = Color::Rgb {
    r: 20,
    g: 20,
    b: 20,
}; // Slightly darker than BG
pub const GUTTER_FG_COLOR: Color = Color::DarkGrey; // Dimmed line numbers
pub const GUTTER_SEPARATOR_COLOR: Color = Color::DarkGrey;
pub const GUTTER_MODIFIED_COLOR: Color = Color::Yellow;
pub const GUTTER_SAVED_COLOR: Color = Color::Green;
pub const GUTTER_CONFLICT_COLOR: Color = Color::Red;

/// Convert a syntax color to crossterm Color
fn syntax_color_to_crossterm(color: &SyntaxColor, default: Color) -> Color {
    match color {
        SyntaxColor::Rgb { r, g, b } => Color::Rgb {
            r: *r,
            g: *g,
            b: *b,
        },
        SyntaxColor::Named(name) => {
            // Map common color names
            match name.to_lowercase().as_str() {
                "black" => Color::Black,
                "red" => Color::Red,
                "green" => Color::Green,
                "yellow" => Color::Yellow,
                "blue" => Color::Blue,
                "magenta" => Color::Magenta,
                "cyan" => Color::Cyan,
                "white" => Color::White,
                "grey" | "gray" => Color::Grey,
                "darkgrey" | "darkgray" => Color::DarkGrey,
                _ => default,
            }
        }
        SyntaxColor::Inherit => default,
    }
}

/// Cached theme colors used by the terminal renderer
#[derive(Clone)]
pub struct CachedTheme {
    pub bg_color: Color,
    pub fg_color: Color,
    pub selection_color: Color,
    pub mode_line_bg_color: Color,
    pub inactive_mode_line_bg_color: Color,
    pub rune_color: Color,
    pub border_color: Color,
    pub active_border_color: Color,
}

impl Default for CachedTheme {
    fn default() -> Self {
        Self {
            bg_color: BG_COLOR,
            fg_color: FG_COLOR,
            selection_color: Color::Yellow,
            mode_line_bg_color: MODE_LINE_BG_COLOR,
            inactive_mode_line_bg_color: INACTIVE_MODE_LINE_BG_COLOR,
            rune_color: RUNE_COLOR,
            border_color: BORDER_COLOR,
            active_border_color: ACTIVE_BORDER_COLOR,
        }
    }
}

/// Terminal-specific renderer using crossterm
pub struct TerminalRenderer<W: Write> {
    device: W,
    dirty_tracker: DirtyTracker,
    theme: CachedTheme,
    presentation_snapshot: Option<PresentationSnapshot>,
    session_presentation: PresentationStreamState,
}

impl<W: Write> TerminalRenderer<W> {
    pub fn new(device: W) -> Self {
        Self {
            device,
            dirty_tracker: DirtyTracker::new(),
            theme: CachedTheme::default(),
            presentation_snapshot: None,
            session_presentation: PresentationStreamState::default(),
        }
    }

    pub fn new_with_theme(device: W, theme: CachedTheme) -> Self {
        Self {
            device,
            dirty_tracker: DirtyTracker::new(),
            theme,
            presentation_snapshot: None,
            session_presentation: PresentationStreamState::default(),
        }
    }

    pub fn apply_session_presentation(
        &mut self,
        update: &PresentationUpdate,
    ) -> Result<(), std::io::Error> {
        self.session_presentation
            .apply(update)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    }

    pub fn session_presentation(&self) -> &PresentationStreamState {
        &self.session_presentation
    }

    /// Realize the authoritative transport-neutral presentation. Production
    /// frontends use this path; the older `Renderer<Editor>` implementation is
    /// retained only as a Phase 0 compatibility/conformance surface.
    pub fn render_session(&mut self) -> Result<(), std::io::Error> {
        let snapshot = self
            .session_presentation
            .current()
            .cloned()
            .ok_or_else(|| std::io::Error::other("session has no presentation snapshot"))?;
        queue!(&mut self.device, cursor::Hide, Clear(ClearType::All))?;

        for view in &snapshot.views {
            self.draw_session_view(view, &snapshot.styles)?;
        }
        if !snapshot.echo_area.is_empty() {
            let message = truncate_echo(&snapshot.echo_area, snapshot.columns as usize);
            queue!(
                &mut self.device,
                cursor::MoveTo(0, snapshot.rows),
                Clear(ClearType::CurrentLine),
                Print(
                    message
                        .as_ref()
                        .with(self.theme.fg_color)
                        .on(self.theme.bg_color)
                )
            )?;
        }

        if let Some(view) = snapshot.views.iter().find(|view| view.active) {
            let (column, line) = cursor_in_visible_slice(view);
            let gutter = if view.show_gutter {
                calculate_gutter_width(
                    view.visible_text.lines().count().max(1),
                    &GutterConfig::default(),
                ) as u16
            } else {
                0
            };
            let x = view
                .geometry
                .x
                .saturating_add(1)
                .saturating_add(gutter)
                .saturating_add(column.saturating_sub(view.scroll.start_column));
            let y = view.geometry.y.saturating_add(1).saturating_add(line);
            queue!(&mut self.device, cursor::MoveTo(x, y))?;
            if view.command_view {
                queue!(&mut self.device, cursor::Hide)?;
            } else {
                queue!(&mut self.device, cursor::Show)?;
            }
        }
        self.device.flush()
    }

    fn draw_session_view(
        &mut self,
        view: &PresentedView,
        styles: &[StyleDefinition],
    ) -> Result<(), std::io::Error> {
        let geometry = view.geometry;
        if geometry.columns < 2 || geometry.rows < 2 {
            return Ok(());
        }
        let border = if view.active {
            self.theme.active_border_color
        } else {
            self.theme.border_color
        };
        let right = geometry.x + geometry.columns - 1;
        let bottom = geometry.y + geometry.rows - 1;
        queue!(
            &mut self.device,
            cursor::MoveTo(geometry.x, geometry.y),
            Print(BORDER_TOP_LEFT.with(border)),
            cursor::MoveTo(right, geometry.y),
            Print(BORDER_TOP_RIGHT.with(border)),
            cursor::MoveTo(geometry.x, bottom),
            Print(BORDER_BOTTOM_LEFT.with(border)),
            cursor::MoveTo(right, bottom),
            Print(BORDER_BOTTOM_RIGHT.with(border))
        )?;
        if geometry.columns > 2 {
            queue!(
                &mut self.device,
                cursor::MoveTo(geometry.x + 1, geometry.y),
                Print(
                    BORDER_HORIZONTAL
                        .repeat((geometry.columns - 2) as usize)
                        .with(border)
                )
            )?;
        }
        for row in geometry.y + 1..bottom {
            queue!(
                &mut self.device,
                cursor::MoveTo(geometry.x, row),
                Print(BORDER_VERTICAL.with(border)),
                cursor::MoveTo(right, row),
                Print(BORDER_VERTICAL.with(border))
            )?;
        }

        let content_rows = geometry.rows.saturating_sub(2) as usize;
        let total_width = geometry.columns.saturating_sub(2) as usize;
        let visible_line_count = view.visible_text.lines().count().max(1);
        let gutter_width = if view.show_gutter {
            calculate_gutter_width(visible_line_count, &GutterConfig::default())
        } else {
            0
        };
        let text_width = total_width.saturating_sub(gutter_width);
        let mut absolute = view.visible_start_char;
        let lines: Vec<&str> = view.visible_text.split_inclusive('\n').collect();
        for row in 0..content_rows {
            let y = geometry.y + 1 + row as u16;
            queue!(
                &mut self.device,
                cursor::MoveTo(geometry.x + 1, y),
                Print(
                    " ".repeat(total_width)
                        .with(self.theme.fg_color)
                        .on(self.theme.bg_color)
                )
            )?;
            let line = lines.get(row).copied().unwrap_or("");
            let line = line.trim_end_matches('\n');
            if view.show_gutter {
                let number = usize::from(view.scroll.start_line) + row + 1;
                let digits = gutter_width.saturating_sub(2);
                let gutter = format!(" {}│", format_line_number(number, digits));
                queue!(
                    &mut self.device,
                    cursor::MoveTo(geometry.x + 1, y),
                    Print(gutter.with(GUTTER_FG_COLOR).on(GUTTER_BG_COLOR))
                )?;
            }
            queue!(
                &mut self.device,
                cursor::MoveTo(geometry.x + 1 + gutter_width as u16, y)
            )?;
            for (offset, character) in line
                .chars()
                .skip(usize::from(view.scroll.start_column))
                .take(text_width)
                .enumerate()
            {
                let position = absolute + usize::from(view.scroll.start_column) + offset;
                let selected = view.selection.is_some_and(|selection| {
                    let start = selection.anchor.min(selection.active);
                    let end = selection.anchor.max(selection.active);
                    position >= start && position < end
                });
                let (foreground, background) = if selected {
                    (Color::Black, self.theme.selection_color)
                } else {
                    session_style(position, &view.styled_ranges, styles, &self.theme)
                };
                queue!(
                    &mut self.device,
                    Print(character.to_string().with(foreground).on(background))
                )?;
            }
            absolute += line.chars().count()
                + usize::from(lines.get(row).is_some_and(|l| l.ends_with('\n')));
        }

        let modeline_bg = if view.active {
            self.theme.mode_line_bg_color
        } else {
            self.theme.inactive_mode_line_bg_color
        };
        let modeline = truncate_echo(&view.modeline, total_width);
        let padded = format!("{:<width$}", modeline, width = total_width);
        queue!(
            &mut self.device,
            cursor::MoveTo(geometry.x + 1, bottom),
            Print(padded.with(self.theme.fg_color).on(modeline_bg))
        )?;
        Ok(())
    }

    /// Render a single line with proper highlighting (region + syntax)
    fn render_line_incremental(
        &mut self,
        editor: &Editor,
        window_id: WindowId,
        buffer_line: usize,
        screen_row: u16,
        _start_col: usize,
        _end_col: usize,
    ) -> Result<(), std::io::Error> {
        let window = &editor.windows[window_id];
        let Some(buffer) = editor.buffers.get(window.active_buffer) else {
            return Ok(()); // Buffer no longer exists
        };

        // Only show region highlighting in the active window
        let region_bounds = if window_id == editor.active_window {
            buffer.get_region(window.cursor)
        } else {
            None
        };

        // Check if gutter should be shown (controlled by major mode)
        let show_gutter = buffer.show_gutter();

        // Calculate gutter width
        let (gutter_width, modified_lines): (usize, HashSet<usize>) = if show_gutter {
            let total_lines = buffer.buffer_len_lines();
            let config = GutterConfig::default();
            let width = calculate_gutter_width(total_lines, &config);
            let buffer_content = buffer.content();
            let modified = editor
                .file_watcher
                .get_modified_lines(window.active_buffer, &buffer_content);
            (width, modified)
        } else {
            (0, HashSet::new())
        };

        let base_content_x = window.x + 1;
        let total_content_width = window.width_chars.saturating_sub(2);
        let content_x = base_content_x + gutter_width as u16;
        let content_width = total_content_width.saturating_sub(gutter_width as u16);
        let line_number_width = gutter_width.saturating_sub(2);

        if buffer_line >= buffer.buffer_len_lines() {
            // Past end of buffer - draw gutter with tilde and clear content
            if show_gutter {
                queue!(&mut self.device, cursor::MoveTo(base_content_x, screen_row))?;
                let empty_gutter = format!(" {:>width$}│", "~", width = line_number_width);
                queue!(
                    &mut self.device,
                    Print(empty_gutter.with(GUTTER_FG_COLOR).on(GUTTER_BG_COLOR))
                )?;
            }

            let spaces = " ".repeat(content_width as usize);
            queue!(
                &mut self.device,
                cursor::MoveTo(content_x, screen_row),
                Print(spaces.with(self.theme.fg_color).on(self.theme.bg_color))
            )?;
            return Ok(());
        }

        let line_text = buffer.buffer_line(buffer_line);
        // Remove trailing newline if present
        let line_text = line_text.trim_end_matches('\n');

        let line_start_char = buffer.buffer_line_to_char(buffer_line);
        let line_char_count = line_text.chars().count();
        let line_end_char = line_start_char + line_char_count;
        let start_column = window.start_column as usize;

        // Draw gutter
        if show_gutter {
            let merged_lines: HashSet<usize> = HashSet::new();
            let line_status =
                get_line_status(line_text, buffer_line, &modified_lines, &merged_lines);

            queue!(&mut self.device, cursor::MoveTo(base_content_x, screen_row))?;

            // Status indicator
            let (status_char, status_color) = match line_status {
                LineStatus::Clean => (" ", GUTTER_FG_COLOR),
                LineStatus::Modified => ("│", GUTTER_MODIFIED_COLOR),
                LineStatus::ModifiedSaved => ("│", GUTTER_SAVED_COLOR),
                LineStatus::Conflict => ("!", GUTTER_CONFLICT_COLOR),
            };
            queue!(
                &mut self.device,
                Print(status_char.with(status_color).on(GUTTER_BG_COLOR))
            )?;

            // Line number
            let line_num_str = format_line_number(buffer_line + 1, line_number_width);
            queue!(
                &mut self.device,
                Print(line_num_str.with(GUTTER_FG_COLOR).on(GUTTER_BG_COLOR))
            )?;

            // Separator
            queue!(
                &mut self.device,
                Print("│".with(GUTTER_SEPARATOR_COLOR).on(GUTTER_BG_COLOR))
            )?;
        }

        // Clear the content area
        queue!(&mut self.device, cursor::MoveTo(content_x, screen_row))?;
        let clear_spaces = " ".repeat(content_width as usize);
        queue!(
            &mut self.device,
            Print(
                clear_spaces
                    .with(self.theme.fg_color)
                    .on(self.theme.bg_color)
            )
        )?;
        queue!(&mut self.device, cursor::MoveTo(content_x, screen_row))?;

        // Apply horizontal scroll - skip start_column characters, then take content_width
        let chars_to_render: Vec<char> = line_text
            .chars()
            .skip(start_column)
            .take(content_width as usize)
            .collect();

        // Highlight spans use the same character offsets as the buffer.
        let syntax_spans: Vec<HighlightSpan> =
            buffer.spans_in_range(line_start_char..line_end_char);

        // Get face registry for looking up face colors
        let face_registry_guard = face_registry().lock().ok();

        // Render character by character with merged highlighting
        for (char_idx, ch) in chars_to_render.iter().enumerate() {
            // Account for horizontal scroll when calculating buffer position (in chars)
            let buffer_pos_char = line_start_char + start_column + char_idx;
            // Determine the style for this character
            // Priority: region selection > syntax highlighting > default
            let (fg, bg) = if let Some((region_start, region_end)) = region_bounds {
                if buffer_pos_char >= region_start && buffer_pos_char < region_end {
                    // Character is in selection region
                    (Color::Black, self.theme.selection_color)
                } else {
                    // Check syntax highlighting
                    self.get_syntax_colors(buffer_pos_char, &syntax_spans, &face_registry_guard)
                }
            } else {
                // No region, check syntax highlighting
                self.get_syntax_colors(buffer_pos_char, &syntax_spans, &face_registry_guard)
            };

            queue!(&mut self.device, Print(ch.to_string().with(fg).on(bg)))?;
        }

        // Handle region extending past line content (fill with selection color)
        if let Some((region_start, region_end)) = region_bounds
            && region_start < line_end_char
            && region_end > line_end_char
        {
            let chars_rendered = chars_to_render.len();
            let remaining_width = content_width as usize - chars_rendered;
            if remaining_width > 0 {
                let highlighted_spaces = " ".repeat(remaining_width);
                queue!(
                    &mut self.device,
                    Print(
                        highlighted_spaces
                            .on(self.theme.selection_color)
                            .with(Color::Black)
                    )
                )?;
            }
        }

        Ok(())
    }

    /// Get the foreground and background colors for a character position based on syntax spans
    fn get_syntax_colors(
        &self,
        buffer_pos: usize,
        syntax_spans: &[HighlightSpan],
        face_registry_guard: &Option<std::sync::MutexGuard<'_, roe_core::FaceRegistry>>,
    ) -> (Color, Color) {
        // Find the last span that contains this position (later spans override earlier ones)
        let matching_span = syntax_spans
            .iter()
            .rev()
            .find(|span| buffer_pos >= span.start && buffer_pos < span.end);

        if let Some(span) = matching_span
            && let Some(registry) = face_registry_guard
            && let Some(face) = registry.get(span.face_id)
        {
            let fg = face
                .foreground
                .as_ref()
                .map(|c| syntax_color_to_crossterm(c, self.theme.fg_color))
                .unwrap_or(self.theme.fg_color);
            let bg = face
                .background
                .as_ref()
                .map(|c| syntax_color_to_crossterm(c, self.theme.bg_color))
                .unwrap_or(self.theme.bg_color);
            return (fg, bg);
        }

        // Default colors
        (self.theme.fg_color, self.theme.bg_color)
    }

    /// Render specific modeline components that are dirty
    fn render_modeline_components(
        &mut self,
        editor: &Editor,
        window_id: WindowId,
        dirty_components: &std::collections::HashSet<ModelineComponent>,
    ) -> Result<(), std::io::Error> {
        let window = &editor.windows[window_id];
        let Some(buffer) = editor.buffers.get(window.active_buffer) else {
            return Ok(()); // Buffer no longer exists
        };
        let is_active = window_id == editor.active_window;

        // Calculate modeline position - now in the bottom border
        let modeline_y = window.y + window.height_chars - 1; // Bottom border row
        let modeline_x = window.x + 1; // Inside left border
        let modeline_width = window.width_chars.saturating_sub(2) as usize; // Inside both borders

        if modeline_width == 0 {
            return Ok(());
        }

        // Choose appropriate background color
        let bg_color = if is_active {
            self.theme.mode_line_bg_color
        } else {
            self.theme.inactive_mode_line_bg_color
        };

        // If All components are dirty, just redraw the entire modeline
        if dirty_components.contains(&ModelineComponent::All) {
            return draw_window_modeline(&mut self.device, editor, window_id, &self.theme);
        }

        // Handle specific component updates
        for component in dirty_components {
            match component {
                ModelineComponent::CursorPosition => {
                    // Update just the cursor position part (right-aligned)
                    let (col, line) = buffer.to_column_line(window.cursor);
                    let position_text = format!("{}:{} ", line + 1, col + 1);

                    // Calculate where the position should be (right-aligned)
                    let position_start = modeline_width.saturating_sub(position_text.len());

                    // Clear the entire right area where position could be (assume max 10 chars for position)
                    let max_position_width = 10; // Should be enough for "9999:9999 "
                    let clear_start = modeline_width.saturating_sub(max_position_width);
                    let clear_width = modeline_width - clear_start;
                    let clear_spaces = " ".repeat(clear_width);

                    // First clear the area
                    queue!(
                        &mut self.device,
                        cursor::MoveTo(modeline_x + clear_start as u16, modeline_y),
                        Print(clear_spaces.on(bg_color).with(self.theme.fg_color))
                    )?;

                    // Then write the new position
                    queue!(
                        &mut self.device,
                        cursor::MoveTo(modeline_x + position_start as u16, modeline_y),
                        Print(position_text.on(bg_color).with(self.theme.fg_color))
                    )?;
                }
                ModelineComponent::BufferName => {
                    // For now, redraw entire modeline since buffer name affects layout
                    return draw_window_modeline(&mut self.device, editor, window_id, &self.theme);
                }
                ModelineComponent::ModeName => {
                    // For now, redraw entire modeline since mode name affects layout
                    return draw_window_modeline(&mut self.device, editor, window_id, &self.theme);
                }
                ModelineComponent::All => {
                    // Already handled above
                }
            }
        }

        Ok(())
    }
}

impl<W: Write> Renderer for TerminalRenderer<W> {
    type Error = std::io::Error;

    fn mark_dirty(&mut self, region: DirtyRegion) {
        self.dirty_tracker.mark_dirty(region);
    }

    fn render_incremental(&mut self, editor: &Editor) -> Result<(), std::io::Error> {
        self.presentation_snapshot = Some(PresentationSnapshot::capture(editor));
        // If full screen is dirty, fall back to full render
        if self.dirty_tracker.is_full_screen_dirty() {
            return self.render_full(editor);
        }

        // Hide cursor during incremental updates to prevent flashing
        queue!(&mut self.device, cursor::Hide)?;

        // Render dirty window chrome (borders, modelines)
        for window_id in editor.windows.keys() {
            if self.dirty_tracker.is_window_chrome_dirty(window_id) {
                // TODO: Implement incremental border/modeline rendering
                // For now, just mark it for full redraw
            }

            // Handle incremental modeline updates
            if let Some(dirty_components) =
                self.dirty_tracker.get_dirty_modeline_components(window_id)
            {
                let components_clone = dirty_components.clone();
                self.render_modeline_components(editor, window_id, &components_clone)?;
            }
        }

        // Render dirty buffer content by lines
        for window_id in editor.windows.keys() {
            let window = &editor.windows[window_id];
            let buffer_id = window.active_buffer;

            // If entire buffer is dirty, mark all lines in the window as dirty
            if self.dirty_tracker.is_buffer_dirty(buffer_id) {
                let buffer = &editor.buffers[buffer_id];
                let content_height = window.height_chars.saturating_sub(2);

                // Mark all visible lines as dirty for our incremental renderer
                for line_idx in 0..content_height.min(buffer.buffer_len_lines() as u16) {
                    let global_line = (window.start_line + line_idx) as usize;
                    if global_line < buffer.buffer_len_lines() {
                        // Force dirty lines to be rendered by our incremental logic
                        // We'll handle this below in the dirty lines iteration
                    }
                }
                // Don't continue here - let it fall through to incremental rendering
            }

            // Render only dirty lines
            let content_y = window.y + 1;
            let content_height = window.height_chars.saturating_sub(2);

            // Collect dirty lines to avoid borrowing issues
            let mut dirty_lines: Vec<(usize, (usize, usize))> = Vec::new();

            // If entire buffer is dirty, add all visible lines to dirty list
            if self.dirty_tracker.is_buffer_dirty(buffer_id) {
                let _buffer = &editor.buffers[buffer_id];
                // Mark all visible lines as dirty, including lines that may now be empty
                for line_idx in 0..content_height {
                    let global_line = (window.start_line + line_idx) as usize;
                    // Always mark the line as dirty, even if it's past the end of buffer
                    // (this ensures empty lines get cleared)
                    dirty_lines.push((global_line, (0, usize::MAX)));
                }
            } else {
                // Only collect specific dirty lines for this buffer
                dirty_lines = self
                    .dirty_tracker
                    .dirty_lines_iter(buffer_id)
                    .map(|(line, span)| (line, (span.start_col, span.end_col)))
                    .collect();
            }

            for (dirty_line, (start_col, end_col)) in dirty_lines {
                // Convert buffer line to screen coordinates
                let screen_line = dirty_line as u16;

                // Skip lines that are scrolled out of view
                if screen_line < window.start_line {
                    continue;
                }

                let content_line = screen_line - window.start_line;

                // Skip lines that are below the window
                if content_line >= content_height {
                    continue;
                }

                let screen_row = content_y + content_line;

                // Render the dirty span of this line
                self.render_line_incremental(
                    editor, window_id, dirty_line, screen_row, start_col, end_col,
                )?;
            }
        }

        // Flush all queued drawing commands first
        self.device.flush()?;

        // Move cursor to correct position and show it (unless in command window)
        let active_window = &editor.windows[editor.active_window];
        let buffer = &editor.buffers[active_window.active_buffer];
        let (col, line) = buffer.to_column_line(active_window.cursor);
        let (mut x, y) = active_window.absolute_cursor_position(col, line);

        // Adjust cursor x for gutter width if gutter is enabled
        if buffer.show_gutter() {
            let total_lines = buffer.buffer_len_lines();
            let config = GutterConfig::default();
            let gutter_width = calculate_gutter_width(total_lines, &config);
            x += gutter_width as u16;
        }

        queue!(&mut self.device, cursor::MoveTo(x, y))?;

        // Hide cursor in command windows (they use visual indicators like ">")
        if matches!(
            active_window.window_type,
            roe_core::editor::WindowType::Command { .. }
        ) {
            queue!(&mut self.device, cursor::Hide)?;
        } else {
            queue!(&mut self.device, cursor::Show)?;
        }

        // Flush cursor positioning commands
        self.device.flush()?;

        Ok(())
    }

    fn render_full(&mut self, editor: &Editor) -> Result<(), std::io::Error> {
        tracing::trace!("terminal redraw requested");
        self.presentation_snapshot = Some(PresentationSnapshot::capture(editor));
        // Hide cursor during redraw
        queue!(&mut self.device, cursor::Hide)?;

        // Clear the screen
        queue!(&mut self.device, Clear(ClearType::All))?;

        // Draw all windows
        for window_id in editor.windows.keys() {
            let window = &editor.windows[window_id];
            draw_window(&mut self.device, editor, window, &self.theme)?;
        }

        // Draw all borders and modelines
        draw_all_window_borders(&mut self.device, editor, &self.theme)?;

        // Draw command windows
        for window_id in editor.windows.keys() {
            let window = &editor.windows[window_id];
            if matches!(
                window.window_type,
                roe_core::editor::WindowType::Command { .. }
            ) {
                draw_command_window(&mut self.device, editor, window_id, &self.theme)?;
            }
        }

        // Draw echo area
        if !editor.echo_message.is_empty() {
            let (x, y) = echo_area_position(&editor.frame);
            let available_width = editor.frame.columns.saturating_sub(x); // Use full terminal width
            let truncated_message = truncate_echo(&editor.echo_message, available_width as usize);
            queue!(
                &mut self.device,
                cursor::MoveTo(x, y),
                Clear(ClearType::CurrentLine)
            )?;
            queue!(
                &mut self.device,
                cursor::MoveTo(x, y),
                Print(
                    truncated_message
                        .as_ref()
                        .with(self.theme.fg_color)
                        .on(self.theme.bg_color)
                )
            )?;
        }

        // Flush all drawing commands first
        self.device.flush()?;

        // Position cursor and show it (unless in command window)
        let active_window = &editor.windows[editor.active_window];
        let buffer = &editor.buffers[active_window.active_buffer];
        let (col, line) = buffer.to_column_line(active_window.cursor);
        let (mut x, y) = active_window.absolute_cursor_position(col, line);

        // Adjust cursor x for gutter width if gutter is enabled
        if buffer.show_gutter() {
            let total_lines = buffer.buffer_len_lines();
            let config = GutterConfig::default();
            let gutter_width = calculate_gutter_width(total_lines, &config);
            x += gutter_width as u16;
        }

        queue!(&mut self.device, cursor::MoveTo(x, y))?;

        // Hide cursor in command windows (they use visual indicators like ">")
        if matches!(
            active_window.window_type,
            roe_core::editor::WindowType::Command { .. }
        ) {
            queue!(&mut self.device, cursor::Hide)?;
        } else {
            queue!(&mut self.device, cursor::Show)?;
        }

        // Flush cursor positioning commands
        self.device.flush()?;

        Ok(())
    }

    fn clear_dirty(&mut self) {
        self.dirty_tracker.clear();
    }

    fn needs_redraw(&self) -> bool {
        self.dirty_tracker.is_dirty()
    }

    fn presentation_snapshot(&self) -> Option<&PresentationSnapshot> {
        self.presentation_snapshot.as_ref()
    }
}

pub fn echo_area_position(frame: &Frame) -> (u16, u16) {
    // Echo area is at the bottom of the terminal, below the frame area
    // Frame.available_lines is the usable area, so echo goes below that
    (0, frame.available_lines)
}

/// Draw borders around all windows in a more sophisticated way that handles adjacency
pub fn draw_all_window_borders(
    device: &mut impl Write,
    editor: &Editor,
    theme: &CachedTheme,
) -> Result<(), std::io::Error> {
    // Create a grid to track what's already drawn to avoid conflicts
    let mut border_grid = vec![
        vec![' '; editor.frame.available_columns as usize];
        editor.frame.available_lines as usize
    ];

    // First pass: mark all window areas and determine border positions
    for window_id in editor.windows.keys() {
        let window = &editor.windows[window_id];
        let is_active = window_id == editor.active_window;
        let border_char = if is_active { 'A' } else { 'I' }; // Active/Inactive marker

        if window.width_chars < 2 || window.height_chars < 2 {
            continue;
        }

        let right = window.x + window.width_chars - 1;
        let bottom = window.y + window.height_chars - 1;

        // Mark corners
        if window.x < editor.frame.available_columns && window.y < editor.frame.available_lines {
            border_grid[window.y as usize][window.x as usize] = border_char;
        }
        if right < editor.frame.available_columns && window.y < editor.frame.available_lines {
            border_grid[window.y as usize][right as usize] = border_char;
        }
        if window.x < editor.frame.available_columns && bottom < editor.frame.available_lines {
            border_grid[bottom as usize][window.x as usize] = border_char;
        }
        if right < editor.frame.available_columns && bottom < editor.frame.available_lines {
            border_grid[bottom as usize][right as usize] = border_char;
        }

        // Mark horizontal borders
        for x in window.x + 1..right {
            if x < editor.frame.available_columns {
                if window.y < editor.frame.available_lines {
                    border_grid[window.y as usize][x as usize] = border_char;
                }
                if bottom < editor.frame.available_lines {
                    border_grid[bottom as usize][x as usize] = border_char;
                }
            }
        }

        // Mark vertical borders
        for y in window.y + 1..bottom {
            if y < editor.frame.available_lines {
                if window.x < editor.frame.available_columns {
                    border_grid[y as usize][window.x as usize] = border_char;
                }
                if right < editor.frame.available_columns {
                    border_grid[y as usize][right as usize] = border_char;
                }
            }
        }
    }

    // Second pass: actually draw the borders
    for window_id in editor.windows.keys() {
        draw_single_window_border(device, editor, window_id, &border_grid, theme)?;
    }

    Ok(())
}

/// Draw borders for a single window
fn draw_single_window_border(
    device: &mut impl Write,
    editor: &Editor,
    window_id: WindowId,
    _border_grid: &[Vec<char>],
    theme: &CachedTheme,
) -> Result<(), std::io::Error> {
    let window = &editor.windows[window_id];
    let is_active = window_id == editor.active_window;
    let border_color = if is_active {
        theme.active_border_color
    } else {
        theme.border_color
    };

    // Only draw borders if the window has space for them
    if window.width_chars < 2 || window.height_chars < 2 {
        return Ok(());
    }

    let right = window.x + window.width_chars - 1;
    let bottom = window.y + window.height_chars - 1;

    // Draw corners
    queue!(
        device,
        cursor::MoveTo(window.x, window.y),
        Print(BORDER_TOP_LEFT.with(border_color))
    )?;
    queue!(
        device,
        cursor::MoveTo(right, window.y),
        Print(BORDER_TOP_RIGHT.with(border_color))
    )?;
    queue!(
        device,
        cursor::MoveTo(window.x, bottom),
        Print(BORDER_BOTTOM_LEFT.with(border_color))
    )?;
    queue!(
        device,
        cursor::MoveTo(right, bottom),
        Print(BORDER_BOTTOM_RIGHT.with(border_color))
    )?;

    // Draw top horizontal border
    if window.x + 1 < right {
        let horizontal_line = BORDER_HORIZONTAL.repeat((right - window.x - 1) as usize);
        queue!(
            device,
            cursor::MoveTo(window.x + 1, window.y),
            Print(horizontal_line.with(border_color))
        )?;
    }

    // Skip drawing bottom horizontal border - modeline will occupy this space
    // The modeline will be drawn separately and fill the bottom border area

    // Draw vertical borders (excluding bottom row which is now the modeline)
    for y in window.y + 1..bottom {
        queue!(
            device,
            cursor::MoveTo(window.x, y),
            Print(BORDER_VERTICAL.with(border_color))
        )?;
        queue!(
            device,
            cursor::MoveTo(right, y),
            Print(BORDER_VERTICAL.with(border_color))
        )?;
    }

    // Draw the actual modeline content
    draw_window_modeline(device, editor, window_id, theme)?;

    Ok(())
}

/// Draw the modeline for a specific window - now integrated into the bottom border
fn draw_window_modeline(
    device: &mut impl Write,
    editor: &Editor,
    window_id: WindowId,
    theme: &CachedTheme,
) -> Result<(), std::io::Error> {
    let window = &editor.windows[window_id];
    let Some(buffer) = editor.buffers.get(window.active_buffer) else {
        return Ok(()); // Buffer no longer exists
    };
    let is_active = window_id == editor.active_window;

    // Calculate modeline position and width - now in the bottom border
    let modeline_y = window.y + window.height_chars - 1; // Bottom border row
    let modeline_x = window.x + 1; // Inside left border
    let modeline_width = window.width_chars.saturating_sub(2) as usize; // Inside both borders

    if modeline_width == 0 {
        return Ok(());
    }

    // Choose appropriate background color
    let bg_color = if is_active {
        theme.mode_line_bg_color
    } else {
        theme.inactive_mode_line_bg_color
    };

    // Move to modeline position
    queue!(device, cursor::MoveTo(modeline_x, modeline_y))?;

    // Handle runes separately for color control, then build the rest
    let rune_section = if is_active {
        " ᚱᛟ "
    } else {
        "    " // Same width as " ᚱᛟ " but with spaces
    };
    let rune_display_width = rune_section.chars().count(); // Use character count, not byte length

    // Build the rest of the modeline content
    let mut rest_content = String::new();

    // Add buffer object name
    let object_part = format!("{} ", buffer.object());
    rest_content.push_str(&object_part);

    // Add major mode name (if set)
    if let Some(major_mode) = buffer.major_mode() {
        let mode_part = format!("({}) ", major_mode);
        rest_content.push_str(&mode_part);
    }

    // Add cursor position
    let (col, line) = buffer.to_column_line(window.cursor);
    let position_part = format!("{}:{} ", line + 1, col + 1); // 1-based for display

    // Calculate remaining space for position (right-aligned) using character counts
    let used_space =
        rune_display_width + rest_content.chars().count() + position_part.chars().count();
    let remaining_space = modeline_width.saturating_sub(used_space);

    // Fill with spaces to right-align position
    rest_content.push_str(&" ".repeat(remaining_space));
    rest_content.push_str(&position_part);

    // Truncate rest_content if too long (preserve rune space) using character counts
    let available_for_rest = modeline_width.saturating_sub(rune_display_width);
    let rest_char_count = rest_content.chars().count();
    if rest_char_count > available_for_rest {
        // Truncate to character boundary, not byte boundary
        rest_content = rest_content.chars().take(available_for_rest).collect();
    } else if rest_char_count < available_for_rest {
        // Pad with spaces to fill the entire remaining modeline
        rest_content.push_str(&" ".repeat(available_for_rest - rest_char_count));
    }

    // Draw rune section with distinct color for active windows
    if is_active {
        queue!(
            device,
            Print(rune_section.on(bg_color).with(theme.rune_color))
        )?;
    } else {
        queue!(
            device,
            Print(rune_section.on(bg_color).with(theme.fg_color))
        )?;
    }

    // Draw the rest of the modeline content
    queue!(
        device,
        Print(rest_content.on(bg_color).with(theme.fg_color))
    )?;

    Ok(())
}

/// Get syntax colors for a position (standalone version for use outside TerminalRenderer methods)
fn get_syntax_colors_standalone(
    buffer_pos: usize,
    syntax_spans: &[HighlightSpan],
    face_registry_guard: &Option<std::sync::MutexGuard<'_, roe_core::FaceRegistry>>,
    theme: &CachedTheme,
) -> (Color, Color) {
    // Find the last span that contains this position (later spans override earlier ones)
    let matching_span = syntax_spans
        .iter()
        .rev()
        .find(|span| buffer_pos >= span.start && buffer_pos < span.end);

    if let Some(span) = matching_span
        && let Some(registry) = face_registry_guard
        && let Some(face) = registry.get(span.face_id)
    {
        let fg = face
            .foreground
            .as_ref()
            .map(|c| syntax_color_to_crossterm(c, theme.fg_color))
            .unwrap_or(theme.fg_color);
        let bg = face
            .background
            .as_ref()
            .map(|c| syntax_color_to_crossterm(c, theme.bg_color))
            .unwrap_or(theme.bg_color);
        return (fg, bg);
    }

    // Default colors
    (theme.fg_color, theme.bg_color)
}

/// Redraw the entire buffer in a window.
pub fn draw_window(
    device: &mut impl Write,
    editor: &Editor,
    window: &Window,
    theme: &CachedTheme,
) -> Result<(), std::io::Error> {
    // Draw the buffer in the window
    let Some(buffer) = editor.buffers.get(window.active_buffer) else {
        return Ok(()); // Buffer no longer exists
    };

    // Calculate base content area (inside the border)
    let base_content_x = window.x + 1;
    let content_y = window.y + 1;
    let total_content_width = window.width_chars.saturating_sub(2);
    let content_height = window.height_chars.saturating_sub(2);

    // Check if gutter should be shown (controlled by major mode)
    let show_gutter = buffer.show_gutter();

    // Calculate gutter width and get modified lines
    let (gutter_width, modified_lines): (usize, HashSet<usize>) = if show_gutter {
        let total_lines = buffer.buffer_len_lines();
        let config = GutterConfig::default();
        let width = calculate_gutter_width(total_lines, &config);

        // Get modified lines from file watcher
        let buffer_content = buffer.content();
        let modified = editor
            .file_watcher
            .get_modified_lines(window.active_buffer, &buffer_content);

        (width, modified)
    } else {
        (0, HashSet::new())
    };

    // Adjust content area for gutter
    let content_x = base_content_x + gutter_width as u16;
    let content_width = total_content_width.saturating_sub(gutter_width as u16);

    // Clear the entire content area first (gutter + text)
    for row in 0..content_height {
        let spaces = " ".repeat(total_content_width as usize);
        queue!(
            device,
            cursor::MoveTo(base_content_x, content_y + row),
            Print(spaces.with(theme.fg_color).on(theme.bg_color))
        )?;
    }

    // Check if there's a region selected for highlighting
    let region_bounds = buffer.get_region(window.cursor);

    // Get face registry for looking up face colors
    let face_registry_guard = face_registry().lock().ok();

    // For detecting conflict lines
    let merged_lines: HashSet<usize> = HashSet::new(); // TODO: track merged lines separately

    // Calculate line number width (for formatting)
    let line_number_width = if show_gutter {
        gutter_width.saturating_sub(2) // Subtract status indicator and separator
    } else {
        0
    };

    // Draw the buffer content within the content bounds
    for (line_idx, line_text) in buffer.buffer_lines().into_iter().enumerate() {
        let screen_line = line_idx as u16;

        // Skip lines that are scrolled out of view
        if screen_line < window.start_line {
            continue;
        }

        let content_line = screen_line - window.start_line;

        // Stop if we've reached the bottom of the content area
        if content_line >= content_height {
            break;
        }

        // Draw gutter for this line
        if show_gutter {
            // Get line status
            let line_status = get_line_status(&line_text, line_idx, &modified_lines, &merged_lines);

            // Draw gutter background
            queue!(
                device,
                cursor::MoveTo(base_content_x, content_y + content_line)
            )?;

            // Status indicator
            let (status_char, status_color) = match line_status {
                LineStatus::Clean => (" ", GUTTER_FG_COLOR),
                LineStatus::Modified => ("│", GUTTER_MODIFIED_COLOR),
                LineStatus::ModifiedSaved => ("│", GUTTER_SAVED_COLOR),
                LineStatus::Conflict => ("!", GUTTER_CONFLICT_COLOR),
            };
            queue!(
                device,
                Print(status_char.with(status_color).on(GUTTER_BG_COLOR))
            )?;

            // Line number (1-based, right-aligned)
            let line_num_str = format_line_number(line_idx + 1, line_number_width);
            queue!(
                device,
                Print(line_num_str.with(GUTTER_FG_COLOR).on(GUTTER_BG_COLOR))
            )?;

            // Separator
            queue!(
                device,
                Print("│".with(GUTTER_SEPARATOR_COLOR).on(GUTTER_BG_COLOR))
            )?;
        }

        // Get the line start position in the buffer (char position)
        let line_start_char = buffer.buffer_line_to_char(line_idx);
        let line_char_count = line_text.chars().count();
        let line_end_char = line_start_char + line_char_count;
        let start_column = window.start_column as usize;

        // Apply horizontal scroll - skip start_column characters, then take content_width
        let line_str = line_text;
        let visible_chars: Vec<char> = line_str
            .chars()
            .skip(start_column)
            .take(content_width as usize)
            .collect();

        // Highlight spans use the same character offsets as the buffer.
        let syntax_spans: Vec<HighlightSpan> =
            buffer.spans_in_range(line_start_char..line_end_char);

        // Move cursor to the start of the text content
        queue!(device, cursor::MoveTo(content_x, content_y + content_line))?;

        // Render character by character with merged highlighting (region + syntax)
        for (char_idx, ch) in visible_chars.iter().enumerate() {
            // Account for horizontal scroll when calculating buffer position (char position)
            let buffer_pos_char = line_start_char + start_column + char_idx;
            // Determine colors: region selection > syntax > default
            let (fg, bg) = if let Some((region_start, region_end)) = region_bounds {
                if buffer_pos_char >= region_start && buffer_pos_char < region_end {
                    // Character is in selection region
                    (Color::Black, Color::Yellow)
                } else {
                    // Check syntax highlighting
                    get_syntax_colors_standalone(
                        buffer_pos_char,
                        &syntax_spans,
                        &face_registry_guard,
                        theme,
                    )
                }
            } else {
                // No region, check syntax highlighting
                get_syntax_colors_standalone(
                    buffer_pos_char,
                    &syntax_spans,
                    &face_registry_guard,
                    theme,
                )
            };

            queue!(device, Print(ch.to_string().with(fg).on(bg)))?;
        }

        // Handle region extending past line content
        if let Some((region_start, region_end)) = region_bounds
            && region_start < line_end_char
            && region_end > line_end_char
        {
            let chars_rendered = visible_chars.len();
            let remaining_width = content_width as usize - chars_rendered;
            if remaining_width > 0 {
                let highlighted_spaces = " ".repeat(remaining_width);
                queue!(
                    device,
                    Print(highlighted_spaces.on(Color::Yellow).with(Color::Black))
                )?;
            }
        }
    }

    // Draw gutter for empty lines (lines that exist in the window but not in buffer)
    if show_gutter {
        let buffer_lines = buffer.buffer_len_lines();
        let first_visible_line = window.start_line as usize;

        for row in 0..content_height as usize {
            let buffer_line = first_visible_line + row;
            if buffer_line >= buffer_lines {
                // This row has no corresponding buffer line - draw empty gutter
                queue!(
                    device,
                    cursor::MoveTo(base_content_x, content_y + row as u16)
                )?;

                // Empty status + tildes for non-existent lines (like vim)
                let empty_gutter = format!(" {:>width$}│", "~", width = line_number_width);
                queue!(
                    device,
                    Print(empty_gutter.with(GUTTER_FG_COLOR).on(GUTTER_BG_COLOR))
                )?;
            }
        }
    }

    Ok(())
}

fn crossterm_modifier_translate(mk: &ModifierKeyCode) -> KeyModifier {
    match mk {
        ModifierKeyCode::LeftAlt => KeyModifier::Alt(Side::Left),
        ModifierKeyCode::RightAlt => KeyModifier::Alt(Side::Right),
        ModifierKeyCode::LeftControl => KeyModifier::Control(Side::Left),
        ModifierKeyCode::RightControl => KeyModifier::Control(Side::Right),
        ModifierKeyCode::LeftShift => KeyModifier::Shift(Side::Left),
        ModifierKeyCode::RightShift => KeyModifier::Shift(Side::Right),
        ModifierKeyCode::LeftSuper => KeyModifier::Super(Side::Left),
        ModifierKeyCode::RightSuper => KeyModifier::Super(Side::Right),
        ModifierKeyCode::LeftHyper => KeyModifier::Hyper(Side::Left),
        ModifierKeyCode::RightHyper => KeyModifier::Hyper(Side::Right),
        ModifierKeyCode::LeftMeta => KeyModifier::Meta(Side::Left),
        ModifierKeyCode::RightMeta => KeyModifier::Meta(Side::Right),
        ModifierKeyCode::IsoLevel3Shift => KeyModifier::Unmapped,
        ModifierKeyCode::IsoLevel5Shift => KeyModifier::Unmapped,
    }
}

fn crossterm_key_translate(ck: &KeyCode, modifiers: KeyModifiers) -> LogicalKey {
    match &ck {
        KeyCode::Backspace => LogicalKey::Backspace,
        KeyCode::Enter => LogicalKey::Enter,
        KeyCode::Left => LogicalKey::Left,
        KeyCode::Right => LogicalKey::Right,
        KeyCode::Up => LogicalKey::Up,
        KeyCode::Down => LogicalKey::Down,
        KeyCode::Home => LogicalKey::Home,
        KeyCode::End => LogicalKey::End,
        KeyCode::PageUp => LogicalKey::PageUp,
        KeyCode::PageDown => LogicalKey::PageDown,
        KeyCode::Tab => LogicalKey::Tab,
        KeyCode::BackTab => LogicalKey::Unmapped,
        KeyCode::Delete => LogicalKey::Delete,
        KeyCode::Insert => LogicalKey::Insert,
        KeyCode::F(f) => LogicalKey::Function(*f),
        KeyCode::Char(c) => {
            // Handle terminal control character translations
            // Ctrl+/ sends 0x1F (Unit Separator) in terminals
            // Ctrl+_ also sends 0x1F
            if modifiers.contains(KeyModifiers::CONTROL) {
                match *c {
                    '\x1f' => LogicalKey::AlphaNumeric('/'), // Ctrl+/ or Ctrl+_
                    '\x00' => LogicalKey::AlphaNumeric(' '), // Ctrl+Space (NUL)
                    _ => LogicalKey::AlphaNumeric(*c),
                }
            } else {
                LogicalKey::AlphaNumeric(*c)
            }
        }
        KeyCode::Null => LogicalKey::Unmapped,
        KeyCode::Esc => LogicalKey::Esc,
        KeyCode::CapsLock => LogicalKey::CapsLock,
        KeyCode::ScrollLock => LogicalKey::ScrollLock,
        KeyCode::NumLock => LogicalKey::Unmapped,
        KeyCode::PrintScreen => LogicalKey::Unmapped,
        KeyCode::Pause => LogicalKey::Unmapped,
        KeyCode::Menu => LogicalKey::Unmapped,
        KeyCode::KeypadBegin => LogicalKey::Unmapped,
        KeyCode::Media(_) => LogicalKey::Unmapped,
        KeyCode::Modifier(m) => LogicalKey::Modifier(crossterm_modifier_translate(m)),
    }
}

pub fn echo(
    device: &mut impl Write,
    editor: &mut Editor,
    message: &str,
    theme: &CachedTheme,
) -> Result<(), std::io::Error> {
    let (x, y) = echo_area_position(&editor.frame);

    // Stash the cursor position
    let cursor_pos = crossterm::cursor::position()?;

    let available_width = editor.frame.columns.saturating_sub(x); // Use full terminal width
    let truncated_message = truncate_echo(message, available_width as usize);
    queue!(device, cursor::MoveTo(x, y), Clear(ClearType::CurrentLine))?;
    queue!(
        device,
        cursor::MoveTo(x, y),
        Print(
            truncated_message
                .as_ref()
                .with(theme.fg_color)
                .on(theme.bg_color)
        )
    )?;
    // Restore the cursor position
    queue!(device, cursor::MoveTo(cursor_pos.0, cursor_pos.1))?;

    device.flush()?;
    Ok(())
}

pub async fn session_event_loop_with_renderer<W: Write>(
    renderer: &mut TerminalRenderer<W>,
    session: &mut HostSession,
    shutdown_requested: &AtomicBool,
) -> Result<(), std::io::Error> {
    let mut event_tick = interval(Duration::from_millis(20));

    loop {
        event_tick.tick().await;
        let tick = session.envelope(InputEvent::Timer { token: 0 });
        let output = session.dispatch(tick).await.map_err(session_io_error)?;
        if apply_session_output(renderer, output)? {
            return Ok(());
        }

        if shutdown_requested.load(Ordering::Acquire) {
            tracing::info!("terminal shutdown requested");
            return Ok(());
        }
        if !crossterm::event::poll(Duration::ZERO)? {
            continue;
        }

        let Some(input) = normalize_terminal_event(crossterm::event::read()?) else {
            continue;
        };
        let envelope = session.envelope(input);
        let output = session.dispatch(envelope).await.map_err(session_io_error)?;
        if apply_session_output(renderer, output)? {
            return Ok(());
        }
    }
}

fn normalize_terminal_event(event: Event) -> Option<InputEvent> {
    match event {
        Event::Key(keystroke) => {
            let mut keys = Vec::new();
            if keystroke.modifiers.contains(KeyModifiers::CONTROL) {
                keys.push(LogicalKey::Modifier(KeyModifier::Control(Side::Left)));
            }
            if keystroke.modifiers.contains(KeyModifiers::ALT) {
                keys.push(LogicalKey::Modifier(KeyModifier::Meta(Side::Left)));
            }
            if keystroke.modifiers.contains(KeyModifiers::SHIFT) {
                keys.push(LogicalKey::Modifier(KeyModifier::Shift(Side::Left)));
            }
            if keystroke.modifiers.contains(KeyModifiers::SUPER) {
                keys.push(LogicalKey::Modifier(KeyModifier::Super(Side::Left)));
            }
            keys.push(crossterm_key_translate(
                &keystroke.code,
                keystroke.modifiers,
            ));
            Some(InputEvent::Keys(keys))
        }
        Event::Resize(columns, rows) => Some(InputEvent::Resize {
            columns,
            rows: rows.saturating_sub(ECHO_AREA_HEIGHT),
        }),
        Event::Mouse(mouse) => normalize_terminal_pointer(mouse),
        Event::FocusGained => Some(InputEvent::Focus(true)),
        Event::FocusLost => Some(InputEvent::Focus(false)),
        Event::Paste(text) => Some(InputEvent::Text(text)),
    }
}

fn normalize_terminal_pointer(mouse: MouseEvent) -> Option<InputEvent> {
    let (kind, button) = match mouse.kind {
        MouseEventKind::Down(button) => (PointerKind::Down, pointer_button(button)),
        MouseEventKind::Drag(button) => (PointerKind::Move, pointer_button(button)),
        MouseEventKind::Up(button) => (PointerKind::Up, pointer_button(button)),
        MouseEventKind::Moved => (PointerKind::Move, PointerButton::None),
        MouseEventKind::ScrollDown
        | MouseEventKind::ScrollUp
        | MouseEventKind::ScrollLeft
        | MouseEventKind::ScrollRight => return None,
    };
    Some(InputEvent::Pointer(PointerEvent {
        column: mouse.column,
        row: mouse.row,
        kind,
        button,
    }))
}

fn pointer_button(button: MouseButton) -> PointerButton {
    match button {
        MouseButton::Left => PointerButton::Primary,
        MouseButton::Right => PointerButton::Secondary,
        MouseButton::Middle => PointerButton::Middle,
    }
}

fn apply_session_output<W: Write>(
    renderer: &mut TerminalRenderer<W>,
    output: SessionOutput,
) -> Result<bool, std::io::Error> {
    let quit = output
        .lifecycle
        .iter()
        .any(|event| matches!(event, LifecycleEvent::QuitRequested));
    for event in &output.lifecycle {
        match event {
            LifecycleEvent::Warning(message) => tracing::warn!(%message, "session warning"),
            LifecycleEvent::Error(message) => tracing::error!(%message, "session error"),
            _ => {}
        }
    }
    if let Some(update) = output.presentation.as_ref() {
        renderer.apply_session_presentation(update)?;
        renderer.render_session()?;
    }
    Ok(quit)
}

fn session_io_error(error: roe_core::session::SessionError) -> std::io::Error {
    std::io::Error::other(error)
}

/// Draw the command window overlay
fn draw_command_window(
    device: &mut impl Write,
    editor: &Editor,
    window_id: WindowId,
    theme: &CachedTheme,
) -> Result<(), std::io::Error> {
    let window = &editor.windows[window_id];

    // Just draw the command window like a normal window with dark blue background
    // The buffer content will handle showing the completions and highlighting
    draw_window(device, editor, window, theme)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_terminal_renderer_creation() {
        let output = Vec::new();
        let renderer = TerminalRenderer::new(output);
        assert!(!renderer.dirty_tracker.is_full_screen_dirty());
    }

    #[test]
    fn test_mark_dirty_functionality() {
        let output = Vec::new();
        let mut renderer = TerminalRenderer::new(output);

        let buffer_id = slotmap::SlotMap::with_key().insert(());

        renderer.mark_dirty(DirtyRegion::Line { buffer_id, line: 5 });
        assert!(renderer.dirty_tracker.is_line_dirty(buffer_id, 5));
        assert!(!renderer.dirty_tracker.is_line_dirty(buffer_id, 4));

        renderer.clear_dirty();
        assert!(!renderer.dirty_tracker.is_line_dirty(buffer_id, 5));
    }

    #[test]
    fn unicode_echo_truncation_preserves_character_boundaries() {
        assert_eq!(truncate_echo("λé猫abc", 5), "λé...");
        assert_eq!(truncate_echo("λé", 5), "λé");
        assert_eq!(truncate_echo("λé", 1), ".");
    }
}
