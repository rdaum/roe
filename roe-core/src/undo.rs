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

//! Undo/Redo system for buffer edits
//!
//! This module provides an undo manager that tracks edit operations and allows
//! undoing/redoing them. The current implementation uses a simple linear model
//! with two stacks, but the design allows for future evolution to undo-tree.
//!
//! # Design notes for future undo-tree
//!
//! The `EditOp` type is self-contained with all info needed to apply or reverse.
//! For undo-tree, the storage would change from stacks to a tree structure:
//! - Each node contains an EditOp
//! - Nodes have parent pointer and Vec<child> pointers
//! - "Current" pointer tracks position in tree
//! - Undo moves to parent, redo moves to a child (with branch selection UI)

use std::time::{Duration, Instant};

/// How long to wait before auto-sealing a group (milliseconds)
const GROUP_TIMEOUT_MS: u64 = 500;

/// Type of edit operation for grouping purposes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditType {
    Insert,
    Delete,
}

/// A single edit operation that can be undone/redone.
/// Contains all information needed to apply or reverse the operation.
#[derive(Debug, Clone)]
pub enum EditOp {
    /// Text was inserted at position
    Insert { pos: usize, text: String },
    /// Text was deleted from position
    Delete { pos: usize, text: String },
    /// A group of operations that should be undone/redone together
    Group(Vec<EditOp>),
}

impl EditOp {
    /// Get the type of this operation for grouping purposes
    pub fn edit_type(&self) -> Option<EditType> {
        match self {
            EditOp::Insert { .. } => Some(EditType::Insert),
            EditOp::Delete { .. } => Some(EditType::Delete),
            EditOp::Group(_) => None,
        }
    }

    /// Create the reverse operation
    pub fn reverse(&self) -> EditOp {
        match self {
            EditOp::Insert { pos, text } => EditOp::Delete {
                pos: *pos,
                text: text.clone(),
            },
            EditOp::Delete { pos, text } => EditOp::Insert {
                pos: *pos,
                text: text.clone(),
            },
            EditOp::Group(ops) => {
                // Reverse order and reverse each op
                EditOp::Group(ops.iter().rev().map(|op| op.reverse()).collect())
            }
        }
    }

    /// Get the cursor position after applying this operation
    pub fn cursor_after(&self) -> usize {
        match self {
            EditOp::Insert { pos, text } => pos + text.chars().count(),
            EditOp::Delete { pos, text: _ } => *pos,
            EditOp::Group(ops) => {
                // Cursor position after the last op in the group
                ops.last().map_or(0, |op| op.cursor_after())
            }
        }
    }

    /// Get the cursor position after reversing this operation
    pub fn cursor_after_reverse(&self) -> usize {
        self.reverse().cursor_after()
    }
}

/// Manages undo/redo history for a buffer.
/// Current implementation: linear two-stack model.
/// Future: can be swapped for undo-tree without changing the interface.
pub struct UndoManager {
    /// Stack of operations that can be undone
    undo_stack: Vec<EditOp>,
    /// Stack of operations that can be redone
    redo_stack: Vec<EditOp>,
    /// Maximum history size (0 = unlimited)
    max_history: usize,
    /// Pending group of operations being accumulated
    pending_group: Option<Vec<EditOp>>,
    /// Type of edits in the current pending group (for auto-grouping)
    pending_edit_type: Option<EditType>,
    /// Timestamp of the last edit (for time-based grouping)
    last_edit_time: Option<Instant>,
    /// Whether we're in an explicit group (begin_group was called)
    explicit_group: bool,
}

impl Default for UndoManager {
    fn default() -> Self {
        Self::new()
    }
}

impl UndoManager {
    /// Create a new undo manager
    pub fn new() -> Self {
        Self {
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            max_history: 1000, // Reasonable default
            pending_group: None,
            pending_edit_type: None,
            last_edit_time: None,
            explicit_group: false,
        }
    }

