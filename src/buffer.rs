use crate::ModeId;

/// Analogous to an emacs buffer.
/// Contains a structure for storing text and metadata like title, and modes.
pub struct Buffer {
    // Title / filename
    pub(crate) object: String,
    /// Modes in order of majority. The first mode is the primary mode, etc.
    /// Like emacs major/minor mode but with N minor modes.
    pub(crate) modes: Vec<ModeId>,
    pub(crate) buffer: ropey::Rope,
    /// Mark position for region selection (None = no mark set)
    pub(crate) mark: Option<usize>,
}

impl Buffer {
    pub fn new(modes: &[ModeId]) -> Self {
        Self {
            object: String::new(),
            modes: modes.to_vec(),
            buffer: ropey::Rope::new(),
            mark: None,
        }
    }

    pub fn load_str(&mut self, text: &str) {
        self.buffer = ropey::Rope::from_str(text);
    }

    /// Create a new buffer and load content from a file
    pub fn from_file(file_path: &str, modes: &[ModeId]) -> Result<Self, std::io::Error> {
        use std::fs;

        let content = fs::read_to_string(file_path)?;
        let buffer = Self {
            object: file_path.to_string(),
            modes: modes.to_vec(),
            buffer: ropey::Rope::from_str(&content),
            mark: None,
        };
        Ok(buffer)
    }

    /// Save buffer content to file
    pub fn save_to_file(&self, file_path: &str) -> Result<(), std::io::Error> {
        use std::fs;
        use std::io::Write;

        let mut file = fs::File::create(file_path)?;
        file.write_all(self.buffer.to_string().as_bytes())?;
        file.sync_all()?;
        Ok(())
    }

    pub fn content(&self) -> String {
        self.buffer.to_string()
    }

    /// Insert a fragment of text into the buffer at the given line/col position.
    pub fn insert_col_line(&mut self, fragment: String, position: (u16, u16)) {
        let buffer_location = self.buffer.line_to_char(position.1 as usize) + position.0 as usize;
        self.insert_pos(fragment, buffer_location);
    }

    pub fn insert_pos(&mut self, fragment: String, position: usize) {
        self.buffer.insert(position, &fragment);
    }

    /// Delete a fragment of text from the buffer at the given line/col position.
    /// Returns the deleted text.
    pub fn delete_col_line(&mut self, position: (u16, u16), count: isize) -> Option<String> {
        let buffer_location = self.buffer.line_to_char(position.1 as usize) + position.0 as usize;
        self.delete_pos(buffer_location, count)
    }

    pub fn delete_pos(&mut self, position: usize, count: isize) -> Option<String> {
        let position = position as isize;

        // If count is negative then start is buffer_location - count and end is buffer_location
        // Otherwise start is buffer_location and end is buffer_location + count
        let (start, end) = if count < 0 {
            (position + count, position)
        } else {
            (position, position + count)
        };

        if start < 0 || end as usize > self.buffer.len_chars() {
            return None;
        }

        let deleted = self.buffer.slice(start as usize..end as usize).to_string();
        self.buffer.remove(start as usize..end as usize);
        Some(deleted)
    }

    /// Return the position of the start of the line relative to the start position
    pub fn bol_pos(&self, start_pos: usize) -> usize {
        let line = self.buffer.char_to_line(start_pos);

        self.buffer.line_to_char(line)
    }

    /// Return the position of the end of the line relative to the start position.
    pub fn eol_pos(&self, start_pos: usize) -> usize {
        // Handle empty buffer
        if self.buffer.len_chars() == 0 {
            return 0;
        }

        // If we're already at or beyond the end of the buffer, stay there
        if start_pos >= self.buffer.len_chars() {
            return self.buffer.len_chars();
        }

        let line = self.buffer.char_to_line(start_pos);
        let line_count = self.buffer.len_lines();

        if line + 1 < line_count {
            // Not the last line - end of line is just before the newline
            let next_line_start = self.buffer.line_to_char(line + 1);
            next_line_start - 1 // Position of the newline
        } else {
            // Last line - end of line is end of buffer
            self.buffer.len_chars()
        }
    }

