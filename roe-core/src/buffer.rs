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

use crate::syntax::{FaceId, HighlightSpan, SpanStore};
use crate::undo::{EditOp, UndoManager};
use crate::ModeId;
use std::ops::Range;
use std::sync::{Arc, RwLock};

/// The internal data structure for a buffer
/// Contains the actual text and metadata
pub struct BufferInner {
    // Title / filename
    pub(crate) object: String,
    /// Modes in order of majority. The first mode is the primary mode, etc.
    /// Like emacs major/minor mode but with N minor modes.
    pub(crate) modes: Vec<ModeId>,
    pub(crate) buffer: ropey::Rope,
    /// Mark position for region selection (None = no mark set)
    pub(crate) mark: Option<usize>,
    /// Whether the mark is transient (CUA-style shift-select) vs persistent (Emacs C-Space)
    /// Transient marks are cleared on non-shift cursor movement
    pub(crate) transient_mark: bool,
    /// Syntax highlighting spans (auto-adjusted on edits)
    pub(crate) spans: SpanStore,
    /// Major mode name (e.g., "julia-mode", "fundamental-mode")
    pub(crate) major_mode: Option<String>,
    /// Whether to show the gutter (line numbers, status) for this buffer
    pub(crate) show_gutter: bool,
    /// Undo/redo history manager
    pub(crate) undo_manager: UndoManager,
}

impl BufferInner {
    pub fn new(modes: &[ModeId]) -> Self {
        Self {
            object: String::new(),
            modes: modes.to_vec(),
            buffer: ropey::Rope::new(),
            mark: None,
            transient_mark: false,
            spans: SpanStore::new(),
            major_mode: None,
            show_gutter: false, // Default to no gutter for scratch buffers
            undo_manager: UndoManager::new(),
        }
    }

    pub fn load_str(&mut self, text: &str) {
        self.buffer = ropey::Rope::from_str(text);
    }

    /// Create a new buffer inner and load content from a file
    pub async fn from_file(file_path: &str, modes: &[ModeId]) -> Result<Self, std::io::Error> {
        let content = tokio::fs::read_to_string(file_path).await?;
        let buffer_inner = Self {
            object: file_path.to_string(),
            modes: modes.to_vec(),
            buffer: ropey::Rope::from_str(&content),
            mark: None,
            transient_mark: false,
            spans: SpanStore::new(),
            major_mode: None,
            show_gutter: true, // Default to show gutter for file buffers
            undo_manager: UndoManager::new(),
        };
        Ok(buffer_inner)
    }

    /// Insert a fragment of text into the buffer at the given line/col position.
    pub fn insert_col_line(&mut self, fragment: String, position: (u16, u16)) {
        let buffer_location = self.buffer.line_to_char(position.1 as usize) + position.0 as usize;
        self.insert_pos(fragment, buffer_location);
    }

