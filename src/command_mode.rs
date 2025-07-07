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

use crate::command_registry::{CommandContext, CommandRegistry};
use crate::editor::ChromeAction;
use crate::keys::KeyAction;
use crate::mode::{ActionPosition, Mode, ModeAction, ModeResult};
use crate::BufferId;

/// Interactive command completion and execution mode
/// This mode manages a command window buffer that displays completions
pub struct CommandMode {
    /// Current user input (what they've typed after "M-x ")
    pub input: String,
    /// Commands that match the current input (filtered)
    pub matches: Vec<String>, // Just command names for simplicity
    /// Index of currently selected command in matches
    pub selected_index: usize,
    /// Maximum number of completions to show at once
    pub max_visible_completions: usize,
    /// Starting index for the visible window of completions
    pub completion_scroll_offset: usize,
    /// Buffer ID this mode is managing
    pub buffer_id: Option<BufferId>,
    /// All available commands (unfiltered)
    all_commands: Vec<String>,
}

impl CommandMode {
    /// Create a new CommandMode with initial state
    pub fn new() -> Self {
        Self {
            input: String::new(),
            matches: Vec::new(),
            selected_index: 0,
            max_visible_completions: 8, // Show 8 completions at once
            completion_scroll_offset: 0,
            buffer_id: None,
            all_commands: Vec::new(),
        }
    }

    /// Initialize with buffer and command list
    pub fn init_with_buffer(&mut self, buffer_id: BufferId, commands: Vec<String>) {
        // Reset all state to ensure clean initialization
        self.input.clear();
        self.buffer_id = Some(buffer_id);
        self.all_commands = commands;
        self.matches = self.all_commands.clone(); // Start with all commands visible
        self.selected_index = 0;
        self.completion_scroll_offset = 0;
        self.update_scroll_to_center();
    }

    /// Update matches based on current input using stored command list
    fn update_matches_internal(&mut self) {
        self.matches = if self.input.is_empty() {
            // Show all commands if no input
            self.all_commands.clone()
        } else {
            // Filter by prefix
            self.all_commands
                .iter()
                .filter(|cmd| cmd.to_lowercase().starts_with(&self.input.to_lowercase()))
                .cloned()
                .collect()
        };

        // Reset selection to first match
        self.selected_index = 0;
        self.completion_scroll_offset = 0;

        // Ensure we keep the selection centered
        self.update_scroll_to_center();
    }

    /// Update matches based on current input
    pub fn update_matches(&mut self, registry: &CommandRegistry) {
        self.matches = if self.input.is_empty() {
            // Show all commands if no input
            registry
                .all_commands()
                .iter()
                .map(|cmd| cmd.name.clone())
                .collect()
        } else {
            // Filter by prefix
            registry
                .find_commands(&self.input)
                .iter()
                .map(|cmd| cmd.name.clone())
                .collect()
        };

        // Reset selection to first match
        self.selected_index = 0;
        self.completion_scroll_offset = 0;

        // Ensure we keep the selection centered
        self.update_scroll_to_center();
    }

    /// Update the buffer content with current prompt and completions
    fn update_buffer_content(&self) {
        // This method will be called from within Mode implementation
        // where we have access to the buffer through the Editor
    }

    /// Generate buffer content string
    pub fn generate_buffer_content(&self) -> String {
        let mut content = String::new();

        // Show user input on first line if any
        if !self.input.is_empty() {
            content.push_str(&format!("{}\n", self.input));
        }

        // Completion lines with highlighting
        let visible_completions = self.visible_completions();
        for (idx, completion) in visible_completions.iter().enumerate() {
            let is_selected = self.visible_selection_index() == Some(idx);
            if is_selected {
                // Mark selected item with arrow or highlighting
                content.push_str(&format!("> {completion}\n"));
            } else {
                content.push_str(&format!("  {completion}\n"));
            }
        }

        content
    }