    pub fn to_column_line(&self, char_index: usize) -> (u16, u16) {
        let line = self.buffer.char_to_line(char_index);
        let col = char_index - self.buffer.line_to_char(line);
        (col as u16, line as u16)
    }

    pub fn to_char_index(&self, col: u16, line: u16) -> usize {
        let linestart_pos = self.buffer.line_to_char(line as usize);
        linestart_pos + col as usize
    }

    // === PHASE 1: CLEAN CHARACTER-POSITION API ===

    /// Move cursor left by one character. O(1)
    pub fn move_left(&self, pos: usize) -> usize {
        pos.saturating_sub(1)
    }

    /// Move cursor right by one character. O(1)  
    pub fn move_right(&self, pos: usize) -> usize {
        (pos + 1).min(self.buffer.len_chars())
    }

    /// Move cursor up one line, preserving column when possible. O(log N)
    pub fn move_up(&self, pos: usize) -> usize {
        if self.buffer.len_chars() == 0 {
            return 0;
        }

        let line = self.buffer.char_to_line(pos);
        if line == 0 {
            return pos; // Already at top
        }

        let current_line_start = self.buffer.line_to_char(line);
        let column = pos - current_line_start;

        let target_line = line - 1;
        let target_line_start = self.buffer.line_to_char(target_line);
        let target_line_len = self.line_length(target_line);

        target_line_start + column.min(target_line_len)
    }

    /// Move cursor down one line, preserving column when possible. O(log N)
    pub fn move_down(&self, pos: usize) -> usize {
        if self.buffer.len_chars() == 0 {
            return 0;
        }

        let line = self.buffer.char_to_line(pos);
        let total_lines = self.buffer.len_lines();
        if line + 1 >= total_lines {
            return pos; // Already at bottom
        }

        let current_line_start = self.buffer.line_to_char(line);
        let column = pos - current_line_start;

        let target_line = line + 1;
        let target_line_start = self.buffer.line_to_char(target_line);
        let target_line_len = self.line_length(target_line);

        target_line_start + column.min(target_line_len)
    }

    /// Move cursor to start of current line. O(log N)
    pub fn move_line_start(&self, pos: usize) -> usize {
        if self.buffer.len_chars() == 0 {
            return 0;
        }

        let line = self.buffer.char_to_line(pos);
        self.buffer.line_to_char(line)
    }

    /// Move cursor to end of current line. O(log N)
    pub fn move_line_end(&self, pos: usize) -> usize {
        self.eol_pos(pos)
    }

    /// Move cursor to start of buffer. O(1)
    pub fn move_buffer_start(&self) -> usize {
        0
    }

    /// Move cursor to end of buffer. O(1)
    pub fn move_buffer_end(&self) -> usize {
        self.buffer.len_chars()
    }

    /// Get the length of a line (excluding newline). O(log N)
    pub fn line_length(&self, line: usize) -> usize {
        if line >= self.buffer.len_lines() {
            return 0;
        }

        let line_start = self.buffer.line_to_char(line);
        if line + 1 < self.buffer.len_lines() {
            let next_line_start = self.buffer.line_to_char(line + 1);
            next_line_start - line_start - 1 // -1 for newline
        } else {
            self.buffer.len_chars() - line_start
        }
    }

    /// Ensure position is within buffer bounds. O(1)
    pub fn clamp_position(&self, pos: usize) -> usize {
        pos.min(self.buffer.len_chars())
    }

    // === MARK AND REGION OPERATIONS ===

    /// Set the mark at the given position
    pub fn set_mark(&mut self, pos: usize) {
        self.mark = Some(self.clamp_position(pos));
    }

    /// Clear the mark
    pub fn clear_mark(&mut self) {
        self.mark = None;
    }

    /// Get the current mark position
    pub fn get_mark(&self) -> Option<usize> {
        self.mark
    }

    /// Check if mark is set
    pub fn has_mark(&self) -> bool {
        self.mark.is_some()
    }

    /// Get the region bounds (start, end) between mark and cursor
    /// Returns None if no mark is set
    /// start <= end always (handles mark before/after cursor)
    pub fn get_region(&self, cursor_pos: usize) -> Option<(usize, usize)> {
        let mark_pos = self.mark?;
        let cursor_pos = self.clamp_position(cursor_pos);
        let mark_pos = self.clamp_position(mark_pos);

        if mark_pos <= cursor_pos {
            Some((mark_pos, cursor_pos))
        } else {
            Some((cursor_pos, mark_pos))
        }
    }

