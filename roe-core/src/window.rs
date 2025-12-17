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

use crate::editor::Window;

impl Window {
    /// Compute the physical cursor position relative to the window's top.
    /// This is relative to the window, not the frame.
    /// column, line
    pub fn cursor_position(&self, buffer_column: u16, buffer_line: u16) -> (u16, u16) {
        // The screen line should be buffer_line minus start_line (the scroll offset)
        let screen_line = buffer_line.saturating_sub(self.start_line);
        let screen_column = buffer_column;
        (screen_column, screen_line)
    }

    /// Compute the absolute cursor position within the frame
    pub fn absolute_cursor_position(&self, buffer_column: u16, buffer_line: u16) -> (u16, u16) {
        let (rel_col, rel_line) = self.cursor_position(buffer_column, buffer_line);
        // Account for the border: add 1 to both x and y to move inside the border
        // Content area is above the modeline
        (self.x + rel_col + 1, self.y + rel_line + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::WindowType;
    use crate::BufferId;

    fn test_window() -> Window {
        Window {
            x: 0,
            y: 0,
            width_chars: 80,
            height_chars: 22,
            active_buffer: BufferId::default(),
            start_line: 0,
            start_column: 0,
            cursor: 0,
            window_type: WindowType::Normal,
        }
    }

    #[test]
    fn test_cursor_position_basic() {
        let window = test_window();
        let (col, line) = window.cursor_position(5, 3);
        assert_eq!(col, 5);
        assert_eq!(line, 3);
    }

    #[test]
    fn test_cursor_position_with_scroll() {
        let mut window = test_window();
        window.start_line = 10;
        let (col, line) = window.cursor_position(5, 13);
        assert_eq!(col, 5);
        // When start_line is 10 and buffer_line is 13, screen position should be 13-10=3
        assert_eq!(line, 3);
    }

    #[test]
    fn test_cursor_position_at_origin() {
        let window = test_window();
        let (col, line) = window.cursor_position(0, 0);
        assert_eq!(col, 0);
        assert_eq!(line, 0);
    }

    #[test]
    fn test_cursor_position_with_negative_scroll_effect() {
        let mut window = test_window();
        window.start_line = 5;
        let (col, line) = window.cursor_position(2, 2);
        assert_eq!(col, 2);
        // When start_line=5 and buffer_line=2, cursor is above visible area (2-5 would be negative)
        // saturating_sub clamps to 0
        assert_eq!(line, 0);
    }
}
