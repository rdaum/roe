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
use std::path::PathBuf;
use std::fs;

/// Interactive file selector mode for C-x C-f (find-file)
/// This mode manages a command window buffer that displays files and directories
pub struct FileSelectorMode {
    /// Current user input (what they've typed to filter files)
    pub input: String,
    /// File/directory names that match the current input (filtered)
    pub matches: Vec<String>,
    /// Full paths corresponding to the matches
    pub paths: Vec<PathBuf>,
    /// Index of currently selected file/directory in matches
    pub selected_index: usize,
    /// Maximum number of items to show at once
    pub max_visible_items: usize,
    /// Starting index for the visible window of items
    pub item_scroll_offset: usize,
    /// Buffer ID this mode is managing
    pub buffer_id: Option<BufferId>,
    /// Current working directory we're browsing
    pub current_dir: PathBuf,
    /// All files/directories in current directory (unfiltered)
    all_items: Vec<String>,
    /// All paths in current directory (unfiltered)
    all_paths: Vec<PathBuf>,
}

impl FileSelectorMode {
    /// Create a new FileSelectorMode with initial state
    pub fn new() -> Self {
        Self {
            input: String::new(),
            matches: Vec::new(),
            paths: Vec::new(),
            selected_index: 0,
            max_visible_items: 8, // Show 8 items at once
            item_scroll_offset: 0,
            buffer_id: None,
            current_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            all_items: Vec::new(),
            all_paths: Vec::new(),
        }
    }
    
    /// Initialize with buffer and load current directory
    pub fn init_with_buffer(&mut self, buffer_id: BufferId) {
        // Reset all state to ensure clean initialization
        self.input.clear();
        self.buffer_id = Some(buffer_id);
        self.selected_index = 0;
        self.item_scroll_offset = 0;
        
        // Load current directory contents
        self.load_directory();
        self.update_matches_internal();
        self.update_scroll_to_center();
    }
    
