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

//! Incremental search mode (isearch) for Emacs-style searching.
//!
//! This mode provides interactive search with:
//! - Incremental matching as you type
//! - Highlighting of all matches with current match distinct
//! - Forward (C-s) and backward (C-r) navigation
//! - Cancel to restore original cursor position

use crate::buffer::Buffer;
use crate::keys::KeyAction;
use crate::mode::{ActionPosition, Mode, ModeAction, ModeResult};
use crate::{BufferId, WindowId};

/// Direction of search
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchDirection {
    Forward,
    Backward,
}

/// Interactive incremental search mode
pub struct IsearchMode {
    /// Current search term
    search_term: String,
    /// Search direction
    direction: SearchDirection,
    /// All match positions in the target buffer (start_char, end_char)
    matches: Vec<(usize, usize)>,
    /// Index of current match (None if no matches)
    current_match_index: Option<usize>,
    /// Original cursor position (for cancel)
    original_cursor: usize,
    /// Target buffer ID (the buffer being searched)
    target_buffer_id: BufferId,
    /// Target window ID (where cursor moves)
    target_window_id: WindowId,
    /// Reference to target buffer for searching
    target_buffer: Buffer,
}

impl IsearchMode {
    /// Create a new isearch mode with optional previous search term
    pub fn new(
        direction: SearchDirection,
        target_buffer_id: BufferId,
        target_window_id: WindowId,
        original_cursor: usize,
        target_buffer: Buffer,
        previous_search: Option<String>,
    ) -> Self {
        let mut mode = Self {
            search_term: previous_search.unwrap_or_default(),
            direction,
            matches: Vec::new(),
            current_match_index: None,
            original_cursor,
            target_buffer_id,
            target_window_id,
            target_buffer,
        };
        // If we have a previous search term, find matches immediately
        if !mode.search_term.is_empty() {
            mode.find_matches();
        }
        mode
    }

    /// Get the current search term (for saving when accepted)
    pub fn search_term(&self) -> &str {
        &self.search_term
    }

    /// Get the current matches (for initial update when prepopulated)
    pub fn matches(&self) -> &[(usize, usize)] {
        &self.matches
    }

    /// Get the current match index
    pub fn current_match_index(&self) -> Option<usize> {
        self.current_match_index
    }

    /// Get the target buffer ID
    pub fn target_buffer_id(&self) -> crate::BufferId {
        self.target_buffer_id
    }

    /// Get the target window ID
    pub fn target_window_id(&self) -> crate::WindowId {
        self.target_window_id
    }

    /// Generate the command window content (the search prompt)
    pub fn generate_buffer_content(&self) -> String {
        let direction_str = match self.direction {
            SearchDirection::Forward => "I-search",
            SearchDirection::Backward => "I-search backward",
        };

        let match_info = if self.search_term.is_empty() {
            String::new()
        } else if self.matches.is_empty() {
            " [no match]".to_string()
        } else if let Some(idx) = self.current_match_index {
            format!(" [{}/{}]", idx + 1, self.matches.len())
        } else {
            format!(" [0/{}]", self.matches.len())
        };

        format!("{}: {}{}", direction_str, self.search_term, match_info)
    }

    /// Find all matches of search_term in the target buffer content
    /// Returns byte positions for spans (consistent with syntax highlighting)
    fn find_matches(&mut self) {
        self.matches.clear();
        self.current_match_index = None;

        if self.search_term.is_empty() {
            return;
        }

        let content = self.target_buffer.content();
        let needle_lower = self.search_term.to_lowercase();
        let content_lower = content.to_lowercase();

        // Find all matches using byte positions (for span highlighting)
        let mut byte_start = 0;
        while let Some(byte_pos) = content_lower[byte_start..].find(&needle_lower) {
            let abs_byte_start = byte_start + byte_pos;
            let abs_byte_end = abs_byte_start + needle_lower.len();
            self.matches.push((abs_byte_start, abs_byte_end));
            byte_start = abs_byte_start + 1;
        }

        // Find the first match at or after original cursor position (for forward)
        // or before for backward
        if !self.matches.is_empty() {
            self.current_match_index = Some(self.find_nearest_match());
        }
    }

    /// Find the nearest match to the original cursor position based on direction
    /// Note: original_cursor is in chars, matches are in bytes
    fn find_nearest_match(&self) -> usize {
        if self.matches.is_empty() {
            return 0;
        }

        // Convert original_cursor (char position) to byte position for comparison
        let content = self.target_buffer.content();
        let cursor_byte_pos = char_to_byte_pos(&content, self.original_cursor);

        match self.direction {
            SearchDirection::Forward => {
                // Find first match at or after original cursor
                for (i, (start, _)) in self.matches.iter().enumerate() {
                    if *start >= cursor_byte_pos {
                        return i;
                    }
                }
                // Wrap to beginning
                0
            }
            SearchDirection::Backward => {
                // Find last match before original cursor
                for (i, (start, _)) in self.matches.iter().enumerate().rev() {
                    if *start < cursor_byte_pos {
                        return i;
                    }
                }
                // Wrap to end
                self.matches.len().saturating_sub(1)
            }
        }
    }

