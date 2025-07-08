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

use crossterm::event::{
    Event, EventStream, KeyCode, KeyModifiers, ModifierKeyCode, MouseButton, MouseEvent,
    MouseEventKind,
};
use crossterm::style::{Color, Print, Stylize};
use crossterm::terminal::{Clear, ClearType};
use crossterm::{cursor, queue};
use futures::{future::FutureExt, select, StreamExt};
use roe_core::editor::{BorderInfo, ChromeAction, DragType, Frame, MouseDragState, Window};
use roe_core::keys::{KeyModifier, LogicalKey, Side};
use roe_core::renderer::{DirtyRegion, DirtyTracker, ModelineComponent, Renderer};
use roe_core::{Editor, WindowId};
use std::io::Write;
use tokio::time::{interval, Duration};

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

/// Parse a hex color string (e.g., "#272822") to crossterm Color
fn parse_hex_color(hex: &str) -> Color {
    if hex.starts_with('#') && hex.len() == 7 {
        if let Ok(r) = u8::from_str_radix(&hex[1..3], 16) {
            if let Ok(g) = u8::from_str_radix(&hex[3..5], 16) {
                if let Ok(b) = u8::from_str_radix(&hex[5..7], 16) {
                    return Color::Rgb { r, g, b };
                }
            }
        }
    }
    Color::White // fallback
}

/// Cached theme colors loaded from Julia at startup
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

/// Load theme colors from Julia runtime at startup
pub async fn load_julia_theme(editor: &Editor) -> CachedTheme {
    let mut theme = CachedTheme::default();
    let mut loaded_colors = Vec::new();

    if let Some(ref julia_runtime) = editor.julia_runtime {
        // Load colours/colors from Julia config (supporting both Canadian and American spelling)

        // Try "colours" first (Canadian), then "colors" (American)
        let bg_result = {
            let runtime = julia_runtime.lock().await;
            match runtime.get_config("colours.background").await {
                Ok(Some(value)) => Ok(Some(value)),
                _ => runtime.get_config("colors.background").await,
            }
        };
        if let Ok(Some(bg)) = bg_result {
            if let Some(color_str) = bg.as_string() {
                loaded_colors.push(format!("bg:{color_str}"));
                let parsed_color = parse_hex_color(&color_str);
                theme.bg_color = parsed_color;
            }
        }

        let fg_result = {
            let runtime = julia_runtime.lock().await;
            match runtime.get_config("colours.foreground").await {
                Ok(Some(value)) => Ok(Some(value)),
                _ => runtime.get_config("colors.foreground").await,
            }
        };
        if let Ok(Some(fg)) = fg_result {
            if let Some(color_str) = fg.as_string() {
                loaded_colors.push(format!("fg:{color_str}"));
                let parsed_color = parse_hex_color(&color_str);
                theme.fg_color = parsed_color;
            }
        }

        let sel_result = {
            let runtime = julia_runtime.lock().await;
            match runtime.get_config("colours.selection").await {
                Ok(Some(value)) => Ok(Some(value)),
                _ => runtime.get_config("colors.selection").await,
            }
        };
        if let Ok(Some(sel)) = sel_result {
            if let Some(color_str) = sel.as_string() {
                loaded_colors.push(format!("sel:{color_str}"));
                let parsed_color = parse_hex_color(&color_str);
                theme.selection_color = parsed_color;
            }
        }

        // Note: loaded_colors is used for tracking what was loaded
        let _ = loaded_colors;
    }

    // Return the configured theme

    theme
}

/// Terminal-specific renderer using crossterm
pub struct TerminalRenderer<W: Write> {
    device: W,
    dirty_tracker: DirtyTracker,
    theme: CachedTheme,
}

impl<W: Write> TerminalRenderer<W> {
    pub fn new(device: W) -> Self {
        Self {
            device,
            dirty_tracker: DirtyTracker::new(),
            theme: CachedTheme::default(),
        }
    }

