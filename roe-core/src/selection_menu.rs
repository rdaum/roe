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
use crate::mode::{ActionPosition, ModeAction};

/// Trait for items that can be displayed and selected in a menu
pub trait MenuItem: Clone {
    /// Get the display text for this item
    fn display_text(&self) -> String;

    /// Check if this item matches the given filter string
    fn matches_filter(&self, filter: &str) -> bool {
        self.display_text()
            .to_lowercase()
            .contains(&filter.to_lowercase())
    }
}

/// Generic selection menu widget that can be used by multiple modes
/// Handles common functionality like filtering, scrolling, selection, and key handling
pub struct SelectionMenu<T: MenuItem> {
    /// Current user input filter
    pub input: String,
    /// All available items (unfiltered)
    all_items: Vec<T>,
    /// Items that match the current filter
    filtered_items: Vec<T>,
    /// Index of currently selected item in filtered_items
    pub selected_index: usize,
    /// Maximum number of items to show at once
    pub max_visible_items: usize,
    /// Starting index for the visible window of items
    pub scroll_offset: usize,
}

impl<T: MenuItem> SelectionMenu<T> {
    /// Create a new selection menu
    pub fn new(max_visible_items: usize) -> Self {
        Self {
            input: String::new(),
            all_items: Vec::new(),
            filtered_items: Vec::new(),
            selected_index: 0,
            max_visible_items,
            scroll_offset: 0,
        }
    }

    /// Initialize with a list of items
    pub fn init_with_items(&mut self, items: Vec<T>) {
        self.input.clear();
        self.all_items = items;
        self.update_filtered_items();
        self.selected_index = 0;
        self.scroll_offset = 0;
        self.update_scroll_to_center();
    }

    /// Add a character to the filter and update matches
    pub fn add_filter_char(&mut self, c: char) {
        self.input.push(c);
        self.update_filtered_items();
    }

    /// Remove the last character from the filter and update matches
    pub fn remove_filter_char(&mut self) -> bool {
        if !self.input.is_empty() {
            self.input.pop();
            self.update_filtered_items();
            true
        } else {
            false
        }
    }

    /// Move selection up
    pub fn move_selection_up(&mut self) -> bool {
        if !self.filtered_items.is_empty() && self.selected_index > 0 {
            self.selected_index -= 1;
            self.update_scroll_to_center();
            true
        } else {
            false
        }
    }

    /// Move selection down
    pub fn move_selection_down(&mut self) -> bool {
        if !self.filtered_items.is_empty() && self.selected_index < self.filtered_items.len() - 1 {
            self.selected_index += 1;
            self.update_scroll_to_center();
            true
        } else {
            false
        }
    }

    /// Cycle to next item (with wrapping)
    pub fn cycle_selection(&mut self) -> bool {
        if !self.filtered_items.is_empty() {
            self.selected_index = (self.selected_index + 1) % self.filtered_items.len();
            self.update_scroll_to_center();
            true
        } else {
            false
        }
    }

    /// Get the currently selected item
    pub fn get_selected_item(&self) -> Option<&T> {
        self.filtered_items.get(self.selected_index)
    }

    /// Get all filtered items
    pub fn get_filtered_items(&self) -> &[T] {
        &self.filtered_items
    }

    /// Check if there are any items
    pub fn is_empty(&self) -> bool {
        self.filtered_items.is_empty()
    }

    /// Get the number of filtered items
    pub fn len(&self) -> usize {
        self.filtered_items.len()
    }

    /// Generate buffer content with the given header and selection indicator
    pub fn generate_buffer_content(&self, header: Option<&str>) -> String {
        let mut content = String::new();

        // Add header if provided
        if let Some(header) = header {
            content.push_str(&format!("{header}\n"));
        }

        // Show user input if any
        if !self.input.is_empty() {
            content.push_str(&format!("{}\n", self.input));
        }

        // Item lines with selection highlighting
        let visible_items = self.visible_items();
        for (idx, item) in visible_items.iter().enumerate() {
            let is_selected = self.visible_selection_index() == Some(idx);
            if is_selected {
                content.push_str(&format!("> {}\n", item.display_text()));
            } else {
                content.push_str(&format!("  {}\n", item.display_text()));
            }
        }

        content
    }