    pub fn insert_pos(&mut self, fragment: String, position: usize) {
        let len = fragment.chars().count();
        // Record for undo before modifying
        self.undo_manager.record_insert(position, fragment.clone());
        self.buffer.insert(position, &fragment);
        // Adjust highlight spans for the insertion
        self.spans.adjust_for_insert(position, len);
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
        // Record for undo before modifying
        self.undo_manager
            .record_delete(start as usize, deleted.clone());
        self.buffer.remove(start as usize..end as usize);
        // Adjust highlight spans for the deletion
        self.spans.adjust_for_delete(start as usize, end as usize);
        Some(deleted)
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
        // Clamp to valid range to prevent panic from stale cursor positions
        let char_index = self.clamp_position(char_index);
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

    /// Move cursor forward by one word. O(N) where N is chars to scan
    pub fn move_word_forward(&self, pos: usize) -> usize {
        if self.buffer.len_chars() == 0 {
            return 0;
        }

        let mut current_pos = self.clamp_position(pos);
        let buffer_len = self.buffer.len_chars();

        if current_pos >= buffer_len {
            return buffer_len;
        }

        // Skip any whitespace we're currently in
        while current_pos < buffer_len {
            let ch = self.buffer.char(current_pos);
            if !ch.is_whitespace() {
                break;
            }
            current_pos += 1;
        }

        // Skip the current word (non-whitespace chars)
        while current_pos < buffer_len {
            let ch = self.buffer.char(current_pos);
            if ch.is_whitespace() {
                break;
            }
            current_pos += 1;
        }

        // Skip trailing whitespace to get to start of next word
        while current_pos < buffer_len {
            let ch = self.buffer.char(current_pos);
            if !ch.is_whitespace() {
                break;
            }
            current_pos += 1;
        }

        current_pos
    }

    /// Move cursor backward by one word. O(N) where N is chars to scan
    pub fn move_word_backward(&self, pos: usize) -> usize {
        if self.buffer.len_chars() == 0 {
            return 0;
        }

        let mut current_pos = self.clamp_position(pos);

        if current_pos == 0 {
            return 0;
        }

        // Move back one position to start
        current_pos = current_pos.saturating_sub(1);

        // Skip any whitespace we're currently in (moving backwards)
        while current_pos > 0 {
            let ch = self.buffer.char(current_pos);
            if !ch.is_whitespace() {
                break;
            }
            current_pos = current_pos.saturating_sub(1);
        }

        // Skip the current word (moving backwards through non-whitespace)
        while current_pos > 0 {
            let ch = self.buffer.char(current_pos.saturating_sub(1));
            if ch.is_whitespace() {
                break;
            }
            current_pos = current_pos.saturating_sub(1);
        }

        current_pos
    }

    /// Check if a line is blank (contains only whitespace)
    fn is_line_blank(&self, line_idx: usize) -> bool {
        if line_idx >= self.buffer.len_lines() {
            return true;
        }
        let line_text = self.buffer.line(line_idx);
        line_text.chars().all(|c| c.is_whitespace())
    }

    /// Move cursor forward by one paragraph. O(N) where N is lines to scan
    pub fn move_paragraph_forward(&self, pos: usize) -> usize {
        if self.buffer.len_chars() == 0 {
            return 0;
        }

        let current_pos = self.clamp_position(pos);
        let current_line = self.buffer.char_to_line(current_pos);
        let total_lines = self.buffer.len_lines();

        let mut target_line = current_line;

        // Skip current paragraph (non-blank lines)
        while target_line < total_lines && !self.is_line_blank(target_line) {
            target_line += 1;
        }

        // Skip blank lines
        while target_line < total_lines && self.is_line_blank(target_line) {
            target_line += 1;
        }

        // If we reached end of buffer, return end
        if target_line >= total_lines {
            return self.buffer.len_chars();
        }

        // Return start of the target line
        self.buffer.line_to_char(target_line)
    }

    /// Move cursor backward by one paragraph. O(N) where N is lines to scan
    pub fn move_paragraph_backward(&self, pos: usize) -> usize {
        if self.buffer.len_chars() == 0 {
            return 0;
        }

        let current_pos = self.clamp_position(pos);
        let current_line = self.buffer.char_to_line(current_pos);

        if current_line == 0 {
            return 0; // Already at start
        }

        // First, find the start of the current paragraph
        let mut line_idx = current_line;

        // If we're already at the start of a non-blank line, we're at paragraph start
        let line_start_pos = self.buffer.line_to_char(current_line);
        let already_at_paragraph_start =
            current_pos == line_start_pos && !self.is_line_blank(current_line);

        if already_at_paragraph_start {
            // We're at the start of a paragraph, move to previous paragraph
            line_idx = line_idx.saturating_sub(1);

            // Skip blank lines backwards
            while line_idx > 0 && self.is_line_blank(line_idx) {
                line_idx = line_idx.saturating_sub(1);
            }

            // Skip current paragraph backwards (non-blank lines)
            while line_idx > 0 && !self.is_line_blank(line_idx) {
                line_idx = line_idx.saturating_sub(1);
            }

            // If we stopped on a blank line, skip forward to start of paragraph
            if self.is_line_blank(line_idx) && line_idx + 1 < self.buffer.len_lines() {
                line_idx += 1;
            }
        } else {
            // We're in the middle of a paragraph, move to start of current paragraph
            // Skip backwards to find start of current paragraph
            while line_idx > 0 {
                let prev_line = line_idx - 1;
                if self.is_line_blank(prev_line) {
                    break; // Found start of current paragraph
                }
                line_idx = prev_line;
            }
        }

        // Return start of the target line
        self.buffer.line_to_char(line_idx)
    }

    // === MARK AND REGION OPERATIONS ===

    /// Set the mark at the given position (persistent, Emacs C-Space style)
    pub fn set_mark(&mut self, pos: usize) {
        self.mark = Some(self.clamp_position(pos));
        self.transient_mark = false;
    }

    /// Set a transient mark at the given position (CUA-style shift-select)
    /// Transient marks are cleared on non-shift cursor movement
    pub fn set_transient_mark(&mut self, pos: usize) {
        self.mark = Some(self.clamp_position(pos));
        self.transient_mark = true;
    }

    /// Clear the mark
    pub fn clear_mark(&mut self) {
        self.mark = None;
        self.transient_mark = false;
    }

    /// Clear the mark only if it's transient (CUA-style)
    /// Returns true if the mark was cleared
    pub fn clear_transient_mark(&mut self) -> bool {
        if self.transient_mark && self.mark.is_some() {
            self.mark = None;
            self.transient_mark = false;
            true
        } else {
            false
        }
    }

    /// Get the current mark position
    pub fn get_mark(&self) -> Option<usize> {
        self.mark
    }

    /// Check if mark is set
    pub fn has_mark(&self) -> bool {
        self.mark.is_some()
    }

    /// Check if the current mark is transient (CUA-style shift-select)
    pub fn is_transient_mark(&self) -> bool {
        self.transient_mark && self.mark.is_some()
    }

    pub fn content(&self) -> String {
        self.buffer.to_string()
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
        // Record for undo before modifying
        self.undo_manager.record_delete(start, deleted.clone());
        self.buffer.remove(start..end);
        // Adjust highlight spans for the deletion
        self.spans.adjust_for_delete(start, end);
        self.clear_mark();
        // Cursor should be at the start of the deleted region
        Some((deleted, start))
    }

    /// Delete a range of text from start to end (exclusive), returns deleted text
    pub fn delete_range(&mut self, start: usize, end: usize) -> Option<String> {
        if start >= end || end > self.buffer.len_chars() {
            return None;
        }

        let deleted = self.buffer.slice(start..end).to_string();
        // Record for undo before modifying
        self.undo_manager.record_delete(start, deleted.clone());
        self.buffer.remove(start..end);
        // Adjust highlight spans for the deletion
        self.spans.adjust_for_delete(start, end);
        Some(deleted)
    }

    // === UNDO/REDO OPERATIONS ===

    /// Perform undo, returns the new cursor position if successful
    pub fn undo(&mut self) -> Option<usize> {
        let op = self.undo_manager.pop_undo()?;
        let cursor = self.apply_edit_op(&op.reverse());
        self.undo_manager.did_undo(op);
        Some(cursor)
    }

    /// Perform redo, returns the new cursor position if successful
    pub fn redo(&mut self) -> Option<usize> {
        let op = self.undo_manager.pop_redo()?;
        let cursor = self.apply_edit_op(&op);
        self.undo_manager.did_redo(op);
        Some(cursor)
    }

    /// Apply an edit operation without recording it (used for undo/redo)
    fn apply_edit_op(&mut self, op: &EditOp) -> usize {
        match op {
            EditOp::Insert { pos, text } => {
                let len = text.chars().count();
                self.buffer.insert(*pos, text);
                self.spans.adjust_for_insert(*pos, len);
                pos + len
            }
            EditOp::Delete { pos, text } => {
                let end = pos + text.chars().count();
                self.buffer.remove(*pos..end);
                self.spans.adjust_for_delete(*pos, end);
                *pos
            }
            EditOp::Group(ops) => {
                let mut cursor = 0;
                for op in ops {
                    cursor = self.apply_edit_op(op);
                }
                cursor
            }
        }
    }

    /// Check if undo is available
    pub fn can_undo(&self) -> bool {
        self.undo_manager.can_undo()
    }

    /// Check if redo is available
    pub fn can_redo(&self) -> bool {
        self.undo_manager.can_redo()
    }

    /// Begin a group of operations for undo
    pub fn begin_undo_group(&mut self) {
        self.undo_manager.begin_group();
    }

    /// End a group of operations for undo
    pub fn end_undo_group(&mut self) {
        self.undo_manager.end_group();
    }

    /// Insert an undo boundary - seals current auto-group, next edit starts fresh
    /// Call this on cursor movement, certain commands, etc.
    pub fn undo_boundary(&mut self) {
        self.undo_manager.boundary();
    }

    // === SYNTAX HIGHLIGHTING SPAN OPERATIONS ===

    /// Add a highlight span to the buffer
    pub fn add_span(&mut self, span: HighlightSpan) {
        self.spans.add_span(span);
    }

    /// Add multiple highlight spans at once
    pub fn add_spans(&mut self, spans: impl IntoIterator<Item = HighlightSpan>) {
        self.spans.add_spans(spans);
    }

    /// Clear all highlight spans
    pub fn clear_spans(&mut self) {
        self.spans.clear();
    }

    /// Clear highlight spans in a specific range (for incremental re-highlighting)
    pub fn clear_spans_in_range(&mut self, range: Range<usize>) {
        self.spans.clear_range(range);
    }

    /// Get the face ID at a specific position
    pub fn face_at(&mut self, pos: usize) -> Option<FaceId> {
        self.spans.face_at(pos)
    }

    /// Get all spans that overlap with a range
    pub fn spans_in_range(&mut self, range: Range<usize>) -> Vec<&HighlightSpan> {
        self.spans.spans_in_range(range)
    }

    /// Get all spans (for debugging/inspection)
    pub fn all_spans(&mut self) -> &[HighlightSpan] {
        self.spans.all_spans()
    }

    /// Check if buffer has any highlight spans
    pub fn has_spans(&self) -> bool {
        !self.spans.is_empty()
    }
}

/// Public Buffer interface that handles synchronization internally
/// This is what the rest of the codebase should use
pub struct Buffer {
    inner: Arc<RwLock<BufferInner>>,
}

impl std::fmt::Debug for Buffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Buffer").finish_non_exhaustive()
    }
}