    pub fn new_with_theme(device: W, theme: CachedTheme) -> Self {
        Self {
            device,
            dirty_tracker: DirtyTracker::new(),
            theme,
        }
    }

    /// Render a single line with proper highlighting
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
        let buffer = &editor.buffers[window.active_buffer];

        // Only show region highlighting in the active window
        let region_bounds = if window_id == editor.active_window {
            buffer.get_region(window.cursor)
        } else {
            None
        };

        if buffer_line >= buffer.buffer_len_lines() {
            // Past end of buffer - clear the entire content line
            let content_x = window.x + 1;
            let content_width = window.width_chars.saturating_sub(2);
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
        let line_start_pos = buffer.buffer_line_to_char(buffer_line);

        let content_x = window.x + 1;
        let content_width = window.width_chars.saturating_sub(2);

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

        let chars_to_render: Vec<char> = line_text.chars().take(content_width as usize).collect();
        if let Some((region_start, region_end)) = region_bounds {
            let line_end_pos = line_start_pos + line_text.len();

            // Check if this entire line is within the region
            if line_start_pos >= region_start && line_end_pos <= region_end {
                // Entire line is highlighted - render text + fill rest of line with highlighted spaces
                let text_to_render: String = chars_to_render.iter().collect();
                queue!(
                    &mut self.device,
                    Print(
                        text_to_render
                            .on(self.theme.selection_color)
                            .with(Color::Black)
                    )
                )?;

                // Fill the remaining width with highlighted spaces for full-line highlighting
                let remaining_width = content_width as usize - chars_to_render.len();
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
            } else {
                // Partial line highlighting - render character by character
                for (char_idx, ch) in chars_to_render.iter().enumerate() {
                    let buffer_pos = line_start_pos + char_idx;

                    // Check if this character is within the selected region
                    if buffer_pos >= region_start && buffer_pos < region_end {
                        // Highlighted character - yellow background with black text
                        queue!(
                            &mut self.device,
                            Print(ch.to_string().on(Color::Yellow).with(Color::Black))
                        )?;
                    } else {
                        // Normal character
                        queue!(
                            &mut self.device,
                            Print(
                                ch.to_string()
                                    .with(self.theme.fg_color)
                                    .on(self.theme.bg_color)
                            )
                        )?;
                    }
                }

                // For partial highlighting, if the region extends past the line content,
                // fill remaining space with highlighted background
                if region_start < line_end_pos && region_end > line_end_pos {
                    let chars_rendered = chars_to_render.len();
                    let remaining_width = content_width as usize - chars_rendered;
                    if remaining_width > 0 {
                        let highlighted_spaces = " ".repeat(remaining_width);
                        queue!(
                            &mut self.device,
                            Print(highlighted_spaces.on(Color::Yellow).with(Color::Black))
                        )?;
                    }
                }
            }
        } else {
            // No region selected, render normally
            let text_to_render: String = chars_to_render.iter().collect();
            queue!(
                &mut self.device,
                Print(
                    text_to_render
                        .with(self.theme.fg_color)
                        .on(self.theme.bg_color)
                )
            )?;
        }

        // Line is already cleared at the beginning, no need to clear again

        Ok(())
    }

    /// Render specific modeline components that are dirty
    fn render_modeline_components(
        &mut self,
        editor: &Editor,
        window_id: WindowId,
        dirty_components: &std::collections::HashSet<ModelineComponent>,
    ) -> Result<(), std::io::Error> {
        let window = &editor.windows[window_id];
        let buffer = &editor.buffers[window.active_buffer];
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
                // Only collect specific dirty lines if buffer is not entirely dirty
                dirty_lines = self
                    .dirty_tracker
                    .dirty_lines_iter()
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

        // Move cursor to correct position and show it
        let active_window = &editor.windows[editor.active_window];
        let (col, line) =
            editor.buffers[active_window.active_buffer].to_column_line(active_window.cursor);
        let (x, y) = active_window.absolute_cursor_position(col, line);
        queue!(&mut self.device, cursor::MoveTo(x, y))?;
        queue!(&mut self.device, cursor::Show)?;

        // Flush cursor positioning commands
        self.device.flush()?;

        Ok(())
    }