    /// Handle common key actions, returning true if the action was handled
    pub fn handle_key_action(&mut self, action: &KeyAction) -> bool {
        match action {
            KeyAction::AlphaNumeric(c) => {
                self.add_filter_char(*c);
                true
            }
            KeyAction::Backspace => self.remove_filter_char(),
            KeyAction::Cursor(crate::keys::CursorDirection::Up) => {
                self.move_selection_up();
                true // Always consume arrow keys
            }
            KeyAction::Cursor(crate::keys::CursorDirection::Down) => {
                self.move_selection_down();
                true // Always consume arrow keys
            }
            KeyAction::Tab => {
                self.cycle_selection();
                true
            }
            _ => false,
        }
    }

    /// Generate the standard mode actions for updating buffer content
    pub fn generate_update_actions(&self, header: Option<&str>) -> Vec<ModeAction> {
        vec![
            ModeAction::ClearText,
            ModeAction::InsertText(
                ActionPosition::start(),
                self.generate_buffer_content(header),
            ),
        ]
    }

    /// Update filtered items based on current input
    fn update_filtered_items(&mut self) {
        if self.input.is_empty() {
            self.filtered_items = self.all_items.clone();
        } else {
            self.filtered_items = self
                .all_items
                .iter()
                .filter(|item| item.matches_filter(&self.input))
                .cloned()
                .collect();
        }

        // Reset selection to first match
        self.selected_index = 0;
        self.scroll_offset = 0;
        self.update_scroll_to_center();
    }

    /// Update scroll offset to keep selection centered
    fn update_scroll_to_center(&mut self) {
        if self.filtered_items.len() <= self.max_visible_items {
            // All items fit, no scrolling needed
            self.scroll_offset = 0;
            return;
        }

        let half_window = self.max_visible_items / 2;

        // Try to center the selection
        if self.selected_index < half_window {
            // Near the beginning, show from start
            self.scroll_offset = 0;
        } else if self.selected_index >= self.filtered_items.len() - half_window {
            // Near the end, show until end
            self.scroll_offset = self.filtered_items.len() - self.max_visible_items;
        } else {
            // Center the selection
            self.scroll_offset = self.selected_index - half_window;
        }
    }

    /// Get the visible items for rendering
    fn visible_items(&self) -> &[T] {
        let start = self.scroll_offset;
        let end = (start + self.max_visible_items).min(self.filtered_items.len());
        &self.filtered_items[start..end]
    }

    /// Get the relative index of the selection within the visible items
    fn visible_selection_index(&self) -> Option<usize> {
        if self.filtered_items.is_empty() {
            return None;
        }

        if self.selected_index >= self.scroll_offset
            && self.selected_index < self.scroll_offset + self.max_visible_items
        {
            Some(self.selected_index - self.scroll_offset)
        } else {
            None
        }
    }
}

impl<T: MenuItem> Default for SelectionMenu<T> {
    fn default() -> Self {
        Self::new(8) // Default to 8 visible items
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq)]
    struct TestItem {
        name: String,
    }

    impl MenuItem for TestItem {
        fn display_text(&self) -> String {
            self.name.clone()
        }
    }

    #[test]
    fn test_selection_menu_basic() {
        let mut menu = SelectionMenu::new(3);
        let items = vec![
            TestItem {
                name: "apple".to_string(),
            },
            TestItem {
                name: "banana".to_string(),
            },
            TestItem {
                name: "cherry".to_string(),
            },
        ];

        menu.init_with_items(items);
        assert_eq!(menu.len(), 3);
        assert_eq!(menu.get_selected_item().unwrap().name, "apple");
    }

    #[test]
    fn test_filtering() {
        let mut menu = SelectionMenu::new(3);
        let items = vec![
            TestItem {
                name: "apple".to_string(),
            },
            TestItem {
                name: "apricot".to_string(),
            },
            TestItem {
                name: "banana".to_string(),
            },
        ];

        menu.init_with_items(items);
        menu.add_filter_char('a');
        menu.add_filter_char('p');

        assert_eq!(menu.len(), 2); // apple and apricot
        assert_eq!(menu.get_selected_item().unwrap().name, "apple");
    }

    #[test]
    fn test_navigation() {
        let mut menu = SelectionMenu::new(3);
        let items = vec![
            TestItem {
                name: "first".to_string(),
            },
            TestItem {
                name: "second".to_string(),
            },
            TestItem {
                name: "third".to_string(),
            },
        ];

        menu.init_with_items(items);

        // Move down
        assert!(menu.move_selection_down());
        assert_eq!(menu.get_selected_item().unwrap().name, "second");

        // Move up
        assert!(menu.move_selection_up());
        assert_eq!(menu.get_selected_item().unwrap().name, "first");

        // Can't move up from first
        assert!(!menu.move_selection_up());
        assert_eq!(menu.get_selected_item().unwrap().name, "first");
    }
}