impl Buffer {
    /// Create a new buffer
    pub fn new(modes: &[ModeId]) -> Self {
        Self {
            inner: Arc::new(RwLock::new(BufferInner::new(modes))),
        }
    }

    /// Create a new buffer and load content from a file
    pub async fn from_file(file_path: &str, modes: &[ModeId]) -> Result<Self, std::io::Error> {
        let buffer_inner = BufferInner::from_file(file_path, modes).await?;
        Ok(Self {
            inner: Arc::new(RwLock::new(buffer_inner)),
        })
    }

    /// Execute a closure with read access to the buffer
    pub fn with_read<R>(&self, f: impl FnOnce(&BufferInner) -> R) -> R {
        f(&self
            .inner
            .read()
            .expect("Buffer lock should not be poisoned"))
    }

    /// Execute a closure with write access to the buffer
    pub fn with_write<R>(&self, f: impl FnOnce(&mut BufferInner) -> R) -> R {
        f(&mut self
            .inner
            .write()
            .expect("Buffer lock should not be poisoned"))
    }

    // Convenience methods for common operations that don't need multiple calls

    pub fn to_column_line(&self, char_index: usize) -> (u16, u16) {
        self.with_read(|b| b.to_column_line(char_index))
    }

    pub fn to_char_index(&self, col: u16, line: u16) -> usize {
        self.with_read(|b| b.to_char_index(col, line))
    }

