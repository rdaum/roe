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

use crate::editor::ChromeAction;
use crate::{BufferId, WindowId};

/// Context information passed to commands when they execute
#[derive(Debug, Clone)]
pub struct CommandContext {
    /// Content of the current buffer
    pub buffer_content: String,
    /// Current cursor position in the buffer
    pub cursor_pos: usize,
    /// ID of the current buffer
    pub buffer_id: BufferId,
    /// ID of the current window
    pub window_id: WindowId,
    /// Name/path of the current buffer
    pub buffer_name: String,
    /// Whether the buffer has been modified
    pub buffer_modified: bool,
    /// Current line number (1-based for display)
    pub current_line: u16,
    /// Current column number (1-based for display)
    pub current_column: u16,
}

/// Category of command for organization and filtering
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandCategory {
    /// Global commands available everywhere
    Global,
    /// Commands provided by a specific mode
    Mode(String),
    /// Future: Commands provided by scripts
    Script(String),
}

/// Handler function type for commands
pub type CommandHandler = Box<dyn Fn(CommandContext) -> Result<Vec<ChromeAction>, String> + Send + Sync>;

/// A single command that can be executed
pub struct Command {
    /// Command name (used for M-x completion)
    pub name: String,
    /// Human-readable description
    pub description: String,
    /// Category for organization
    pub category: CommandCategory,
    /// Function to execute the command
    pub handler: CommandHandler,
}

impl Command {
    /// Create a new command
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        category: CommandCategory,
        handler: CommandHandler,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            category,
            handler,
        }
    }
    
    /// Execute this command with the given context
    pub fn execute(&self, context: CommandContext) -> Result<Vec<ChromeAction>, String> {
        (self.handler)(context)
    }
}

/// Registry of all available commands
pub struct CommandRegistry {
    commands: Vec<Command>,
}

impl CommandRegistry {
    /// Create a new empty command registry
    pub fn new() -> Self {
        Self {
            commands: Vec::new(),
        }
    }
    
    /// Register a new command
    pub fn register_command(&mut self, command: Command) {
        // Remove any existing command with the same name
        self.commands.retain(|c| c.name != command.name);
        self.commands.push(command);
    }
    
    /// Find all commands matching the given prefix
    pub fn find_commands(&self, prefix: &str) -> Vec<&Command> {
        let prefix_lower = prefix.to_lowercase();
        let mut matches: Vec<&Command> = self.commands
            .iter()
            .filter(|cmd| cmd.name.to_lowercase().starts_with(&prefix_lower))
            .collect();
        
        // Sort by name for consistent ordering
        matches.sort_by(|a, b| a.name.cmp(&b.name));
        matches
    }
    
    /// Get a specific command by exact name
    pub fn get_command(&self, name: &str) -> Option<&Command> {
        self.commands.iter().find(|cmd| cmd.name == name)
    }
    
    /// Get all commands in a specific category
    pub fn get_commands_by_category(&self, category: &CommandCategory) -> Vec<&Command> {
        self.commands
            .iter()
            .filter(|cmd| &cmd.category == category)
            .collect()
    }
    
    /// Get all registered commands
    pub fn all_commands(&self) -> &[Command] {
        &self.commands
    }
    
    /// Remove all commands from a specific category (useful for mode cleanup)
    pub fn remove_commands_by_category(&mut self, category: &CommandCategory) {
        self.commands.retain(|cmd| &cmd.category != category);
    }
}

