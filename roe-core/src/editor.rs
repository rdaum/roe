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
use crate::buffer_host::{self, BufferHostClient};
use crate::buffer_switch_mode::{BufferSwitchMode, BufferSwitchPurpose};
use crate::command_mode::CommandMode;
use crate::command_registry::CommandRegistry;
use crate::file_selector_mode::FileSelectorMode;
use crate::keys::KeyAction::ChordNext;
use crate::keys::{Bindings, CursorDirection, KeyAction, KeyState, LogicalKey};
use crate::kill_ring::KillRing;
use crate::mode::{ActionPosition, MessagesMode, Mode};
use crate::renderer::{DirtyRegion, ModelineComponent};
use crate::{BufferId, ModeId, WindowId};
use slotmap::SlotMap;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// How long echo messages remain visible (in seconds)
const ECHO_TIMEOUT_SECS: u64 = 3;

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

/// Type of command being executed in a command window
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandType {
    /// M-x command execution
    Execute,
    /// C-x b buffer switching
    BufferSwitch,
    /// C-x k buffer killing
    KillBuffer,
    /// File opening
    FindFile,
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
    /// What line is the top left corner of the window in the buffer at?
    pub start_line: u16,
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
    pub buffer_hosts: HashMap<BufferId, BufferHostClient>,
    pub windows: SlotMap<WindowId, Window>,
    pub modes: SlotMap<ModeId, Box<dyn Mode>>, // Keep for now for compatibility
    pub active_window: WindowId,
    pub key_state: KeyState,
    pub bindings: Box<dyn Bindings>,
    /// Tree structure representing window layout
    pub window_tree: WindowNode,
    /// Global kill-ring for cut/copy/paste operations
    pub kill_ring: KillRing,
    /// Command registry for M-x commands
    pub command_registry: CommandRegistry,
    /// Window that was active before opening command/buffer switch window
    pub previous_active_window: Option<WindowId>,
    /// Buffer history (most recently used first) for smart buffer switching
    pub buffer_history: Vec<BufferId>,
    /// Current echo area message
    pub echo_message: String,
    /// When the echo message was set (for auto-clearing)
    pub echo_message_time: Option<Instant>,
    /// Current key chord being typed (for echo area display)
    pub current_key_chord: Vec<LogicalKey>,
    /// Mouse drag state for window resizing
    pub mouse_drag_state: Option<MouseDragState>,
    /// Messages buffer for collecting echo messages and logs
    pub messages_buffer_id: Option<BufferId>,
}

/// The main event loop, which receives keystrokes and dispatches them to the mode in the buffer
/// in the active window.
impl Editor {}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ChromeAction {
    /// Open the find-file dialog
    FindFile,
    /// Open the command palette (M-x)
    CommandMode,
    /// Open buffer switch dialog
    SwitchBuffer,
    /// Open kill buffer dialog  
    KillBuffer,
    /// Save current buffer
    Save,
    /// Move cursor to position
    CursorMove((u16, u16)),
    /// Unknown/unhandled action
    Huh,
    /// Show message in echo area
    Echo(String),
    /// Mark region as dirty for redraw
    MarkDirty(DirtyRegion),
    /// Quit the editor
    Quit,
    /// Split window horizontally
    SplitHorizontal,
    /// Split window vertically  
    SplitVertical,
    /// Switch to other window
    SwitchWindow,
    /// Delete current window
    DeleteWindow,
    /// Delete all other windows
    DeleteOtherWindows,
    /// Show messages buffer
    ShowMessages,
}