    pub fn has_mark(&self) -> bool {
        self.with_read(|b| b.has_mark())
    }

    pub fn get_region(&self, cursor_pos: usize) -> Option<(usize, usize)> {
        self.with_read(|b| b.get_region(cursor_pos))
    }

    // Movement operations
    pub fn move_left(&self, pos: usize) -> usize {
        self.with_read(|b| b.move_left(pos))
    }

    pub fn move_right(&self, pos: usize) -> usize {
        self.with_read(|b| b.move_right(pos))
    }

    pub fn move_up(&self, pos: usize) -> usize {
        self.with_read(|b| b.move_up(pos))
    }

    pub fn move_down(&self, pos: usize) -> usize {
        self.with_read(|b| b.move_down(pos))
    }

    pub fn move_line_start(&self, pos: usize) -> usize {
        self.with_read(|b| b.move_line_start(pos))
    }

    pub fn move_line_end(&self, pos: usize) -> usize {
        self.with_read(|b| b.move_line_end(pos))
    }

    pub fn move_buffer_start(&self) -> usize {
        self.with_read(|b| b.move_buffer_start())
    }

    pub fn move_buffer_end(&self) -> usize {
        self.with_read(|b| b.move_buffer_end())
    }

    pub fn eol_pos(&self, start_pos: usize) -> usize {
        self.with_read(|b| b.eol_pos(start_pos))
    }