    /// Handle a key action in command mode
    pub fn handle_key(
        &mut self,
        action: KeyAction,
        _registry: &CommandRegistry,
    ) -> CommandModeResult {
        match action {
            KeyAction::AlphaNumeric(c) => {
                // Add character to input
                self.input.push(c);
                self.update_matches_internal();
                CommandModeResult::Continue
            }
            KeyAction::Backspace => {
                // Remove last character
                if !self.input.is_empty() {
                    self.input.pop();
                    self.update_matches_internal();
                }
                CommandModeResult::Continue
            }
            KeyAction::Cursor(crate::keys::CursorDirection::Up) => {
                // Move selection up
                if !self.matches.is_empty() && self.selected_index > 0 {
                    self.selected_index -= 1;
                    self.update_scroll_to_center();
                }
                CommandModeResult::Continue
            }
            KeyAction::Cursor(crate::keys::CursorDirection::Down) => {
                // Move selection down
                if !self.matches.is_empty() && self.selected_index < self.matches.len() - 1 {
                    self.selected_index += 1;
                    self.update_scroll_to_center();
                }
                CommandModeResult::Continue
            }
            KeyAction::Tab => {
                // Tab completion - complete to longest common prefix
                self.complete_to_common_prefix();
                self.update_matches_internal();
                CommandModeResult::Continue
            }
            KeyAction::Enter => {
                // Execute selected command
                if let Some(command_name) = self.get_selected_command() {
                    CommandModeResult::Execute(command_name)
                } else {
                    CommandModeResult::Continue
                }
            }
            KeyAction::Escape => CommandModeResult::Cancel,
            _ => CommandModeResult::Continue,
        }
    }

    /// Get the currently selected command name
    pub fn get_selected_command(&self) -> Option<String> {
        self.matches.get(self.selected_index).cloned()
    }

    /// Update scroll offset to keep selection centered
    fn update_scroll_to_center(&mut self) {
        if self.matches.len() <= self.max_visible_completions {
            // All matches fit, no scrolling needed
            self.completion_scroll_offset = 0;
            return;
        }

        let half_window = self.max_visible_completions / 2;

        // Try to center the selection
        if self.selected_index < half_window {
            // Near the beginning, show from start
            self.completion_scroll_offset = 0;
        } else if self.selected_index >= self.matches.len() - half_window {
            // Near the end, show until end
            self.completion_scroll_offset = self.matches.len() - self.max_visible_completions;
        } else {
            // Center the selection
            self.completion_scroll_offset = self.selected_index - half_window;
        }
    }

    /// Get the visible completions for rendering
    pub fn visible_completions(&self) -> &[String] {
        let start = self.completion_scroll_offset;
        let end = (start + self.max_visible_completions).min(self.matches.len());
        &self.matches[start..end]
    }

    /// Get the relative index of the selection within the visible completions
    pub fn visible_selection_index(&self) -> Option<usize> {
        if self.matches.is_empty() {
            return None;
        }

        if self.selected_index >= self.completion_scroll_offset
            && self.selected_index < self.completion_scroll_offset + self.max_visible_completions
        {
            Some(self.selected_index - self.completion_scroll_offset)
        } else {
            None
        }
    }

    /// Complete input to the longest common prefix of all matches
    fn complete_to_common_prefix(&mut self) {
        if self.matches.len() <= 1 {
            return;
        }

        // Find longest common prefix
        let first = &self.matches[0];
        let mut common_len = first.len();

        for other in &self.matches[1..] {
            let mut len = 0;
            for (a, b) in first.chars().zip(other.chars()) {
                if a.to_lowercase().eq(b.to_lowercase()) {
                    len += a.len_utf8();
                } else {
                    break;
                }
            }
            common_len = common_len.min(len);
        }

        if common_len > self.input.len() {
            self.input = first[..common_len].to_string();
        }
    }

    /// Get the current prompt line (what should be displayed in the input area)
    pub fn get_prompt_line(&self) -> String {
        self.input.clone()
    }

    /// Execute the given command with context
    pub fn execute_command(
        command_name: &str,
        registry: &CommandRegistry,
        context: CommandContext,
    ) -> Result<Vec<ChromeAction>, String> {
        if let Some(command) = registry.get_command(command_name) {
            command.execute(context)
        } else {
            Err(format!("Command not found: {command_name}"))
        }
    }
}