impl Editor {
    /// Create a command window and associated buffer
    pub fn create_command_window(
        &mut self,
        command_type: CommandType,
        position: CommandWindowPosition,
        height: u16,
    ) -> WindowId {
        // Create a new buffer for command input
        let command_buffer = Buffer::new(&[]);
        command_buffer.set_object(format!(
            "*Command:{}*",
            match command_type {
                CommandType::Execute => "Execute",
                CommandType::BufferSwitch => "Switch Buffer",
                CommandType::KillBuffer => "Kill Buffer",
                CommandType::FindFile => "Find File",
            }
        ));

        let command_buffer_id = self.buffers.insert(command_buffer.clone());

        // Create the appropriate mode based on command type
        let (mode_box, mode_name, initial_content) = match command_type {
            CommandType::Execute => {
                // Create CommandMode for M-x
                let mut command_names: Vec<String> = self
                    .command_registry
                    .all_commands()
                    .iter()
                    .filter(|cmd| cmd.name != crate::command_registry::CMD_COMMAND_MODE) // Exclude command-mode
                    .map(|cmd| cmd.name.clone())
                    .collect();
                command_names.sort(); // Sort alphabetically
                let mut command_mode = CommandMode::new();
                command_mode.init_with_buffer(command_buffer_id, command_names);

                let content = command_mode.generate_buffer_content();
                (
                    Box::new(command_mode) as Box<dyn Mode>,
                    "command".to_string(),
                    content,
                )
            }
            CommandType::BufferSwitch => {
                // Create BufferSwitchMode for C-x b
                // Show all buffers except command window buffers (including the current one being created)
                let mut command_buffer_ids: HashSet<BufferId> = self
                    .windows
                    .iter()
                    .filter(|(_, window)| matches!(window.window_type, WindowType::Command { .. }))
                    .map(|(_, window)| window.active_buffer)
                    .collect();

                // Also exclude the command buffer we're about to create
                command_buffer_ids.insert(command_buffer_id);

                let buffer_list: Vec<(BufferId, String)> = self
                    .buffers
                    .iter()
                    .filter(|(id, _)| !command_buffer_ids.contains(id))
                    .map(|(id, buffer)| (id, buffer.object()))
                    .collect();
                let mut buffer_switch_mode =
                    BufferSwitchMode::new_with_purpose(BufferSwitchPurpose::Switch);

                // For switch mode, pre-select the previous buffer (most recently used other than current)
                let current_buffer_id = self.windows[self.active_window].active_buffer;
                if let Some(previous_buffer_id) = self.get_previous_buffer(current_buffer_id) {
                    buffer_switch_mode.init_with_buffer_and_preselect(
                        command_buffer_id,
                        buffer_list,
                        previous_buffer_id,
                    );
                } else {
                    buffer_switch_mode.init_with_buffer(command_buffer_id, buffer_list);
                }

                let content = buffer_switch_mode.generate_buffer_content();
                (
                    Box::new(buffer_switch_mode) as Box<dyn Mode>,
                    "buffer-switch".to_string(),
                    content,
                )
            }
            CommandType::KillBuffer => {
                // Create BufferSwitchMode for C-x k (reuse buffer switch UI)
                // Show all buffers except command window buffers (including the current one being created)
                let mut command_buffer_ids: HashSet<BufferId> = self
                    .windows
                    .iter()
                    .filter(|(_, window)| matches!(window.window_type, WindowType::Command { .. }))
                    .map(|(_, window)| window.active_buffer)
                    .collect();

                // Also exclude the command buffer we're about to create
                command_buffer_ids.insert(command_buffer_id);

                let buffer_list: Vec<(BufferId, String)> = self
                    .buffers
                    .iter()
                    .filter(|(id, _)| !command_buffer_ids.contains(id))
                    .map(|(id, buffer)| (id, buffer.object()))
                    .collect();
                let mut buffer_switch_mode =
                    BufferSwitchMode::new_with_purpose(BufferSwitchPurpose::Kill);
                // For kill mode, pre-select the current buffer
                let current_buffer_id = self.windows[self.active_window].active_buffer;
                buffer_switch_mode.init_with_buffer_and_preselect(
                    command_buffer_id,
                    buffer_list,
                    current_buffer_id,
                );

                let content = buffer_switch_mode.generate_buffer_content();
                (
                    Box::new(buffer_switch_mode) as Box<dyn Mode>,
                    "buffer-kill".to_string(),
                    content,
                )
            }
            CommandType::FindFile => {
                // Create FileSelectorMode for C-x C-f
                let mut file_selector_mode = FileSelectorMode::new();
                file_selector_mode.init_with_buffer(command_buffer_id);

                let content = file_selector_mode.generate_buffer_content();
                (
                    Box::new(file_selector_mode) as Box<dyn Mode>,
                    "file-selector".to_string(),
                    content,
                )
            }
        };

        // Generate initial buffer content with completions
        command_buffer.load_str(&initial_content);

        // Create mode ID and add to modes collection
        let mode_id = self.modes.insert(mode_box);

        // Create BufferHost with the appropriate mode
        let mode_list = vec![(
            mode_id,
            mode_name,
            self.modes
                .remove(mode_id)
                .expect("Mode should exist in SlotMap"),
        )];

        let (buffer_client, _buffer_handle) =
            crate::buffer_host::create_buffer_host(command_buffer, mode_list, command_buffer_id);

        // Insert the BufferHost using the buffer ID as the key for easy lookup/cleanup
        self.buffer_hosts.insert(command_buffer_id, buffer_client);

        // Calculate position and size for command window
        // Frame.available_lines already excludes echo area
        let (x, y) = match position {
            CommandWindowPosition::Top => (0, 0),
            CommandWindowPosition::Bottom => (0, self.frame.available_lines.saturating_sub(height)),
        };

        // Create command window
        let command_window = Window {
            x,
            y,
            width_chars: self.frame.available_columns,
            height_chars: height,
            active_buffer: command_buffer_id,
            start_line: 0,
            cursor: 0, // Start at beginning
            window_type: WindowType::Command {
                position,
                command_type,
            },
        };

        let window_id = self.windows.insert(command_window);

        // Save the current active window before switching
        self.previous_active_window = Some(self.active_window);

        // Make the command window the active window so keys go to it
        self.active_window = window_id;

        self.calculate_window_layout();
        window_id
    }

