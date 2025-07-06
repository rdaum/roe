use crate::buffer::Buffer;
#[cfg(not(test))]
use crate::echo;
use crate::keys::KeyAction::ChordNext;
use crate::keys::{Bindings, CursorDirection, KeyAction, KeyState, LogicalKey};
use crate::kill_ring::KillRing;
use crate::mode::{ActionPosition, Mode, ModeAction};
use crate::{BufferId, ModeId, WindowId, ECHO_AREA_HEIGHT};
use slotmap::SlotMap;

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
    pub columns: u16,
    pub rows: u16,
    pub available_columns: u16,
    pub available_lines: u16,
}

impl Frame {
    pub fn new(columns: u16, rows: u16) -> Self {
        Frame {
            columns,
            rows,
            available_columns: columns,
            available_lines: rows - ECHO_AREA_HEIGHT, /* no global modeline */
        }
    }
}

pub struct Editor {
    pub frame: Frame,
    pub buffers: SlotMap<BufferId, Buffer>,
    pub windows: SlotMap<WindowId, Window>,
    pub modes: SlotMap<ModeId, Box<dyn Mode>>,
    pub active_window: WindowId,
    pub key_state: KeyState,
    pub bindings: Box<dyn Bindings>,
    /// Tree structure representing window layout
    pub window_tree: WindowNode,
    /// Global kill-ring for cut/copy/paste operations
    pub kill_ring: KillRing,
}

/// The main event loop, which receives keystrokes and dispatches them to the mode in the buffer
/// in the active window.
impl Editor {}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ChromeAction {
    FileOpen,
    CommandMode,
    CursorMove((u16, u16)),
    Huh,
    Echo(String),
    Refresh(WindowId),
    Quit,
    SplitHorizontal,
    SplitVertical,
    SwitchWindow,
    DeleteWindow,
    DeleteOtherWindows,
}

impl Editor {
    /// Calculate and update window positions and sizes based on the window tree
    pub fn calculate_window_layout(&mut self) {
        let available_width = self.frame.available_columns;
        let available_height = self.frame.available_lines;

        self.layout_node(
            &self.window_tree.clone(),
            0,
            0,
            available_width,
            available_height,
        );
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
            return Ok(vec![]);
        }

        let _ = self.key_state.take();

        // Skip echo in tests to avoid terminal issues
        #[cfg(not(test))]
        echo(&mut std::io::stdout(), self, &format!("{key_action:?}"))?;
        let window = &mut self.windows.get_mut(self.active_window).unwrap();
        let buffer = &self.buffers.get_mut(window.active_buffer).unwrap();

        // Some actions like save, quit, etc. are out of the control of the mode.
        match &key_action {
            KeyAction::CommandMode => {
                return Ok(vec![ChromeAction::CommandMode]);
            }
            KeyAction::Quit => {
                return Ok(vec![
                    ChromeAction::Echo("Quitting".to_string()),
                    ChromeAction::Quit,
                ]);
            }
            KeyAction::FindFile => {
                return Ok(vec![ChromeAction::FileOpen]);
            }
            KeyAction::SplitHorizontal => {
                return Ok(vec![ChromeAction::SplitHorizontal]);
            }
            KeyAction::SplitVertical => {
                return Ok(vec![ChromeAction::SplitVertical]);
            }
            KeyAction::SwitchWindow => {
                return Ok(vec![ChromeAction::SwitchWindow]);
            }
            KeyAction::DeleteWindow => {
                return Ok(vec![ChromeAction::DeleteWindow]);
            }
            KeyAction::DeleteOtherWindows => {
                return Ok(vec![ChromeAction::DeleteOtherWindows]);
            }

            KeyAction::Cursor(cd) => {
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
                        // TODO: implement page movement
                        window.cursor
                    }
                    CursorDirection::PageDown => {
                        // TODO: implement page movement
                        window.cursor
                    }
                };

                window.cursor = new_pos;

                // Now compute the physical position of the cursor in the window.
                let (col, line) = buffer.to_column_line(new_pos);

