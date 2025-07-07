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

/// Emacs-style kill-ring implementation
///
/// The kill-ring is a circular buffer that stores killed (cut/copied) text.
/// It maintains a history of killed text that can be yanked (pasted) back.
/// Multiple consecutive kills are appended together, following Emacs conventions.

#[derive(Debug, Clone)]
pub struct KillRing {
    /// Ring buffer of killed text entries
    entries: Vec<String>,
    /// Maximum number of entries to keep
    max_size: usize,
    /// Current position in the ring (for yank cycling)
    current_index: usize,
    /// Whether the last operation was a kill (for appending consecutive kills)
    last_was_kill: bool,
}

impl Default for KillRing {
    fn default() -> Self {
        Self::new()
    }
}

impl KillRing {
    /// Create a new kill-ring with default size
    pub fn new() -> Self {
        Self::with_capacity(60) // Emacs default
    }

    /// Create a new kill-ring with specified maximum capacity
    pub fn with_capacity(max_size: usize) -> Self {
        KillRing {
            entries: Vec::new(),
            max_size: max_size.max(1), // Ensure at least 1 entry
            current_index: 0,
            last_was_kill: false,
        }
    }

    /// Add text to the kill-ring
    /// If the last operation was also a kill, append to the most recent entry
    pub fn kill(&mut self, text: String) {
        if text.is_empty() {
            return;
        }

        if self.last_was_kill && !self.entries.is_empty() {
            // Append to the most recent kill
            if let Some(last_entry) = self.entries.last_mut() {
                last_entry.push_str(&text);
            }
        } else {
            // Add as new entry
            self.entries.push(text);

            // Maintain ring size limit
            if self.entries.len() > self.max_size {
                self.entries.remove(0);
            }
        }

        // Reset current index to point to the most recent entry
        self.current_index = if self.entries.is_empty() {
            0
        } else {
            self.entries.len() - 1
        };
        self.last_was_kill = true;
    }

    /// Add text to the kill-ring, prepending to the most recent entry if last was kill
    /// This is used for backward kills (like C-Backspace)
    pub fn kill_prepend(&mut self, text: String) {
        if text.is_empty() {
            return;
        }

        if self.last_was_kill && !self.entries.is_empty() {
            // Prepend to the most recent kill
            if let Some(last_entry) = self.entries.last_mut() {
                *last_entry = text + last_entry;
            }
        } else {
            // Add as new entry
            self.entries.push(text);

            // Maintain ring size limit
            if self.entries.len() > self.max_size {
                self.entries.remove(0);
            }
        }

        // Reset current index to point to the most recent entry
        self.current_index = if self.entries.is_empty() {
            0
        } else {
            self.entries.len() - 1
        };
        self.last_was_kill = true;
    }

    /// Get the most recent kill for yanking
    pub fn yank(&mut self) -> Option<&str> {
        self.last_was_kill = false;
        if self.entries.is_empty() {
            return None;
        }

        // Reset to most recent entry
        self.current_index = self.entries.len() - 1;
        Some(&self.entries[self.current_index])
    }

    /// Get a specific entry by index (0 = most recent)
    pub fn yank_index(&mut self, index: usize) -> Option<&str> {
        self.last_was_kill = false;
        if self.entries.is_empty() || index >= self.entries.len() {
            return None;
        }

        // Index 0 is most recent, so convert to actual array index
        self.current_index = self.entries.len() - 1 - index;
        Some(&self.entries[self.current_index])
    }

    /// Cycle to the previous entry in the kill-ring (for yank-pop)
    /// This is typically bound to M-y after a yank
    pub fn yank_pop(&mut self) -> Option<&str> {
        if self.entries.is_empty() {
            return None;
        }

        // Move to previous entry (cycling)
        if self.current_index == 0 {
            self.current_index = self.entries.len() - 1;
        } else {
            self.current_index -= 1;
        }

        Some(&self.entries[self.current_index])
    }