    /// Get the text content of the current region
    /// Returns None if no mark is set
    pub fn get_region_text(&self, cursor_pos: usize) -> Option<String> {
        let (start, end) = self.get_region(cursor_pos)?;
        if start == end {
            Some(String::new())
        } else {
            Some(self.buffer.slice(start..end).to_string())
        }
    }

    /// Delete the region and return the deleted text and new cursor position
    /// Returns None if no mark is set
    pub fn delete_region(&mut self, cursor_pos: usize) -> Option<(String, usize)> {
        let (start, end) = self.get_region(cursor_pos)?;
        if start == end {
            self.clear_mark();
            return Some((String::new(), cursor_pos));
        }

        let deleted = self.buffer.slice(start..end).to_string();
        self.buffer.remove(start..end);
        self.clear_mark();
        // Cursor should be at the start of the deleted region
        Some((deleted, start))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_buffer() -> Buffer {
        let mut buffer = Buffer::new(&[]);
        buffer.load_str("Hello\ncruel\nworld!");
        buffer
    }

    // Verify that position conversions are symmetrical
    #[test]
    fn test_position() {
        let buffer = test_buffer();
        let (col, line) = buffer.to_column_line(7);
        let pos = buffer.to_char_index(col, line);
        assert_eq!(pos, 7);
    }

    // Verify that line adjustments work
    #[test]
    fn test_position_conversions() {
        let buffer = test_buffer();
        let (_col, line) = buffer.to_column_line(7);
        let pos = buffer.to_char_index(5, line - 1);
        // Go up to the end of previous line (the \n)
        assert_eq!(pos, 5);
        // Go down to the start of the next line
        let pos = buffer.to_char_index(0, line + 1);
        assert_eq!(pos, 12);

        // Convert those positions back to line/col
        let (col, line) = buffer.to_column_line(5);
        assert_eq!(col, 5);
        assert_eq!(line, 0);

        let (col, line) = buffer.to_column_line(12);
        assert_eq!(col, 0);
        assert_eq!(line, 2);
    }

    #[test]
    fn test_insert() {
        let mut buffer = Buffer::new(&[]);
        buffer.load_str("Hello, world!");
        buffer.insert_col_line("cruel ".to_string(), (7, 0));
        assert_eq!(buffer.content(), "Hello, cruel world!");

        buffer.insert_col_line("world!".to_string(), (0, 0));
        assert_eq!(buffer.content(), "world!Hello, cruel world!");

        buffer.insert_col_line("Hello, ".to_string(), (0, 1));
        assert_eq!(buffer.content(), "world!Hello, cruel world!Hello, ");
    }

    #[test]
    fn test_delete() {
        let mut buffer = Buffer::new(&[]);
        buffer.load_str("Hello, cruel world!");
        assert_eq!(
            buffer.delete_col_line((7, 0), 6),
            Some("cruel ".to_string())
        );
        assert_eq!(buffer.content(), "Hello, world!");

        assert_eq!(
            buffer.delete_col_line((0, 0), 6),
            Some("Hello,".to_string())
        );
        assert_eq!(buffer.content(), " world!");

        assert_eq!(
            buffer.delete_col_line((0, 0), 7),
            Some(" world!".to_string())
        );
        assert_eq!(buffer.content(), "");

        assert_eq!(buffer.delete_col_line((0, 0), 1), None);
    }

    #[test]
    fn test_delete_backwards() {
        let mut buffer = Buffer::new(&[]);
        buffer.load_str("Hello, cruel world!");
        assert_eq!(
            buffer.delete_col_line((13, 0), -6),
            Some("cruel ".to_string())
        );
        assert_eq!(buffer.content(), "Hello, world!");

        assert_eq!(
            buffer.delete_col_line((6, 0), -6),
            Some("Hello,".to_string())
        );
        assert_eq!(buffer.content(), " world!");

        assert_eq!(buffer.delete_col_line((0, 0), -7), None);
        assert_eq!(buffer.content(), " world!");

        assert_eq!(
            buffer.delete_col_line((7, 0), -7),
            Some(" world!".to_string())
        );
        assert_eq!(buffer.content(), "");

        // Emulate backspace
        buffer.load_str("Hello, cruel world!");
        assert_eq!(buffer.delete_col_line((13, 0), -1), Some(" ".to_string()));
        assert_eq!(buffer.content(), "Hello, cruelworld!");
    }

    #[test]
    fn test_eol_pos_middle_of_line() {
        let buffer = test_buffer(); // "Hello\ncruel\nworld!"

        // From middle of first line
        let eol = buffer.eol_pos(2); // From 'l' in "Hello"
        let (_col, line) = buffer.to_column_line(eol);
        assert_eq!(line, 0);
        // Should go to after 'o' in "Hello" (position 5)
        assert_eq!(eol, 5);
    }

    #[test]
    fn test_eol_pos_last_line() {
        let buffer = test_buffer(); // "Hello\ncruel\nworld!"

        // From start of last line
        let eol = buffer.eol_pos(12); // From 'w' in "world!"
        let (_col, line) = buffer.to_column_line(eol);
        assert_eq!(line, 2);
        // Should go to after '!' (end of buffer)
        assert_eq!(eol, 18);
    }

    #[test]
    fn test_eol_pos_end_of_buffer() {
        let buffer = test_buffer(); // "Hello\ncruel\nworld!"
        let buffer_len = buffer.buffer.len_chars();

        // From end of buffer
        let eol = buffer.eol_pos(buffer_len);

        // When already at end of buffer, should stay at end
        assert_eq!(eol, buffer_len);
    }

    #[test]
    fn test_eol_pos_already_at_eol() {
        let buffer = test_buffer(); // "Hello\ncruel\nworld!"

        // From end of first line (position 5, after 'o')
        let eol = buffer.eol_pos(5);
        let (_col, line) = buffer.to_column_line(eol);
        assert_eq!(line, 0);
        // Should stay at end of line
        assert_eq!(eol, 5);
    }

    #[test]
    fn test_simple_movement_api() {
        let buffer = test_buffer(); // "Hello\ncruel\nworld!"

        // Test horizontal movement
        assert_eq!(buffer.move_left(5), 4); // From end of "Hello"
        assert_eq!(buffer.move_left(0), 0); // From start - should stay
        assert_eq!(buffer.move_right(4), 5); // To end of "Hello"
        assert_eq!(buffer.move_right(18), 18); // From end - should stay

        // Test line movement
        assert_eq!(buffer.move_line_start(3), 0); // From middle of "Hello" to start
        assert_eq!(buffer.move_line_end(0), 5); // From start to end of line

        // Test buffer movement
        assert_eq!(buffer.move_buffer_start(), 0);
        assert_eq!(buffer.move_buffer_end(), 18);
    }

    #[test]
    fn test_vertical_movement_api() {
        let buffer = test_buffer(); // "Hello\ncruel\nworld!"

        // Move up from "cruel" to "Hello"
        let pos = buffer.move_up(8); // From 'u' in "cruel"
        let (col, line) = buffer.to_column_line(pos);
        assert_eq!(line, 0); // First line
        assert_eq!(col, 2); // Same column position ('l' in "Hello")

        // Move down from "Hello" to "cruel"
        let pos = buffer.move_down(2); // From 'l' in "Hello"
        let (col, line) = buffer.to_column_line(pos);
        assert_eq!(line, 1); // Second line
        assert_eq!(col, 2); // Same column position ('u' in "cruel")
    }

    #[test]
    fn test_movement_edge_cases() {
        let buffer = test_buffer(); // "Hello\ncruel\nworld!"

        // Move up from first line - should stay
        assert_eq!(buffer.move_up(3), 3);

        // Move down from last line - should stay
        assert_eq!(buffer.move_down(15), 15);

        // Test with empty buffer
        let empty_buffer = Buffer::new(&[]);
        assert_eq!(empty_buffer.move_up(0), 0);
        assert_eq!(empty_buffer.move_down(0), 0);
        assert_eq!(empty_buffer.move_left(0), 0);
        assert_eq!(empty_buffer.move_right(0), 0);
    }

    #[test]
    fn test_phase1_api_handles_original_edge_cases() {
        let buffer = test_buffer(); // "Hello\ncruel\nworld!"

        // Test case from failing test_char_index_relative_offset_column_bounds
        // Move far left from position 7 - should go to start of buffer
        let pos = buffer.move_left(7);
        // Keep moving left until we hit start
        let mut current = pos;
        for _ in 0..10 {
            current = buffer.move_left(current);
        }
        assert_eq!(current, 0); // Should reach start of buffer

        // Test case from failing test_relative_offset_left_to_prev_line
        // Position 7 is 'u' in "cruel", move left should go to 'r'
        assert_eq!(buffer.move_left(7), 6);
    }

    #[test]
    fn test_mark_operations() {
        let mut buffer = test_buffer(); // "Hello\ncruel\nworld!"

        // Initially no mark
        assert!(!buffer.has_mark());
        assert_eq!(buffer.get_mark(), None);

        // Set mark at position 5 (end of "Hello")
        buffer.set_mark(5);
        assert!(buffer.has_mark());
        assert_eq!(buffer.get_mark(), Some(5));

        // Clear mark
        buffer.clear_mark();
        assert!(!buffer.has_mark());
        assert_eq!(buffer.get_mark(), None);
    }

    #[test]
    fn test_region_operations() {
        let mut buffer = test_buffer(); // "Hello\ncruel\nworld!"

        // No mark set - should return None
        assert_eq!(buffer.get_region(5), None);
        assert_eq!(buffer.get_region_text(5), None);

        // Set mark at position 2 ('l' in "Hello")
        buffer.set_mark(2);

        // Test region from mark to cursor at position 7 ('u' in "cruel")
        let region = buffer.get_region(7);
        assert_eq!(region, Some((2, 7))); // start <= end

        // Test region text
        let region_text = buffer.get_region_text(7);
        assert_eq!(region_text, Some("llo\nc".to_string()));

        // Test reverse region (cursor before mark)
        let region = buffer.get_region(0);
        assert_eq!(region, Some((0, 2))); // start <= end (swapped)

        let region_text = buffer.get_region_text(0);
        assert_eq!(region_text, Some("He".to_string()));
    }

    #[test]
    fn test_region_delete() {
        let mut buffer = test_buffer(); // "Hello\ncruel\nworld!"

        // No mark set - should return None
        assert_eq!(buffer.delete_region(5), None);

        // Set mark at position 2 ('l' in "Hello")
        buffer.set_mark(2);

        // Delete region from mark to cursor at position 7 ('u' in "cruel")
        let result = buffer.delete_region(7);
        assert_eq!(result, Some(("llo\nc".to_string(), 2)));

        // Check buffer content after deletion
        assert_eq!(buffer.content(), "Heruel\nworld!");

        // Mark should be cleared
        assert!(!buffer.has_mark());
    }

    #[test]
    fn test_empty_region() {
        let mut buffer = test_buffer(); // "Hello\ncruel\nworld!"

        // Set mark at position 5
        buffer.set_mark(5);

        // Get region at same position (empty region)
        let region = buffer.get_region(5);
        assert_eq!(region, Some((5, 5)));

        let region_text = buffer.get_region_text(5);
        assert_eq!(region_text, Some(String::new()));

        // Delete empty region
        let result = buffer.delete_region(5);
        assert_eq!(result, Some((String::new(), 5)));

        // Buffer should be unchanged
        assert_eq!(buffer.content(), "Hello\ncruel\nworld!");

        // Mark should be cleared
        assert!(!buffer.has_mark());
    }

    #[test]
    fn test_region_bounds_clamping() {
        let mut buffer = test_buffer(); // "Hello\ncruel\nworld!"
        let buffer_len = buffer.buffer.len_chars();

        // Set mark beyond buffer end
        buffer.set_mark(buffer_len + 10);
        assert_eq!(buffer.get_mark(), Some(buffer_len)); // Should be clamped

        // Get region with cursor beyond buffer end
        let region = buffer.get_region(buffer_len + 5);
        assert_eq!(region, Some((buffer_len, buffer_len))); // Both should be clamped
    }
}