                return Ok(vec![ChromeAction::CursorMove(
                    window.absolute_cursor_position(col, line),
                )]);
            }
            KeyAction::Unbound => return Ok(vec![ChromeAction::Huh]),
            _ => {}
        }

        // Dispatch the key to the modes of the active-buffer in the active-window

        let mut chrome_actions = vec![];
        for mode_id in buffer.modes.clone() {
            let mode = self.modes.get_mut(mode_id).unwrap();
            let actions = mode.perform(&key_action);
            for action in actions {
                match &action {
                    ModeAction::InsertText(p, t) => {
                        chrome_actions.extend(self.insert_text(t.clone(), p));
                    }
                    ModeAction::DeleteText(p, c) => {
                        chrome_actions.extend(self.delete_text(p, *c));
                    }
                    ModeAction::KillText(p, c) => {
                        chrome_actions.extend(self.kill_text(p, *c));
                    }
                    ModeAction::KillLine => {
                        chrome_actions.extend(self.kill_line());
                    }
                    ModeAction::KillRegion => {
                        chrome_actions.extend(self.kill_region());
                    }
                    ModeAction::Yank(p) => {
                        chrome_actions.extend(self.yank(p));
                    }
                    ModeAction::YankIndex(p, idx) => {
                        chrome_actions.extend(self.yank_index(p, *idx));
                    }
                    ModeAction::CursorUp => {}
                    ModeAction::CursorDown => {}
                    ModeAction::CursorLeft => {}
                    ModeAction::CursorRight => {}
                    ModeAction::NextLine => {}
                }
                chrome_actions.push(ChromeAction::Echo(format!("{action:?}")));
            }
        }

        // Skip echo in tests to avoid terminal issues
        #[cfg(not(test))]
        crate::echo(&mut std::io::stdout(), self, &format!("{chrome_actions:?}"))?;
        Ok(chrome_actions)
    }

    /// Perform insert action, based on the position passed and taking into account the window's
    /// cursor position.
    pub fn insert_text(&mut self, text: String, position: &ActionPosition) -> Vec<ChromeAction> {
        // Break kill sequence since we're doing a non-kill operation
        self.kill_ring.break_kill_sequence();

        let window = &mut self.windows.get_mut(self.active_window).unwrap();
        let buffer = &mut self.buffers.get_mut(window.active_buffer).unwrap();
        match position {
            ActionPosition::Cursor => {
                let length = text.len();
                buffer.insert_pos(text, window.cursor);

                // Advance the cursor
                window.cursor += length;

                let new_cursor = buffer.to_column_line(window.cursor);
                let window_cursor = window.absolute_cursor_position(new_cursor.0, new_cursor.1);

                // Refresh the window
                // TODO: actually just print the portion rather than the whole thing
                vec![
                    ChromeAction::Echo("Inserted text".to_string()),
                    ChromeAction::Refresh(self.active_window),
                    ChromeAction::CursorMove(window_cursor),
                ]
            }
            ActionPosition::Absolute(l, c) => {
                buffer.insert_col_line(text, (*l, *c));

                let new_cursor = buffer.to_column_line(window.cursor);
                let window_cursor = window.absolute_cursor_position(new_cursor.0, new_cursor.1);
                vec![
                    ChromeAction::Echo("Inserted text".to_string()),
                    ChromeAction::Refresh(self.active_window),
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

        let window = &mut self.windows.get_mut(self.active_window).unwrap();
        let buffer = &mut self.buffers.get_mut(window.active_buffer).unwrap();

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
                vec![
                    ChromeAction::Echo("Deleted text".to_string()),
                    ChromeAction::Refresh(self.active_window),
                    ChromeAction::CursorMove(window_cursor),
                ]
            }
            ActionPosition::Absolute(l, c) => {
                buffer.delete_col_line((*l, *c), count);
                let new_cursor = buffer.to_column_line(window.cursor);
                let window_cursor = window.absolute_cursor_position(new_cursor.0, new_cursor.1);
                vec![
                    ChromeAction::Echo("Deleted text".to_string()),
                    ChromeAction::Refresh(self.active_window),
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
        let window = &mut self.windows.get_mut(self.active_window).unwrap();
        let buffer = &mut self.buffers.get_mut(window.active_buffer).unwrap();

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
                    ChromeAction::Refresh(self.active_window),
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
                    ChromeAction::Refresh(self.active_window),
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
        let window = &mut self.windows.get_mut(self.active_window).unwrap();
        let buffer = &mut self.buffers.get_mut(window.active_buffer).unwrap();

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
                    ChromeAction::Refresh(self.active_window),
                    ChromeAction::CursorMove(window_cursor),
                ]
            }
            _ => {
                vec![ChromeAction::Echo("Nothing to kill".to_string())]
            }
        }
    }

    /// Kill the selected region (placeholder - requires mark implementation)
    pub fn kill_region(&mut self) -> Vec<ChromeAction> {
        // TODO: Implement when mark/region selection is added
        vec![ChromeAction::Echo(
            "Kill region not yet implemented (need mark)".to_string(),
        )]
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
        let scratch_buffer = Buffer {
            object: "test".to_string(),
            modes: vec![scratch_mode_id],
            buffer: ropey::Rope::from_str("Hello\nWorld\nTest"),
        };
        let scratch_buffer_id = buffers.insert(scratch_buffer);

        let window = Window {
            x: 0,
            y: 0,
            width_chars: 80,
            height_chars: 22,
            active_buffer: scratch_buffer_id,
            start_line: 0,
            cursor: 0,
        };
        let mut windows: SlotMap<WindowId, Window> = SlotMap::default();
        let window_id = windows.insert(window);

        Editor {
            frame: Frame::new(80, 24),
            buffers,
            windows,
            modes,
            active_window: window_id,
            key_state: KeyState::new(),
            bindings: Box::new(DefaultBindings {}),
            window_tree: WindowNode::new_leaf(window_id),
            kill_ring: KillRing::new(),
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
            buffer.buffer.len_chars()
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
            .any(|a| matches!(a, ChromeAction::Refresh(_))));

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
            .any(|a| matches!(a, ChromeAction::Refresh(_))));

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
}