    /// Create with specified history limit
    pub fn with_max_history(max_history: usize) -> Self {
        Self {
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            max_history,
            pending_group: None,
            pending_edit_type: None,
            last_edit_time: None,
            explicit_group: false,
        }
    }

    /// Check if we should start a new group based on time or edit type
    fn should_break_group(&self, new_edit_type: Option<EditType>) -> bool {
        // Never break explicit groups
        if self.explicit_group {
            return false;
        }

        // No pending group means nothing to break
        if self.pending_group.is_none() {
            return false;
        }

        // Check time gap
        if let Some(last_time) = self.last_edit_time {
            if last_time.elapsed() > Duration::from_millis(GROUP_TIMEOUT_MS) {
                return true;
            }
        }

        // Check edit type change
        if let (Some(pending_type), Some(new_type)) = (self.pending_edit_type, new_edit_type) {
            if pending_type != new_type {
                return true;
            }
        }

        false
    }

    /// Seal the current pending group (commit it to undo stack)
    pub fn seal_group(&mut self) {
        // Don't seal explicit groups - they must be ended with end_group
        if self.explicit_group {
            return;
        }

        if let Some(ops) = self.pending_group.take() {
            if !ops.is_empty() {
                if ops.len() == 1 {
                    // Single op - no need for Group wrapper
                    self.undo_stack.push(ops.into_iter().next().unwrap());
                } else {
                    self.undo_stack.push(EditOp::Group(ops));
                }
                self.trim_history();
            }
        }
        self.pending_edit_type = None;
    }

    /// Record an edit operation with auto-grouping
    pub fn record(&mut self, op: EditOp) {
        let edit_type = op.edit_type();

        // New edit clears redo stack (in linear model)
        self.redo_stack.clear();

        // Check if we need to break the current group
        if self.should_break_group(edit_type) {
            self.seal_group();
        }

        // Start a new auto-group if needed
        if self.pending_group.is_none() && !self.explicit_group {
            self.pending_group = Some(Vec::new());
            self.pending_edit_type = edit_type;
        }

        // Record the operation
        if let Some(ref mut group) = self.pending_group {
            group.push(op);
        } else {
            // Shouldn't happen, but fallback
            self.undo_stack.push(op);
            self.trim_history();
        }

        // Update timestamp
        self.last_edit_time = Some(Instant::now());
    }

    /// Record an insert operation
    pub fn record_insert(&mut self, pos: usize, text: String) {
        if !text.is_empty() {
            self.record(EditOp::Insert { pos, text });
        }
    }

    /// Record a delete operation
    pub fn record_delete(&mut self, pos: usize, text: String) {
        if !text.is_empty() {
            self.record(EditOp::Delete { pos, text });
        }
    }

    /// Start an explicit group of operations that will be undone together
    pub fn begin_group(&mut self) {
        // Seal any auto-group first
        self.seal_group();
        self.pending_group = Some(Vec::new());
        self.explicit_group = true;
    }

    /// End the current explicit group and commit it as a single undo unit
    pub fn end_group(&mut self) {
        if !self.explicit_group {
            // Just seal any auto-group
            self.seal_group();
            return;
        }

        if let Some(ops) = self.pending_group.take() {
            if !ops.is_empty() {
                self.redo_stack.clear();
                if ops.len() == 1 {
                    // Single op - no need for Group wrapper
                    self.undo_stack.push(ops.into_iter().next().unwrap());
                } else {
                    self.undo_stack.push(EditOp::Group(ops));
                }
                self.trim_history();
            }
        }
        self.explicit_group = false;
        self.pending_edit_type = None;
    }

    /// Cancel the current group without committing
    pub fn cancel_group(&mut self) {
        self.pending_group = None;
        self.explicit_group = false;
        self.pending_edit_type = None;
    }

    /// Check if we're currently in a group
    pub fn in_group(&self) -> bool {
        self.pending_group.is_some()
    }

