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

use crate::keys::KeyAction;
use crate::mode::{Mode, ModeResult, ModeAction, ActionPosition};
use crate::{BufferId};

/// Purpose of the buffer selection mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferSwitchPurpose {
    /// Buffer switching (C-x b)
    Switch,
    /// Buffer killing (C-x k)
    Kill,
}

/// Interactive buffer switching mode 
/// This mode manages a command window buffer that displays available buffers
pub struct BufferSwitchMode {
    /// Current user input (what they've typed to filter buffers)
    pub input: String,
    /// Buffer names that match the current input (filtered)
    pub matches: Vec<String>,
    /// Buffer IDs corresponding to the matches
    pub buffer_ids: Vec<BufferId>,
    /// Index of currently selected buffer in matches
    pub selected_index: usize,
    /// Maximum number of buffers to show at once
    pub max_visible_buffers: usize,
    /// Starting index for the visible window of buffers
    pub buffer_scroll_offset: usize,
    /// Buffer ID this mode is managing
    pub buffer_id: Option<BufferId>,
    /// All available buffer names (unfiltered)
    all_buffer_names: Vec<String>,
    /// All available buffer IDs (unfiltered)
    all_buffer_ids: Vec<BufferId>,
    /// Purpose of this mode instance (switch vs kill)
    purpose: BufferSwitchPurpose,
}

impl BufferSwitchMode {
    /// Create a new BufferSwitchMode with initial state
    pub fn new() -> Self {
        Self {
            input: String::new(),
            matches: Vec::new(),
            buffer_ids: Vec::new(),
            selected_index: 0,
            max_visible_buffers: 8, // Show 8 buffers at once
            buffer_scroll_offset: 0,
            buffer_id: None,
            all_buffer_names: Vec::new(),
            all_buffer_ids: Vec::new(),
            purpose: BufferSwitchPurpose::Switch, // Default to switch
        }
    }
    
    /// Create a new BufferSwitchMode with specific purpose
    pub fn new_with_purpose(purpose: BufferSwitchPurpose) -> Self {
        Self {
            input: String::new(),
            matches: Vec::new(),
            buffer_ids: Vec::new(),
            selected_index: 0,
            max_visible_buffers: 8, // Show 8 buffers at once
            buffer_scroll_offset: 0,
            buffer_id: None,
            all_buffer_names: Vec::new(),
            all_buffer_ids: Vec::new(),
            purpose,
        }
    }
    
    /// Initialize with buffer and buffer list
    pub fn init_with_buffer(&mut self, buffer_id: BufferId, buffer_list: Vec<(BufferId, String)>) {
        // Reset all state to ensure clean initialization
        self.input.clear();
        self.buffer_id = Some(buffer_id);
        
        // Split buffer list into separate vectors
        self.all_buffer_ids = buffer_list.iter().map(|(id, _)| *id).collect();
        self.all_buffer_names = buffer_list.iter().map(|(_, name)| name.clone()).collect();
        
        // Start with all buffers visible
        self.matches = self.all_buffer_names.clone();
        self.buffer_ids = self.all_buffer_ids.clone();
        self.selected_index = 0;
        self.buffer_scroll_offset = 0;
        self.update_scroll_to_center();
    }
    
    /// Initialize with buffer list and pre-select a specific buffer
    pub fn init_with_buffer_and_preselect(&mut self, buffer_id: BufferId, buffer_list: Vec<(BufferId, String)>, current_buffer_id: BufferId) {
        // First do normal initialization
        self.init_with_buffer(buffer_id, buffer_list);
        
        // Then find and select the current buffer (for kill mode)
        if let Some(index) = self.all_buffer_ids.iter().position(|&id| id == current_buffer_id) {
            self.selected_index = index;
            self.update_scroll_to_center();
        }
    }
    
    /// Update matches based on current input using stored buffer list
    fn update_matches_internal(&mut self) {
        if self.input.is_empty() {
            // Show all buffers if no input
            self.matches = self.all_buffer_names.clone();
            self.buffer_ids = self.all_buffer_ids.clone();
        } else {
            // Filter by prefix
            let mut filtered_names = Vec::new();
            let mut filtered_ids = Vec::new();
            
            for (i, name) in self.all_buffer_names.iter().enumerate() {
                if name.to_lowercase().contains(&self.input.to_lowercase()) {
                    filtered_names.push(name.clone());
                    filtered_ids.push(self.all_buffer_ids[i]);
                }
            }
            
            self.matches = filtered_names;
            self.buffer_ids = filtered_ids;
        }
        
        // Reset selection to first match
        self.selected_index = 0;
        self.buffer_scroll_offset = 0;
        
        // Ensure we keep the selection centered
        self.update_scroll_to_center();
    }
    
    /// Generate buffer content string
    pub fn generate_buffer_content(&self) -> String {
        let mut content = String::new();
        
        // Show user input on first line if any
        if !self.input.is_empty() {
            content.push_str(&format!("{}\n", self.input));
        }
        
        // Buffer lines with highlighting
        let visible_buffers = self.visible_buffers();
        for (idx, buffer_name) in visible_buffers.iter().enumerate() {
            let is_selected = self.visible_selection_index() == Some(idx);
            if is_selected {
                // Mark selected item with arrow or highlighting
                content.push_str(&format!("> {buffer_name}\n"));
            } else {
                content.push_str(&format!("  {buffer_name}\n"));
            }
        }
        
        content
    }
    