    pub fn move_word_forward(&self, pos: usize) -> usize {
        self.with_read(|b| b.move_word_forward(pos))
    }

    pub fn move_word_backward(&self, pos: usize) -> usize {
        self.with_read(|b| b.move_word_backward(pos))
    }

    pub fn move_paragraph_forward(&self, pos: usize) -> usize {
        self.with_read(|b| b.move_paragraph_forward(pos))
    }

    pub fn move_paragraph_backward(&self, pos: usize) -> usize {
        self.with_read(|b| b.move_paragraph_backward(pos))
    }

    // Write operations that need mutable access
    pub fn insert_pos(&self, fragment: String, position: usize) {
        self.with_write(|b| b.insert_pos(fragment, position))
    }

    pub fn insert_col_line(&self, fragment: String, position: (u16, u16)) {
        self.with_write(|b| b.insert_col_line(fragment, position))
    }

    pub fn delete_pos(&self, position: usize, count: isize) -> Option<String> {
        self.with_write(|b| b.delete_pos(position, count))
    }

    pub fn delete_col_line(&self, position: (u16, u16), count: isize) -> Option<String> {
        self.with_write(|b| b.delete_col_line(position, count))
    }

    pub fn set_mark(&self, pos: usize) {
        self.with_write(|b| b.set_mark(pos))
    }

    pub fn set_transient_mark(&self, pos: usize) {
        self.with_write(|b| b.set_transient_mark(pos))
    }

    pub fn clear_mark(&self) {
        self.with_write(|b| b.clear_mark())
    }

    pub fn clear_transient_mark(&self) -> bool {
        self.with_write(|b| b.clear_transient_mark())
    }

    pub fn is_transient_mark(&self) -> bool {
        self.with_read(|b| b.is_transient_mark())
    }

    pub fn delete_region(&self, cursor_pos: usize) -> Option<(String, usize)> {
        self.with_write(|b| b.delete_region(cursor_pos))
    }

    pub fn delete_region_range(&self, start: usize, end: usize) -> Option<String> {
        self.with_write(|b| b.delete_range(start, end))
    }

    // Undo/redo operations
    pub fn undo(&self) -> Option<usize> {
        self.with_write(|b| b.undo())
    }

    pub fn redo(&self) -> Option<usize> {
        self.with_write(|b| b.redo())
    }

    pub fn can_undo(&self) -> bool {
        self.with_read(|b| b.can_undo())
    }

    pub fn can_redo(&self) -> bool {
        self.with_read(|b| b.can_redo())
    }

    pub fn begin_undo_group(&self) {
        self.with_write(|b| b.begin_undo_group())
    }

    pub fn end_undo_group(&self) {
        self.with_write(|b| b.end_undo_group())
    }

    pub fn undo_boundary(&self) {
        self.with_write(|b| b.undo_boundary())
    }

    // Properties that need read access
    pub fn object(&self) -> String {
        self.with_read(|b| b.object.clone())
    }

