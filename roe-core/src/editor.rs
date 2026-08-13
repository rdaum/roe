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

use crate::buffer::Buffer;
use crate::keys::{CursorDirection, KeyAction};
use crate::kill_ring::KillRing;
use crate::native_services::Clock;
use crate::renderer::{DirtyRegion, ModelineComponent};
use crate::{BufferId, WindowId};
use slotmap::SlotMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long echo messages remain visible (in seconds)
const ECHO_TIMEOUT_SECS: u64 = 3;
const MAX_MESSAGES_CHARS: usize = 65_536;

/// Type of window - normal editing window or special command window
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowType {
    /// Normal editing window
    Normal,
    /// Command window for M-x, C-x b, etc.
    Command {
        position: CommandWindowPosition,
        command_type: CommandType,
    },
}

/// How to open a file
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenType {
    /// Open in new buffer (find-file behavior)
    New,
    /// Replace current buffer (visit-file behavior)
    Visit,
}

/// Type of command being executed in a command window
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandType {
    /// M-x command execution
    Execute,
    /// Generic Mica interactive argument acquisition
    Argument,
    /// C-x b buffer switching
    BufferSwitch,
    /// C-x k buffer killing
    KillBuffer,
    /// File opening
    OpenFile(OpenType),
    /// Incremental search
    ISearch { forward: bool },
}

/// Command window position
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandWindowPosition {
    Top,
    Bottom,
}

/// A "window" in the emacs sense, not the OS sense.
/// Represents a subsection of the "frame" (OS window or screen)
#[derive(Clone, PartialEq)]
pub struct Window {
    /// X position (in characters) within the frame
    pub x: u16,
    /// Y position (in characters) within the frame
    pub y: u16,
    /// Width in characters
    pub width_chars: u16,
    /// Height in characters
    pub height_chars: u16,
    pub active_buffer: BufferId,
    /// Cursor offset
    /// The position of the cursor inside the buffer for this window.
    /// The actual physical cursor position on the screen is calculated from this and the window's
    /// position in the frame.
    pub cursor: usize,
    /// Type of window (normal or command)
    pub window_type: WindowType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitDirection {
    Horizontal,
    Vertical,
}

/// Window layout tree node
#[derive(Clone)]
pub enum WindowNode {
    /// Leaf node containing an actual window
    Leaf { window_id: WindowId },
    /// Internal node representing a split
    Split {
        direction: SplitDirection,
        ratio: f32, // 0.0 to 1.0, how much space the first child gets
        first: Box<WindowNode>,
        second: Box<WindowNode>,
    },
}

impl WindowNode {
    pub fn new_leaf(window_id: WindowId) -> Self {
        WindowNode::Leaf { window_id }
    }

    pub fn new_split(
        direction: SplitDirection,
        ratio: f32,
        first: WindowNode,
        second: WindowNode,
    ) -> Self {
        let ratio = if ratio.is_finite() {
            ratio.clamp(0.0, 1.0)
        } else {
            0.5
        };
        WindowNode::Split {
            direction,
            ratio,
            first: Box::new(first),
            second: Box::new(second),
        }
    }
}

/// A "frame" in the emacs sense, not the OS sense.
/// Represents the entire screen or window, including the modeline and echo area.
pub struct Frame {
    #[allow(dead_code)]
    pub columns: u16,
    #[allow(dead_code)]
    pub rows: u16,
    pub available_columns: u16,
    pub available_lines: u16,
}

/// Mouse drag state for window resizing
#[derive(Debug, Clone)]
pub struct MouseDragState {
    /// The type of drag operation
    pub drag_type: DragType,
    /// Starting mouse position
    pub start_pos: (u16, u16),
    /// Last processed mouse position (to calculate incremental changes)
    pub last_pos: (u16, u16),
    /// Current mouse position
    pub current_pos: (u16, u16),
    /// Window being resized (if applicable)
    pub target_window: Option<WindowId>,
    /// Border being dragged (if applicable)
    pub border_info: Option<BorderInfo>,
}

/// Type of drag operation
#[derive(Debug, Clone, Copy)]
pub enum DragType {
    /// Dragging a window border to resize
    WindowBorder,
    /// Other drag operations (reserved for future use)
    Other,
}

/// Information about the border being dragged
#[derive(Debug, Clone)]
pub struct BorderInfo {
    /// Whether this is a vertical or horizontal border
    pub is_vertical: bool,
    /// The window node being resized (path to the split node in the window tree)
    pub split_node_path: Vec<usize>,
    /// Original ratio of the split
    pub original_ratio: f32,
}

impl Frame {
    pub fn new(columns: u16, rows: u16) -> Self {
        Frame {
            columns,
            rows,
            available_columns: columns,
            available_lines: rows,
        }
    }
}

pub struct Editor {
    pub frame: Frame,
    pub buffers: SlotMap<BufferId, Buffer>,
    pub windows: SlotMap<WindowId, Window>,
    pub active_window: WindowId,
    /// Tree structure representing window layout
    pub window_tree: WindowNode,
    /// Global kill-ring for cut/copy/paste operations
    pub kill_ring: KillRing,
    /// Window that was active before opening command/buffer switch window
    pub previous_active_window: Option<WindowId>,
    /// Buffer history (most recently used first) for smart buffer switching
    pub buffer_history: Vec<BufferId>,
    /// Current echo area message
    pub echo_message: String,
    /// When the echo message was set (for auto-clearing)
    pub echo_message_time: Option<Instant>,
    pub clock: Arc<dyn Clock>,
    /// Mouse drag state for window resizing
    pub mouse_drag_state: Option<MouseDragState>,
    /// Messages buffer for collecting echo messages and logs
    pub messages_buffer_id: Option<BufferId>,
    /// File watcher for detecting external changes
    pub file_watcher: crate::file_watcher::FileWatcher,
}

/// Character-oriented location for a native text mutation.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ActionPosition {
    Cursor,
    Absolute(u16, u16),
    End,
}

