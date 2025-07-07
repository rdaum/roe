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
use crate::mode::{Mode, ModeAction, ModeResult};
use crate::selection_menu::{MenuItem, SelectionMenu};
use crate::BufferId;

/// Purpose of the buffer selection mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferSwitchPurpose {
    /// Buffer switching (C-x b)
    Switch,
    /// Buffer killing (C-x k)
    Kill,
}

/// Buffer item for the selection menu
#[derive(Clone)]
pub struct BufferItem {
    pub buffer_id: BufferId,
    pub name: String,
}

impl MenuItem for BufferItem {
    fn display_text(&self) -> String {
        self.name.clone()
    }
}

/// Interactive buffer switching mode
/// This mode manages a command window buffer that displays available buffers
pub struct BufferSwitchMode {
    /// Selection menu for buffer items
    menu: SelectionMenu<BufferItem>,
    /// Buffer ID this mode is managing
    pub buffer_id: Option<BufferId>,
    /// Purpose of this mode instance (switch vs kill)
    purpose: BufferSwitchPurpose,
}

impl BufferSwitchMode {
    /// Create a new BufferSwitchMode with initial state
    pub fn new() -> Self {
        Self {
            menu: SelectionMenu::new(8), // Show 8 buffers at once
            buffer_id: None,
            purpose: BufferSwitchPurpose::Switch, // Default to switch
        }
    }

    /// Create a new BufferSwitchMode with specific purpose
    pub fn new_with_purpose(purpose: BufferSwitchPurpose) -> Self {
        Self {
            menu: SelectionMenu::new(8), // Show 8 buffers at once
            buffer_id: None,
            purpose,
        }
    }

    /// Initialize with buffer and buffer list
    pub fn init_with_buffer(&mut self, buffer_id: BufferId, buffer_list: Vec<(BufferId, String)>) {
        self.buffer_id = Some(buffer_id);

        // Convert to BufferItems
        let items: Vec<BufferItem> = buffer_list
            .into_iter()
            .map(|(id, name)| BufferItem {
                buffer_id: id,
                name,
            })
            .collect();

        self.menu.init_with_items(items);
    }

    /// Initialize with buffer list and pre-select a specific buffer
    pub fn init_with_buffer_and_preselect(
        &mut self,
        buffer_id: BufferId,
        buffer_list: Vec<(BufferId, String)>,
        current_buffer_id: BufferId,
    ) {
        self.buffer_id = Some(buffer_id);

        // Convert to BufferItems
        let items: Vec<BufferItem> = buffer_list
            .into_iter()
            .map(|(id, name)| BufferItem {
                buffer_id: id,
                name,
            })
            .collect();

        self.menu.init_with_items(items);

        // Find and select the current buffer
        if let Some(index) = self
            .menu
            .get_filtered_items()
            .iter()
            .position(|item| item.buffer_id == current_buffer_id)
        {
            self.menu.selected_index = index;
        }
    }

    /// Generate buffer content string
    pub fn generate_buffer_content(&self) -> String {
        self.menu.generate_buffer_content(None)
    }

    /// Get the currently selected buffer ID
    pub fn get_selected_buffer(&self) -> Option<BufferId> {
        self.menu.get_selected_item().map(|item| item.buffer_id)
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
        // Try to handle with the generic menu first
        if self.menu.handle_key_action(action) {
            return ModeResult::Consumed(self.menu.generate_update_actions(None));
        }

        // Handle buffer switch mode specific actions
        match action {
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