    pub fn modes(&self) -> Vec<ModeId> {
        self.with_read(|b| b.modes.clone())
    }

    pub fn load_str(&self, text: &str) {
        self.with_write(|b| b.load_str(text))
    }

    // Additional methods needed by the renderer
    pub fn buffer_len_lines(&self) -> usize {
        self.with_read(|b| b.buffer.len_lines())
    }

    pub fn buffer_line(&self, line_idx: usize) -> String {
        self.with_read(|b| b.buffer.line(line_idx).to_string())
    }

    pub fn buffer_line_to_char(&self, line_idx: usize) -> usize {
        self.with_read(|b| b.buffer.line_to_char(line_idx))
    }

    pub fn buffer_lines(&self) -> Vec<String> {
        self.with_read(|b| b.buffer.lines().map(|line| line.to_string()).collect())
    }

    // Add mutable field access for main.rs compatibility
    pub fn set_object(&self, object: String) {
        self.with_write(|b| b.object = object)
    }

    /// Get the major mode name for this buffer
    pub fn major_mode(&self) -> Option<String> {
        self.with_read(|b| b.major_mode.clone())
    }

    /// Set the major mode for this buffer
    pub fn set_major_mode(&self, mode_name: String) {
        self.with_write(|b| b.major_mode = Some(mode_name))
    }

    /// Get whether the gutter should be shown for this buffer
    pub fn show_gutter(&self) -> bool {
        self.with_read(|b| b.show_gutter)
    }

    /// Set whether the gutter should be shown for this buffer
    pub fn set_show_gutter(&self, show: bool) {
        self.with_write(|b| b.show_gutter = show)
    }

    pub fn content(&self) -> String {
        self.with_read(|b| b.content())
    }

    pub fn get_mark(&self) -> Option<usize> {
        self.with_read(|b| b.get_mark())
    }

    pub fn get_region_text(&self, cursor_pos: usize) -> Option<String> {
        self.with_read(|b| b.get_region_text(cursor_pos))
    }

    pub fn buffer_len_chars(&self) -> usize {
        self.with_read(|b| b.buffer.len_chars())
    }

    // === SYNTAX HIGHLIGHTING SPAN OPERATIONS ===

    /// Add a highlight span to the buffer
    pub fn add_span(&self, span: HighlightSpan) {
        self.with_write(|b| b.add_span(span))
    }

    /// Add multiple highlight spans at once
    pub fn add_spans(&self, spans: Vec<HighlightSpan>) {
        self.with_write(|b| b.add_spans(spans))
    }

    /// Clear all highlight spans
    pub fn clear_spans(&self) {
        self.with_write(|b| b.clear_spans())
    }

    /// Clear highlight spans in a specific range (for incremental re-highlighting)
    pub fn clear_spans_in_range(&self, range: Range<usize>) {
        self.with_write(|b| b.clear_spans_in_range(range))
    }

    /// Get the face ID at a specific position
    pub fn face_at(&self, pos: usize) -> Option<FaceId> {
        self.with_write(|b| b.face_at(pos))
    }

    /// Get all spans that overlap with a range (returns cloned spans for thread safety)
    pub fn spans_in_range(&self, range: Range<usize>) -> Vec<HighlightSpan> {
        self.with_write(|b| b.spans_in_range(range).into_iter().cloned().collect())
    }

    /// Check if buffer has any highlight spans
    pub fn has_spans(&self) -> bool {
        self.with_read(|b| b.has_spans())
    }
}

impl Clone for Buffer {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_buffer() -> BufferInner {
        let mut buffer = BufferInner::new(&[]);
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
        let mut buffer = BufferInner::new(&[]);
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
        let mut buffer = BufferInner::new(&[]);
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
        let mut buffer = BufferInner::new(&[]);
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
        let empty_buffer = BufferInner::new(&[]);
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