impl ActionPosition {
    pub fn cursor() -> Self {
        Self::Cursor
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ChromeAction {
    Save,
    Echo(String),
    MarkDirty(DirtyRegion),
    BufferChanged {
        buffer_id: BufferId,
        start: usize,
        old_end: usize,
        new_end: usize,
    },
}

impl Editor {
    /// Realize a native action selected by Mica's keymap. This vocabulary does
    /// not assign platform keys or editor policy.
    pub async fn perform_native_action(
        &mut self,
        action: KeyAction,
    ) -> Result<Vec<ChromeAction>, std::io::Error> {
        // Editing primitives selected by Mica are realized directly against the
        // native editor mechanism. They do not pass through a second Rust
        // policy layer.
        let actions = match action {
            KeyAction::AlphaNumeric(character) => {
                self.insert_text(character.to_string(), &ActionPosition::Cursor)
            }
            KeyAction::Backspace => self.delete_text(&ActionPosition::Cursor, -1),
            KeyAction::Delete => self.delete_text(&ActionPosition::Cursor, 1),
            KeyAction::Enter => self.insert_text("\n".to_owned(), &ActionPosition::Cursor),
            KeyAction::Tab => self.insert_text("\t".to_owned(), &ActionPosition::Cursor),
            KeyAction::MarkStart => self.set_mark(),
            KeyAction::KillRegion(true) => self.kill_region(),
            KeyAction::KillRegion(false) => self.copy_region(),
            KeyAction::KillLine(_) => self.kill_line(),
            KeyAction::Yank(Some(index)) => self.yank_index(&ActionPosition::Cursor, index),
            KeyAction::Yank(None) => self.yank(&ActionPosition::Cursor),
            KeyAction::DeleteWord => self.forward_kill_word(),
            KeyAction::BackspaceWord => self.backward_kill_word(),
            KeyAction::Cursor(direction) => self.move_cursor(direction, false),
            KeyAction::CursorSelect(direction) => self.move_cursor(direction, true),
            KeyAction::Undo => self.undo_or_redo(false),
            KeyAction::Redo => self.undo_or_redo(true),
            KeyAction::Cancel | KeyAction::Escape => {
                let window = &self.windows[self.active_window];
                let buffer = &self.buffers[window.active_buffer];
                buffer.undo_boundary();
                if buffer.has_mark() {
                    self.clear_mark()
                } else {
                    vec![ChromeAction::Echo("Quit".to_owned())]
                }
            }
            KeyAction::Redraw => vec![ChromeAction::MarkDirty(DirtyRegion::FullScreen)],
        };
        Ok(actions)
    }

    fn move_cursor(&mut self, direction: CursorDirection, selecting: bool) -> Vec<ChromeAction> {
        let window = &mut self.windows[self.active_window];
        let buffer = &self.buffers[window.active_buffer];
        buffer.undo_boundary();
        if selecting {
            if !buffer.has_mark() {
                buffer.set_transient_mark(window.cursor);
            }
        } else {
            buffer.clear_transient_mark();
        }
        let new_pos = match direction {
            CursorDirection::Left => buffer.move_left(window.cursor),
            CursorDirection::Right => buffer.move_right(window.cursor),
            CursorDirection::Up => buffer.move_up(window.cursor),
            CursorDirection::Down => buffer.move_down(window.cursor),
            CursorDirection::LineStart => buffer.move_line_start(window.cursor),
            CursorDirection::LineEnd => buffer.move_line_end(window.cursor),
            CursorDirection::BufferStart => buffer.move_buffer_start(),
            CursorDirection::BufferEnd => buffer.move_buffer_end(),
            CursorDirection::PageUp => {
                let height = window.height_chars.saturating_sub(3);
                let (column, line) = buffer.to_column_line(window.cursor);
                buffer.to_char_index(column, line.saturating_sub(height))
            }
            CursorDirection::PageDown => {
                let height = window.height_chars.saturating_sub(3);
                let (column, line) = buffer.to_column_line(window.cursor);
                let last = buffer.buffer_len_lines().saturating_sub(1) as u16;
                buffer.to_char_index(column, line.saturating_add(height).min(last))
            }
            CursorDirection::WordForward => buffer.move_word_forward(window.cursor),
            CursorDirection::WordBackward => buffer.move_word_backward(window.cursor),
            CursorDirection::ParagraphForward => buffer.move_paragraph_forward(window.cursor),
            CursorDirection::ParagraphBackward => buffer.move_paragraph_backward(window.cursor),
        };
        window.cursor = new_pos;
        vec![
            ChromeAction::MarkDirty(DirtyRegion::Modeline {
                window_id: self.active_window,
                component: ModelineComponent::CursorPosition,
            }),
            ChromeAction::MarkDirty(DirtyRegion::Buffer {
                buffer_id: window.active_buffer,
            }),
        ]
    }

    /// Realize an exact character cursor chosen by external editor policy.
    pub fn move_cursor_to(&mut self, position: usize) -> Vec<ChromeAction> {
        let window = &mut self.windows[self.active_window];
        let buffer = &self.buffers[window.active_buffer];
        buffer.undo_boundary();
        buffer.clear_transient_mark();
        window.cursor = position.min(buffer.buffer_len_chars());
        vec![ChromeAction::MarkDirty(DirtyRegion::Buffer {
            buffer_id: window.active_buffer,
        })]
    }

    fn undo_or_redo(&mut self, redo: bool) -> Vec<ChromeAction> {
        let window = &mut self.windows[self.active_window];
        let buffer = &self.buffers[window.active_buffer];
        let cursor = if redo { buffer.redo() } else { buffer.undo() };
        let Some(cursor) = cursor else {
            return vec![ChromeAction::Echo(if redo {
                "No further redo information".to_owned()
            } else {
                "No further undo information".to_owned()
            })];
        };
        window.cursor = cursor;
        vec![
            ChromeAction::MarkDirty(DirtyRegion::Buffer {
                buffer_id: window.active_buffer,
            }),
            ChromeAction::Echo(if redo { "Redo" } else { "Undo" }.to_owned()),
        ]
    }

    /// Create a renderer-neutral prompt surface whose state and key handling
    /// live in Mica. The Rust side owns only its text buffer and geometry.
    pub fn create_mica_prompt_window(
        &mut self,
        command_type: CommandType,
        height: u16,
        content: String,
        cursor: usize,
    ) -> WindowId {
        let command_buffer = Buffer::new();
        command_buffer.set_object("*Mica Prompt*".to_owned());
        command_buffer.load_str(&content);
        let command_buffer_id = self.buffers.insert(command_buffer);
        let position = CommandWindowPosition::Bottom;
        let window = self.windows.insert(Window {
            x: 0,
            y: self.frame.available_lines.saturating_sub(height),
            width_chars: self.frame.available_columns,
            height_chars: height,
            active_buffer: command_buffer_id,
            cursor: cursor.min(content.chars().count()),
            window_type: WindowType::Command {
                position,
                command_type,
            },
        });
        self.previous_active_window = Some(self.active_window);
        self.active_window = window;
        self.calculate_window_layout();
        window
    }

    pub fn update_mica_prompt_window(&mut self, content: &str, cursor: usize) -> Option<WindowId> {
        let window_id = self.find_command_window()?;
        let buffer_id = self.windows[window_id].active_buffer;
        self.buffers[buffer_id].load_str(content);
        self.windows[window_id].cursor = cursor.min(content.chars().count());
        Some(window_id)
    }

    pub fn select_mica_buffer(&mut self, buffer_id: BufferId, kill: bool) -> Vec<ChromeAction> {
        if let Some(prompt) = self.find_command_window() {
            self.close_command_window(prompt);
        }
        if !self.buffers.contains_key(buffer_id) || self.is_command_buffer(buffer_id) {
            return vec![ChromeAction::Echo("Buffer no longer exists".to_owned())];
        }
        if !kill {
            self.windows[self.active_window].active_buffer = buffer_id;
            self.windows[self.active_window].cursor = 0;
            self.record_buffer_access(buffer_id);
            return vec![
                ChromeAction::Echo(format!(
                    "Switched to buffer: {}",
                    self.buffers[buffer_id].object()
                )),
                ChromeAction::MarkDirty(DirtyRegion::FullScreen),
            ];
        }

        let name = self.buffers[buffer_id].object();
        let replacement = self
            .buffers
            .iter()
            .find_map(|(candidate, _)| (candidate != buffer_id).then_some(candidate));
        let Some(replacement) = replacement else {
            return vec![ChromeAction::Echo("Cannot kill the only buffer".to_owned())];
        };
        for (_, window) in &mut self.windows {
            if window.active_buffer == buffer_id {
                window.active_buffer = replacement;
                window.cursor = 0;
            }
        }
        if let Err(error) = self.file_watcher.unwatch_file(buffer_id) {
            self.set_echo_message(format!("Killed {name}; watcher cleanup failed: {error}"));
        }
        self.buffers.remove(buffer_id);
        self.buffer_history
            .retain(|candidate| *candidate != buffer_id);
        self.record_buffer_access(replacement);
        vec![
            ChromeAction::Echo(format!("Killed buffer: {name}")),
            ChromeAction::MarkDirty(DirtyRegion::FullScreen),
        ]
    }

    pub fn open_mica_file(
        &mut self,
        path: std::path::PathBuf,
        open_type: OpenType,
        content: Option<String>,
    ) -> Vec<ChromeAction> {
        let mut actions = Vec::new();
        if let Some(command_window_id) = self.find_command_window() {
            self.close_command_window(command_window_id);
            actions.push(ChromeAction::MarkDirty(DirtyRegion::FullScreen));
        }
        let window = self
            .previous_active_window
            .filter(|window_id| self.windows.contains_key(*window_id))
            .unwrap_or(self.active_window);
        let replaced = (open_type == OpenType::Visit)
            .then(|| self.windows[window].active_buffer)
            .filter(|buffer| !self.is_command_buffer(*buffer));
        let existed = content.is_some();
        match self.open_file_content_in_window(path.clone(), window, content) {
            Ok(message) => {
                let opened = self.windows[window].active_buffer;
                let watch_error = if existed {
                    self.file_watcher
                        .watch_file(opened, &path, self.buffers[opened].content())
                        .err()
                        .map(|error| {
                            format!("Opened {}, but failed to watch it: {error}", path.display())
                        })
                } else {
                    None
                };
                let unwatch_error = if let Some(replaced) = replaced
                    && !self
                        .windows
                        .values()
                        .any(|candidate| candidate.active_buffer == replaced)
                {
                    let error = self.file_watcher.unwatch_file(replaced).err();
                    self.buffers.remove(replaced);
                    error.map(|error| {
                        format!("Replaced buffer, but failed to stop watching it: {error}")
                    })
                } else {
                    None
                };
                actions.push(ChromeAction::Echo(message));
                actions.extend(watch_error.into_iter().map(ChromeAction::Echo));
                actions.extend(unwatch_error.into_iter().map(ChromeAction::Echo));
                actions.push(ChromeAction::MarkDirty(DirtyRegion::FullScreen));
            }
            Err(error) => actions.push(ChromeAction::Echo(format!("Error opening file: {error}"))),
        }
        actions
    }

    fn open_file_content_in_window(
        &mut self,
        path: std::path::PathBuf,
        window: WindowId,
        content: Option<String>,
    ) -> Result<String, String> {
        if !self.windows.contains_key(window) {
            return Err("Window no longer exists".to_owned());
        }
        let buffer = Buffer::new();
        buffer.set_object(path.to_string_lossy().to_string());
        if let Some(content) = content {
            buffer.load_str(&content);
            buffer.set_show_gutter(true);
        }
        let buffer_id = self.buffers.insert(buffer);
        self.windows[window].active_buffer = buffer_id;
        self.windows[window].cursor = 0;
        Ok(format!("Opened: {}", path.display()))
    }

    /// Create a command window and associated buffer
    /// Close command window and clean up its buffer
    pub fn close_command_window(&mut self, window_id: WindowId) -> bool {
        if let Some(window) = self.windows.get(window_id)
            && matches!(window.window_type, WindowType::Command { .. })
        {
            let buffer_id = window.active_buffer;
            self.windows.remove(window_id);
            self.buffers.remove(buffer_id);

            // Restore the previous active window if it still exists
            if let Some(prev_window_id) = self.previous_active_window {
                if self.windows.contains_key(prev_window_id) {
                    self.active_window = prev_window_id;
                } else {
                    // Previous window was deleted, find any normal window
                    if let Some(normal_window_id) = self.windows.iter().find_map(|(id, w)| {
                        if matches!(w.window_type, WindowType::Normal) {
                            Some(id)
                        } else {
                            None
                        }
                    }) {
                        self.active_window = normal_window_id;
                    }
                }
                self.previous_active_window = None; // Clear the saved window
            } else {
                // No previous window saved, find any normal window
                if let Some(normal_window_id) = self.windows.iter().find_map(|(id, w)| {
                    if matches!(w.window_type, WindowType::Normal) {
                        Some(id)
                    } else {
                        None
                    }
                }) {
                    self.active_window = normal_window_id;
                }
            }

            // Record buffer access for the restored active window
            let restored_buffer_id = self.windows[self.active_window].active_buffer;
            self.record_buffer_access(restored_buffer_id);

            self.calculate_window_layout();
            return true;
        }
        false
    }

    /// Find active command window if any
    pub fn find_command_window(&self) -> Option<WindowId> {
        self.windows.iter().find_map(|(id, window)| {
            if matches!(window.window_type, WindowType::Command { .. }) {
                Some(id)
            } else {
                None
            }
        })
    }

    /// Check if a buffer belongs to a command window
    pub fn is_command_buffer(&self, buffer_id: BufferId) -> bool {
        self.windows.iter().any(|(_, window)| {
            window.active_buffer == buffer_id
                && matches!(window.window_type, WindowType::Command { .. })
        })
    }

    /// Get or create the Messages buffer
    pub fn get_messages_buffer(&mut self) -> BufferId {
        if let Some(buffer_id) = self.messages_buffer_id {
            // Messages buffer already exists, return it
            buffer_id
        } else {
            let messages_buffer = Buffer::new();
            messages_buffer.set_object("*Messages*".to_string());
            messages_buffer
                .load_str("Messages buffer - echo messages and logs will appear here.\n\n");

            let messages_buffer_id = self.buffers.insert(messages_buffer);

            // Store the Messages buffer ID for future use
            self.messages_buffer_id = Some(messages_buffer_id);

            messages_buffer_id
        }
    }

    /// Add a message to the Messages buffer
    pub fn add_message_to_buffer(&mut self, message: String) {
        let messages_buffer_id = self.get_messages_buffer();
        if let Some(buffer) = self.buffers.get(messages_buffer_id) {
            // Add timestamp and message to the buffer
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("Buffer host should be created successfully")
                .as_secs();
            let formatted_message = format!("[{now}] {message}\n");

            // Append message to end of buffer
            let buffer_len = buffer.buffer_len_chars();
            buffer.insert_pos(formatted_message, buffer_len);
            let excess = buffer.buffer_len_chars().saturating_sub(MAX_MESSAGES_CHARS);
            if excess > 0 {
                buffer.delete_pos(0, isize::try_from(excess).unwrap_or(isize::MAX));
            }
        }
    }

    /// Create an in-memory buffer. Mica assigns its logical mode when the
    /// session synchronizes the new buffer identity.
    pub fn create_buffer(&mut self, buffer_name: String, initial_content: String) -> BufferId {
        let buffer = Buffer::new();
        buffer.set_object(buffer_name);
        buffer.load_str(&initial_content);
        self.buffers.insert(buffer)
    }

    /// Set the echo area message (this will override any chord display)
    pub fn set_echo_message(&mut self, message: String) {
        self.echo_message = message.clone();
        self.echo_message_time = Some(self.clock.now());
        // Also add the message to the Messages buffer
        self.add_message_to_buffer(message);
    }

    /// Clear the echo area message
    pub fn clear_echo_message(&mut self) {
        self.echo_message.clear();
        self.echo_message_time = None;
    }

    /// Check if echo message should be auto-cleared and clear it if needed
    /// Returns true if the message was cleared
    pub fn check_and_clear_expired_echo(&mut self) -> bool {
        if let Some(echo_time) = self.echo_message_time
            && self.clock.now().saturating_duration_since(echo_time)
                >= Duration::from_secs(ECHO_TIMEOUT_SECS)
        {
            self.clear_echo_message();
            return true;
        }
        false
    }

    /// Update buffer history when switching to a buffer
    pub fn record_buffer_access(&mut self, buffer_id: BufferId) {
        // Remove buffer from history if it exists
        self.buffer_history.retain(|&id| id != buffer_id);
        // Add to front (most recent)
        self.buffer_history.insert(0, buffer_id);
        // Keep history reasonably sized
        if self.buffer_history.len() > 20 {
            self.buffer_history.truncate(20);
        }
    }

    /// Get the previous buffer (most recent that's not current and not a command buffer)
    pub fn get_previous_buffer(&self, current_buffer_id: BufferId) -> Option<BufferId> {
        self.buffer_history
            .iter()
            .find(|&&id| {
                id != current_buffer_id
                    && self.buffers.contains_key(id)
                    && !self.is_command_buffer(id)
            })
            .copied()
    }

    /// Get the available space for normal windows, accounting for command windows
    pub fn get_available_window_area(&self) -> (u16, u16, u16, u16) {
        let x = 0;
        let mut y = 0;
        let width = self.frame.available_columns;
        let mut height = self.frame.available_lines;

        // Account for command windows
        for window in self.windows.values() {
            if let WindowType::Command { position, .. } = window.window_type {
                match position {
                    CommandWindowPosition::Top => {
                        y += window.height_chars;
                        height = height.saturating_sub(window.height_chars);
                    }
                    CommandWindowPosition::Bottom => {
                        height = height.saturating_sub(window.height_chars);
                    }
                }
            }
        }

        (x, y, width, height)
    }

    /// Calculate and update window positions and sizes based on the window tree
    pub fn calculate_window_layout(&mut self) {
        let (x, y, available_width, available_height) = self.get_available_window_area();

        self.layout_node(
            &self.window_tree.clone(),
            x,
            y,
            available_width,
            available_height,
        );
    }

    /// Handle terminal resize event
    pub fn handle_resize(&mut self, width: u16, height: u16) {
        // Update the frame dimensions
        self.frame.columns = width;
        self.frame.rows = height;
        self.frame.available_columns = width;
        self.frame.available_lines = height;

        // Recalculate window layout with new dimensions
        self.calculate_window_layout();
    }

    /// Debug function to print window tree structure
    #[allow(dead_code)]
    fn debug_window_tree(&self, node: &WindowNode, depth: usize) -> String {
        Self::debug_window_tree_impl(node, depth)
    }

    /// Implementation of debug_window_tree that doesn't use self
    fn debug_window_tree_impl(node: &WindowNode, depth: usize) -> String {
        let indent = "  ".repeat(depth);
        match node {
            WindowNode::Leaf { window_id } => {
                format!("{indent}Leaf({window_id:?})")
            }
            WindowNode::Split {
                direction,
                ratio,
                first,
                second,
            } => {
                format!(
                    "{}Split({:?}, {:.2})\n{}\n{}",
                    indent,
                    direction,
                    ratio,
                    Self::debug_window_tree_impl(first, depth + 1),
                    Self::debug_window_tree_impl(second, depth + 1)
                )
            }
        }
    }

    /// Recursively layout a window tree node
    fn layout_node(&mut self, node: &WindowNode, x: u16, y: u16, width: u16, height: u16) {
        match node {
            WindowNode::Leaf { window_id } => {
                // Update the leaf window's position and size
                // Ensure minimum size for border + content + modeline (4x4 minimum)
                if let Some(window) = self.windows.get_mut(*window_id) {
                    window.x = x;
                    window.y = y;
                    window.width_chars = width.max(4);
                    window.height_chars = height.max(4);
                }
            }
            WindowNode::Split {
                direction,
                ratio,
                first,
                second,
            } => {
                match direction {
                    SplitDirection::Horizontal => {
                        // Split horizontally (one above the other)
                        let first_height = (height as f32 * ratio) as u16;
                        let second_height = height - first_height;

                        self.layout_node(first, x, y, width, first_height);
                        self.layout_node(second, x, y + first_height, width, second_height);
                    }
                    SplitDirection::Vertical => {
                        // Split vertically (side by side)
                        let first_width = (width as f32 * ratio) as u16;
                        let second_width = width - first_width;

                        self.layout_node(first, x, y, first_width, height);
                        self.layout_node(second, x + first_width, y, second_width, height);
                    }
                }
            }
        }
    }

    /// Split the current window horizontally
    pub fn split_horizontal(&mut self) -> WindowId {
        let current_window = self.windows[self.active_window].clone();
        let new_window = current_window.clone();
        let new_window_id = self.windows.insert(new_window);

        // Update the tree structure
        self.window_tree = self.split_node_horizontal(
            &self.window_tree.clone(),
            self.active_window,
            new_window_id,
        );
        self.calculate_window_layout();
        new_window_id
    }

    /// Split the current window vertically
    pub fn split_vertical(&mut self) -> WindowId {
        let current_window = self.windows[self.active_window].clone();
        let new_window = current_window.clone();
        let new_window_id = self.windows.insert(new_window);

        // Update the tree structure
        self.window_tree =
            self.split_node_vertical(&self.window_tree.clone(), self.active_window, new_window_id);
        self.calculate_window_layout();
        new_window_id
    }

    /// Split a node horizontally in the tree
    fn split_node_horizontal(
        &self,
        node: &WindowNode,
        target_window: WindowId,
        new_window: WindowId,
    ) -> WindowNode {
        Self::split_node_horizontal_impl(node, target_window, new_window)
    }

    /// Implementation of split_node_horizontal that doesn't use self
    fn split_node_horizontal_impl(
        node: &WindowNode,
        target_window: WindowId,
        new_window: WindowId,
    ) -> WindowNode {
        match node {
            WindowNode::Leaf { window_id } => {
                if *window_id == target_window {
                    // Replace this leaf with a horizontal split
                    WindowNode::new_split(
                        SplitDirection::Horizontal,
                        0.5, // 50/50 split
                        WindowNode::new_leaf(*window_id),
                        WindowNode::new_leaf(new_window),
                    )
                } else {
                    node.clone()
                }
            }
            WindowNode::Split {
                direction,
                ratio,
                first,
                second,
            } => {
                let new_first = Self::split_node_horizontal_impl(first, target_window, new_window);
                let new_second =
                    Self::split_node_horizontal_impl(second, target_window, new_window);
                WindowNode::new_split(*direction, *ratio, new_first, new_second)
            }
        }
    }

    /// Split a node vertically in the tree
    fn split_node_vertical(
        &self,
        node: &WindowNode,
        target_window: WindowId,
        new_window: WindowId,
    ) -> WindowNode {
        Self::split_node_vertical_impl(node, target_window, new_window)
    }

    /// Implementation of split_node_vertical that doesn't use self
    fn split_node_vertical_impl(
        node: &WindowNode,
        target_window: WindowId,
        new_window: WindowId,
    ) -> WindowNode {
        match node {
            WindowNode::Leaf { window_id } => {
                if *window_id == target_window {
                    // Replace this leaf with a vertical split
                    WindowNode::new_split(
                        SplitDirection::Vertical,
                        0.5, // 50/50 split
                        WindowNode::new_leaf(*window_id),
                        WindowNode::new_leaf(new_window),
                    )
                } else {
                    node.clone()
                }
            }
            WindowNode::Split {
                direction,
                ratio,
                first,
                second,
            } => {
                let new_first = Self::split_node_vertical_impl(first, target_window, new_window);
                let new_second = Self::split_node_vertical_impl(second, target_window, new_window);
                WindowNode::new_split(*direction, *ratio, new_first, new_second)
            }
        }
    }

    /// Switch to the next window in spatial order (emacs-like)
    pub fn switch_window(&mut self) -> WindowId {
        if self.windows.len() <= 1 {
            return self.active_window;
        }

        let window_ids = self.get_windows_in_spatial_order();
        let current_index = window_ids
            .iter()
            .position(|&id| id == self.active_window)
            .unwrap_or(0);
        let next_index = (current_index + 1) % window_ids.len();
        self.active_window = window_ids[next_index];

        // Record buffer access for the newly active window
        let new_buffer_id = self.windows[self.active_window].active_buffer;
        self.record_buffer_access(new_buffer_id);

        self.active_window
    }

    /// Get all windows in spatial order (left-to-right, top-to-bottom)
    fn get_windows_in_spatial_order(&self) -> Vec<WindowId> {
        let mut windows_with_pos: Vec<(WindowId, (u16, u16))> = Vec::new();

        // Collect all windows with their top-left positions
        for (window_id, window) in &self.windows {
            windows_with_pos.push((window_id, (window.x, window.y)));
        }

        // Sort by position: first by y (top-to-bottom), then by x (left-to-right)
        windows_with_pos.sort_by(|a, b| {
            let (_, (x1, y1)) = a;
            let (_, (x2, y2)) = b;
            y1.cmp(y2).then(x1.cmp(x2))
        });

        windows_with_pos.into_iter().map(|(id, _)| id).collect()
    }

    /// Delete the current window
    pub fn delete_window(&mut self) -> bool {
        // Can't delete if it's the only window
        if self.windows.len() <= 1 {
            return false;
        }

        // Remove the window from the tree and rebalance, getting suggested new active window
        let (new_tree, deleted, suggested_active) = self
            .delete_node_from_tree_with_selection(&self.window_tree.clone(), self.active_window);

        if deleted {
            self.window_tree = new_tree;
            self.windows.remove(self.active_window);

            // Use the suggested active window (the one that expanded to fill deleted space)
            if let Some(new_active) = suggested_active {
                self.active_window = new_active;
            } else if let Some(fallback_active) = self.windows.keys().next() {
                // Fallback to first available window if suggestion failed
                self.active_window = fallback_active;
            }

            self.calculate_window_layout();
            true
        } else {
            false
        }
    }

    /// Delete all other windows, keeping only the current window (emacs C-x 1)
    pub fn delete_other_windows(&mut self) -> bool {
        // If there's only one window, nothing to do
        if self.windows.len() <= 1 {
            return false;
        }

        let current_window = self.active_window;

        // Remove all windows except the current one
        let other_windows: Vec<WindowId> = self
            .windows
            .keys()
            .filter(|&id| id != current_window)
            .collect();

        for window_id in other_windows {
            self.windows.remove(window_id);
        }

        // Reset the tree to just a single leaf with the current window
        self.window_tree = WindowNode::new_leaf(current_window);

        // Update the current window to fill the entire available space
        if let Some(window) = self.windows.get_mut(current_window) {
            window.x = 0;
            window.y = 0;
            window.width_chars = self.frame.available_columns;
            window.height_chars = self.frame.available_lines;
        }

        true
    }

    /// Remove a window from the tree, returning the new tree, whether deletion occurred, and suggested new active window
    fn delete_node_from_tree_with_selection(
        &self,
        node: &WindowNode,
        target_window: WindowId,
    ) -> (WindowNode, bool, Option<WindowId>) {
        match node {
            WindowNode::Leaf { window_id } => {
                if *window_id == target_window {
                    // Found the target window - mark for deletion but return a placeholder
                    // The parent will handle the actual replacement
                    (node.clone(), true, None)
                } else {
                    (node.clone(), false, None)
                }
            }
            WindowNode::Split {
                direction,
                ratio,
                first,
                second,
            } => {
                // Check if either child contains the target
                let (new_first, first_deleted, first_suggestion) =
                    self.delete_node_from_tree_with_selection(first, target_window);
                let (new_second, second_deleted, second_suggestion) =
                    self.delete_node_from_tree_with_selection(second, target_window);

                if first_deleted && second_deleted {
                    // This shouldn't happen - target can't be in both children
                    panic!("Target window found in both children of split");
                } else if first_deleted {
                    // First child contained a deletion - check if it was completely removed
                    match &**first {
                        WindowNode::Leaf { window_id } if *window_id == target_window => {
                            // First child was a leaf that got deleted, promote second child
                            // The suggested active window should be from the promoted subtree
                            let suggested = self.find_first_window_in_tree(&new_second);
                            (new_second, true, suggested)
                        }
                        _ => {
                            // First child was a split that handled deletion internally, keep the split
                            (
                                WindowNode::new_split(*direction, *ratio, new_first, new_second),
                                true,
                                first_suggestion,
                            )
                        }
                    }
                } else if second_deleted {
                    // Second child contained a deletion - check if it was completely removed
                    match &**second {
                        WindowNode::Leaf { window_id } if *window_id == target_window => {
                            // Second child was a leaf that got deleted, promote first child
                            // The suggested active window should be from the promoted subtree
                            let suggested = self.find_first_window_in_tree(&new_first);
                            (new_first, true, suggested)
                        }
                        _ => {
                            // Second child was a split that handled deletion internally, keep the split
                            (
                                WindowNode::new_split(*direction, *ratio, new_first, new_second),
                                true,
                                second_suggestion,
                            )
                        }
                    }
                } else {
                    // No deletion in this subtree, reconstruct with possibly updated children
                    (
                        WindowNode::new_split(*direction, *ratio, new_first, new_second),
                        false,
                        None,
                    )
                }
            }
        }
    }

    /// Find the first window in a tree (for selecting a representative window)
    fn find_first_window_in_tree(&self, node: &WindowNode) -> Option<WindowId> {
        Self::find_first_window_in_tree_impl(node)
    }

    /// Implementation of find_first_window_in_tree that doesn't use self
    fn find_first_window_in_tree_impl(node: &WindowNode) -> Option<WindowId> {
        match node {
            WindowNode::Leaf { window_id } => Some(*window_id),
            WindowNode::Split { first, .. } => Self::find_first_window_in_tree_impl(first),
        }
    }

    /// Convert BufferResponse to ChromeActions
    /// Perform insert action, based on the position passed and taking into account the window's
    /// cursor position.
    pub fn insert_text(&mut self, text: String, position: &ActionPosition) -> Vec<ChromeAction> {
        // Break kill sequence since we're doing a non-kill operation
        self.kill_ring.break_kill_sequence();

        let window = &mut self
            .windows
            .get_mut(self.active_window)
            .expect("Active window should exist");
        let buffer = &mut self
            .buffers
            .get_mut(window.active_buffer)
            .expect("Active buffer should exist");
        match position {
            ActionPosition::Cursor => {
                let start = window.cursor;
                let length = text.chars().count();
                let has_newline = text.contains('\n');
                let buffer_id = window.active_buffer;
                buffer.insert_pos(text, window.cursor);

                // Advance the cursor
                window.cursor += length;

                // Mark dirty regions based on what was inserted
                let cursor_line = buffer.to_column_line(window.cursor).1 as usize;
                let dirty_action = if has_newline {
                    // Newlines affect multiple lines, mark entire buffer dirty
                    ChromeAction::MarkDirty(DirtyRegion::Buffer { buffer_id })
                } else {
                    // Simple text insertion, only current line affected
                    ChromeAction::MarkDirty(DirtyRegion::Line {
                        buffer_id,
                        line: cursor_line,
                    })
                };

                vec![
                    ChromeAction::Echo("Inserted text".to_string()),
                    dirty_action,
                    // Notify major mode of buffer change for syntax highlighting
                    ChromeAction::BufferChanged {
                        buffer_id,
                        start,
                        old_end: start,
                        new_end: start + length,
                    },
                ]
            }
            ActionPosition::Absolute(l, c) => {
                let buffer_id = window.active_buffer;
                let start = buffer.to_char_index(*c, *l);
                let length = text.chars().count();
                buffer.insert_col_line(text.clone(), (*l, *c));

                let dirty_action = if text.contains('\n') {
                    // Newlines affect multiple lines, mark entire buffer dirty
                    ChromeAction::MarkDirty(DirtyRegion::Buffer { buffer_id })
                } else {
                    // Simple text insertion, only current line affected
                    ChromeAction::MarkDirty(DirtyRegion::Line {
                        buffer_id,
                        line: *l as usize,
                    })
                };

                vec![
                    ChromeAction::Echo("Inserted text".to_string()),
                    dirty_action,
                    ChromeAction::BufferChanged {
                        buffer_id,
                        start,
                        old_end: start,
                        new_end: start + length,
                    },
                ]
            }
            ActionPosition::End => {
                vec![ChromeAction::Echo("End insert not implemented".to_string())]
            }
        }
    }

    pub fn delete_text(&mut self, position: &ActionPosition, count: isize) -> Vec<ChromeAction> {
        // Break kill sequence since we're doing a non-kill operation
        self.kill_ring.break_kill_sequence();

        let window = &mut self
            .windows
            .get_mut(self.active_window)
            .expect("Active window should exist");
        let buffer = &mut self
            .buffers
            .get_mut(window.active_buffer)
            .expect("Active buffer should exist");

        match position {
            ActionPosition::Cursor => {
                let buffer_id = window.active_buffer;
                let cursor_before = window.cursor;
                let Some(deleted) = buffer.delete_pos(window.cursor, count) else {
                    return vec![];
                };
                if deleted.is_empty() {
                    return vec![];
                }
                let deleted_len = deleted.chars().count();

                // Calculate change region for after-change hook
                let (start, old_end) = if count < 0 {
                    // Deleted backward: text was before cursor
                    let start = cursor_before.saturating_sub(deleted_len);
                    (start, cursor_before)
                } else {
                    // Deleted forward: text was after cursor
                    (cursor_before, cursor_before + deleted_len)
                };

                // If the count was negative, then we need to adjust the cursor back by the size
                // of the deleted fragment.
                if count < 0 {
                    window.cursor = window.cursor.saturating_sub(deleted_len);
                }
                let cursor_line = buffer.to_column_line(window.cursor).1 as usize;

                // If we deleted a newline, mark entire buffer dirty to handle line merging
                let dirty_action = if deleted.contains('\n') {
                    ChromeAction::MarkDirty(DirtyRegion::Buffer { buffer_id })
                } else {
                    ChromeAction::MarkDirty(DirtyRegion::Line {
                        buffer_id,
                        line: cursor_line,
                    })
                };

                vec![
                    ChromeAction::Echo("Deleted text".to_string()),
                    dirty_action,
                    ChromeAction::BufferChanged {
                        buffer_id,
                        start,
                        old_end,
                        new_end: start, // After delete, start == new_end
                    },
                ]
            }
            ActionPosition::Absolute(l, c) => {
                let buffer_id = window.active_buffer;
                let start = buffer.to_char_index(*c, *l);
                let Some(deleted) = buffer.delete_col_line((*l, *c), count) else {
                    return vec![];
                };
                if deleted.is_empty() {
                    return vec![];
                }
                let deleted_len = deleted.chars().count();
                let (change_start, old_end) = if count < 0 {
                    (start.saturating_sub(deleted_len), start)
                } else {
                    (start, start + deleted_len)
                };

                // If we deleted a newline, mark entire buffer dirty to handle line merging
                let dirty_action = if deleted.contains('\n') {
                    ChromeAction::MarkDirty(DirtyRegion::Buffer { buffer_id })
                } else {
                    ChromeAction::MarkDirty(DirtyRegion::Line {
                        buffer_id,
                        line: *l as usize,
                    })
                };

                vec![
                    ChromeAction::Echo("Deleted text".to_string()),
                    dirty_action,
                    ChromeAction::BufferChanged {
                        buffer_id,
                        start: change_start,
                        old_end,
                        new_end: change_start,
                    },
                ]
            }
            ActionPosition::End => {
                vec![ChromeAction::Echo("End delete not implemented".to_string())]
            }
        }
    }

    /// Kill (cut) text and add it to the kill-ring
    pub fn kill_text(&mut self, position: &ActionPosition, count: isize) -> Vec<ChromeAction> {
        let window = &mut self
            .windows
            .get_mut(self.active_window)
            .expect("Active window should exist");
        let buffer = &mut self
            .buffers
            .get_mut(window.active_buffer)
            .expect("Active buffer should exist");

        match position {
            ActionPosition::Cursor => {
                let Some(deleted) = buffer.delete_pos(window.cursor, count) else {
                    return vec![];
                };
                if deleted.is_empty() {
                    return vec![];
                }

                // Add to kill-ring
                if count < 0 {
                    self.kill_ring.kill_prepend(deleted.clone());
                    // Adjust cursor for backward kill
                    let length = deleted.chars().count();
                    window.cursor = window.cursor.saturating_sub(length);
                } else {
                    self.kill_ring.kill(deleted.clone());
                }

                vec![
                    ChromeAction::Echo(format!("Killed: {deleted}")),
                    ChromeAction::MarkDirty(DirtyRegion::Buffer {
                        buffer_id: window.active_buffer,
                    }),
                ]
            }
            ActionPosition::Absolute(l, c) => {
                let Some(deleted) = buffer.delete_col_line((*l, *c), count) else {
                    return vec![];
                };
                if deleted.is_empty() {
                    return vec![];
                }

                // Add to kill-ring
                if count < 0 {
                    self.kill_ring.kill_prepend(deleted.clone());
                } else {
                    self.kill_ring.kill(deleted.clone());
                }

                vec![
                    ChromeAction::Echo(format!("Killed: {deleted}")),
                    ChromeAction::MarkDirty(DirtyRegion::Buffer {
                        buffer_id: window.active_buffer,
                    }),
                ]
            }
            ActionPosition::End => {
                vec![ChromeAction::Echo("End kill not implemented".to_string())]
            }
        }
    }

    /// Kill from cursor to end of line
    pub fn kill_line(&mut self) -> Vec<ChromeAction> {
        let window = &mut self
            .windows
            .get_mut(self.active_window)
            .expect("Active window should exist");
        let buffer = &mut self
            .buffers
            .get_mut(window.active_buffer)
            .expect("Active buffer should exist");

        let eol_pos = buffer.eol_pos(window.cursor);
        let text_to_kill = if eol_pos > window.cursor {
            // Kill to end of line
            let count = eol_pos - window.cursor;
            buffer.delete_pos(window.cursor, count as isize)
        } else {
            // At end of line, kill the newline character if it exists
            buffer.delete_pos(window.cursor, 1)
        };

        match text_to_kill {
            Some(killed) if !killed.is_empty() => {
                self.kill_ring.kill(killed.clone());
                vec![
                    ChromeAction::Echo(format!("Killed line: {}", killed.replace('\n', "\\n"))),
                    ChromeAction::MarkDirty(DirtyRegion::Buffer {
                        buffer_id: window.active_buffer,
                    }),
                ]
            }
            _ => {
                vec![ChromeAction::Echo("Nothing to kill".to_string())]
            }
        }
    }

    /// Kill word backward (like M-DEL or C-Backspace in Emacs)
    pub fn backward_kill_word(&mut self) -> Vec<ChromeAction> {
        let window = &mut self
            .windows
            .get_mut(self.active_window)
            .expect("Active window should exist");
        let buffer = &mut self
            .buffers
            .get_mut(window.active_buffer)
            .expect("Active buffer should exist");

        let current_pos = window.cursor;
        let word_start = buffer.move_word_backward(current_pos);

        if word_start >= current_pos {
            return vec![ChromeAction::Echo("Nothing to kill".to_string())];
        }

        // Delete from word_start to current_pos
        let count = current_pos - word_start;
        let text_to_kill = buffer.delete_pos(word_start, count as isize);

        match text_to_kill {
            Some(killed) if !killed.is_empty() => {
                self.kill_ring.kill(killed.clone());
                window.cursor = word_start;
                vec![ChromeAction::MarkDirty(DirtyRegion::Buffer {
                    buffer_id: window.active_buffer,
                })]
            }
            _ => {
                vec![ChromeAction::Echo("Nothing to kill".to_string())]
            }
        }
    }

    /// Kill word forward (like M-d in Emacs)
    pub fn forward_kill_word(&mut self) -> Vec<ChromeAction> {
        let window = &mut self
            .windows
            .get_mut(self.active_window)
            .expect("Active window should exist");
        let buffer = &mut self
            .buffers
            .get_mut(window.active_buffer)
            .expect("Active buffer should exist");

        let current_pos = window.cursor;
        let word_end = buffer.move_word_forward(current_pos);

        if word_end <= current_pos {
            return vec![ChromeAction::Echo("Nothing to kill".to_string())];
        }

        // Delete from current_pos to word_end
        let count = word_end - current_pos;
        let text_to_kill = buffer.delete_pos(current_pos, count as isize);

        match text_to_kill {
            Some(killed) if !killed.is_empty() => {
                self.kill_ring.kill(killed.clone());
                // Cursor stays at current_pos
                vec![ChromeAction::MarkDirty(DirtyRegion::Buffer {
                    buffer_id: window.active_buffer,
                })]
            }
            _ => {
                vec![ChromeAction::Echo("Nothing to kill".to_string())]
            }
        }
    }

    /// Kill the selected region
    pub fn kill_region(&mut self) -> Vec<ChromeAction> {
        let window = &mut self
            .windows
            .get_mut(self.active_window)
            .expect("Active window should exist");
        let buffer = &mut self
            .buffers
            .get_mut(window.active_buffer)
            .expect("Active buffer should exist");

        let Some((deleted, new_cursor_pos)) = buffer.delete_region(window.cursor) else {
            return vec![ChromeAction::Echo("No mark set".to_string())];
        };

        if deleted.is_empty() {
            return vec![ChromeAction::Echo("Empty region".to_string())];
        }

        // Add to kill-ring
        self.kill_ring.kill(deleted.clone());

        // Update cursor to the start of the deleted region
        window.cursor = new_cursor_pos;

        vec![
            ChromeAction::Echo(format!("Killed region: {}", deleted.replace('\n', "\\n"))),
            ChromeAction::MarkDirty(DirtyRegion::Buffer {
                buffer_id: window.active_buffer,
            }),
        ]
    }

    /// Copy region to kill-ring without deleting
    pub fn copy_region(&mut self) -> Vec<ChromeAction> {
        let window = &self.windows[self.active_window];
        let buffer = &self.buffers[window.active_buffer];

        let Some(region_text) = buffer.get_region_text(window.cursor) else {
            return vec![ChromeAction::Echo("No mark set".to_string())];
        };

        if region_text.is_empty() {
            // Clear mark for empty region
            buffer.clear_mark();
            return vec![ChromeAction::Echo("Empty region".to_string())];
        }

        // Add to kill-ring without deleting
        self.kill_ring.kill(region_text.clone());

        // Clear the mark after copying to stop region highlighting
        buffer.clear_mark();

        vec![
            ChromeAction::Echo(format!(
                "Copied region: {}",
                region_text.replace('\n', "\\n")
            )),
            ChromeAction::MarkDirty(DirtyRegion::Buffer {
                buffer_id: window.active_buffer,
            }),
        ]
    }

    /// Set mark at cursor position
    pub fn set_mark(&mut self) -> Vec<ChromeAction> {
        let window = &self.windows[self.active_window];
        let buffer = &mut self
            .buffers
            .get_mut(window.active_buffer)
            .expect("Active buffer should exist");

        buffer.set_mark(window.cursor);

        vec![ChromeAction::Echo("Mark set".to_string())]
    }

    /// Clear the mark
    pub fn clear_mark(&mut self) -> Vec<ChromeAction> {
        let window = &self.windows[self.active_window];
        let buffer = &mut self
            .buffers
            .get_mut(window.active_buffer)
            .expect("Active buffer should exist");

        if buffer.has_mark() {
            buffer.clear_mark();
            vec![
                ChromeAction::Echo("Mark cleared".to_string()),
                ChromeAction::MarkDirty(DirtyRegion::Buffer {
                    buffer_id: window.active_buffer,
                }),
            ]
        } else {
            vec![ChromeAction::Echo("No mark to clear".to_string())]
        }
    }

    /// Yank (paste) from kill-ring
    pub fn yank(&mut self, position: &ActionPosition) -> Vec<ChromeAction> {
        let Some(text) = self.kill_ring.yank().map(str::to_string) else {
            return vec![ChromeAction::Echo("Kill ring is empty".to_string())];
        };

        // Break the kill sequence since we're doing a yank
        self.kill_ring.break_kill_sequence();

        // Insert the yanked text
        self.insert_text(text, position)
    }

    /// Yank from specific kill-ring index
    pub fn yank_index(&mut self, position: &ActionPosition, index: usize) -> Vec<ChromeAction> {
        let text = match self.kill_ring.yank_index(index) {
            Some(text) => text.to_string(),
            None => return vec![ChromeAction::Echo(format!("No kill at index {index}"))],
        };

        // Break the kill sequence since we're doing a yank
        self.kill_ring.break_kill_sequence();

        // Insert the yanked text
        self.insert_text(text, position)
    }

    #[cfg(test)]
    async fn handle_open_file_action(
        &mut self,
        path: std::path::PathBuf,
        open_type: OpenType,
    ) -> Vec<ChromeAction> {
        let mut actions = Vec::new();
        if let Some(command_window_id) = self.find_command_window() {
            self.close_command_window(command_window_id);
            actions.push(ChromeAction::MarkDirty(DirtyRegion::FullScreen));
        }

        let window_to_open = self
            .previous_active_window
            .filter(|window_id| self.windows.contains_key(*window_id))
            .unwrap_or(self.active_window);
        let replaced_buffer = (open_type == OpenType::Visit)
            .then(|| self.windows[window_to_open].active_buffer)
            .filter(|buffer_id| !self.is_command_buffer(*buffer_id));

        match self.open_file_in_window(path.clone(), window_to_open).await {
            Ok(message) => {
                let opened_buffer = self.windows[window_to_open].active_buffer;
                let watch_error = if path.exists()
                    && let Some(buffer) = self.buffers.get(opened_buffer)
                {
                    self.file_watcher
                        .watch_file(opened_buffer, &path, buffer.content())
                        .err()
                        .map(|error| {
                            format!("Opened {}, but failed to watch it: {error}", path.display())
                        })
                } else {
                    None
                };

                let unwatch_error = if let Some(replaced_buffer) = replaced_buffer
                    && !self
                        .windows
                        .values()
                        .any(|window| window.active_buffer == replaced_buffer)
                {
                    let error = self.file_watcher.unwatch_file(replaced_buffer).err();
                    self.buffers.remove(replaced_buffer);
                    error.map(|error| {
                        format!("Replaced buffer, but failed to stop watching it: {error}")
                    })
                } else {
                    None
                };

                actions.push(ChromeAction::Echo(message));
                actions.extend(watch_error.into_iter().map(ChromeAction::Echo));
                actions.extend(unwatch_error.into_iter().map(ChromeAction::Echo));
                actions.push(ChromeAction::MarkDirty(DirtyRegion::FullScreen));
            }
            Err(error) => actions.push(ChromeAction::Echo(format!("Error opening file: {error}"))),
        }
        actions
    }

    /// Open a file in the specified window
    #[cfg(test)]
    async fn open_file_in_window(
        &mut self,
        file_path: std::path::PathBuf,
        window_id: WindowId,
    ) -> Result<String, String> {
        if !self.windows.contains_key(window_id) {
            return Err("Window no longer exists".to_string());
        }

        // Try to load the file
        let buffer = match Buffer::from_file(&file_path.to_string_lossy()).await {
            Ok(buffer) => buffer,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // A missing file is the normal create-new-file path.
                let buffer = Buffer::new();
                buffer.set_object(file_path.to_string_lossy().to_string());
                buffer
            }
            Err(error) => {
                return Err(format!("Failed to open {}: {error}", file_path.display()));
            }
        };

        self.open_file_content_in_window(file_path, window_id, Some(buffer.content()))
    }

    /// Create a CommandContext from the current editor state
    /// Process ChromeActions and handle those that need editor state changes
    /// Poll for external file changes and handle them with CRDT-lite merge
    /// Returns actions to update the UI if any changes were applied
    pub fn poll_file_changes(&mut self) -> Vec<ChromeAction> {
        use crate::file_watcher::{MergeResult, merge_changes};

        let mut actions = Vec::new();
        if let Some(error) = self.file_watcher.take_backend_error() {
            actions.push(ChromeAction::Echo(format!(
                "File watcher backend failed: {error}"
            )));
        }
        let events = self.file_watcher.poll_events();

        for event in events {
            // Read the new file content
            let new_content = match std::fs::read_to_string(&event.file_path) {
                Ok(content) => content,
                Err(error) => {
                    actions.push(ChromeAction::Echo(format!(
                        "External file change unavailable for {}: {error}",
                        event.file_path.display()
                    )));
                    continue;
                }
            };

            // Get the buffer and sync state
            let buffer = match self.buffers.get(event.buffer_id) {
                Some(b) => b,
                None => continue,
            };

            let base_content = match self.file_watcher.get_sync_state(event.buffer_id) {
                Some(state) => state.base_content.clone(),
                None => continue,
            };

            let local_content = buffer.content();

            // Attempt merge
            match merge_changes(&base_content, &local_content, &new_content) {
                MergeResult::NoChange => {
                    // Nothing to do
                }
                MergeResult::CleanReload(content) => {
                    // No local changes, just reload
                    // Push current state to undo first (for safety)
                    buffer.begin_undo_group();
                    let old_len = buffer.buffer_len_chars();
                    if old_len > 0 {
                        buffer.delete_region_range(0, old_len);
                    }
                    let new_len = content.chars().count();
                    buffer.insert_pos(content.clone(), 0);
                    buffer.end_undo_group();

                    // Update base
                    self.file_watcher.update_base(event.buffer_id, content);

                    actions.push(ChromeAction::Echo("Reloaded from disk".to_string()));
                    actions.push(ChromeAction::MarkDirty(DirtyRegion::Buffer {
                        buffer_id: event.buffer_id,
                    }));
                    // Trigger syntax highlighting
                    actions.push(ChromeAction::BufferChanged {
                        buffer_id: event.buffer_id,
                        start: 0,
                        old_end: old_len,
                        new_end: new_len,
                    });
                }
                MergeResult::Merged { content, message } => {
                    // Actual merge - replace buffer with merged content
                    buffer.begin_undo_group();
                    let old_len = buffer.buffer_len_chars();
                    if old_len > 0 {
                        buffer.delete_region_range(0, old_len);
                    }
                    let new_len = content.chars().count();
                    buffer.insert_pos(content.clone(), 0);
                    buffer.end_undo_group();

                    // Update base to what's on disk (new_content), NOT merged content!
                    // The buffer now has merged content which differs from disk.
                    // Base must track disk state so future changes can be detected correctly.
                    self.file_watcher
                        .update_base(event.buffer_id, new_content.clone());

                    actions.push(ChromeAction::Echo(message));
                    actions.push(ChromeAction::MarkDirty(DirtyRegion::Buffer {
                        buffer_id: event.buffer_id,
                    }));
                    // Trigger syntax highlighting
                    actions.push(ChromeAction::BufferChanged {
                        buffer_id: event.buffer_id,
                        start: 0,
                        old_end: old_len,
                        new_end: new_len,
                    });
                }
                MergeResult::LocalPreserved { new_base, message } => {
                    // Local changes kept, just update base to what's on disk
                    // Don't touch the buffer - it already has the user's changes
                    self.file_watcher.update_base(event.buffer_id, new_base);
                    actions.push(ChromeAction::Echo(message));
                }
                MergeResult::MergedWithConflicts {
                    content,
                    conflict_count,
                } => {
                    // Merge with conflicts - apply but warn user
                    buffer.begin_undo_group();
                    let old_len = buffer.buffer_len_chars();
                    if old_len > 0 {
                        buffer.delete_region_range(0, old_len);
                    }
                    let new_len = content.chars().count();
                    buffer.insert_pos(content.clone(), 0);
                    buffer.end_undo_group();

                    // Update base to what's on disk, NOT the conflict-marked content
                    self.file_watcher
                        .update_base(event.buffer_id, new_content.clone());

                    actions.push(ChromeAction::Echo(format!(
                        "Merged with {} conflict(s) - see <<<<<<< markers",
                        conflict_count
                    )));
                    actions.push(ChromeAction::MarkDirty(DirtyRegion::Buffer {
                        buffer_id: event.buffer_id,
                    }));
                    // Trigger syntax highlighting
                    actions.push(ChromeAction::BufferChanged {
                        buffer_id: event.buffer_id,
                        start: 0,
                        old_end: old_len,
                        new_end: new_len,
                    });
                }
            }
        }

        actions
    }

    /// Register a buffer for file watching (call when opening a file)
    pub fn watch_buffer(&mut self, buffer_id: BufferId, file_path: &std::path::Path) {
        if let Some(buffer) = self.buffers.get(buffer_id) {
            let content = buffer.content();
            if let Err(e) = self.file_watcher.watch_file(buffer_id, file_path, content) {
                self.set_echo_message(format!("Warning: Failed to watch file: {e}"));
            }
        }
    }

    /// Stop watching a buffer's file (call when closing a buffer)
    pub fn unwatch_buffer(&mut self, buffer_id: BufferId) {
        if let Err(error) = self.file_watcher.unwatch_file(buffer_id) {
            self.set_echo_message(format!("Warning: Failed to stop watching file: {error}"));
        }
    }

    /// Stop native delivery before platform and renderer resources are released.
    pub fn shutdown_native_work(&mut self) -> Vec<String> {
        tracing::info!("shutting down editor native work");
        self.file_watcher.shutdown()
    }

    /// Mark that we're about to save a buffer (prevents false external change detection)
    pub fn mark_buffer_saving(&mut self, buffer_id: BufferId) {
        self.file_watcher.mark_saving(buffer_id);
    }

    /// Update base content after saving
    pub fn update_buffer_base(&mut self, buffer_id: BufferId) {
        if let Some(buffer) = self.buffers.get(buffer_id) {
            let content = buffer.content();
            self.file_watcher.update_base(buffer_id, content);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;
    use slotmap::SlotMap;
    use std::sync::Mutex;

    static COMPIO_RUNTIME_LOCK: Mutex<()> = Mutex::new(());

    struct TestClock {
        now: Mutex<Instant>,
    }

    impl Clock for TestClock {
        fn now(&self) -> Instant {
            *self.now.lock().unwrap()
        }
    }

    fn test_editor() -> Editor {
        let mut buffers: SlotMap<BufferId, Buffer> = SlotMap::default();
        let scratch_buffer = Buffer::new();
        scratch_buffer.set_object("test".to_string());
        scratch_buffer.load_str("Hello\nWorld\nTest");
        let scratch_buffer_id = buffers.insert(scratch_buffer);

        let window = Window {
            x: 0,
            y: 0,
            width_chars: 80,
            height_chars: 22,
            active_buffer: scratch_buffer_id,
            cursor: 0,
            window_type: WindowType::Normal,
        };
        let mut windows: SlotMap<WindowId, Window> = SlotMap::default();
        let window_id = windows.insert(window);

        Editor {
            frame: Frame::new(80, 24),
            buffers,
            windows,
            active_window: window_id,
            previous_active_window: None,
            window_tree: WindowNode::new_leaf(window_id),
            kill_ring: KillRing::with_capacity(60),
            buffer_history: vec![],
            echo_message: "".to_string(),
            echo_message_time: None,
            clock: Arc::new(crate::native_services::SystemClock),
            mouse_drag_state: None,
            messages_buffer_id: None,
            file_watcher: crate::file_watcher::FileWatcher::new(),
        }
    }

    #[test]
    fn test_cursor_move_right() {
        let _runtime_guard = COMPIO_RUNTIME_LOCK.lock().unwrap();
        compio::runtime::Runtime::new().unwrap().block_on(async {
            let mut editor = test_editor();
            let window = &editor.windows[editor.active_window];
            let initial_cursor = window.cursor;

            // Move cursor right
            editor
                .perform_native_action(KeyAction::Cursor(CursorDirection::Right))
                .await
                .unwrap();

            // Cursor should have moved
            let window = &editor.windows[editor.active_window];
            assert_eq!(window.cursor, initial_cursor + 1);
        });
    }

    #[test]
    fn test_echo_expiry_uses_injected_clock() {
        let start = Instant::now();
        let clock = Arc::new(TestClock {
            now: Mutex::new(start),
        });
        let mut editor = test_editor();
        editor.clock = clock.clone();
        editor.echo_message = "hello".to_string();
        editor.echo_message_time = Some(start);

        assert!(!editor.check_and_clear_expired_echo());
        *clock.now.lock().unwrap() = start + Duration::from_secs(ECHO_TIMEOUT_SECS);
        assert!(editor.check_and_clear_expired_echo());
        assert!(editor.echo_message.is_empty());
    }

    #[test]
    fn open_file_reports_non_not_found_io_errors() {
        let _runtime_guard = COMPIO_RUNTIME_LOCK.lock().unwrap();
        compio::runtime::Runtime::new().unwrap().block_on(async {
            let mut editor = test_editor();
            let directory =
                std::env::temp_dir().join(format!("roe-open-error-{}", std::process::id()));
            let _ = std::fs::remove_dir(&directory);
            std::fs::create_dir(&directory).unwrap();

            let error = editor
                .open_file_in_window(directory.clone(), editor.active_window)
                .await
                .expect_err("a directory must not be treated as a new empty file");

            assert!(error.contains("Failed to open"), "{error}");
            std::fs::remove_dir(directory).unwrap();
        });
    }

    #[test]
    fn visit_file_is_transactional_and_preserves_shared_buffers() {
        let _runtime_guard = COMPIO_RUNTIME_LOCK.lock().unwrap();
        compio::runtime::Runtime::new().unwrap().block_on(async {
            let mut editor = test_editor();
            let original_buffer = editor.windows[editor.active_window].active_buffer;
            let directory =
                std::env::temp_dir().join(format!("roe-visit-error-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&directory);
            std::fs::create_dir(&directory).unwrap();

            let actions = editor
                .handle_open_file_action(directory.clone(), OpenType::Visit)
                .await;

            assert!(actions.iter().any(
                |action| matches!(action, ChromeAction::Echo(message) if message.contains("Error opening file"))
            ));
            assert_eq!(
                editor.windows[editor.active_window].active_buffer,
                original_buffer
            );
            assert!(editor.buffers.contains_key(original_buffer));

            let other_window = editor.windows[editor.active_window].clone();
            let other_window_id = editor.windows.insert(other_window);
            let visited_path = directory.join("visited.txt");
            std::fs::write(&visited_path, "visited").unwrap();
            let actions = editor
                .handle_open_file_action(visited_path, OpenType::Visit)
                .await;

            assert!(actions.iter().any(
                |action| matches!(action, ChromeAction::Echo(message) if message.contains("Opened:"))
            ));
            assert_ne!(
                editor.windows[editor.active_window].active_buffer,
                original_buffer
            );
            let visited_buffer = editor.windows[editor.active_window].active_buffer;
            assert!(editor.file_watcher.get_sync_state(visited_buffer).is_some());
            assert_eq!(editor.windows[other_window_id].active_buffer, original_buffer);
            assert!(editor.buffers.contains_key(original_buffer));

            let visited_path = editor.file_watcher.get_sync_state(visited_buffer).unwrap().file_path.clone();
            std::fs::write(&visited_path, "external").unwrap();
            let deadline = Instant::now() + Duration::from_secs(2);
            while editor.buffers[visited_buffer].content() != "external"
                && Instant::now() < deadline
            {
                editor.poll_file_changes();
                std::thread::sleep(Duration::from_millis(10));
            }
            assert_eq!(editor.buffers[visited_buffer].content(), "external");

            assert!(editor.shutdown_native_work().is_empty());
            assert!(editor.file_watcher.get_sync_state(visited_buffer).is_none());
            std::fs::remove_dir_all(directory).unwrap();
        });
    }

    #[test]
    fn public_edit_paths_use_character_offsets_for_unicode() {
        let mut editor = test_editor();
        let window_id = editor.active_window;
        let buffer_id = editor.windows[window_id].active_buffer;
        editor.buffers[buffer_id].load_str("éx");
        editor.windows[window_id].cursor = 2;
        editor.kill_ring.kill("é".to_string());

        let yank_actions = editor.yank(&ActionPosition::Cursor);
        assert_eq!(editor.buffers[buffer_id].content(), "éxé");
        assert_eq!(editor.windows[window_id].cursor, 3);
        assert!(yank_actions.iter().any(|action| matches!(
            action,
            ChromeAction::BufferChanged {
                start: 2,
                old_end: 2,
                new_end: 3,
                ..
            }
        )));

        editor.insert_text("Z".to_string(), &ActionPosition::Cursor);
        assert_eq!(editor.buffers[buffer_id].content(), "éxéZ");
        assert_eq!(editor.windows[window_id].cursor, 4);

        let delete_actions = editor.delete_text(&ActionPosition::Cursor, -2);
        assert_eq!(editor.buffers[buffer_id].content(), "éx");
        assert_eq!(editor.windows[window_id].cursor, 2);
        assert!(delete_actions.iter().any(|action| matches!(
            action,
            ChromeAction::BufferChanged {
                start: 2,
                old_end: 4,
                new_end: 2,
                ..
            }
        )));

        editor.kill_text(&ActionPosition::Cursor, -1);
        assert_eq!(editor.buffers[buffer_id].content(), "é");
        assert_eq!(editor.windows[window_id].cursor, 1);
    }

    #[test]
    fn test_cursor_move_down() {
        let _runtime_guard = COMPIO_RUNTIME_LOCK.lock().unwrap();
        compio::runtime::Runtime::new().unwrap().block_on(async {
            let mut editor = test_editor();

            // Move cursor down
            editor
                .perform_native_action(KeyAction::Cursor(CursorDirection::Down))
                .await
                .unwrap();

            // Cursor should have moved to next line
            let window = &editor.windows[editor.active_window];
            let buffer = &editor.buffers[window.active_buffer];
            let (_, line) = buffer.to_column_line(window.cursor);
            assert_eq!(line, 1);
        });
    }

    #[test]
    fn test_cursor_move_beyond_buffer() {
        let _runtime_guard = COMPIO_RUNTIME_LOCK.lock().unwrap();
        compio::runtime::Runtime::new().unwrap().block_on(async {
            let mut editor = test_editor();
            let buffer_len = {
                let window = &editor.windows[editor.active_window];
                let buffer = &editor.buffers[window.active_buffer];
                buffer.buffer_len_chars()
            };

            // Move cursor to end of buffer
            let window = &mut editor.windows[editor.active_window];
            window.cursor = buffer_len;

            // Try to move right beyond end
            let _actions = editor
                .perform_native_action(KeyAction::Cursor(CursorDirection::Right))
                .await
                .unwrap();

            // Cursor should stay at end
            let window = &editor.windows[editor.active_window];
            assert_eq!(window.cursor, buffer_len);
        });
    }

    #[test]
    fn test_window_split_horizontal() {
        let mut editor = test_editor();
        let initial_window_count = editor.windows.len();

        // Split horizontally
        let new_window_id = editor.split_horizontal();

        // Should have one more window
        assert_eq!(editor.windows.len(), initial_window_count + 1);
        assert!(editor.windows.contains_key(new_window_id));

        // Check that the layout was updated
        editor.calculate_window_layout();
        let original_window = &editor.windows[editor.active_window];
        let new_window = &editor.windows[new_window_id];

        // Both windows should be positioned correctly
        assert_eq!(original_window.x, 0);
        assert_eq!(original_window.y, 0);
        assert_eq!(new_window.x, 0);
        assert!(new_window.y > 0); // Should be below the first window

        // Check that windows have minimum size for borders and modeline
        assert!(original_window.width_chars >= 4);
        assert!(original_window.height_chars >= 4);
        assert!(new_window.width_chars >= 4);
        assert!(new_window.height_chars >= 4);
    }

    #[test]
    fn test_window_split_vertical() {
        let mut editor = test_editor();
        let initial_window_count = editor.windows.len();

        // Split vertically
        let new_window_id = editor.split_vertical();

        // Should have one more window
        assert_eq!(editor.windows.len(), initial_window_count + 1);
        assert!(editor.windows.contains_key(new_window_id));

        // Check that the layout was updated
        editor.calculate_window_layout();
        let original_window = &editor.windows[editor.active_window];
        let new_window = &editor.windows[new_window_id];

        // Both windows should be positioned correctly
        assert_eq!(original_window.x, 0);
        assert_eq!(original_window.y, 0);
        assert!(new_window.x > 0); // Should be to the right of the first window
        assert_eq!(new_window.y, 0);

        // Check that windows have minimum size for borders and modeline
        assert!(original_window.width_chars >= 4);
        assert!(original_window.height_chars >= 4);
        assert!(new_window.width_chars >= 4);
        assert!(new_window.height_chars >= 4);
    }

    #[test]
    fn test_window_delete() {
        let mut editor = test_editor();

        // Split to have two windows
        let _new_window_id = editor.split_horizontal();
        assert_eq!(editor.windows.len(), 2);

        // Delete the current window
        let deleted = editor.delete_window();
        assert!(deleted);
        assert_eq!(editor.windows.len(), 1);

        // Should not be able to delete the last window
        let deleted = editor.delete_window();
        assert!(!deleted);
        assert_eq!(editor.windows.len(), 1);
    }

    #[test]
    fn test_window_switch() {
        let mut editor = test_editor();
        let original_active = editor.active_window;

        // Split to have two windows
        let _new_window_id = editor.split_horizontal();

        // Switch windows
        editor.switch_window();

        // Active window should have changed
        assert_ne!(editor.active_window, original_active);

        // Switch again should go back
        editor.switch_window();
        assert_eq!(editor.active_window, original_active);
    }

    #[test]
    fn test_window_deletion_geometry_restoration() {
        let mut editor = test_editor();
        let original_window = editor.active_window;

        // Get initial size of the single window
        let initial_width = editor.windows[original_window].width_chars;
        let initial_height = editor.windows[original_window].height_chars;

        // Split horizontally to create two windows
        let _new_window_id = editor.split_horizontal();

        // Both windows should be smaller now
        let after_split_height = editor.windows[original_window].height_chars;
        assert!(after_split_height < initial_height);

        // Delete one window
        editor.delete_window();

        // Should only have one window remaining
        assert_eq!(editor.windows.len(), 1);

        // The remaining window should expand to fill the available space
        let remaining_window = editor.windows.keys().next().unwrap();
        let final_window = &editor.windows[remaining_window];

        // Window should be close to original size (allowing for some variance)
        assert!(final_window.width_chars >= initial_width - 2);
        assert!(final_window.height_chars >= initial_height - 2);
    }

    #[test]
    fn test_multiple_splits_then_delete_phantom_window() {
        let mut editor = test_editor();

        // Start with one window
        assert_eq!(editor.windows.len(), 1);

        // First split: horizontal (creates 2 windows)
        let _second_window = editor.split_horizontal();
        assert_eq!(editor.windows.len(), 2);

        // Second split: split the active window vertically (creates 3 windows)
        let _third_window = editor.split_vertical();
        assert_eq!(editor.windows.len(), 3);

        // Delete the current window
        let deleted = editor.delete_window();
        assert!(deleted);

        // Should have 2 windows remaining
        assert_eq!(editor.windows.len(), 2);

        // Verify that all remaining windows are valid in the tree
        verify_window_tree_integrity(&editor);
    }

    #[test]
    fn split_ratios_are_normalized_at_construction() {
        let mut windows: SlotMap<WindowId, ()> = SlotMap::with_key();
        let first = WindowNode::new_leaf(windows.insert(()));
        let second = WindowNode::new_leaf(windows.insert(()));

        for (input, expected) in [(-1.0, 0.0), (2.0, 1.0), (f32::NAN, 0.5)] {
            let node = WindowNode::new_split(
                SplitDirection::Horizontal,
                input,
                first.clone(),
                second.clone(),
            );
            let WindowNode::Split { ratio, .. } = node else {
                unreachable!();
            };
            assert_eq!(ratio, expected);
        }
    }

    #[test]
    fn test_complex_split_delete_scenario() {
        let mut editor = test_editor();

        // Create a complex tree: horizontal split, then vertical split in each half
        let _window2 = editor.split_horizontal();
        let _window3 = editor.split_vertical();
        editor.switch_window(); // Switch to the other half
        let _window4 = editor.split_vertical();

        // Should have 4 windows total
        assert_eq!(editor.windows.len(), 4);

        // Delete one window from a nested split
        let deleted = editor.delete_window();
        assert!(deleted);
        assert_eq!(editor.windows.len(), 3);

        // Verify integrity
        verify_window_tree_integrity(&editor);

        // Delete another window
        let deleted = editor.delete_window();
        assert!(deleted);
        assert_eq!(editor.windows.len(), 2);

        // Verify integrity again
        verify_window_tree_integrity(&editor);
    }

    #[test]
    fn test_deep_nested_splits() {
        let mut editor = test_editor();

        // Create a deeply nested structure
        let _w2 = editor.split_horizontal();
        let _w3 = editor.split_vertical();
        let _w4 = editor.split_horizontal();
        let _w5 = editor.split_vertical();

        assert_eq!(editor.windows.len(), 5);

        // Delete from the deepest nesting
        let deleted = editor.delete_window();
        assert!(deleted);
        assert_eq!(editor.windows.len(), 4);

        verify_window_tree_integrity(&editor);

        // Delete another deep window
        let deleted = editor.delete_window();
        assert!(deleted);
        assert_eq!(editor.windows.len(), 3);

        verify_window_tree_integrity(&editor);
    }

    #[test]
    fn test_window_selection_after_delete() {
        let mut editor = test_editor();
        let original_window = editor.active_window;

        // Create horizontal split: original window (top) and new window (bottom)
        let bottom_window = editor.split_horizontal();

        // Active window should still be the original (top) window
        assert_eq!(editor.active_window, original_window);

        // Delete the active (top) window
        let deleted = editor.delete_window();
        assert!(deleted);

        // Should now be active in the bottom window (the one that expanded)
        assert_eq!(editor.active_window, bottom_window);
        assert_eq!(editor.windows.len(), 1);

        // Test vertical split scenario
        let right_window = editor.split_vertical();

        // Delete the left window (current active window)
        let left_window = editor.active_window;
        let deleted = editor.delete_window();
        assert!(deleted);

        // Should now be active in the right window (the one that expanded)
        assert_eq!(editor.active_window, right_window);
        assert_ne!(editor.active_window, left_window); // Shouldn't be the deleted window
    }

    #[test]
    fn test_nested_window_selection_after_delete() {
        let mut editor = test_editor();

        // Create complex nested structure
        let _w2 = editor.split_horizontal(); // Split horizontally
        let w3 = editor.split_vertical(); // Split the top window vertically

        // Now we have:
        // [ w1 | w3 ]  (top half)
        // [    w2   ]  (bottom half)

        // Delete w1 (top-left)
        let w1 = editor.active_window;
        let deleted = editor.delete_window();
        assert!(deleted);

        // Should select w3 (the window that expanded horizontally to fill w1's space)
        assert_eq!(editor.active_window, w3);
        assert_ne!(editor.active_window, w1);
    }

    #[test]
    fn test_spatial_window_switching() {
        let mut editor = test_editor();
        let w1 = editor.active_window;

        // Create a layout like this:
        // [ w1 | w3 ]  (top half)
        // [    w2   ]  (bottom half)
        let w2 = editor.split_horizontal(); // Split horizontally
        let w3 = editor.split_vertical(); // Split the top window vertically

        // Now get the positions to verify our expected order
        let _w1_pos = (editor.windows[w1].x, editor.windows[w1].y);
        let _w2_pos = (editor.windows[w2].x, editor.windows[w2].y);
        let _w3_pos = (editor.windows[w3].x, editor.windows[w3].y);

        // Spatial order should be: w1 (top-left), w3 (top-right), w2 (bottom)
        // This is because we sort by y first (top-to-bottom), then by x (left-to-right)

        // Start at w1 (top-left)
        assert_eq!(editor.active_window, w1);

        // Switch to next window (should go to w3 - top-right)
        editor.switch_window();
        assert_eq!(editor.active_window, w3);

        // Switch again (should go to w2 - bottom)
        editor.switch_window();
        assert_eq!(editor.active_window, w2);

        // Switch again (should wrap back to w1 - top-left)
        editor.switch_window();
        assert_eq!(editor.active_window, w1);

        // Verify the spatial order function directly
        let spatial_order = editor.get_windows_in_spatial_order();
        assert_eq!(spatial_order.len(), 3);

        // The order should follow the spatial layout: top row (left to right), then bottom row
        let positions: Vec<(u16, u16)> = spatial_order
            .iter()
            .map(|&id| (editor.windows[id].x, editor.windows[id].y))
            .collect();

        // Verify positions are in spatial order
        for i in 1..positions.len() {
            let (x1, y1) = positions[i - 1];
            let (x2, y2) = positions[i];
            // Either same row and x2 > x1, or y2 > y1
            assert!(
                y2 > y1 || (y2 == y1 && x2 > x1),
                "Windows not in spatial order: ({x1}, {y1}) should come before ({x2}, {y2})"
            );
        }
    }

    #[test]
    fn test_spatial_order_with_complex_layout() {
        let mut editor = test_editor();

        // Create a more complex layout:
        // [ w1 | w3 | w5 ]  (top row)
        // [   w2   |  w4 ]  (bottom row)

        let w1 = editor.active_window;
        let w2 = editor.split_horizontal(); // w1 on top, w2 on bottom

        // Go back to w1 and split it vertically
        editor.active_window = w1;
        let w3 = editor.split_vertical(); // w1 left, w3 right in top half

        // Split w3 vertically to create w5
        editor.active_window = w3;
        let _w5 = editor.split_vertical(); // w3 left, w5 right in top-right

        // Split w2 vertically to create w4
        editor.active_window = w2;
        let _w4 = editor.split_vertical(); // w2 left, w4 right in bottom half

        // Test spatial switching starting from w1
        editor.active_window = w1;

        let spatial_order = editor.get_windows_in_spatial_order();

        // The spatial order should visit all windows in predictable top-to-bottom, left-to-right order
        assert_eq!(spatial_order.len(), 5);
    }

    fn verify_window_tree_integrity(editor: &Editor) {
        let remaining_windows: std::collections::HashSet<_> = editor.windows.keys().collect();
        let tree_windows = extract_windows_from_tree(&editor.window_tree);

        // All windows in the tree should exist in the SlotMap
        for tree_window in &tree_windows {
            assert!(
                editor.windows.contains_key(*tree_window),
                "Window {tree_window:?} exists in tree but not in SlotMap"
            );
        }

        // All windows in SlotMap should exist in the tree
        for window_id in remaining_windows {
            assert!(
                tree_windows.contains(&window_id),
                "Window {window_id:?} exists in SlotMap but not in tree"
            );
        }

        // Active window should exist in both
        assert!(
            editor.windows.contains_key(editor.active_window),
            "Active window {:?} not in SlotMap",
            editor.active_window
        );
        assert!(
            tree_windows.contains(&editor.active_window),
            "Active window {:?} not in tree",
            editor.active_window
        );
    }

    fn extract_windows_from_tree(node: &WindowNode) -> std::collections::HashSet<WindowId> {
        let mut windows = std::collections::HashSet::new();
        extract_windows_recursive(node, &mut windows);
        windows
    }

    fn extract_windows_recursive(
        node: &WindowNode,
        windows: &mut std::collections::HashSet<WindowId>,
    ) {
        match node {
            WindowNode::Leaf { window_id } => {
                windows.insert(*window_id);
            }
            WindowNode::Split { first, second, .. } => {
                extract_windows_recursive(first, windows);
                extract_windows_recursive(second, windows);
            }
        }
    }

    #[test]
    fn test_delete_other_windows() {
        let mut editor = test_editor();
        let original_window = editor.active_window;

        // Create multiple windows
        let _w2 = editor.split_horizontal();
        let _w3 = editor.split_vertical();
        assert_eq!(editor.windows.len(), 3);

        // Switch to a different window to test that the active one is preserved
        editor.switch_window();
        let active_before = editor.active_window;
        assert_ne!(active_before, original_window);

        // Delete other windows
        let deleted = editor.delete_other_windows();
        assert!(deleted);

        // Should only have one window left
        assert_eq!(editor.windows.len(), 1);

        // The remaining window should be the one that was active
        assert_eq!(editor.active_window, active_before);
        assert!(editor.windows.contains_key(active_before));

        // The window should fill the entire available space
        let window = &editor.windows[active_before];
        assert_eq!(window.x, 0);
        assert_eq!(window.y, 0);
        assert_eq!(window.width_chars, editor.frame.available_columns);
        assert_eq!(window.height_chars, editor.frame.available_lines);

        // Tree should be a single leaf
        match &editor.window_tree {
            WindowNode::Leaf { window_id } => {
                assert_eq!(*window_id, active_before);
            }
            WindowNode::Split { .. } => {
                panic!("Tree should be a single leaf after delete_other_windows");
            }
        }
    }

    #[test]
    fn test_delete_other_windows_single_window() {
        let mut editor = test_editor();

        // Try to delete other windows when there's only one
        let deleted = editor.delete_other_windows();
        assert!(!deleted); // Should return false

        // Should still have one window
        assert_eq!(editor.windows.len(), 1);
    }

    #[test]
    fn test_kill_line() {
        let mut editor = test_editor();
        let window = &mut editor.windows[editor.active_window];
        window.cursor = 2; // Position at 'l' in "Hello"

        // Kill from cursor to end of line
        let actions = editor.kill_line();

        // Should have a killed message and refresh
        assert!(actions.iter().any(|a| matches!(a, ChromeAction::Echo(_))));
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, ChromeAction::MarkDirty(_)))
        );

        // Check that text was killed and kill-ring has content
        assert!(!editor.kill_ring.is_empty());
        let killed_text = editor.kill_ring.current().unwrap();
        assert_eq!(killed_text, "llo"); // "llo" from "He[l]lo"

        // Check buffer content
        let window = &editor.windows[editor.active_window];
        let buffer = &editor.buffers[window.active_buffer];
        assert_eq!(buffer.content(), "He\nWorld\nTest");
    }

    #[test]
    fn test_kill_line_consecutive() {
        let mut editor = test_editor();
        let window = &mut editor.windows[editor.active_window];
        window.cursor = 5; // At end of "Hello"

        // Kill the newline
        editor.kill_line();

        // Kill from beginning of next line
        let window = &mut editor.windows[editor.active_window];
        window.cursor = 5; // Still at position 5, but now it's at "World"
        editor.kill_line();

        // Should have appended kills
        let killed_text = editor.kill_ring.current().unwrap();
        assert_eq!(killed_text, "\nWorld"); // Newline + "World"
    }

    #[test]
    fn test_yank_basic() {
        let mut editor = test_editor();

        // First kill some text
        let window = &mut editor.windows[editor.active_window];
        window.cursor = 0; // Start of buffer
        editor.kill_line(); // Kill "Hello"

        // Move cursor and yank
        let window = &mut editor.windows[editor.active_window];
        window.cursor = 0; // Start of buffer (now at "\ncruel...")
        let actions = editor.yank(&ActionPosition::cursor());

        // Should have inserted text
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, ChromeAction::MarkDirty(_)))
        );