    /// Get the current yank text without changing state
    pub fn current(&self) -> Option<&str> {
        if self.entries.is_empty() {
            None
        } else {
            Some(&self.entries[self.current_index])
        }
    }

    /// Mark that a non-kill operation occurred (breaks kill sequence)
    pub fn break_kill_sequence(&mut self) {
        self.last_was_kill = false;
    }

    /// Check if the kill-ring is empty
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Get the number of entries in the kill-ring
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Get all entries (for debugging/inspection)
    pub fn entries(&self) -> &[String] {
        &self.entries
    }

    /// Clear the kill-ring
    pub fn clear(&mut self) {
        self.entries.clear();
        self.current_index = 0;
        self.last_was_kill = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_kill_and_yank() {
        let mut ring = KillRing::new();

        ring.kill("hello".to_string());
        assert_eq!(ring.yank(), Some("hello"));
    }

    #[test]
    fn test_consecutive_kills_append() {
        let mut ring = KillRing::new();

        ring.kill("hello".to_string());
        ring.kill(" world".to_string());

        assert_eq!(ring.yank(), Some("hello world"));
        assert_eq!(ring.len(), 1); // Should be one combined entry
    }

    #[test]
    fn test_non_consecutive_kills() {
        let mut ring = KillRing::new();

        ring.kill("first".to_string());
        ring.break_kill_sequence(); // Simulate non-kill operation
        ring.kill("second".to_string());

        assert_eq!(ring.len(), 2);
        assert_eq!(ring.yank(), Some("second")); // Most recent
    }

    #[test]
    fn test_kill_prepend() {
        let mut ring = KillRing::new();

        ring.kill("world".to_string());
        ring.kill_prepend("hello ".to_string());

        assert_eq!(ring.yank(), Some("hello world"));
    }

    #[test]
    fn test_yank_index() {
        let mut ring = KillRing::new();

        ring.kill("first".to_string());
        ring.break_kill_sequence();
        ring.kill("second".to_string());
        ring.break_kill_sequence();
        ring.kill("third".to_string());

        assert_eq!(ring.yank_index(0), Some("third")); // Most recent
        assert_eq!(ring.yank_index(1), Some("second")); // Second most recent
        assert_eq!(ring.yank_index(2), Some("first")); // Oldest
        assert_eq!(ring.yank_index(3), None); // Out of bounds
    }

    #[test]
    fn test_yank_pop() {
        let mut ring = KillRing::new();

        ring.kill("first".to_string());
        ring.break_kill_sequence();
        ring.kill("second".to_string());
        ring.break_kill_sequence();
        ring.kill("third".to_string());

        // Start with most recent
        assert_eq!(ring.yank(), Some("third"));

        // Pop to previous entries
        assert_eq!(ring.yank_pop(), Some("second"));
        assert_eq!(ring.yank_pop(), Some("first"));
        assert_eq!(ring.yank_pop(), Some("third")); // Should cycle back
    }

    #[test]
    fn test_max_capacity() {
        let mut ring = KillRing::with_capacity(2);

        ring.kill("first".to_string());
        ring.break_kill_sequence();
        ring.kill("second".to_string());
        ring.break_kill_sequence();
        ring.kill("third".to_string()); // Should evict "first"

        assert_eq!(ring.len(), 2);
        assert_eq!(ring.yank_index(0), Some("third"));
        assert_eq!(ring.yank_index(1), Some("second"));
        assert_eq!(ring.yank_index(2), None); // "first" was evicted
    }

    #[test]
    fn test_empty_kill_ignored() {
        let mut ring = KillRing::new();

        ring.kill("".to_string());
        assert!(ring.is_empty());

        ring.kill("hello".to_string());
        ring.kill("".to_string()); // Should not affect existing entry
        assert_eq!(ring.yank(), Some("hello"));
    }

    #[test]
    fn test_current() {
        let mut ring = KillRing::new();

        ring.kill("test".to_string());
        assert_eq!(ring.current(), Some("test"));

        // current() should not change state
        assert_eq!(ring.current(), Some("test"));
        assert_eq!(ring.yank(), Some("test"));
    }
}