    /// Close command window and clean up its buffer
    pub fn close_command_window(&mut self, window_id: WindowId) -> bool {
        if let Some(window) = self.windows.get(window_id) {
            if matches!(window.window_type, WindowType::Command { .. }) {
                let buffer_id = window.active_buffer;
                self.windows.remove(window_id);
                self.buffers.remove(buffer_id);

                // Clean up the buffer host - this is critical for proper state cleanup
                self.buffer_hosts.remove(&buffer_id);

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
            // Create new Messages buffer
            let messages_mode = Box::new(MessagesMode {});
            let messages_mode_id = self.modes.insert(messages_mode);

            let messages_buffer = Buffer::new(&[messages_mode_id]);
            messages_buffer.set_object("*Messages*".to_string());
            messages_buffer
                .load_str("Messages buffer - echo messages and logs will appear here.\n\n");

            let messages_buffer_id = self.buffers.insert(messages_buffer.clone());

            // Create BufferHost for the messages buffer
            let mode_list = vec![(
                messages_mode_id,
                "messages".to_string(),
                self.modes
                    .remove(messages_mode_id)
                    .expect("Messages mode should exist in SlotMap"),
            )];
            let (buffer_client, _buffer_handle) = crate::buffer_host::create_buffer_host(
                messages_buffer,
                mode_list,
                messages_buffer_id,
            );
            self.buffer_hosts.insert(messages_buffer_id, buffer_client);

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
        }
    }

    /// Set the echo area message (this will override any chord display)
    pub fn set_echo_message(&mut self, message: String) {
        self.echo_message = message.clone();
        self.echo_message_time = Some(Instant::now());
        // Clear chord since we're showing a different message
        self.current_key_chord.clear();

        // Also add the message to the Messages buffer
        self.add_message_to_buffer(message);
    }

    /// Clear the echo area message
    pub fn clear_echo_message(&mut self) {
        self.echo_message.clear();
        self.echo_message_time = None;
    }

    /// Clear the current key chord sequence
    pub fn clear_key_chord(&mut self) {
        self.current_key_chord.clear();
        self.clear_echo_message();
    }

    /// Update echo area with current key chord
    pub fn update_echo_with_chord(&mut self) {
        if !self.current_key_chord.is_empty() {
            self.echo_message = self.format_key_chord(&self.current_key_chord);
        }
    }

    /// Check if echo message should be auto-cleared and clear it if needed
    /// Returns true if the message was cleared
    pub fn check_and_clear_expired_echo(&mut self) -> bool {
        if let Some(echo_time) = self.echo_message_time {
            if echo_time.elapsed() >= Duration::from_secs(ECHO_TIMEOUT_SECS) {
                self.clear_echo_message();
                return true;
            }
        }
        false
    }

    /// Format a key chord in Emacs style (e.g., "C-x", "M-x", "C-x C-c")
    fn format_key_chord(&self, keys: &[LogicalKey]) -> String {
        let mut result = Vec::new();
        let mut i = 0;

        while i < keys.len() {
            match &keys[i] {
                LogicalKey::Modifier(_modifier) => {
                    // Check if there's a following non-modifier key
                    if i + 1 < keys.len() {
                        match &keys[i + 1] {
                            LogicalKey::Modifier(_) => {
                                // Two modifiers in a row, treat separately
                                result.push(keys[i].as_display_string());
                                i += 1;
                            }
                            _ => {
                                // Modifier followed by regular key, combine with dash
                                let modifier_str = keys[i].as_display_string();
                                let key_str = keys[i + 1].as_display_string();
                                result.push(format!("{modifier_str}-{key_str}"));
                                i += 2;
                            }
                        }
                    } else {
                        // Modifier at end, display as-is
                        result.push(keys[i].as_display_string());
                        i += 1;
                    }
                }
                _ => {
                    // Non-modifier key
                    result.push(keys[i].as_display_string());
                    i += 1;
                }
            }
        }

        result.join(" ")
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

    pub fn key_event(
        &mut self,
        keys: Vec<LogicalKey>,
    ) -> Result<Vec<ChromeAction>, std::io::Error> {
        // Check if echo message has expired and clear it
        let echo_cleared = self.check_and_clear_expired_echo();

        for key in keys {
            self.key_state.press(key);
        }

        // Send pressed keys through to the bindings.
        // If responds with ChordNext, we keep.
        // Otherwise, we take() and pass that to the mode for execution.
        // If the mode returns an action, we execute that action.
        let pressed = self.key_state.pressed();
        let key_action = self
            .bindings
            .keystroke(pressed.iter().map(|k| k.key).collect());

        if key_action == ChordNext {
            // Update chord display with current pressed keys
            self.current_key_chord = pressed.iter().map(|k| k.key).collect();
            self.update_echo_with_chord();
            // Return an Echo action to trigger redraw of echo area
            return Ok(vec![ChromeAction::Echo(self.echo_message.clone())]);
        }

        // For unbound keys, capture the full key sequence before clearing
        let unbound_key_sequence = if key_action == KeyAction::Unbound {
            pressed.iter().map(|k| k.key).collect::<Vec<_>>()
        } else {
            vec![]
        };

        let _ = self.key_state.take();

        // Clear the key chord after processing (action completed)
        self.clear_key_chord();

        // Skip echo in tests to avoid terminal issues
        let active_buffer_id = {
            let window = &self.windows[self.active_window];
            window.active_buffer
        };

        // Command mode is now handled by the Mode system, not here

        // Some actions like save, quit, etc. are out of the control of the mode.
        match &key_action {
            KeyAction::Escape => {
                // If command window is active, close it
                if let Some(command_window_id) = self.find_command_window() {
                    self.close_command_window(command_window_id);
                    return Ok(vec![
                        ChromeAction::Echo("Command mode cancelled".to_string()),
                        ChromeAction::MarkDirty(DirtyRegion::FullScreen),
                    ]);
                }
                // Otherwise, pass to modes
            }

            KeyAction::Cursor(cd) => {
                // Check if we're in a command window - if so, delegate to Mode system
                let current_window = &self.windows[self.active_window];
                if matches!(current_window.window_type, WindowType::Command { .. }) {
                    // Let the Mode system handle cursor movement in command windows
                    // Fall through to the BufferHost dispatch below
                } else {
                    // Handle normal cursor movement in regular windows
                    // Get fresh references for cursor movement
                    let window = &mut self
                        .windows
                        .get_mut(self.active_window)
                        .expect("Active window should exist");
                    let buffer = &self.buffers[window.active_buffer];

                    // Use clean character-position API
                    let new_pos = match cd {
                        CursorDirection::Left => buffer.move_left(window.cursor),
                        CursorDirection::Right => buffer.move_right(window.cursor),
                        CursorDirection::Up => buffer.move_up(window.cursor),
                        CursorDirection::Down => buffer.move_down(window.cursor),
                        CursorDirection::LineStart => buffer.move_line_start(window.cursor),
                        CursorDirection::LineEnd => buffer.move_line_end(window.cursor),
                        CursorDirection::BufferStart => buffer.move_buffer_start(),
                        CursorDirection::BufferEnd => buffer.move_buffer_end(),
                        CursorDirection::PageUp => {
                            let content_height = window.height_chars.saturating_sub(3); // Account for border + modeline
                            let (current_col, current_line) = buffer.to_column_line(window.cursor);
                            let target_line = current_line.saturating_sub(content_height);
                            buffer.to_char_index(current_col, target_line)
                        }
                        CursorDirection::PageDown => {
                            let content_height = window.height_chars.saturating_sub(3); // Account for border + modeline
                            let (current_col, current_line) = buffer.to_column_line(window.cursor);
                            let target_line = current_line + content_height;
                            // Bounds check: don't go past the last line
                            let max_line = buffer.buffer_len_lines().saturating_sub(1) as u16;
                            let safe_target_line = target_line.min(max_line);
                            buffer.to_char_index(current_col, safe_target_line)
                        }
                        CursorDirection::WordForward => buffer.move_word_forward(window.cursor),
                        CursorDirection::WordBackward => buffer.move_word_backward(window.cursor),
                        CursorDirection::ParagraphForward => {
                            buffer.move_paragraph_forward(window.cursor)
                        }
                        CursorDirection::ParagraphBackward => {
                            buffer.move_paragraph_backward(window.cursor)
                        }
                    };

                    window.cursor = new_pos;

                    // Now compute the physical position of the cursor in the window.
                    let (col, line) = buffer.to_column_line(new_pos);

                    // Auto-scroll to keep cursor visible
                    let content_height = window.height_chars.saturating_sub(3); // Account for border + modeline
                    let needs_redraw =
                        Self::ensure_cursor_visible_static(window, line, content_height);

                    let mut actions = vec![ChromeAction::CursorMove(
                        window.absolute_cursor_position(col, line),
                    )];

                    // If we scrolled, mark the entire buffer dirty to redraw everything
                    if needs_redraw {
                        actions.push(ChromeAction::MarkDirty(DirtyRegion::Buffer {
                            buffer_id: window.active_buffer,
                        }));
                    }

                    // If there's a mark set, cursor movement changes the region highlighting
                    // so we need to mark the buffer dirty to trigger a redraw
                    if buffer.has_mark() {
                        actions.push(ChromeAction::MarkDirty(DirtyRegion::Buffer {
                            buffer_id: window.active_buffer,
                        }));
                    }

                    // Cursor movement always updates the position in the modeline
                    actions.push(ChromeAction::MarkDirty(DirtyRegion::Modeline {
                        window_id: self.active_window,
                        component: ModelineComponent::CursorPosition,
                    }));

                    return Ok(actions);
                }
            }
            KeyAction::Cancel => {
                // Cancel current operation - check command window first, then mark
                if let Some(command_window_id) = self.find_command_window() {
                    self.close_command_window(command_window_id);
                    return Ok(vec![
                        ChromeAction::Echo("Command mode cancelled".to_string()),
                        ChromeAction::MarkDirty(DirtyRegion::FullScreen),
                    ]);
                }

                let window = &self.windows[self.active_window];
                let buffer = &self.buffers[window.active_buffer];

                if buffer.has_mark() {
                    return Ok(self.clear_mark());
                } else {
                    return Ok(vec![ChromeAction::Echo("Quit".to_string())]);
                }
            }
            KeyAction::Unbound => {
                // Include the key sequence in the undefined message, like Emacs
                let unbound_message = if !unbound_key_sequence.is_empty() {
                    format!(
                        "{} is undefined",
                        self.format_key_chord(&unbound_key_sequence)
                    )
                } else {
                    "Key is undefined".to_string()
                };
                return Ok(vec![ChromeAction::Echo(unbound_message)]);
            }
            KeyAction::Command(command_name) => {
                // Execute command through unified command system
                let context = self.create_command_context();

                if let Some(command) = self.command_registry.get_command(command_name) {
                    match command.execute(context) {
                        Ok(actions) => return Ok(self.process_chrome_actions(actions)),
                        Err(error_msg) => {
                            return Ok(vec![ChromeAction::Echo(format!("Error: {error_msg}"))]);
                        }
                    }
                } else {
                    return Ok(vec![ChromeAction::Echo(format!(
                        "Command not found: '{}'. Available commands: {}",
                        command_name,
                        self.command_registry
                            .all_commands()
                            .iter()
                            .map(|c| &c.name)
                            .take(5)
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))]);
                }
            }
            _ => {}
        }

        // Dispatch the key to the BufferHost for the active buffer

        let buffer_id = active_buffer_id;
        let cursor_pos = {
            let window = &self.windows[self.active_window];
            window.cursor
        };

        let chrome_actions = if let Some(buffer_host) = self.buffer_hosts.get(&buffer_id).cloned() {
            // Use async runtime to handle the async BufferHost call
            let response_result = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current()
                    .block_on(async { buffer_host.handle_key(key_action, cursor_pos).await })
            });

            match response_result {
                Ok(response) => tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current()
                        .block_on(async { self.handle_buffer_response(response).await })
                }),
                Err(err) => vec![ChromeAction::Echo(format!("Buffer error: {err}"))],
            }
        } else {
            vec![ChromeAction::Echo("No buffer host available".to_string())]
        };

        // If echo was cleared due to timeout, add an echo action to trigger redraw
        let mut final_actions = chrome_actions;
        if echo_cleared {
            final_actions.push(ChromeAction::Echo(self.echo_message.clone()));
        }

        Ok(final_actions)
    }

    /// Convert BufferResponse to ChromeActions
    pub async fn handle_buffer_response(
        &mut self,
        response: crate::buffer_host::BufferResponse,
    ) -> Vec<ChromeAction> {
        use crate::buffer_host::BufferResponse;

        match response {
            BufferResponse::ActionsCompleted {
                dirty_regions,
                new_cursor_pos,
                editor_action,
            } => {
                let mut actions = vec![];

                // Handle dirty regions
                for dirty_region in dirty_regions {
                    actions.push(ChromeAction::MarkDirty(dirty_region));
                }

                // Handle cursor movement
                if let Some(new_pos) = new_cursor_pos {
                    // Update the window's cursor position
                    let window = &mut self
                        .windows
                        .get_mut(self.active_window)
                        .expect("Active window should exist");
                    window.cursor = new_pos;

                    let buffer = &self.buffers[window.active_buffer];
                    let (col, line) = buffer.to_column_line(new_pos);
                    actions.push(ChromeAction::CursorMove(
                        window.absolute_cursor_position(col, line),
                    ));
                }

                // Handle editor action
                if let Some(action) = editor_action {
                    use crate::buffer_host::EditorAction;
                    match action {
                        EditorAction::ExecuteCommand(command_name) => {
                            // Close the command window after command selection
                            if let Some(command_window_id) = self.find_command_window() {
                                self.close_command_window(command_window_id);
                                actions.push(ChromeAction::MarkDirty(DirtyRegion::FullScreen));
                            }
                            // Execute the command using the command registry
                            let context = self.create_command_context();
                            match crate::command_mode::CommandMode::execute_command(
                                &command_name,
                                &self.command_registry,
                                context,
                            ) {
                                Ok(command_actions) => {
                                    // Process actions through unified system
                                    let mut processed_actions =
                                        self.process_chrome_actions(command_actions);
                                    actions.append(&mut processed_actions);
                                }
                                Err(error_msg) => {
                                    actions.push(ChromeAction::Echo(format!(
                                        "Command error: {error_msg}"
                                    )));
                                }
                            }
                        }
                        EditorAction::SwitchToBuffer(target_buffer_id) => {
                            // Close the buffer switch window after selection
                            if let Some(command_window_id) = self.find_command_window() {
                                self.close_command_window(command_window_id);
                                actions.push(ChromeAction::MarkDirty(DirtyRegion::FullScreen));
                            }

                            // Determine which window to switch the buffer in
                            let window_to_switch =
                                if let Some(prev_window_id) = self.previous_active_window {
                                    if self.windows.contains_key(prev_window_id) {
                                        prev_window_id
                                    } else {
                                        self.active_window
                                    }
                                } else {
                                    self.active_window
                                };

                            // Switch the determined window to the selected buffer
                            if self.buffers.contains_key(target_buffer_id) {
                                let window = &mut self
                                    .windows
                                    .get_mut(window_to_switch)
                                    .expect("Window to switch should exist");
                                window.active_buffer = target_buffer_id;
                                window.cursor = 0;

                                // Record this buffer access for buffer history
                                self.record_buffer_access(target_buffer_id);

                                let buffer = &self.buffers[target_buffer_id];
                                let buffer_name = buffer.object();
                                actions.push(ChromeAction::Echo(format!(
                                    "Switched to buffer: {buffer_name}"
                                )));
                                actions.push(ChromeAction::MarkDirty(DirtyRegion::FullScreen));
                            } else {
                                actions.push(ChromeAction::Echo(
                                    "Buffer no longer exists".to_string(),
                                ));
                            }
                        }
                        EditorAction::KillBuffer(buffer_id) => {
                            // Close the kill buffer window after selection
                            if let Some(command_window_id) = self.find_command_window() {
                                self.close_command_window(command_window_id);
                                actions.push(ChromeAction::MarkDirty(DirtyRegion::FullScreen));
                            }

                            // Implement buffer killing logic
                            if self.buffers.contains_key(buffer_id) {
                                let buffer_name = self.buffers[buffer_id].object().clone();

                                // Find all windows using this buffer and switch them to another buffer
                                let mut windows_to_switch = Vec::new();
                                for (window_id, window) in &self.windows {
                                    if window.active_buffer == buffer_id {
                                        windows_to_switch.push(window_id);
                                    }
                                }

                                // Find an alternative buffer to switch to (avoid command windows)
                                let alternative_buffer = self
                                    .buffers
                                    .iter()
                                    .find(|(bid, _)| {
                                        *bid != buffer_id && !self.is_command_buffer(*bid)
                                    })
                                    .map(|(bid, _)| bid);

                                if let Some(alt_buffer_id) = alternative_buffer {
                                    // Switch all windows using the killed buffer to the alternative
                                    for window_id in windows_to_switch {
                                        if let Some(window) = self.windows.get_mut(window_id) {
                                            window.active_buffer = alt_buffer_id;
                                            window.cursor = 0;
                                        }
                                    }
                                } else {
                                    // No alternative buffer available - create a new scratch buffer like Emacs
                                    use crate::mode::ScratchMode;
                                    let scratch_mode = Box::new(ScratchMode {});
                                    let scratch_mode_id = self.modes.insert(scratch_mode);

                                    let scratch_buffer = Buffer::new(&[scratch_mode_id]);
                                    scratch_buffer.set_object("*scratch*".to_string());
                                    scratch_buffer.load_str("; This buffer is for text that is not saved.\n; To create a file, visit it with C-x C-f and enter text in its buffer.\n\n");
                                    let scratch_buffer_id =
                                        self.buffers.insert(scratch_buffer.clone());

                                    // Create BufferHost for the scratch buffer
                                    let mode_list = vec![(
                                        scratch_mode_id,
                                        "scratch".to_string(),
                                        self.modes
                                            .remove(scratch_mode_id)
                                            .expect("Scratch mode should exist"),
                                    )];
                                    let (buffer_client, _buffer_handle) =
                                        buffer_host::create_buffer_host(
                                            scratch_buffer,
                                            mode_list,
                                            scratch_buffer_id,
                                        );
                                    self.buffer_hosts.insert(scratch_buffer_id, buffer_client);

                                    // Switch all windows using the killed buffer to the new scratch buffer
                                    for window_id in windows_to_switch {
                                        if let Some(window) = self.windows.get_mut(window_id) {
                                            window.active_buffer = scratch_buffer_id;
                                            window.cursor = 0;
                                        }
                                    }
                                }

                                // Remove the buffer host
                                self.buffer_hosts.remove(&buffer_id);

                                // Remove the buffer itself
                                self.buffers.remove(buffer_id);

                                actions.push(ChromeAction::Echo(format!(
                                    "Killed buffer: {buffer_name}"
                                )));
                                actions.push(ChromeAction::MarkDirty(DirtyRegion::FullScreen));
                            } else {
                                actions.push(ChromeAction::Echo(
                                    "Buffer no longer exists".to_string(),
                                ));
                            }
                        }
                        EditorAction::OpenFile(file_path) => {
                            // Close the file selector window after selection
                            if let Some(command_window_id) = self.find_command_window() {
                                self.close_command_window(command_window_id);
                                actions.push(ChromeAction::MarkDirty(DirtyRegion::FullScreen));
                            }

                            // Determine which window to open the file in
                            let window_to_open =
                                if let Some(prev_window_id) = self.previous_active_window {
                                    if self.windows.contains_key(prev_window_id) {
                                        prev_window_id
                                    } else {
                                        self.active_window
                                    }
                                } else {
                                    self.active_window
                                };

                            // Open the file in the determined window
                            match self.open_file_in_window(file_path, window_to_open).await {
                                Ok(message) => {
                                    actions.push(ChromeAction::Echo(message));
                                    actions.push(ChromeAction::MarkDirty(DirtyRegion::FullScreen));
                                }
                                Err(error) => {
                                    actions.push(ChromeAction::Echo(format!(
                                        "Error opening file: {error}"
                                    )));
                                }
                            }
                        }
                        EditorAction::KillLine => {
                            // Delegate to kill_line method which handles kill-ring
                            let kill_actions = self.kill_line();
                            actions.extend(kill_actions);
                        }
                        EditorAction::KillRegion => {
                            // Delegate to kill_region method which handles kill-ring
                            let kill_actions = self.kill_region();
                            actions.extend(kill_actions);
                        }
                        EditorAction::CopyRegion => {
                            // Delegate to copy_region method which handles kill-ring
                            let copy_actions = self.copy_region();
                            actions.extend(copy_actions);
                        }
                        EditorAction::Yank { position } => {
                            // Delegate to yank method
                            let yank_actions = self.yank(&position);
                            actions.extend(yank_actions);
                        }
                        EditorAction::YankIndex { position, index } => {
                            // Delegate to yank_index method
                            let yank_actions = self.yank_index(&position, index);
                            actions.extend(yank_actions);
                        }
                    }
                }

                actions
            }
            BufferResponse::Saved(file_path) => {
                vec![ChromeAction::Echo(format!("Saved: {file_path}"))]
            }
            BufferResponse::Loaded(file_path) => {
                vec![ChromeAction::Echo(format!("Loaded: {file_path}"))]
            }
            BufferResponse::Error(error) => {
                vec![ChromeAction::Echo(format!("Error: {error}"))]
            }
            BufferResponse::NoChange => {
                vec![]
            }
        }
    }

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
                let length = text.len();
                let has_newline = text.contains('\n');
                buffer.insert_pos(text, window.cursor);

                // Advance the cursor
                window.cursor += length;

                let new_cursor = buffer.to_column_line(window.cursor);
                let window_cursor = window.absolute_cursor_position(new_cursor.0, new_cursor.1);

                // Mark dirty regions based on what was inserted
                let cursor_line = buffer.to_column_line(window.cursor).1 as usize;
                let dirty_action = if has_newline {
                    // Newlines affect multiple lines, mark entire buffer dirty
                    ChromeAction::MarkDirty(DirtyRegion::Buffer {
                        buffer_id: window.active_buffer,
                    })
                } else {
                    // Simple text insertion, only current line affected
                    ChromeAction::MarkDirty(DirtyRegion::Line {
                        buffer_id: window.active_buffer,
                        line: cursor_line,
                    })
                };

                vec![
                    ChromeAction::Echo("Inserted text".to_string()),
                    dirty_action,
                    ChromeAction::CursorMove(window_cursor),
                ]
            }
            ActionPosition::Absolute(l, c) => {
                buffer.insert_col_line(text.clone(), (*l, *c));

                let new_cursor = buffer.to_column_line(window.cursor);
                let window_cursor = window.absolute_cursor_position(new_cursor.0, new_cursor.1);

                let dirty_action = if text.contains('\n') {
                    // Newlines affect multiple lines, mark entire buffer dirty
                    ChromeAction::MarkDirty(DirtyRegion::Buffer {
                        buffer_id: window.active_buffer,
                    })
                } else {
                    // Simple text insertion, only current line affected
                    ChromeAction::MarkDirty(DirtyRegion::Line {
                        buffer_id: window.active_buffer,
                        line: *l as usize,
                    })
                };

                vec![
                    ChromeAction::Echo("Inserted text".to_string()),
                    dirty_action,
                    ChromeAction::CursorMove(window_cursor),
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
                let Some(deleted) = buffer.delete_pos(window.cursor, count) else {
                    return vec![];
                };
                if deleted.is_empty() {
                    return vec![];
                }
                // If the count was negative, then we need to adjust the cursor back by the size
                // of the deleted fragment.
                if count < 0 {
                    let length = deleted.len();
                    window.cursor -= length;
                }
                let new_cursor = buffer.to_column_line(window.cursor);
                let window_cursor = window.absolute_cursor_position(new_cursor.0, new_cursor.1);
                let cursor_line = new_cursor.1 as usize;

                // If we deleted a newline, mark entire buffer dirty to handle line merging
                let dirty_action = if deleted.contains('\n') {
                    ChromeAction::MarkDirty(DirtyRegion::Buffer {
                        buffer_id: window.active_buffer,
                    })
                } else {
                    ChromeAction::MarkDirty(DirtyRegion::Line {
                        buffer_id: window.active_buffer,
                        line: cursor_line,
                    })
                };

                vec![
                    ChromeAction::Echo("Deleted text".to_string()),
                    dirty_action,
                    ChromeAction::CursorMove(window_cursor),
                ]
            }
            ActionPosition::Absolute(l, c) => {
                let Some(deleted) = buffer.delete_col_line((*l, *c), count) else {
                    return vec![];
                };
                if deleted.is_empty() {
                    return vec![];
                }
                let new_cursor = buffer.to_column_line(window.cursor);
                let window_cursor = window.absolute_cursor_position(new_cursor.0, new_cursor.1);

                // If we deleted a newline, mark entire buffer dirty to handle line merging
                let dirty_action = if deleted.contains('\n') {
                    ChromeAction::MarkDirty(DirtyRegion::Buffer {
                        buffer_id: window.active_buffer,
                    })
                } else {
                    ChromeAction::MarkDirty(DirtyRegion::Line {
                        buffer_id: window.active_buffer,
                        line: *l as usize,
                    })
                };

                vec![
                    ChromeAction::Echo("Deleted text".to_string()),
                    dirty_action,
                    ChromeAction::CursorMove(window_cursor),
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
                    let length = deleted.len();
                    window.cursor -= length;
                } else {
                    self.kill_ring.kill(deleted.clone());
                }

                let new_cursor = buffer.to_column_line(window.cursor);
                let window_cursor = window.absolute_cursor_position(new_cursor.0, new_cursor.1);
                vec![
                    ChromeAction::Echo(format!("Killed: {deleted}")),
                    ChromeAction::MarkDirty(DirtyRegion::Buffer {
                        buffer_id: window.active_buffer,
                    }),
                    ChromeAction::CursorMove(window_cursor),
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

                let new_cursor = buffer.to_column_line(window.cursor);
                let window_cursor = window.absolute_cursor_position(new_cursor.0, new_cursor.1);
                vec![
                    ChromeAction::Echo(format!("Killed: {deleted}")),
                    ChromeAction::MarkDirty(DirtyRegion::Buffer {
                        buffer_id: window.active_buffer,
                    }),
                    ChromeAction::CursorMove(window_cursor),
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
                let new_cursor = buffer.to_column_line(window.cursor);
                let window_cursor = window.absolute_cursor_position(new_cursor.0, new_cursor.1);
                vec![
                    ChromeAction::Echo(format!("Killed line: {}", killed.replace('\n', "\\n"))),
                    ChromeAction::MarkDirty(DirtyRegion::Buffer {
                        buffer_id: window.active_buffer,
                    }),
                    ChromeAction::CursorMove(window_cursor),
                ]
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
        let new_cursor = buffer.to_column_line(window.cursor);
        let window_cursor = window.absolute_cursor_position(new_cursor.0, new_cursor.1);

        vec![
            ChromeAction::Echo(format!("Killed region: {}", deleted.replace('\n', "\\n"))),
            ChromeAction::MarkDirty(DirtyRegion::Buffer {
                buffer_id: window.active_buffer,
            }),
            ChromeAction::CursorMove(window_cursor),
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

    /// Save the current buffer to file
    pub fn save_buffer(&mut self) -> Vec<ChromeAction> {
        let window = &self.windows[self.active_window];
        let buffer = &self.buffers[window.active_buffer];

        // For now, we need to know the file path. In a real implementation,
        // this would be stored with the buffer or mode. For now, let's look
        // for a FileMode that has the file path.
        let file_path = if let Some(mode_id) = buffer.modes().first() {
            if let Some(_mode) = self.modes.get(*mode_id) {
                // Try to downcast to FileMode to get the file path
                // For now, we'll use the buffer's object name as the file path
                buffer.object()
            } else {
                return vec![ChromeAction::Echo("No mode found for save".to_string())];
            }
        } else {
            return vec![ChromeAction::Echo("No mode found for save".to_string())];
        };

        // Start async save operation without blocking
        let content = buffer.with_read(|b| b.buffer.to_string());
        let file_path_clone = file_path.clone();

        tokio::spawn(async move {
            match tokio::fs::write(&file_path_clone, content.as_bytes()).await {
                Ok(()) => {
                    // TODO: Send success message back to editor
                    eprintln!("Saved {file_path_clone}");
                }
                Err(err) => {
                    // TODO: Send error message back to editor
                    eprintln!("Error saving {file_path_clone}: {err}");
                }
            }
        });

        vec![ChromeAction::Echo(format!("Saving {file_path}..."))]
    }

    /// Ensure the cursor is visible in the window, scrolling if necessary.
    /// Returns true if scrolling occurred (requiring a redraw).
    fn ensure_cursor_visible_static(
        window: &mut Window,
        cursor_line: u16,
        content_height: u16,
    ) -> bool {
        let old_start_line = window.start_line;

        // Check if cursor is below the visible area
        if cursor_line >= window.start_line + content_height {
            // Cursor is below visible area - scroll down
            window.start_line = cursor_line.saturating_sub(content_height.saturating_sub(1));
        }
        // Check if cursor is above the visible area
        else if cursor_line < window.start_line {
            // Cursor is above visible area - scroll up
            window.start_line = cursor_line;
        }

        // Return true if we scrolled (start_line changed)
        old_start_line != window.start_line
    }

    /// Yank (paste) from kill-ring
    pub fn yank(&mut self, position: &ActionPosition) -> Vec<ChromeAction> {
        let text = match self.kill_ring.yank() {
            Some(text) => text.to_string(),
            None => return vec![ChromeAction::Echo("Kill ring is empty".to_string())],
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

    /// Open a file in the specified window
    async fn open_file_in_window(
        &mut self,
        file_path: std::path::PathBuf,
        window_id: WindowId,
    ) -> Result<String, String> {
        use crate::mode::FileMode;

        // Try to load the file
        let buffer = match Buffer::from_file(&file_path.to_string_lossy(), &[]).await {
            Ok(buffer) => buffer,
            Err(_) => {
                // File doesn't exist, create empty buffer
                let buffer = Buffer::new(&[]);
                buffer.set_object(file_path.to_string_lossy().to_string());
                buffer
            }
        };

        let buffer_id = self.buffers.insert(buffer.clone());

        // Create FileMode for this file
        let file_mode = Box::new(FileMode {
            file_path: file_path.to_string_lossy().to_string(),
        });
        let file_mode_id = self.modes.insert(file_mode);

        // Create BufferHost with FileMode for this buffer
        let file_mode = self
            .modes
            .remove(file_mode_id)
            .expect("File mode should exist in SlotMap");
        let mode_list = vec![(file_mode_id, "file".to_string(), file_mode)];

        // Create BufferHost and client
        let (buffer_client, _buffer_handle) =
            crate::buffer_host::create_buffer_host(buffer, mode_list, buffer_id);
        self.buffer_hosts.insert(buffer_id, buffer_client);

        // Switch the window to the new buffer
        if let Some(window) = self.windows.get_mut(window_id) {
            window.active_buffer = buffer_id;
            window.cursor = 0; // Reset cursor to start of buffer

            Ok(format!("Opened: {}", file_path.display()))
        } else {
            Err("Window no longer exists".to_string())
        }
    }

    /// Create a CommandContext from the current editor state
    /// Process ChromeActions and handle those that need editor state changes
    fn process_chrome_actions(&mut self, actions: Vec<ChromeAction>) -> Vec<ChromeAction> {
        let mut result_actions = Vec::new();

        for action in actions {
            match action {
                ChromeAction::CommandMode => {
                    // If command window is already open, close it first
                    if let Some(existing_command_window_id) = self.find_command_window() {
                        self.close_command_window(existing_command_window_id);
                    }

                    // Create command window at bottom with enough height for completions
                    let window_height = 10; // Total window height
                    let _command_window_id = self.create_command_window(
                        CommandType::Execute,
                        CommandWindowPosition::Bottom,
                        window_height,
                    );

                    result_actions.push(ChromeAction::Echo("Command selection".to_string()));
                    result_actions.push(ChromeAction::MarkDirty(DirtyRegion::FullScreen));
                }
                ChromeAction::SwitchBuffer => {
                    // If buffer switch window is already open, close it first
                    if let Some(existing_command_window_id) = self.find_command_window() {
                        self.close_command_window(existing_command_window_id);
                    }

                    // Create buffer switch window at bottom with enough height for buffer list
                    let window_height = 10; // Dynamic sizing based on available space
                    let _buffer_switch_window_id = self.create_command_window(
                        CommandType::BufferSwitch,
                        CommandWindowPosition::Bottom,
                        window_height,
                    );

                    result_actions.push(ChromeAction::Echo("Buffer selection".to_string()));
                    result_actions.push(ChromeAction::MarkDirty(DirtyRegion::FullScreen));
                }
                ChromeAction::KillBuffer => {
                    // If kill buffer window is already open, close it first
                    if let Some(existing_command_window_id) = self.find_command_window() {
                        self.close_command_window(existing_command_window_id);
                    }

                    // Create kill buffer window at bottom with enough height for buffer list
                    let window_height = 10; // Dynamic sizing based on available space
                    let _kill_buffer_window_id = self.create_command_window(
                        CommandType::KillBuffer,
                        CommandWindowPosition::Bottom,
                        window_height,
                    );

                    result_actions.push(ChromeAction::Echo("Kill buffer selection".to_string()));
                    result_actions.push(ChromeAction::MarkDirty(DirtyRegion::FullScreen));
                }
                ChromeAction::FindFile => {
                    // If file selector window is already open, close it first
                    if let Some(existing_command_window_id) = self.find_command_window() {
                        self.close_command_window(existing_command_window_id);
                    }

                    // Create file selector window at bottom with enough height for file list
                    let window_height = 10; // Dynamic sizing based on available space
                    let _file_selector_window_id = self.create_command_window(
                        CommandType::FindFile,
                        CommandWindowPosition::Bottom,
                        window_height,
                    );

                    result_actions.push(ChromeAction::Echo(
                        "Find file: opening file selector".to_string(),
                    ));
                    result_actions.push(ChromeAction::MarkDirty(DirtyRegion::FullScreen));
                }
                ChromeAction::Save => {
                    // Dispatch save action to the active buffer host
                    let buffer_id = self.windows[self.active_window].active_buffer;
                    let cursor_pos = self.windows[self.active_window].cursor;

                    if let Some(buffer_host) = self.buffer_hosts.get(&buffer_id).cloned() {
                        // Use async runtime to handle the async BufferHost call
                        let response_result = tokio::task::block_in_place(|| {
                            tokio::runtime::Handle::current().block_on(async {
                                buffer_host.handle_key(KeyAction::Save, cursor_pos).await
                            })
                        });

                        match response_result {
                            Ok(response) => {
                                let save_actions = tokio::task::block_in_place(|| {
                                    tokio::runtime::Handle::current().block_on(async {
                                        self.handle_buffer_response(response).await
                                    })
                                });
                                result_actions.extend(save_actions);
                            }
                            Err(e) => {
                                result_actions.push(ChromeAction::Echo(format!("Save error: {e}")));
                            }
                        }
                    } else {
                        result_actions.push(ChromeAction::Echo("No buffer to save".to_string()));
                    }
                }
                // All other actions pass through unchanged
                other => result_actions.push(other),
            }
        }

        result_actions
    }

    fn create_command_context(&self) -> crate::command_registry::CommandContext {
        let window = &self.windows[self.active_window];
        let buffer = &self.buffers[window.active_buffer];
        let (current_column, current_line) = buffer.to_column_line(window.cursor);

        crate::command_registry::CommandContext {
            buffer_content: buffer.content(),
            cursor_pos: window.cursor,
            buffer_id: window.active_buffer,
            window_id: self.active_window,
            buffer_name: buffer.object(),
            buffer_modified: false, // TODO: Implement buffer modification tracking
            current_line: current_line + 1, // Convert to 1-based
            current_column: current_column + 1, // Convert to 1-based
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;
    use crate::keys::{DefaultBindings, KeyState, LogicalKey};
    use crate::mode::ScratchMode;
    use slotmap::SlotMap;

    fn test_editor() -> Editor {
        let scratch_mode = Box::new(ScratchMode {});
        let mut modes: SlotMap<ModeId, Box<dyn Mode>> = SlotMap::default();
        let scratch_mode_id = modes.insert(scratch_mode);

        let mut buffers: SlotMap<BufferId, Buffer> = SlotMap::default();
        let scratch_buffer = Buffer::new(&[scratch_mode_id]);
        scratch_buffer.set_object("test".to_string());
        scratch_buffer.load_str("Hello\nWorld\nTest");
        let scratch_buffer_id = buffers.insert(scratch_buffer);

        let window = Window {
            x: 0,
            y: 0,
            width_chars: 80,
            height_chars: 22,
            active_buffer: scratch_buffer_id,
            start_line: 0,
            cursor: 0,
            window_type: WindowType::Normal,
        };
        let mut windows: SlotMap<WindowId, Window> = SlotMap::default();
        let window_id = windows.insert(window);

        Editor {
            frame: Frame::new(80, 24),
            buffers,
            buffer_hosts: HashMap::new(),
            windows,
            modes,
            active_window: window_id,
            previous_active_window: None,
            key_state: KeyState::new(),
            bindings: Box::new(DefaultBindings {}),
            window_tree: WindowNode::new_leaf(window_id),
            kill_ring: KillRing::new(),
            command_registry: Default::default(),
            buffer_history: vec![],
            echo_message: "".to_string(),
            echo_message_time: None,
            current_key_chord: vec![],
            mouse_drag_state: None,
            messages_buffer_id: None,
        }
    }

    #[test]
    fn test_cursor_move_right() {
        let mut editor = test_editor();
        let window = &editor.windows[editor.active_window];
        let initial_cursor = window.cursor;

        // Move cursor right
        let actions = editor.key_event(vec![LogicalKey::Right]).unwrap();

        // Should get a CursorMove action
        assert!(actions
            .iter()
            .any(|action| matches!(action, ChromeAction::CursorMove(_))));

        // Cursor should have moved
        let window = &editor.windows[editor.active_window];
        assert_eq!(window.cursor, initial_cursor + 1);
    }

    #[test]
    fn test_cursor_move_down() {
        let mut editor = test_editor();

        // Move cursor down
        let actions = editor.key_event(vec![LogicalKey::Down]).unwrap();

        // Should get a CursorMove action
        assert!(actions
            .iter()
            .any(|action| matches!(action, ChromeAction::CursorMove(_))));

        // Cursor should have moved to next line
        let window = &editor.windows[editor.active_window];
        let buffer = &editor.buffers[window.active_buffer];
        let (_, line) = buffer.to_column_line(window.cursor);
        assert_eq!(line, 1);
    }

    #[test]
    fn test_cursor_move_beyond_buffer() {
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
        let _actions = editor.key_event(vec![LogicalKey::Right]).unwrap();

        // Cursor should stay at end
        let window = &editor.windows[editor.active_window];
        assert_eq!(window.cursor, buffer_len);
    }

    #[test]
    fn test_cursor_position_calculation() {
        let mut editor = test_editor();

        // Move to a specific position
        let window = &mut editor.windows[editor.active_window];
        window.cursor = 7; // Should be at "World" line, column 1

        let actions = editor.key_event(vec![LogicalKey::Right]).unwrap();

        // Check the CursorMove action has correct coordinates
        if let Some(ChromeAction::CursorMove((x, y))) = actions
            .iter()
            .find(|a| matches!(a, ChromeAction::CursorMove(_)))
        {
            // Should be at column 2 of line 1, plus border offset (+1, +1)
            assert_eq!(*x, 3);
            assert_eq!(*y, 2);
        } else {
            panic!("Expected CursorMove action");
        }
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
        assert!(actions
            .iter()
            .any(|a| matches!(a, ChromeAction::MarkDirty(_))));

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
        let actions = editor.yank(&crate::mode::ActionPosition::cursor());

        // Should have inserted text
        assert!(actions
            .iter()
            .any(|a| matches!(a, ChromeAction::MarkDirty(_))));

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
        editor.yank_index(&crate::mode::ActionPosition::cursor(), 1);

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
        editor.insert_text("test".to_string(), &crate::mode::ActionPosition::cursor());

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
        let actions = editor.yank(&crate::mode::ActionPosition::cursor());

        // Should get an error message
        assert!(actions
            .iter()
            .any(|a| matches!(a, ChromeAction::Echo(msg) if msg.contains("empty"))));
    }

    #[test]
    fn test_set_mark() {
        let mut editor = test_editor();
        let window = &mut editor.windows[editor.active_window];
        window.cursor = 5; // End of "Hello"

        // Set mark at cursor position
        let actions = editor.set_mark();

        // Should get confirmation message
        assert!(actions
            .iter()
            .any(|a| matches!(a, ChromeAction::Echo(msg) if msg.contains("Mark set"))));

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
        assert!(actions
            .iter()
            .any(|a| matches!(a, ChromeAction::Echo(msg) if msg.contains("Mark cleared"))));

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
        assert!(actions
            .iter()
            .any(|a| matches!(a, ChromeAction::Echo(msg) if msg.contains("No mark to clear"))));
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
        assert!(actions
            .iter()
            .any(|a| matches!(a, ChromeAction::MarkDirty(_))));

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
        assert!(actions
            .iter()
            .any(|a| matches!(a, ChromeAction::Echo(msg) if msg.contains("No mark set"))));

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
        assert!(actions
            .iter()
            .any(|a| matches!(a, ChromeAction::Echo(msg) if msg.contains("Empty region"))));

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
        let actions = editor.yank(&crate::mode::ActionPosition::cursor());

        // Should have refresh action
        assert!(actions
            .iter()
            .any(|a| matches!(a, ChromeAction::MarkDirty(_))));

        // Check buffer content - should have yanked text at end
        let window = &editor.windows[editor.active_window];
        let buffer = &editor.buffers[window.active_buffer];
        assert_eq!(buffer.content(), "Herld\nTestllo\nWo");
    }
}