impl Default for CommandMode {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of handling a key action in command mode
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandModeResult {
    /// Continue in command mode
    Continue,
    /// Execute the named command and exit command mode
    Execute(String),
    /// Cancel command mode
    Cancel,
}

/// Mode implementation for CommandMode - manages command window buffer
impl Mode for CommandMode {
    fn name(&self) -> &str {
        "command"
    }

    fn perform(&mut self, action: &KeyAction) -> ModeResult {
        // Handle command mode specific actions
        match action {
            KeyAction::AlphaNumeric(c) => {
                self.input.push(*c);
                self.update_matches_internal();
                // Clear buffer and replace with new content
                ModeResult::Consumed(vec![
                    ModeAction::ClearText,
                    ModeAction::InsertText(ActionPosition::start(), self.generate_buffer_content()),
                ])
            }
            KeyAction::Backspace => {
                if !self.input.is_empty() {
                    self.input.pop();
                    self.update_matches_internal();
                    ModeResult::Consumed(vec![
                        ModeAction::ClearText,
                        ModeAction::InsertText(
                            ActionPosition::start(),
                            self.generate_buffer_content(),
                        ),
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
                // Always consume arrow keys in command mode, even if we can't move
                ModeResult::Consumed(vec![
                    ModeAction::ClearText,
                    ModeAction::InsertText(ActionPosition::start(), self.generate_buffer_content()),
                ])
            }
            KeyAction::Cursor(crate::keys::CursorDirection::Down) => {
                if !self.matches.is_empty() && self.selected_index < self.matches.len() - 1 {
                    self.selected_index += 1;
                    self.update_scroll_to_center();
                }
                // Always consume arrow keys in command mode, even if we can't move
                ModeResult::Consumed(vec![
                    ModeAction::ClearText,
                    ModeAction::InsertText(ActionPosition::start(), self.generate_buffer_content()),
                ])
            }
            KeyAction::Tab => {
                self.complete_to_common_prefix();
                self.update_matches_internal();
                ModeResult::Consumed(vec![
                    ModeAction::ClearText,
                    ModeAction::InsertText(ActionPosition::start(), self.generate_buffer_content()),
                ])
            }
            KeyAction::Enter => {
                // Execute the selected command by returning a special action
                if let Some(command_name) = self.get_selected_command() {
                    // Return a special action that the Editor can recognize as command execution
                    ModeResult::Consumed(vec![ModeAction::ExecuteCommand(command_name)])
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command_registry::create_default_registry;

    #[test]
    fn test_command_mode_basic() {
        let registry = create_default_registry();
        let mut cmd_mode = CommandMode::new();

        // Initially no matches
        assert_eq!(cmd_mode.matches.len(), 0);

        // Update with empty input should show all commands
        cmd_mode.update_matches(&registry);
        assert!(!cmd_mode.matches.is_empty());

        // Type 'q' should filter to quit commands
        cmd_mode.input = "q".to_string();
        cmd_mode.update_matches(&registry);
        assert!(cmd_mode.matches.iter().any(|m| m.contains("quit")));
    }

    #[test]
    fn test_navigation() {
        let registry = create_default_registry();
        let mut cmd_mode = CommandMode::new();
        cmd_mode.update_matches(&registry);

        let initial_selection = cmd_mode.selected_index;

        // Move down
        let result = cmd_mode.handle_key(
            KeyAction::Cursor(crate::keys::CursorDirection::Down),
            &registry,
        );
        assert_eq!(result, CommandModeResult::Continue);
        assert_eq!(cmd_mode.selected_index, initial_selection + 1);

        // Move up
        let result = cmd_mode.handle_key(
            KeyAction::Cursor(crate::keys::CursorDirection::Up),
            &registry,
        );
        assert_eq!(result, CommandModeResult::Continue);
        assert_eq!(cmd_mode.selected_index, initial_selection);
    }

    #[test]
    fn test_execution() {
        let registry = create_default_registry();
        let mut cmd_mode = CommandMode::new();
        cmd_mode.input = "quit".to_string();
        cmd_mode.update_matches(&registry);

        let result = cmd_mode.handle_key(KeyAction::Enter, &registry);
        if let CommandModeResult::Execute(command_name) = result {
            assert_eq!(command_name, "quit");
        } else {
            panic!("Expected Execute result");
        }
    }
}