    /// Get the currently selected buffer ID
    pub fn get_selected_buffer(&self) -> Option<BufferId> {
        self.buffer_ids.get(self.selected_index).copied()
    }
    
    /// Update scroll offset to keep selection centered
    fn update_scroll_to_center(&mut self) {
        if self.matches.len() <= self.max_visible_buffers {
            // All matches fit, no scrolling needed
            self.buffer_scroll_offset = 0;
            return;
        }
        
        let half_window = self.max_visible_buffers / 2;
        
        // Try to center the selection
        if self.selected_index < half_window {
            // Near the beginning, show from start
            self.buffer_scroll_offset = 0;
        } else if self.selected_index >= self.matches.len() - half_window {
            // Near the end, show until end
            self.buffer_scroll_offset = self.matches.len() - self.max_visible_buffers;
        } else {
            // Center the selection
            self.buffer_scroll_offset = self.selected_index - half_window;
        }
    }
    
    /// Get the visible buffers for rendering
    pub fn visible_buffers(&self) -> &[String] {
        let start = self.buffer_scroll_offset;
        let end = (start + self.max_visible_buffers).min(self.matches.len());
        &self.matches[start..end]
    }
    
    /// Get the relative index of the selection within the visible buffers
    pub fn visible_selection_index(&self) -> Option<usize> {
        if self.matches.is_empty() {
            return None;
        }
        
        if self.selected_index >= self.buffer_scroll_offset 
            && self.selected_index < self.buffer_scroll_offset + self.max_visible_buffers {
            Some(self.selected_index - self.buffer_scroll_offset)
        } else {
            None
        }
    }
}

impl Default for BufferSwitchMode {
    fn default() -> Self {
        Self::new()
    }
}

/// Mode implementation for BufferSwitchMode - manages buffer switch window buffer
impl Mode for BufferSwitchMode {
    fn name(&self) -> &str {
        "buffer-switch"
    }
    
    fn perform(&mut self, action: &KeyAction) -> ModeResult {
        // Handle buffer switch mode specific actions
        match action {
            KeyAction::AlphaNumeric(c) => {
                self.input.push(*c);
                self.update_matches_internal();
                // Clear buffer and replace with new content
                ModeResult::Consumed(vec![
                    ModeAction::ClearText,
                    ModeAction::InsertText(ActionPosition::start(), self.generate_buffer_content())
                ])
            }
            KeyAction::Backspace => {
                if !self.input.is_empty() {
                    self.input.pop();
                    self.update_matches_internal();
                    ModeResult::Consumed(vec![
                        ModeAction::ClearText,
                        ModeAction::InsertText(ActionPosition::start(), self.generate_buffer_content())
                    ])
                } else {
                    ModeResult::Ignored
                }
            }
            KeyAction::Cursor(crate::keys::CursorDirection::Up) => {
                if !self.matches.is_empty() && self.selected_index > 0 {
                    self.selected_index -= 1;
                    self.update_scroll_to_center();
                }
                // Always consume arrow keys in buffer switch mode, even if we can't move
                ModeResult::Consumed(vec![
                    ModeAction::ClearText,
                    ModeAction::InsertText(ActionPosition::start(), self.generate_buffer_content())
                ])
            }
            KeyAction::Cursor(crate::keys::CursorDirection::Down) => {
                if !self.matches.is_empty() && self.selected_index < self.matches.len() - 1 {
                    self.selected_index += 1;
                    self.update_scroll_to_center();
                }
                // Always consume arrow keys in buffer switch mode, even if we can't move
                ModeResult::Consumed(vec![
                    ModeAction::ClearText,
                    ModeAction::InsertText(ActionPosition::start(), self.generate_buffer_content())
                ])
            }
            KeyAction::Tab => {
                // Tab to cycle through matches
                if !self.matches.is_empty() {
                    self.selected_index = (self.selected_index + 1) % self.matches.len();
                    self.update_scroll_to_center();
                    ModeResult::Consumed(vec![
                        ModeAction::ClearText,
                        ModeAction::InsertText(ActionPosition::start(), self.generate_buffer_content())
                    ])
                } else {
                    ModeResult::Ignored
                }
            }
            KeyAction::Enter => {
                // Execute the appropriate action based on purpose
                if let Some(buffer_id) = self.get_selected_buffer() {
                    let action = match self.purpose {
                        BufferSwitchPurpose::Switch => ModeAction::SwitchToBuffer(buffer_id),
                        BufferSwitchPurpose::Kill => ModeAction::KillBuffer(buffer_id),
                    };
                    ModeResult::Consumed(vec![action])
                } else {
                    ModeResult::Ignored
                }
            }
            KeyAction::Escape => {
                // Escape will be handled by the Editor level
                ModeResult::Ignored
            }
            _ => ModeResult::Ignored,
        }
    }
}