    /// Move to next match
    fn next_match(&mut self) {
        if self.matches.is_empty() {
            return;
        }
        if let Some(idx) = self.current_match_index {
            self.current_match_index = Some((idx + 1) % self.matches.len());
        } else {
            self.current_match_index = Some(0);
        }
    }

    /// Move to previous match
    fn prev_match(&mut self) {
        if self.matches.is_empty() {
            return;
        }
        if let Some(idx) = self.current_match_index {
            self.current_match_index = Some(if idx == 0 {
                self.matches.len() - 1
            } else {
                idx - 1
            });
        } else {
            self.current_match_index = Some(self.matches.len().saturating_sub(1));
        }
    }

    /// Build the update action to send to the editor
    fn build_update_action(&self) -> ModeAction {
        ModeAction::UpdateIsearch {
            target_buffer_id: self.target_buffer_id,
            target_window_id: self.target_window_id,
            matches: self.matches.clone(),
            current_match: self.current_match_index,
        }
    }

    /// Build the accept action
    fn build_accept_action(&self) -> ModeAction {
        ModeAction::AcceptIsearch {
            target_buffer_id: self.target_buffer_id,
            search_term: self.search_term.clone(),
        }
    }

    /// Build the cancel action
    fn build_cancel_action(&self) -> ModeAction {
        ModeAction::CancelIsearch {
            target_buffer_id: self.target_buffer_id,
            target_window_id: self.target_window_id,
            original_cursor: self.original_cursor,
        }
    }
}

impl Mode for IsearchMode {
    fn name(&self) -> &str {
        "isearch"
    }

    fn perform(&mut self, action: &KeyAction) -> ModeResult {
        match action {
            KeyAction::AlphaNumeric(c) => {
                // Add character to search term
                self.search_term.push(*c);
                self.find_matches();

                // Update command window display and isearch state
                ModeResult::Consumed(vec![
                    ModeAction::ClearText,
                    ModeAction::InsertText(ActionPosition::start(), self.generate_buffer_content()),
                    self.build_update_action(),
                ])
            }
            KeyAction::Backspace => {
                if !self.search_term.is_empty() {
                    self.search_term.pop();
                    self.find_matches();

                    ModeResult::Consumed(vec![
                        ModeAction::ClearText,
                        ModeAction::InsertText(
                            ActionPosition::start(),
                            self.generate_buffer_content(),
                        ),
                        self.build_update_action(),
                    ])
                } else {
                    ModeResult::Ignored
                }
            }
            // C-s or Down - next match
            KeyAction::Cursor(crate::keys::CursorDirection::Down) => {
                self.direction = SearchDirection::Forward;
                self.next_match();
                ModeResult::Consumed(vec![
                    ModeAction::ClearText,
                    ModeAction::InsertText(ActionPosition::start(), self.generate_buffer_content()),
                    self.build_update_action(),
                ])
            }
            // C-r or Up - previous match
            KeyAction::Cursor(crate::keys::CursorDirection::Up) => {
                self.direction = SearchDirection::Backward;
                self.prev_match();
                ModeResult::Consumed(vec![
                    ModeAction::ClearText,
                    ModeAction::InsertText(ActionPosition::start(), self.generate_buffer_content()),
                    self.build_update_action(),
                ])
            }
            KeyAction::Enter => {
                // Accept current match position
                ModeResult::Consumed(vec![self.build_accept_action()])
            }
            KeyAction::Escape | KeyAction::Cancel => {
                // Cancel and restore original position
                ModeResult::Consumed(vec![self.build_cancel_action()])
            }
            // Handle C-s and C-r when they come as Command actions
            KeyAction::Command(cmd) if cmd == "isearch-forward" => {
                self.direction = SearchDirection::Forward;
                self.next_match();
                ModeResult::Consumed(vec![
                    ModeAction::ClearText,
                    ModeAction::InsertText(ActionPosition::start(), self.generate_buffer_content()),
                    self.build_update_action(),
                ])
            }
            KeyAction::Command(cmd) if cmd == "isearch-backward" => {
                self.direction = SearchDirection::Backward;
                self.prev_match();
                ModeResult::Consumed(vec![
                    ModeAction::ClearText,
                    ModeAction::InsertText(ActionPosition::start(), self.generate_buffer_content()),
                    self.build_update_action(),
                ])
            }
            _ => ModeResult::Ignored,
        }
    }
}

/// Convert a character position to byte position in a string
fn char_to_byte_pos(s: &str, char_pos: usize) -> usize {
    s.char_indices()
        .nth(char_pos)
        .map(|(byte_idx, _)| byte_idx)
        .unwrap_or(s.len())
}

/// Convert a byte position to character position in a string
pub fn byte_to_char_pos(s: &str, byte_pos: usize) -> usize {
    s[..byte_pos.min(s.len())].chars().count()
}