        // Check buffer content
        let window = &editor.windows[editor.active_window];
        let buffer = &editor.buffers[window.active_buffer];
        let content = buffer.content();
        assert!(content.starts_with("Hello")); // Yanked text should be at start
    }

    #[test]
    fn test_yank_index() {
        let mut editor = test_editor();

        // Kill multiple pieces of text
        let window = &mut editor.windows[editor.active_window];
        window.cursor = 0;
        editor.kill_line(); // Kill "Hello"

        // Break sequence and kill something else
        editor.kill_ring.break_kill_sequence();
        let window = &mut editor.windows[editor.active_window];
        window.cursor = 0;
        editor.kill_line(); // Kill "cruel"

        // Yank the first kill (index 1, "Hello")
        let window = &mut editor.windows[editor.active_window];
        window.cursor = 0;
        editor.yank_index(&ActionPosition::cursor(), 1);

        // Check that we got the older kill
        let window = &editor.windows[editor.active_window];
        let buffer = &editor.buffers[window.active_buffer];
        let content = buffer.content();
        assert!(content.starts_with("Hello"));
    }

    #[test]
    fn test_kill_ring_max_capacity() {
        let mut editor = test_editor();

        // Fill up the kill ring beyond capacity
        for i in 0..65 {
            // More than default capacity of 60
            editor.kill_ring.break_kill_sequence();
            editor.kill_ring.kill(format!("kill-{i}"));
        }

        // Should be at max capacity
        assert_eq!(editor.kill_ring.len(), 60);

        // Most recent should be kill-64
        let recent = editor.kill_ring.yank().unwrap();
        assert_eq!(recent, "kill-64");
    }

    #[test]
    fn test_kill_sequence_break() {
        let mut editor = test_editor();

        // Kill some text
        editor.kill_ring.kill("first".to_string());

        // Do a non-kill operation (insert text)
        let window = &mut editor.windows[editor.active_window];
        window.cursor = 0;
        editor.insert_text("test".to_string(), &ActionPosition::cursor());

        // Kill again - should be separate entry
        editor.kill_ring.kill("second".to_string());

        // Should have two separate entries
        assert_eq!(editor.kill_ring.len(), 2);
        assert_eq!(editor.kill_ring.yank().unwrap(), "second");
    }

    #[test]
    fn test_empty_kill_ring_yank() {
        let mut editor = test_editor();

        // Try to yank from empty kill ring
        let actions = editor.yank(&ActionPosition::cursor());

        // Should get an error message
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, ChromeAction::Echo(msg) if msg.contains("empty")))
        );
    }

    #[test]
    fn test_set_mark() {
        let mut editor = test_editor();
        let window = &mut editor.windows[editor.active_window];
        window.cursor = 5; // End of "Hello"

        // Set mark at cursor position
        let actions = editor.set_mark();

        // Should get confirmation message
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, ChromeAction::Echo(msg) if msg.contains("Mark set")))
        );

        // Check that mark was set in buffer
        let window = &editor.windows[editor.active_window];
        let buffer = &editor.buffers[window.active_buffer];
        assert!(buffer.has_mark());
        assert_eq!(buffer.get_mark(), Some(5));
    }

    #[test]
    fn test_clear_mark() {
        let mut editor = test_editor();
        let window = &mut editor.windows[editor.active_window];
        let buffer = &mut editor.buffers.get_mut(window.active_buffer).unwrap();

        // Set a mark first
        buffer.set_mark(3);
        assert!(buffer.has_mark());

        // Clear mark
        let actions = editor.clear_mark();

        // Should get confirmation message
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, ChromeAction::Echo(msg) if msg.contains("Mark cleared")))
        );

        // Check that mark was cleared
        let window = &editor.windows[editor.active_window];
        let buffer = &editor.buffers[window.active_buffer];
        assert!(!buffer.has_mark());
    }

    #[test]
    fn test_clear_mark_when_no_mark() {
        let mut editor = test_editor();

        // Try to clear mark when none is set
        let actions = editor.clear_mark();

        // Should get error message
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, ChromeAction::Echo(msg) if msg.contains("No mark to clear")))
        );
    }

    #[test]
    fn test_kill_region_basic() {
        let mut editor = test_editor(); // "Hello\nWorld\nTest"

        // Set mark at position 2 ('l' in "Hello")
        let window = &mut editor.windows[editor.active_window];
        let buffer = &mut editor.buffers.get_mut(window.active_buffer).unwrap();
        buffer.set_mark(2);

        // Move cursor to position 8 ('o' in "World")
        let window = &mut editor.windows[editor.active_window];
        window.cursor = 8;

        // Kill region
        let actions = editor.kill_region();

        // Should have killed message and refresh
        assert!(actions.iter().any(|a| matches!(a, ChromeAction::Echo(_))));
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, ChromeAction::MarkDirty(_)))
        );

        // Check that text was killed and added to kill-ring
        assert!(!editor.kill_ring.is_empty());
        let killed_text = editor.kill_ring.current().unwrap();
        assert_eq!(killed_text, "llo\nWo"); // "llo\nWo" from "He[llo\nWo]rld"

        // Check buffer content after kill
        let window = &editor.windows[editor.active_window];
        let buffer = &editor.buffers[window.active_buffer];
        assert_eq!(buffer.content(), "Herld\nTest");

        // Check cursor position (should be at start of killed region)
        assert_eq!(window.cursor, 2);

        // Mark should be cleared
        assert!(!buffer.has_mark());
    }

    #[test]
    fn test_kill_region_no_mark() {
        let mut editor = test_editor();

        // Try to kill region without setting mark
        let actions = editor.kill_region();

        // Should get error message
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, ChromeAction::Echo(msg) if msg.contains("No mark set")))
        );

        // Buffer should be unchanged
        let window = &editor.windows[editor.active_window];
        let buffer = &editor.buffers[window.active_buffer];
        assert_eq!(buffer.content(), "Hello\nWorld\nTest");

        // Kill-ring should be empty
        assert!(editor.kill_ring.is_empty());
    }

    #[test]
    fn test_kill_region_empty() {
        let mut editor = test_editor();

        // Set mark at cursor position (empty region)
        let window = &mut editor.windows[editor.active_window];
        window.cursor = 5;
        let buffer = &mut editor.buffers.get_mut(window.active_buffer).unwrap();
        buffer.set_mark(5);

        // Kill empty region
        let actions = editor.kill_region();

        // Should get empty region message
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, ChromeAction::Echo(msg) if msg.contains("Empty region")))
        );

        // Buffer should be unchanged
        let window = &editor.windows[editor.active_window];
        let buffer = &editor.buffers[window.active_buffer];
        assert_eq!(buffer.content(), "Hello\nWorld\nTest");

        // Mark should be cleared
        assert!(!buffer.has_mark());
    }

    #[test]
    fn test_kill_region_reverse() {
        let mut editor = test_editor(); // "Hello\nWorld\nTest"

        // Set mark at position 8 ('o' in "World")
        let window = &mut editor.windows[editor.active_window];
        let buffer = &mut editor.buffers.get_mut(window.active_buffer).unwrap();
        buffer.set_mark(8);

        // Move cursor to position 2 ('l' in "Hello") - before mark
        let window = &mut editor.windows[editor.active_window];
        window.cursor = 2;

        // Kill region (should work in reverse)
        let actions = editor.kill_region();

        // Should have killed message
        assert!(actions.iter().any(|a| matches!(a, ChromeAction::Echo(_))));

        // Check that same text was killed
        let killed_text = editor.kill_ring.current().unwrap();
        assert_eq!(killed_text, "llo\nWo"); // Same region regardless of direction

        // Check buffer content
        let window = &editor.windows[editor.active_window];
        let buffer = &editor.buffers[window.active_buffer];
        assert_eq!(buffer.content(), "Herld\nTest");

        // Cursor should be at start of region (position 2)
        assert_eq!(window.cursor, 2);
    }

    #[test]
    fn test_region_kill_integration_with_yank() {
        let mut editor = test_editor(); // "Hello\nWorld\nTest"

        // Set mark and kill region
        let window = &mut editor.windows[editor.active_window];
        let buffer = &mut editor.buffers.get_mut(window.active_buffer).unwrap();
        buffer.set_mark(2); // 'l' in "Hello"

        let window = &mut editor.windows[editor.active_window];
        window.cursor = 8; // 'o' in "World"

        editor.kill_region(); // Kill "llo\nWo"

        // Move cursor to end of buffer
        let window = &mut editor.windows[editor.active_window];
        window.cursor = editor.buffers[window.active_buffer].buffer_len_chars();

        // Yank the killed region
        let actions = editor.yank(&ActionPosition::cursor());

        // Should have refresh action
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, ChromeAction::MarkDirty(_)))
        );

        // Check buffer content - should have yanked text at end
        let window = &editor.windows[editor.active_window];
        let buffer = &editor.buffers[window.active_buffer];
        assert_eq!(buffer.content(), "Herld\nTestllo\nWo");
    }

    #[test]
    fn messages_buffer_retains_a_bounded_recent_tail() {
        let mut editor = test_editor();
        for index in 0..2_000 {
            editor.add_message_to_buffer(format!("{index:04} {}", "x".repeat(64)));
        }
        let messages = editor.messages_buffer_id.unwrap();
        let content = editor.buffers[messages].content();
        assert!(content.chars().count() <= MAX_MESSAGES_CHARS);
        assert!(content.contains("1999"));
        assert!(!content.contains("0000 x"));
    }
}