    #[test]
    fn test_word_movement() {
        let buffer = BufferInner::new(&[]);
        // Load some test text with various word patterns
        let mut buffer = buffer;
        buffer.load_str("hello world  test\n  another line");

        // Test forward word movement
        // From start (position 0), should move to 'w' in "world"
        assert_eq!(buffer.move_word_forward(0), 6); // "hello " -> "world"

        // From 'w' in "world", should move to 't' in "test"
        assert_eq!(buffer.move_word_forward(6), 13); // "world  " -> "test"

        // From 't' in "test", should move to 'a' in "another" (skipping newline and spaces)
        assert_eq!(buffer.move_word_forward(13), 20); // "test\n  " -> "another"

        // From 'a' in "another", should move to 'l' in "line"
        assert_eq!(buffer.move_word_forward(20), 28); // "another " -> "line"

        // From end of buffer, should stay at end
        let end_pos = buffer.buffer.len_chars();
        assert_eq!(buffer.move_word_forward(end_pos), end_pos);

        // Test backward word movement
        // From end, should move to start of "line"
        assert_eq!(buffer.move_word_backward(end_pos), 28);

        // From 'l' in "line", should move to start of "another"
        assert_eq!(buffer.move_word_backward(28), 20);

        // From 'a' in "another", should move to start of "test"
        assert_eq!(buffer.move_word_backward(20), 13);

        // From 't' in "test", should move to start of "world"
        assert_eq!(buffer.move_word_backward(13), 6);

        // From 'w' in "world", should move to start of "hello"
        assert_eq!(buffer.move_word_backward(6), 0);

        // From start, should stay at start
        assert_eq!(buffer.move_word_backward(0), 0);
    }

    #[test]
    fn test_paragraph_movement() {
        let mut buffer = BufferInner::new(&[]);
        // Load text with multiple paragraphs separated by blank lines
        buffer.load_str("First paragraph\nstill first paragraph\n\nSecond paragraph\nstill second\n\n\nThird paragraph\nstill third");

        // Text layout:
        // Line 0: "First paragraph"                    (chars 0-15)
        // Line 1: "still first paragraph"             (chars 16-37)
        // Line 2: ""                                  (chars 38-38, blank line)
        // Line 3: "Second paragraph"                  (chars 39-55)
        // Line 4: "still second"                      (chars 56-68)
        // Line 5: ""                                  (chars 69-69, blank line)
        // Line 6: ""                                  (chars 70-70, blank line)
        // Line 7: "Third paragraph"                   (chars 71-86)
        // Line 8: "still third"                       (chars 87-98)

        // Test forward paragraph movement
        // From start of first paragraph (pos 0), should move to start of second paragraph
        assert_eq!(buffer.move_paragraph_forward(0), 39); // Start of "Second paragraph"

        // From middle of first paragraph, should still move to start of second paragraph
        assert_eq!(buffer.move_paragraph_forward(10), 39); // Start of "Second paragraph"

        // From start of second paragraph, should move to start of third paragraph
        assert_eq!(buffer.move_paragraph_forward(39), 71); // Start of "Third paragraph"

        // From middle of second paragraph, should move to start of third paragraph
        assert_eq!(buffer.move_paragraph_forward(60), 71); // Start of "Third paragraph"

        // From start of third paragraph, should move to end of buffer
        let end_pos = buffer.buffer.len_chars();
        assert_eq!(buffer.move_paragraph_forward(71), end_pos); // End of buffer

        // Test backward paragraph movement
        // From end of buffer, should move to start of third paragraph
        assert_eq!(buffer.move_paragraph_backward(end_pos), 71);

        // From middle of third paragraph, should move to start of third paragraph
        assert_eq!(buffer.move_paragraph_backward(90), 71);

        // From start of third paragraph, should move to start of second paragraph
        assert_eq!(buffer.move_paragraph_backward(71), 39);

        // From middle of second paragraph, should move to start of second paragraph
        assert_eq!(buffer.move_paragraph_backward(50), 39);

        // From start of second paragraph, should move to start of first paragraph
        assert_eq!(buffer.move_paragraph_backward(39), 0);

        // From start of first paragraph, should stay at start
        assert_eq!(buffer.move_paragraph_backward(0), 0);
    }
}
