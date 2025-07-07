use crate::editor::{ChromeAction, Frame, Window};
use crate::keys::{KeyModifier, LogicalKey, Side};
use crate::renderer::{DirtyRegion, DirtyTracker, ModelineComponent, Renderer};
use crate::{Editor, WindowId};
use crossterm::event::{Event, EventStream, KeyCode, KeyModifiers, ModifierKeyCode};
use crossterm::style::{Color, Print, Stylize};
use crossterm::terminal::{Clear, ClearType};
use crossterm::{cursor, queue};
use futures::{future::FutureExt, select, StreamExt};
use std::io::Write;

pub const ECHO_AREA_HEIGHT: u16 = 1;
pub const BG_COLOR: Color = Color::Black;
pub const FG_COLOR: Color = Color::White;
pub const MODE_LINE_BG_COLOR: Color = Color::Blue;
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

/// Terminal-specific renderer using crossterm
pub struct TerminalRenderer<W: Write> {
    device: W,
    dirty_tracker: DirtyTracker,
}

impl<W: Write> TerminalRenderer<W> {
    pub fn new(device: W) -> Self {
        Self {
            device,
            dirty_tracker: DirtyTracker::new(),
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

        // Check if there's a region selected for highlighting
        let region_bounds = buffer.get_region(window.cursor);

        // Get line content
        if buffer_line >= buffer.buffer_len_lines() {
            // Past end of buffer - clear the entire content line
            let content_x = window.x + 1;
            let content_width = window.width_chars.saturating_sub(2);
            let spaces = " ".repeat(content_width as usize);
            queue!(
                &mut self.device,
                cursor::MoveTo(content_x, screen_row),
                Print(spaces.with(FG_COLOR).on(BG_COLOR))
            )?;
            return Ok(());
        }

        let line_text = buffer.buffer_line(buffer_line);
        // Remove trailing newline if present
        let line_text = line_text.trim_end_matches('\n');
        let line_start_pos = buffer.buffer_line_to_char(buffer_line);

        // Calculate window content area
        let content_x = window.x + 1;
        let content_width = window.width_chars.saturating_sub(2);

        // Position cursor at start of the content area for this line
        queue!(&mut self.device, cursor::MoveTo(content_x, screen_row))?;

        // Clear the entire content line first to avoid artifacts
        let clear_spaces = " ".repeat(content_width as usize);
        queue!(
            &mut self.device,
            Print(clear_spaces.with(FG_COLOR).on(BG_COLOR))
        )?;

        // Position cursor back to start of content area
        queue!(&mut self.device, cursor::MoveTo(content_x, screen_row))?;

        // Extract the characters we need to render (entire line since we cleared it)
        let chars_to_render: Vec<char> = line_text.chars().take(content_width as usize).collect();

        // Render with region highlighting
        if let Some((region_start, region_end)) = region_bounds {
            let line_end_pos = line_start_pos + line_text.len();

            // Check if this entire line is within the region
            if line_start_pos >= region_start && line_end_pos <= region_end {
                // Entire line is highlighted - render text + fill rest of line with highlighted spaces
                let text_to_render: String = chars_to_render.iter().collect();
                queue!(
                    &mut self.device,
                    Print(text_to_render.on(Color::Yellow).with(Color::Black))
                )?;

                // Fill the remaining width with highlighted spaces for full-line highlighting
                let remaining_width = content_width as usize - chars_to_render.len();
                if remaining_width > 0 {
                    let highlighted_spaces = " ".repeat(remaining_width);
                    queue!(
                        &mut self.device,
                        Print(highlighted_spaces.on(Color::Yellow).with(Color::Black))
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
                            Print(ch.to_string().with(FG_COLOR).on(BG_COLOR))
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
                Print(text_to_render.with(FG_COLOR).on(BG_COLOR))
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

        // Calculate modeline position
        let modeline_y = window.y + window.height_chars - 2; // One row above bottom border
        let modeline_x = window.x + 1; // Inside left border
        let modeline_width = window.width_chars.saturating_sub(2) as usize; // Inside both borders

        if modeline_width == 0 {
            return Ok(());
        }

        // If All components are dirty, just redraw the entire modeline
        if dirty_components.contains(&ModelineComponent::All) {
            return draw_window_modeline(&mut self.device, editor, window_id);
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
                        Print(clear_spaces.on(MODE_LINE_BG_COLOR).with(FG_COLOR))
                    )?;

                    // Then write the new position
                    queue!(
                        &mut self.device,
                        cursor::MoveTo(modeline_x + position_start as u16, modeline_y),
                        Print(position_text.on(MODE_LINE_BG_COLOR).with(FG_COLOR))
                    )?;
                }
                ModelineComponent::BufferName => {
                    // For now, redraw entire modeline since buffer name affects layout
                    return draw_window_modeline(&mut self.device, editor, window_id);
                }
                ModelineComponent::ModeName => {
                    // For now, redraw entire modeline since mode name affects layout
                    return draw_window_modeline(&mut self.device, editor, window_id);
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

    fn render_incremental(&mut self, editor: &Editor) -> Result<(), Self::Error> {
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
                let content_height = window.height_chars.saturating_sub(3);

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
            let content_height = window.height_chars.saturating_sub(3);

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

        // Move cursor to correct position and show it
        let active_window = &editor.windows[editor.active_window];
        let (col, line) =
            editor.buffers[active_window.active_buffer].to_column_line(active_window.cursor);
        let (x, y) = active_window.absolute_cursor_position(col, line);
        queue!(&mut self.device, cursor::MoveTo(x, y))?;
        queue!(&mut self.device, cursor::Show)?;

        // Flush all queued commands
        self.device.flush()?;

        Ok(())
    }

    fn render_full(&mut self, editor: &Editor) -> Result<(), Self::Error> {
        // Hide cursor during redraw
        queue!(&mut self.device, cursor::Hide)?;

        // Clear the screen
        queue!(&mut self.device, Clear(ClearType::All))?;

        // Draw all windows
        for window_id in editor.windows.keys() {
            let window = &editor.windows[window_id];
            draw_window(&mut self.device, editor, window)?;
        }

        // Draw all borders and modelines
        draw_all_window_borders(&mut self.device, editor)?;

        // Draw command windows
        for window_id in editor.windows.keys() {
            let window = &editor.windows[window_id];
            if matches!(window.window_type, crate::editor::WindowType::Command { .. }) {
                draw_command_window(&mut self.device, editor, window_id)?;
            }
        }

        // Draw echo area
        if !editor.echo_message.is_empty() {
            let (x, y) = echo_area_position(&editor.frame);
            let available_width = editor.frame.available_columns.saturating_sub(x);
            let truncated_message = if editor.echo_message.len() > available_width as usize {
                &editor.echo_message[..available_width.saturating_sub(3) as usize]
            } else {
                &editor.echo_message
            };
            queue!(&mut self.device, cursor::MoveTo(x, y), Clear(ClearType::CurrentLine))?;
            queue!(
                &mut self.device,
                cursor::MoveTo(x, y),
                Print(truncated_message.with(FG_COLOR).on(BG_COLOR))
            )?;
        }

        // Position cursor and show it
        let active_window = &editor.windows[editor.active_window];
        let (col, line) =
            editor.buffers[active_window.active_buffer].to_column_line(active_window.cursor);
        let (x, y) = active_window.absolute_cursor_position(col, line);
        queue!(&mut self.device, cursor::MoveTo(x, y))?;
        queue!(&mut self.device, cursor::Show)?;

        // Flush
        self.device.flush()?;

        Ok(())
    }

    fn clear_dirty(&mut self) {
        self.dirty_tracker.clear();
    }
}

pub fn echo_area_position(frame: &Frame) -> (u16, u16) {
    (0, frame.rows - ECHO_AREA_HEIGHT)
}

/// Draw borders around all windows in a more sophisticated way that handles adjacency
pub fn draw_all_window_borders(
    device: &mut impl Write,
    editor: &Editor,
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
        draw_single_window_border(device, editor, window_id, &border_grid)?;
    }

    Ok(())
}

/// Draw borders for a single window
fn draw_single_window_border(
    device: &mut impl Write,
    editor: &Editor,
    window_id: WindowId,
    _border_grid: &[Vec<char>],
) -> Result<(), std::io::Error> {
    let window = &editor.windows[window_id];
    let is_active = window_id == editor.active_window;
    let border_color = if is_active {
        ACTIVE_BORDER_COLOR
    } else {
        BORDER_COLOR
    };

    // Only draw borders if the window has space for them
    if window.width_chars < 2 || window.height_chars < 2 {
        return Ok(());
    }

    let right = window.x + window.width_chars - 1;
    let bottom = window.y + window.height_chars - 1;
    let modeline_row = bottom - 1; // Modeline is one row above the bottom border

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

    // Draw modeline separator (horizontal line above bottom border)
    if window.x + 1 < right {
        queue!(
            device,
            cursor::MoveTo(window.x, modeline_row),
            Print(BORDER_T_RIGHT.with(border_color))
        )?;
        let horizontal_line = BORDER_HORIZONTAL.repeat((right - window.x - 1) as usize);
        queue!(
            device,
            cursor::MoveTo(window.x + 1, modeline_row),
            Print(horizontal_line.with(border_color))
        )?;
        queue!(
            device,
            cursor::MoveTo(right, modeline_row),
            Print(BORDER_T_LEFT.with(border_color))
        )?;
    }

    // Draw bottom horizontal border
    if window.x + 1 < right {
        let horizontal_line = BORDER_HORIZONTAL.repeat((right - window.x - 1) as usize);
        queue!(
            device,
            cursor::MoveTo(window.x + 1, bottom),
            Print(horizontal_line.with(border_color))
        )?;
    }

    // Draw vertical borders (excluding modeline row)
    for y in window.y + 1..modeline_row {
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

    // Draw vertical borders for modeline row to bottom
    for y in (modeline_row + 1)..bottom {
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
    draw_window_modeline(device, editor, window_id)?;

    Ok(())
}

/// Draw the modeline for a specific window
fn draw_window_modeline(
    device: &mut impl Write,
    editor: &Editor,
    window_id: WindowId,
) -> Result<(), std::io::Error> {
    let window = &editor.windows[window_id];
    let buffer = &editor.buffers[window.active_buffer];

    // Calculate modeline position and width
    let modeline_y = window.y + window.height_chars - 2; // One row above bottom border
    let modeline_x = window.x + 1; // Inside left border
    let modeline_width = window.width_chars.saturating_sub(2) as usize; // Inside both borders

    if modeline_width == 0 {
        return Ok(());
    }

    // Move to modeline position
    queue!(device, cursor::MoveTo(modeline_x, modeline_y))?;

    // Create modeline content
    let mut modeline_content = String::new();

    // Add buffer object name
    let object_part = format!(" {} ", buffer.object());
    modeline_content.push_str(&object_part);

    // Add mode name
    if let Some(mode_id) = buffer.modes().first() {
        if let Some(mode) = editor.modes.get(*mode_id) {
            let mode_part = format!("({}) ", mode.name());
            modeline_content.push_str(&mode_part);
        }
    }

    // Add cursor position
    let (col, line) = buffer.to_column_line(window.cursor);
    let position_part = format!("{}:{} ", line + 1, col + 1); // 1-based for display

    // Calculate remaining space for position (right-aligned)
    let used_space = modeline_content.len() + position_part.len();
    let remaining_space = modeline_width.saturating_sub(used_space);

    // Fill with spaces to right-align position
    modeline_content.push_str(&" ".repeat(remaining_space));
    modeline_content.push_str(&position_part);

    // Truncate if too long
    if modeline_content.len() > modeline_width {
        modeline_content.truncate(modeline_width);
    } else if modeline_content.len() < modeline_width {
        // Pad with spaces to fill the entire modeline
        modeline_content.push_str(&" ".repeat(modeline_width - modeline_content.len()));
    }

    // Draw with modeline colors
    queue!(
        device,
        Print(modeline_content.on(MODE_LINE_BG_COLOR).with(FG_COLOR))
    )?;

    Ok(())
}

/// Redraw the entire buffer in a window.
pub fn draw_window(
    device: &mut impl Write,
    editor: &Editor,
    window: &Window,
) -> Result<(), std::io::Error> {
    // Draw the buffer in the window
    let buffer = &editor.buffers[window.active_buffer];

    // Calculate content area (inside the border, above the modeline)
    let content_x = window.x + 1;
    let content_y = window.y + 1;
    let content_width = window.width_chars.saturating_sub(2);
    let content_height = window.height_chars.saturating_sub(3); // Reserve space for modeline

    // Clear the content area first (only the content, not the whole line)
    for row in 0..content_height {
        let spaces = " ".repeat(content_width as usize);
        queue!(
            device,
            cursor::MoveTo(content_x, content_y + row),
            Print(spaces.with(FG_COLOR).on(BG_COLOR))
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
                        queue!(device, Print(ch.to_string().with(FG_COLOR).on(BG_COLOR)))?;
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
            queue!(device, Print(truncated_line.with(FG_COLOR).on(BG_COLOR)))?;
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
) -> Result<(), std::io::Error> {
    let (x, y) = echo_area_position(&editor.frame);

    // Stash the cursor position
    let cursor_pos = crossterm::cursor::position()?;

    let available_width = editor.frame.available_columns.saturating_sub(x);
    let truncated_message = if message.len() > available_width as usize {
        &message[..available_width.saturating_sub(3) as usize]
    } else {
        message
    };
    queue!(device, cursor::MoveTo(x, y), Clear(ClearType::CurrentLine))?;
    queue!(
        device,
        cursor::MoveTo(x, y),
        Print(truncated_message.with(FG_COLOR).on(BG_COLOR))
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

    loop {
        // Get the next event asynchronously
        let mut maybe_event = event_stream.next().fuse();
        let event = select! {
            event = maybe_event => {
                match event {
                    Some(Ok(event)) => event,
                    Some(Err(e)) => return Err(e),
                    None => continue, // Stream ended, shouldn't happen but handle gracefully
                }
            }
        };
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
            _ => vec![],
        };

        // Display the keys pressed in echo with - between, using as_display_string, but only if there's
        // modifiers in play
        let actions = editor.key_event(keys)?;

        for action in actions {
            match action {
                ChromeAction::Echo(message) => {
                    // Set the echo message in the editor and render it
                    editor.set_echo_message(message.clone());
                    echo(&mut renderer.device, editor, &message)?;
                }

                ChromeAction::FileOpen => {
                    // TODO: Implement file open dialog
                }
                ChromeAction::CommandMode => {
                    // TODO: Implement command mode
                }
                ChromeAction::Huh => {}
                ChromeAction::Quit => {
                    return Ok(());
                }
                ChromeAction::CursorMove((_col, _line)) => {
                    // TODO: Handle cursor movement if needed
                }
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
    window_id: crate::WindowId,
) -> Result<(), std::io::Error> {
    let window = &editor.windows[window_id];
    
    // Just draw the command window like a normal window with dark blue background
    // The buffer content will handle showing the completions and highlighting
    draw_window(device, editor, window)?;
    
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
        assert!(renderer.dirty_tracker.is_line_dirty(5));
        assert!(!renderer.dirty_tracker.is_line_dirty(4));

        renderer.clear_dirty();
        assert!(!renderer.dirty_tracker.is_line_dirty(5));
    }
}