    /// Insert an undo boundary - seals current group, next edit starts fresh
    pub fn boundary(&mut self) {
        if !self.explicit_group {
            self.seal_group();
        }
    }

    /// Pop an operation to undo. Returns the operation to reverse.
    /// Caller should apply the reverse and then call `did_undo` with the original.
    pub fn pop_undo(&mut self) -> Option<EditOp> {
        // Close any pending group first
        self.end_group();
        self.undo_stack.pop()
    }

    /// Call after successfully applying an undo to record it for redo
    pub fn did_undo(&mut self, op: EditOp) {
        self.redo_stack.push(op);
    }

    /// Pop an operation to redo. Returns the operation to apply.
    pub fn pop_redo(&mut self) -> Option<EditOp> {
        self.redo_stack.pop()
    }

    /// Call after successfully applying a redo to record it for undo
    pub fn did_redo(&mut self, op: EditOp) {
        self.undo_stack.push(op);
        self.trim_history();
    }

    /// Check if undo is available
    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty() || self.pending_group.as_ref().map_or(false, |g| !g.is_empty())
    }

    /// Check if redo is available
    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    /// Clear all history
    pub fn clear(&mut self) {
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.pending_group = None;
    }

    /// Trim history to max size
    fn trim_history(&mut self) {
        if self.max_history > 0 && self.undo_stack.len() > self.max_history {
            let excess = self.undo_stack.len() - self.max_history;
            self.undo_stack.drain(0..excess);
        }
    }

    /// Get undo stack size (for debugging/status)
    pub fn undo_count(&self) -> usize {
        self.undo_stack.len()
    }

    /// Get redo stack size (for debugging/status)
    pub fn redo_count(&self) -> usize {
        self.redo_stack.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_undo_redo() {
        let mut mgr = UndoManager::new();

        mgr.record_insert(0, "hello".to_string());
        assert!(mgr.can_undo());
        assert!(!mgr.can_redo());

        let op = mgr.pop_undo().unwrap();
        mgr.did_undo(op.clone());

        assert!(!mgr.can_undo());
        assert!(mgr.can_redo());

        let redo_op = mgr.pop_redo().unwrap();
        mgr.did_redo(redo_op);

        assert!(mgr.can_undo());
        assert!(!mgr.can_redo());
    }

    #[test]
    fn test_new_edit_clears_redo() {
        let mut mgr = UndoManager::new();

        mgr.record_insert(0, "hello".to_string());
        let op = mgr.pop_undo().unwrap();
        mgr.did_undo(op);

        assert!(mgr.can_redo());

        // New edit should clear redo
        mgr.record_insert(0, "world".to_string());
        assert!(!mgr.can_redo());
    }

    #[test]
    fn test_group_operations() {
        let mut mgr = UndoManager::new();

        mgr.begin_group();
        mgr.record_insert(0, "a".to_string());
        mgr.record_insert(1, "b".to_string());
        mgr.record_insert(2, "c".to_string());
        mgr.end_group();

        // Should be one undo unit
        assert_eq!(mgr.undo_count(), 1);

        let op = mgr.pop_undo().unwrap();
        match op {
            EditOp::Group(ops) => assert_eq!(ops.len(), 3),
            _ => panic!("Expected Group"),
        }
    }

    #[test]
    fn test_reverse_operations() {
        let insert = EditOp::Insert {
            pos: 5,
            text: "hello".to_string(),
        };
        let reversed = insert.reverse();

        match reversed {
            EditOp::Delete { pos, text } => {
                assert_eq!(pos, 5);
                assert_eq!(text, "hello");
            }
            _ => panic!("Expected Delete"),
        }
    }

    #[test]
    fn test_cursor_positions() {
        let insert = EditOp::Insert {
            pos: 0,
            text: "hello".to_string(),
        };
        assert_eq!(insert.cursor_after(), 5);

        let delete = EditOp::Delete {
            pos: 3,
            text: "abc".to_string(),
        };
        assert_eq!(delete.cursor_after(), 3);
    }
}
