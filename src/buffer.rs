use crate::ModeId;
use std::cmp::min;

/// Analogous to an emacs buffer.
/// Contains a structure for storing text and metadata like title, and modes.
pub struct Buffer {
    // Title / filename
    pub(crate) object: String,
    /// Modes in order of majority. The first mode is the primary mode, etc.
    /// Like emacs major/minor mode but with N minor modes.
    pub(crate) modes: Vec<ModeId>,
    pub(crate) buffer: ropey::Rope,
}

impl Buffer {
    pub fn new(modes: &[ModeId]) -> Self {
        Self {
            object: String::new(),
            modes: modes.to_vec(),
            buffer: ropey::Rope::new(),
        }
    }

    pub fn load_str(&mut self, text: &str) {
        self.buffer = ropey::Rope::from_str(text);
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
        let bol_pos = self.buffer.line_to_char(line);
        bol_pos
    }

    /// Return the position of the end of the line relative to the start position.
    pub fn eol_pos(&self, start_pos: usize) -> usize {
        let line = self.buffer.char_to_line(start_pos);
        let end_pos = self.buffer.line_to_char(line + 1);
        if end_pos <= self.buffer.len_chars() {
            return end_pos - 1;
        }
        end_pos
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

    pub fn char_index_relative_offset(
        &self,
        position: usize,
        col_offset: isize,
        line_offset: isize,
    ) -> usize {
        // Special handling for when 'pos' is 1 beyond the end of the buffer. This is a legitimate
        // position, but causes issues with calculating the new line position relative to that.
        let position = min(position, self.buffer.len_chars() - 1);

        // Calculate the current position
        let (col, line) = self.to_column_line(position);
        // Calculate the new position
        let col = col as isize + col_offset;
        let line = line as isize + line_offset;
        if line < 0 {
            return 0;
        }
        let new_line_pos = self.buffer.line_to_char(line as usize) as isize;
        let new_pos = (new_line_pos + col) as usize;
        if new_pos > self.buffer.len_chars() {
            return self.buffer.len_chars();
        }
        if new_pos < 0 {
            return 0;
        }
        new_pos
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
        let (col, line) = buffer.to_column_line(7);
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
    fn test_relative_offset_up() {
        let buffer = test_buffer();

        let (col, line) = (0, 2);
        // Move up one line
        let pos = buffer.char_index_relative_offset(buffer.to_char_index(col, line), 0, -1);
        assert_eq!(pos, 6);
        let (col, line) = buffer.to_column_line(pos);
        assert_eq!((col, line), (0, 1));
    }

    // Saw some problems in practice, so verify that hitting up cursor from end of buffer does the
    // right thing.
    // Up one line to the end of the previous line.
    // If the position gets calculated from the very end (1 past the last character), then we
    // end up at the start of the current line instead, which is wrong.
    #[test]
    fn test_relative_up_from_end_of_buffer() {
        let buffer = test_buffer();
        let (col, line) = (6, 2);
        let pos = buffer.to_char_index(col, line);
        assert_eq!(pos, buffer.buffer.len_chars());
        assert_eq!(buffer.buffer.char(pos - 1), '!');
        let pos = buffer.char_index_relative_offset(pos, 0, -1);
        assert_eq!(buffer.buffer.char(pos), '\n');
        // Should now be at the end of the second line
        // assert_eq!(pos, 6);
        let (col, line) = buffer.to_column_line(pos);
        assert_eq!((col, line), (5, 1));
    }

    #[test]
    fn test_relative_offset_right() {
        let buffer = test_buffer();
        let (col, line) = (0, 1);

        // Move right one column
        let pos = buffer.char_index_relative_offset(buffer.to_char_index(col, line), 1, 0);
        assert_eq!(pos, 7);
        let (col, line) = buffer.to_column_line(pos);
        assert_eq!((col, line), (1, 1));
    }

    #[test]
    fn test_relative_offset_left() {
        let buffer = test_buffer();
        let (col, line) = (1, 1);

        // Moving left one, takes us to the start of the line
        let pos = buffer.char_index_relative_offset(buffer.to_char_index(col, line), -1, 0);
        assert_eq!(pos, 6);
        let (col, line) = buffer.to_column_line(pos);
        assert_eq!((col, line), (0, 1));
    }

    #[test]
    fn test_relative_offset_left_to_prev_line() {
        let buffer = test_buffer();
        let (col, line) = (1, 1);
        let pos = buffer.to_char_index(col, line);
        assert_eq!(pos, 7);
        let pos = buffer.char_index_relative_offset(pos, -2, 0);
        assert_eq!(pos, 5);
        let (col, line) = buffer.to_column_line(pos);
        assert_eq!((col, line), (5, 0));
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
}