    /// Load the contents of the current directory
    fn load_directory(&mut self) {
        self.all_items.clear();
        self.all_paths.clear();
        
        // Always add ".." to go up a directory (unless we're at root)
        if self.current_dir.parent().is_some() {
            self.all_items.push("../".to_string());
            self.all_paths.push(self.current_dir.parent().unwrap().to_path_buf());
        }
        
        // Read directory contents
        if let Ok(entries) = fs::read_dir(&self.current_dir) {
            let mut dirs = Vec::new();
            let mut files = Vec::new();
            
            for entry in entries.flatten() {
                if let Ok(metadata) = entry.metadata() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    let path = entry.path();
                    
                    if metadata.is_dir() {
                        dirs.push((format!("{name}/"), path));
                    } else {
                        files.push((name, path));
                    }
                }
            }
            
            // Sort directories and files separately
            dirs.sort_by(|a, b| a.0.cmp(&b.0));
            files.sort_by(|a, b| a.0.cmp(&b.0));
            
            // Add directories first, then files
            for (name, path) in dirs {
                self.all_items.push(name);
                self.all_paths.push(path);
            }
            for (name, path) in files {
                self.all_items.push(name);
                self.all_paths.push(path);
            }
        }
    }
    
    /// Update matches based on current input using stored file list
    fn update_matches_internal(&mut self) {
        if self.input.is_empty() {
            // Show all items if no input
            self.matches = self.all_items.clone();
            self.paths = self.all_paths.clone();
        } else {
            // Filter by prefix/contains
            let mut filtered_names = Vec::new();
            let mut filtered_paths = Vec::new();
            
            for (i, name) in self.all_items.iter().enumerate() {
                if name.to_lowercase().contains(&self.input.to_lowercase()) {
                    filtered_names.push(name.clone());
                    filtered_paths.push(self.all_paths[i].clone());
                }
            }
            
            self.matches = filtered_names;
            self.paths = filtered_paths;
        }
        
        // Reset selection to first match
        self.selected_index = 0;
        self.item_scroll_offset = 0;
        
        // Ensure we keep the selection centered
        self.update_scroll_to_center();
    }
    
    /// Generate buffer content string
    pub fn generate_buffer_content(&self) -> String {
        let mut content = String::new();
        
        // Show current directory path
        content.push_str(&format!("Directory: {}\n", self.current_dir.display()));
        
        // Show user input on next line if any
        if !self.input.is_empty() {
            content.push_str(&format!("Filter: {}\n", self.input));
        }
        
        // File/directory lines with highlighting
        let visible_items = self.visible_items();
        for (idx, item_name) in visible_items.iter().enumerate() {
            let is_selected = self.visible_selection_index() == Some(idx);
            if is_selected {
                // Mark selected item with arrow or highlighting
                content.push_str(&format!("> {item_name}\n"));
            } else {
                content.push_str(&format!("  {item_name}\n"));
            }
        }
        
        content
    }
    
    /// Get the currently selected file path
    pub fn get_selected_path(&self) -> Option<PathBuf> {
        self.paths.get(self.selected_index).cloned()
    }
    
    /// Navigate to a directory
    pub fn navigate_to_directory(&mut self, path: PathBuf) -> bool {
        if path.is_dir() {
            self.current_dir = path;
            self.input.clear(); // Clear filter when changing directories
            self.load_directory();
            self.update_matches_internal();
            self.update_scroll_to_center();
            true
        } else {
            false
        }
    }
    
    /// Update scroll offset to keep selection centered
    fn update_scroll_to_center(&mut self) {
        if self.matches.len() <= self.max_visible_items {
            // All matches fit, no scrolling needed
            self.item_scroll_offset = 0;
            return;
        }
        
        let half_window = self.max_visible_items / 2;
        
        // Try to center the selection
        if self.selected_index < half_window {
            // Near the beginning, show from start
            self.item_scroll_offset = 0;
        } else if self.selected_index >= self.matches.len() - half_window {
            // Near the end, show until end
            self.item_scroll_offset = self.matches.len() - self.max_visible_items;
        } else {
            // Center the selection
            self.item_scroll_offset = self.selected_index - half_window;
        }
    }
    
    /// Get the visible items for rendering
    pub fn visible_items(&self) -> &[String] {
        let start = self.item_scroll_offset;
        let end = (start + self.max_visible_items).min(self.matches.len());
        &self.matches[start..end]
    }
    
    /// Get the relative index of the selection within the visible items
    pub fn visible_selection_index(&self) -> Option<usize> {
        if self.matches.is_empty() {
            return None;
        }
        
        if self.selected_index >= self.item_scroll_offset 
            && self.selected_index < self.item_scroll_offset + self.max_visible_items {
            Some(self.selected_index - self.item_scroll_offset)
        } else {
            None
        }
    }
}

impl Default for FileSelectorMode {
    fn default() -> Self {
        Self::new()
    }
}

/// Mode implementation for FileSelectorMode - manages file selector window buffer
impl Mode for FileSelectorMode {
    fn name(&self) -> &str {
        "file-selector"
    }
    
    fn perform(&mut self, action: &KeyAction) -> ModeResult {
        // Handle file selector mode specific actions
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
                // Always consume arrow keys in file selector mode, even if we can't move
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
                // Always consume arrow keys in file selector mode, even if we can't move
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
                if let Some(selected_path) = self.get_selected_path() {
                    if selected_path.is_dir() {
                        // Navigate to directory
                        self.navigate_to_directory(selected_path);
                        ModeResult::Consumed(vec![
                            ModeAction::ClearText,
                            ModeAction::InsertText(ActionPosition::start(), self.generate_buffer_content())
                        ])
                    } else {
                        // Open the selected file
                        ModeResult::Consumed(vec![
                            ModeAction::OpenFile(selected_path)
                        ])
                    }
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