impl Default for CommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Initialize the command registry with comprehensive global commands
pub fn create_default_registry() -> CommandRegistry {
    let mut registry = CommandRegistry::new();
    
    // File operations
    registry.register_command(Command::new(
        "find-file",
        "Open a file",
        CommandCategory::Global,
        Box::new(|_context| {
            Ok(vec![ChromeAction::FileOpen])
        }),
    ));
    
    registry.register_command(Command::new(
        "save-buffer",
        "Save current buffer to file",
        CommandCategory::Global,
        Box::new(|context| {
            Ok(vec![
                ChromeAction::Echo(format!("Saving {}...", context.buffer_name))
            ])
        }),
    ));
    
    // Editor lifecycle
    registry.register_command(Command::new(
        "quit",
        "Quit the editor",
        CommandCategory::Global,
        Box::new(|_context| {
            Ok(vec![ChromeAction::Quit])
        }),
    ));
    
    registry.register_command(Command::new(
        "exit",
        "Exit the editor (alias for quit)",
        CommandCategory::Global,
        Box::new(|_context| {
            Ok(vec![ChromeAction::Quit])
        }),
    ));
    
    // Window management
    registry.register_command(Command::new(
        "split-window-horizontally",
        "Split current window horizontally",
        CommandCategory::Global,
        Box::new(|_context| {
            Ok(vec![ChromeAction::SplitHorizontal])
        }),
    ));
    
    registry.register_command(Command::new(
        "split-window-vertically", 
        "Split current window vertically",
        CommandCategory::Global,
        Box::new(|_context| {
            Ok(vec![ChromeAction::SplitVertical])
        }),
    ));
    
    registry.register_command(Command::new(
        "delete-window",
        "Delete current window",
        CommandCategory::Global,
        Box::new(|_context| {
            Ok(vec![ChromeAction::DeleteWindow])
        }),
    ));
    
    registry.register_command(Command::new(
        "delete-other-windows",
        "Delete all windows except current",
        CommandCategory::Global,
        Box::new(|_context| {
            Ok(vec![ChromeAction::DeleteOtherWindows])
        }),
    ));
    
    registry.register_command(Command::new(
        "other-window",
        "Switch to next window",
        CommandCategory::Global,
        Box::new(|_context| {
            Ok(vec![ChromeAction::SwitchWindow])
        }),
    ));
    
    // Alternative command names (common aliases)
    registry.register_command(Command::new(
        "split-window-below",
        "Split current window horizontally (alias)",
        CommandCategory::Global,
        Box::new(|_context| {
            Ok(vec![ChromeAction::SplitHorizontal])
        }),
    ));
    
    registry.register_command(Command::new(
        "split-window-right",
        "Split current window vertically (alias)",
        CommandCategory::Global,
        Box::new(|_context| {
            Ok(vec![ChromeAction::SplitVertical])
        }),
    ));
    
    // Information commands
    registry.register_command(Command::new(
        "describe-buffer",
        "Show information about current buffer",
        CommandCategory::Global,
        Box::new(|context| {
            Ok(vec![
                ChromeAction::Echo(format!(
                    "Buffer: {} ({}:{}) {} chars", 
                    context.buffer_name,
                    context.current_line,
                    context.current_column,
                    context.buffer_content.len()
                ))
            ])
        }),
    ));
    
    registry.register_command(Command::new(
        "describe-mode",
        "Show information about current major mode",
        CommandCategory::Global,
        Box::new(|_context| {
            Ok(vec![
                ChromeAction::Echo("Current mode: file-mode".to_string())
            ])
        }),
    ));
    
    // Utility commands
    registry.register_command(Command::new(
        "keyboard-quit",
        "Cancel current operation",
        CommandCategory::Global,
        Box::new(|_context| {
            Ok(vec![
                ChromeAction::Echo("Quit".to_string())
            ])
        }),
    ));
    
    registry
}

#[cfg(test)]
mod tests {
    use super::*;
    
    fn dummy_context() -> CommandContext {
        CommandContext {
            buffer_content: "test".to_string(),
            cursor_pos: 0,
            buffer_id: slotmap::SlotMap::with_key().insert(()),
            window_id: slotmap::SlotMap::with_key().insert(()),
            buffer_name: "test.txt".to_string(),
            buffer_modified: false,
            current_line: 1,
            current_column: 1,
        }
    }
    
    #[test]
    fn test_command_registry_basic() {
        let mut registry = CommandRegistry::new();
        
        registry.register_command(Command::new(
            "test-command",
            "A test command",
            CommandCategory::Global,
            Box::new(|_| Ok(vec![])),
        ));
        
        assert_eq!(registry.all_commands().len(), 1);
        assert!(registry.get_command("test-command").is_some());
        assert!(registry.get_command("nonexistent").is_none());
    }
    
    #[test]
    fn test_prefix_matching() {
        let mut registry = CommandRegistry::new();
        
        registry.register_command(Command::new(
            "save-buffer",
            "Save buffer",
            CommandCategory::Global,
            Box::new(|_| Ok(vec![])),
        ));
        
        registry.register_command(Command::new(
            "save-all",
            "Save all",
            CommandCategory::Global,
            Box::new(|_| Ok(vec![])),
        ));
        
        registry.register_command(Command::new(
            "quit",
            "Quit",
            CommandCategory::Global,
            Box::new(|_| Ok(vec![])),
        ));
        
        let save_matches = registry.find_commands("save");
        assert_eq!(save_matches.len(), 2);
        
        let save_buffer_matches = registry.find_commands("save-b");
        assert_eq!(save_buffer_matches.len(), 1);
        assert_eq!(save_buffer_matches[0].name, "save-buffer");
        
        let q_matches = registry.find_commands("q");
        assert_eq!(q_matches.len(), 1);
        assert_eq!(q_matches[0].name, "quit");
    }
}