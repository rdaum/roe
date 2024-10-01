use crate::editor::Window;

impl Window {
    /// Compute the physical cursor position relative to the window's top.
    /// This is relative to the window, not the frame.
    /// column, line
    pub fn cursor_position(&self, buffer_column: u16, buffer_line: u16) -> (u16, u16) {
        let top_line = self.start_line + buffer_line;
        let top_column = buffer_column;
        (top_column, top_line)
    }
}