    fn render_full(&mut self, editor: &Editor) -> Result<(), std::io::Error> {
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
            let truncated_message = if editor.echo_message.len() > available_width as usize {
                &editor.echo_message[..available_width.saturating_sub(3) as usize]
            } else {
                &editor.echo_message
            };
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
                        .with(self.theme.fg_color)
                        .on(self.theme.bg_color)
                )
            )?;
        }

        // Flush all drawing commands first
        self.device.flush()?;

        // Position cursor and show it
        let active_window = &editor.windows[editor.active_window];
        let (col, line) =
            editor.buffers[active_window.active_buffer].to_column_line(active_window.cursor);
        let (x, y) = active_window.absolute_cursor_position(col, line);
        queue!(&mut self.device, cursor::MoveTo(x, y))?;
        queue!(&mut self.device, cursor::Show)?;

        // Flush cursor positioning commands
        self.device.flush()?;

        Ok(())
    }

    fn clear_dirty(&mut self) {
        self.dirty_tracker.clear();
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
    let buffer = &editor.buffers[window.active_buffer];
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

    // Add mode name
    if let Some(mode_id) = buffer.modes().first() {
        if let Some(mode) = editor.modes.get(*mode_id) {
            let mode_part = format!("({}) ", mode.name());
            rest_content.push_str(&mode_part);
        }
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

/// Redraw the entire buffer in a window.
pub fn draw_window(
    device: &mut impl Write,
    editor: &Editor,
    window: &Window,
    theme: &CachedTheme,
) -> Result<(), std::io::Error> {
    // Draw the buffer in the window
    let buffer = &editor.buffers[window.active_buffer];

    // Calculate content area (inside the border, above the modeline which is now in the bottom border)
    let content_x = window.x + 1;
    let content_y = window.y + 1;
    let content_width = window.width_chars.saturating_sub(2);
    let content_height = window.height_chars.saturating_sub(2); // Only subtract for top and bottom borders

    // Clear the content area first (only the content, not the whole line)
    for row in 0..content_height {
        let spaces = " ".repeat(content_width as usize);
        queue!(
            device,
            cursor::MoveTo(content_x, content_y + row),
            Print(spaces.with(theme.fg_color).on(theme.bg_color))
        )?;
    }

    // Check if there's a region selected for highlighting
    let region_bounds = buffer.get_region(window.cursor);

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

        // Get the line start position in the buffer
        let line_start_pos = buffer.buffer_line_to_char(line_idx);

        // Truncate line if it's too long for the content area
        let line_str = line_text;
        let truncated_line = if line_str.len() > content_width as usize {
            &line_str[..content_width as usize]
        } else {
            &line_str
        };

        // Move cursor to the start of the line
        queue!(device, cursor::MoveTo(content_x, content_y + content_line))?;

        // Draw the line with region highlighting
        if let Some((region_start, region_end)) = region_bounds {
            let line_end_pos = line_start_pos + line_str.len();

            // Check if this entire line is within the region
            if line_start_pos >= region_start && line_end_pos <= region_end {
                // Entire line is highlighted - render text + fill rest of line with highlighted spaces
                queue!(
                    device,
                    Print(truncated_line.on(Color::Yellow).with(Color::Black))
                )?;

                // Fill the remaining width with highlighted spaces for full-line highlighting
                let remaining_width = content_width as usize - truncated_line.len();
                if remaining_width > 0 {
                    let highlighted_spaces = " ".repeat(remaining_width);
                    queue!(
                        device,
                        Print(highlighted_spaces.on(Color::Yellow).with(Color::Black))
                    )?;
                }
            } else {
                // Partial line highlighting - render character by character
                for (char_idx, ch) in truncated_line.chars().enumerate() {
                    let buffer_pos = line_start_pos + char_idx;

                    // Check if this character is within the selected region
                    if buffer_pos >= region_start && buffer_pos < region_end {
                        // Highlighted character - yellow background with black text
                        queue!(
                            device,
                            Print(ch.to_string().on(Color::Yellow).with(Color::Black))
                        )?;
                    } else {
                        // Normal character
                        queue!(
                            device,
                            Print(ch.to_string().with(theme.fg_color).on(theme.bg_color))
                        )?;
                    }
                }

                // For partial highlighting, if the region extends past the line content,
                // fill remaining space with highlighted background
                if region_start < line_end_pos && region_end > line_end_pos {
                    let chars_rendered = truncated_line.len();
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
        } else {
            // No region selected, draw normally
            queue!(
                device,
                Print(truncated_line.with(theme.fg_color).on(theme.bg_color))
            )?;
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

fn crossterm_key_translate(ck: &KeyCode) -> LogicalKey {
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
        KeyCode::Char(c) => LogicalKey::AlphaNumeric(*c),
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
    let truncated_message = if message.len() > available_width as usize {
        &message[..available_width.saturating_sub(3) as usize]
    } else {
        message
    };
    queue!(device, cursor::MoveTo(x, y), Clear(ClearType::CurrentLine))?;
    queue!(
        device,
        cursor::MoveTo(x, y),
        Print(truncated_message.with(theme.fg_color).on(theme.bg_color))
    )?;
    // Restore the cursor position
    queue!(device, cursor::MoveTo(cursor_pos.0, cursor_pos.1))?;

    device.flush()?;
    Ok(())
}

pub async fn event_loop_with_renderer<W: Write>(
    renderer: &mut TerminalRenderer<W>,
    editor: &mut Editor,
) -> Result<(), std::io::Error> {
    let mut event_stream = EventStream::new();
    let mut echo_timer = interval(Duration::from_millis(500)); // Check every 500ms

    loop {
        // Get the next event asynchronously
        let event = select! {
            event = event_stream.next().fuse() => {
                match event {
                    Some(Ok(event)) => Some(event),
                    Some(Err(e)) => return Err(e),
                    None => continue, // Stream ended, shouldn't happen but handle gracefully
                }
            }
            _ = echo_timer.tick().fuse() => None, // Timer tick, check for expired echo
        };

        // Handle timer tick (check for expired echo messages)
        if event.is_none() {
            if editor.check_and_clear_expired_echo() {
                // Echo message expired, trigger a redraw
                renderer.render_full(editor)?;
            }
            continue;
        }

        let event = event.expect("Event stream should provide valid events");
        let keys = match event {
            Event::Key(keystroke) => {
                let key = crossterm_key_translate(&keystroke.code);

                let mut keys = vec![];

                // Modifiers first
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

                // Then key.
                keys.push(key);
                keys
            }
            Event::Resize(width, height) => {
                // Handle terminal resize event - subtract echo area height
                editor.handle_resize(width, height.saturating_sub(ECHO_AREA_HEIGHT));
                // Trigger full screen redraw
                renderer.mark_dirty(DirtyRegion::FullScreen);
                // No keys to process for resize event
                vec![]
            }
            Event::Mouse(mouse_event) => {
                // Handle mouse events for window resizing
                handle_mouse_event(editor, renderer, mouse_event).await;
                // No keys to process for mouse events
                vec![]
            }
            _ => vec![],
        };

        // Display the keys pressed in echo with - between, using as_display_string, but only if there's
        // modifiers in play
        let actions = if keys.is_empty() {
            // No keys to process (e.g., mouse events, resize events)
            vec![]
        } else {
            editor.key_event(keys)?
        };

        for action in actions {
            match action {
                ChromeAction::Echo(message) => {
                    // Set the echo message in the editor and render it
                    editor.set_echo_message(message.clone());
                    echo(&mut renderer.device, editor, &message, &renderer.theme)?;
                }

                ChromeAction::OpenFile(_) => {}
                ChromeAction::CommandMode => {}
                ChromeAction::SwitchBuffer => {}
                ChromeAction::KillBuffer => {}
                ChromeAction::Save => {}
                ChromeAction::Huh => {}
                ChromeAction::Quit => {
                    return Ok(());
                }
                ChromeAction::CursorMove((_col, _line)) => {}
                ChromeAction::MarkDirty(dirty_region) => {
                    renderer.mark_dirty(dirty_region);
                }
                ChromeAction::SplitHorizontal => {
                    editor.split_horizontal();
                    renderer.mark_dirty(DirtyRegion::FullScreen);
                }
                ChromeAction::SplitVertical => {
                    editor.split_vertical();
                    renderer.mark_dirty(DirtyRegion::FullScreen);
                }
                ChromeAction::SwitchWindow => {
                    editor.switch_window();
                    renderer.mark_dirty(DirtyRegion::FullScreen);
                }
                ChromeAction::DeleteWindow => {
                    if editor.delete_window() {
                        renderer.mark_dirty(DirtyRegion::FullScreen);
                    }
                }
                ChromeAction::DeleteOtherWindows => {
                    if editor.delete_other_windows() {
                        renderer.mark_dirty(DirtyRegion::FullScreen);
                    }
                }
                ChromeAction::ShowMessages => {
                    // Switch to the Messages buffer
                    let messages_buffer_id = editor.get_messages_buffer();
                    if let Some(current_window) = editor.windows.get_mut(editor.active_window) {
                        current_window.active_buffer = messages_buffer_id;
                        current_window.cursor = 0; // Start at beginning of messages
                    }
                    renderer.mark_dirty(DirtyRegion::FullScreen);
                }
                ChromeAction::NewBufferWithMode {
                    buffer_name,
                    mode_name,
                    initial_content,
                } => {
                    // Create a new buffer with the specified mode
                    let cursor_pos = initial_content.len();
                    if let Some(buffer_id) =
                        editor.create_buffer_with_mode(buffer_name, mode_name, initial_content)
                    {
                        // Switch current window to the new buffer
                        if let Some(current_window) = editor.windows.get_mut(editor.active_window) {
                            current_window.active_buffer = buffer_id;
                            current_window.cursor = cursor_pos; // Position cursor at end of initial content
                        }
                        renderer.mark_dirty(DirtyRegion::FullScreen);
                    }
                }
            }
        }

        // Render any dirty regions
        renderer.render_incremental(editor)?;
        renderer.clear_dirty();
    }
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

/// Handle mouse events for window resizing
async fn handle_mouse_event<W: Write>(
    editor: &mut Editor,
    renderer: &mut TerminalRenderer<W>,
    mouse_event: MouseEvent,
) {
    match mouse_event.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if let Some((border_info, target_window)) =
                detect_border_click(editor, mouse_event.column, mouse_event.row)
            {
                editor.mouse_drag_state = Some(MouseDragState {
                    drag_type: DragType::WindowBorder,
                    start_pos: (mouse_event.column, mouse_event.row),
                    last_pos: (mouse_event.column, mouse_event.row),
                    current_pos: (mouse_event.column, mouse_event.row),
                    target_window: Some(target_window),
                    border_info: Some(border_info),
                });
                return;
            }

            let Some(window_id) =
                find_window_at_position(editor, mouse_event.column, mouse_event.row)
            else {
                return;
            };

            if editor.active_window != window_id {
                editor.previous_active_window = Some(editor.active_window);
                editor.active_window = window_id;
                renderer.mark_dirty(DirtyRegion::FullScreen);
            }

            let window = &editor.windows[window_id];
            let relative_x = mouse_event.column.saturating_sub(window.x + 1);
            let relative_y = mouse_event.row.saturating_sub(window.y + 1);
            let buffer_row = relative_y + window.start_line;
            let buffer_col = relative_x;

            let mode_mouse_event = roe_core::mode::MouseEvent {
                position: (buffer_col, buffer_row),
                event_type: roe_core::mode::MouseEventType::LeftClick,
            };

            let Some(actions) = handle_mode_mouse_event(editor, window_id, &mode_mouse_event).await
            else {
                return;
            };

            for action in actions {
                if let ChromeAction::MarkDirty(dirty_region) = action {
                    renderer.mark_dirty(dirty_region);
                }
            }
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            let Some(drag_state) = editor.mouse_drag_state.clone() else {
                return;
            };

            let new_pos = (mouse_event.column, mouse_event.row);
            let dx = new_pos.0 as i32 - drag_state.last_pos.0 as i32;
            let dy = new_pos.1 as i32 - drag_state.last_pos.1 as i32;

            if dx == 0 && dy == 0 {
                return;
            }

            if let Some(ref mut drag_state_mut) = editor.mouse_drag_state {
                drag_state_mut.current_pos = new_pos;
                drag_state_mut.last_pos = new_pos;
            }

            let Some(border_info) = drag_state.border_info else {
                return;
            };

            update_window_resize_incremental(
                editor,
                drag_state.target_window,
                &border_info,
                dx,
                dy,
            );
            renderer.mark_dirty(DirtyRegion::FullScreen);
        }
        MouseEventKind::Up(MouseButton::Left) => {
            // End dragging
            if editor.mouse_drag_state.is_some() {
                editor.mouse_drag_state = None;
                renderer.mark_dirty(DirtyRegion::FullScreen);
            }
        }
        _ => {
            // Ignore other mouse events for now
        }
    }
}

/// Find which window contains the given screen position
fn find_window_at_position(editor: &Editor, x: u16, y: u16) -> Option<WindowId> {
    for (window_id, window) in &editor.windows {
        // Check if position is within window content area (not on borders)
        let content_left = window.x + 1; // +1 for left border
        let content_right = window.x + window.width_chars - 1; // -1 for right border
        let content_top = window.y;
        let content_bottom = window.y + window.height_chars - 1;

        if x >= content_left && x < content_right && y >= content_top && y <= content_bottom {
            return Some(window_id);
        }
    }
    None
}

/// Handle mouse events for modes
async fn handle_mode_mouse_event(
    editor: &mut Editor,
    window_id: WindowId,
    mouse_event: &roe_core::mode::MouseEvent,
) -> Option<Vec<roe_core::editor::ChromeAction>> {
    let window = &editor.windows[window_id];
    let buffer_id = window.active_buffer;
    let cursor_pos = window.cursor;

    if let Some(buffer_host) = editor.buffer_hosts.get(&buffer_id) {
        // Send mouse event to buffer host and wait for response
        if let Ok(response) = buffer_host
            .handle_mouse(mouse_event.clone(), cursor_pos)
            .await
        {
            // Process the response like key events do
            return Some(editor.handle_buffer_response(response).await);
        }
    }
    None
}

/// Detect if a mouse click is on a window border
fn detect_border_click(editor: &Editor, x: u16, y: u16) -> Option<(BorderInfo, WindowId)> {
    // Check all windows to see if the click is on a border
    for (window_id, window) in &editor.windows {
        // Check if click is on window borders
        let left_border = window.x;
        let right_border = window.x + window.width_chars - 1;
        let top_border = window.y;
        let bottom_border = window.y + window.height_chars - 1;

        // Check vertical borders (left and right sides)
        if (x == left_border || x == right_border) && y >= top_border && y <= bottom_border {
            // This is a vertical border
            if let Some(split_info) = find_split_for_border(editor, window_id, true) {
                return Some((
                    BorderInfo {
                        is_vertical: true,
                        split_node_path: split_info.0,
                        original_ratio: split_info.1,
                    },
                    window_id,
                ));
            }
        }

        // Check horizontal borders (top and bottom sides)
        if (y == top_border || y == bottom_border) && x >= left_border && x <= right_border {
            // This is a horizontal border
            if let Some(split_info) = find_split_for_border(editor, window_id, false) {
                return Some((
                    BorderInfo {
                        is_vertical: false,
                        split_node_path: split_info.0,
                        original_ratio: split_info.1,
                    },
                    window_id,
                ));
            }
        }
    }

    None
}

/// Find the split node that controls the given border
fn find_split_for_border(
    editor: &Editor,
    window_id: WindowId,
    is_vertical_border: bool,
) -> Option<(Vec<usize>, f32)> {
    // This is a simplified implementation
    // In a real implementation, we would traverse the window tree to find the exact split node
    // For now, we'll return a placeholder that works with simple two-window splits

    // Find if this window has a sibling that shares the border
    for (other_window_id, other_window) in &editor.windows {
        if other_window_id == window_id {
            continue;
        }

        let window = &editor.windows[window_id];

        if is_vertical_border {
            // Check if windows are horizontally adjacent
            if (window.x + window.width_chars == other_window.x
                || other_window.x + other_window.width_chars == window.x)
                && window.y < other_window.y + other_window.height_chars
                && other_window.y < window.y + window.height_chars
            {
                return Some((vec![0], 0.5)); // Simplified path and ratio
            }
        } else {
            // Check if windows are vertically adjacent
            if (window.y + window.height_chars == other_window.y
                || other_window.y + other_window.height_chars == window.y)
                && window.x < other_window.x + other_window.width_chars
                && other_window.x < window.x + window.width_chars
            {
                return Some((vec![0], 0.5)); // Simplified path and ratio
            }
        }
    }

    None
}

/// Update window layout based on incremental mouse drag
fn update_window_resize_incremental(
    editor: &mut Editor,
    target_window_id: Option<WindowId>,
    border_info: &BorderInfo,
    dx: i32,
    dy: i32,
) {
    // Use incremental changes with much finer granularity
    if let Some(_target_window_id) = target_window_id {
        // Use a sensitivity factor to make resizing smoother
        // Each pixel of mouse movement = 0.5% ratio change (adjustable)
        const SENSITIVITY: f32 = 0.005;

        // Calculate the incremental ratio change
        if border_info.is_vertical && dx != 0 {
            // For vertical borders, adjust the split ratio based on horizontal movement
            let ratio_change = dx as f32 * SENSITIVITY;
            adjust_window_tree_ratio_incremental(&mut editor.window_tree, ratio_change, true);
        } else if !border_info.is_vertical && dy != 0 {
            // For horizontal borders, adjust the split ratio based on vertical movement
            let ratio_change = dy as f32 * SENSITIVITY;
            adjust_window_tree_ratio_incremental(&mut editor.window_tree, ratio_change, false);
        }

        // Recalculate layout to apply the new ratios
        editor.calculate_window_layout();
    }
}

/// Recursively adjust window tree ratios for incremental resizing
fn adjust_window_tree_ratio_incremental(
    node: &mut roe_core::editor::WindowNode,
    ratio_change: f32,
    is_vertical: bool,
) {
    use roe_core::editor::{SplitDirection, WindowNode};

    match node {
        WindowNode::Leaf { .. } => {
            // Nothing to adjust for leaf nodes
        }
        WindowNode::Split {
            direction,
            ratio,
            first,
            second,
        } => {
            // Only adjust if the split direction matches the resize direction
            let should_adjust = match direction {
                SplitDirection::Vertical => is_vertical,
                SplitDirection::Horizontal => !is_vertical,
            };

            if should_adjust {
                // Adjust the ratio incrementally, keeping it within bounds
                // Use tighter bounds to prevent extreme layouts
                *ratio = (*ratio + ratio_change).clamp(0.15, 0.85);
            } else {
                // Recurse into child nodes
                adjust_window_tree_ratio_incremental(first, ratio_change, is_vertical);
                adjust_window_tree_ratio_incremental(second, ratio_change, is_vertical);
            }
        }
    }
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
        assert!(renderer.dirty_tracker.is_line_dirty(5));
        assert!(!renderer.dirty_tracker.is_line_dirty(4));

        renderer.clear_dirty();
        assert!(!renderer.dirty_tracker.is_line_dirty(5));
    }
}
