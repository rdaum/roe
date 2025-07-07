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

use crate::command_registry::{Command, CommandCategory};
use crate::keys::KeyAction;

/// Mode dispatch functions all return actions in response to events.
/// Actions are things like "insert text", "delete text", "move cursor", etc.
/// Example events are things like "keystroke" or "mouse click" (with logical key, not physical)
/// A mode is described with the following hooks:
///     Init: called when the mode is established for a buffer
///     Update: called when the buffer has been updated by some external force
///     Key: called when a key is pressed
///     Mouse: called when a mouse event occurs
///     Enter: called when the window for the mode is entered
///     Exit: called when the window for the mode is exited
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ModeAction {
    /// Add a piece of text at the cursor's row/line position in the buffer, moving the cursor
    /// to the end of the inserted text (if in insert mode)
    /// Stick a piece of text somewhere else in the buffer.
    InsertText(ActionPosition, String),
    /// Delete a piece of text from the buffer
    DeleteText(ActionPosition, isize),
    /// Kill (cut) text and add it to kill-ring
    #[allow(dead_code)]
    KillText(ActionPosition, isize),
    /// Kill from cursor to end of line
    KillLine,
    /// Kill the selected region (requires mark to be set)
    KillRegion,
    /// Copy the selected region to kill-ring without deleting (requires mark to be set)
    CopyRegion,
    /// Yank (paste) from kill-ring
    Yank(ActionPosition),
    /// Yank from specific kill-ring index
    YankIndex(ActionPosition, usize),
    /// Set mark at cursor position
    SetMark,
    /// Clear the mark
    ClearMark,
    /// Save the buffer to file
    Save,
    /// Clear all text from the buffer
    ClearText,
    /// Execute a command by name
    ExecuteCommand(String),
    /// Switch to a specific buffer
    SwitchToBuffer(crate::BufferId),
    /// Kill a specific buffer
    KillBuffer(crate::BufferId),
    /// Open a file by path
    OpenFile(std::path::PathBuf),

    CursorUp,
    CursorDown,
    CursorLeft,
    CursorRight,
    NextLine,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ActionPosition {
    /// Action should occur relative to the cursor's position in the buffer
    Cursor,
    /// Action should occur at an absolute position in the buffer, row/column
    #[allow(dead_code)]
    Absolute(u16, u16),
    /// Action should happen at the very end of the buffer, appending or removing etc.
    #[allow(dead_code)]
    End,
}

impl ActionPosition {
    pub fn cursor() -> ActionPosition {
        ActionPosition::Cursor
    }

    pub fn absolute(row: u16, col: u16) -> ActionPosition {
        ActionPosition::Absolute(row, col)
    }

    pub fn end() -> ActionPosition {
        ActionPosition::End
    }

    pub fn start() -> ActionPosition {
        ActionPosition::Absolute(0, 0)
    }
}

/// Result of mode processing - controls event flow through mode chain
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ModeResult {
    /// I consumed this event completely - no other modes should see it
    Consumed(Vec<ModeAction>),
    /// I annotated this event but others can still process it
    Annotated(Vec<ModeAction>),
    /// I didn't handle this event at all
    Ignored,
}

pub trait Mode: Send + Sync {
    fn name(&self) -> &str;
    fn perform(&mut self, action: &KeyAction) -> ModeResult;

    /// Return commands that this mode provides
    /// Default implementation returns no commands
    fn available_commands(&self) -> Vec<Command> {
        vec![]
    }
}

pub struct ScratchMode {}

impl Mode for ScratchMode {
    fn name(&self) -> &str {
        "scratch"
    }

    fn perform(&mut self, action: &KeyAction) -> ModeResult {
        match action {
            KeyAction::Cursor(_) => ModeResult::Ignored,
            KeyAction::InsertModeToggle => ModeResult::Ignored,
            KeyAction::Undo => ModeResult::Ignored,
            KeyAction::Redo => ModeResult::Ignored,
            KeyAction::MarkStart => ModeResult::Consumed(vec![ModeAction::SetMark]),
            KeyAction::MarkEnd => ModeResult::Ignored,
            KeyAction::KillRegion(destructive) => {
                if *destructive {
                    // C-w - kill region (delete and copy to kill-ring)
                    ModeResult::Consumed(vec![ModeAction::KillRegion])
                } else {
                    // M-w - copy region to kill-ring without deleting
                    ModeResult::Consumed(vec![ModeAction::CopyRegion])
                }
            }
            KeyAction::KillLine(_whole_line) => ModeResult::Consumed(vec![ModeAction::KillLine]),
            KeyAction::Yank(index) => match index {
                Some(idx) => ModeResult::Consumed(vec![ModeAction::YankIndex(
                    ActionPosition::cursor(),
                    *idx,
                )]),
                None => ModeResult::Consumed(vec![ModeAction::Yank(ActionPosition::cursor())]),
            },
            KeyAction::ForceIndent => ModeResult::Ignored,
            KeyAction::Tab => ModeResult::Ignored,
            KeyAction::Delete => {
                ModeResult::Consumed(vec![ModeAction::DeleteText(ActionPosition::cursor(), 1)])
            }
            KeyAction::Backspace => {
                ModeResult::Consumed(vec![ModeAction::DeleteText(ActionPosition::cursor(), -1)])
            }
            KeyAction::Enter => ModeResult::Consumed(vec![ModeAction::InsertText(
                ActionPosition::cursor(),
                "\n".to_string(),
            )]),
            KeyAction::Escape => ModeResult::Ignored,
            KeyAction::DeleteWord => ModeResult::Ignored,
            KeyAction::ToggleCapsLock => ModeResult::Ignored,
            KeyAction::ToggleScrollLock => ModeResult::Ignored,
            KeyAction::BackspaceWord => ModeResult::Ignored,
            KeyAction::AlphaNumeric(x) => ModeResult::Annotated(vec![ModeAction::InsertText(
                ActionPosition::cursor(),
                x.to_string(),
            )]),
            KeyAction::ChordNext => ModeResult::Ignored,
            KeyAction::CommandMode => ModeResult::Ignored,
            KeyAction::Save => ModeResult::Ignored,
            KeyAction::Quit => ModeResult::Ignored,
            KeyAction::FindFile => ModeResult::Ignored,
            KeyAction::SplitHorizontal => ModeResult::Ignored,
            KeyAction::SplitVertical => ModeResult::Ignored,
            KeyAction::SwitchWindow => ModeResult::Ignored,
            KeyAction::DeleteWindow => ModeResult::Ignored,
            KeyAction::DeleteOtherWindows => ModeResult::Ignored,
            KeyAction::Cancel => ModeResult::Consumed(vec![ModeAction::ClearMark]),
            KeyAction::SwitchBuffer => ModeResult::Ignored,
            KeyAction::KillBuffer => ModeResult::Ignored,
            KeyAction::Unbound => ModeResult::Ignored,
        }
    }
}

/// A mode for editing text files with load/save capability
pub struct FileMode {
    pub file_path: String,
}

impl Mode for FileMode {
    fn name(&self) -> &str {
        "file"
    }

    fn perform(&mut self, action: &KeyAction) -> ModeResult {
        match action {
            KeyAction::Cursor(_) => ModeResult::Ignored,
            KeyAction::InsertModeToggle => ModeResult::Ignored,
            KeyAction::Undo => ModeResult::Ignored,
            KeyAction::Redo => ModeResult::Ignored,
            KeyAction::MarkStart => ModeResult::Consumed(vec![ModeAction::SetMark]),
            KeyAction::MarkEnd => ModeResult::Ignored,
            KeyAction::KillRegion(destructive) => {
                if *destructive {
                    // C-w - kill region (delete and copy to kill-ring)
                    ModeResult::Consumed(vec![ModeAction::KillRegion])
                } else {
                    // M-w - copy region to kill-ring without deleting
                    ModeResult::Consumed(vec![ModeAction::CopyRegion])
                }
            }
            KeyAction::KillLine(_whole_line) => ModeResult::Consumed(vec![ModeAction::KillLine]),
            KeyAction::Yank(index) => match index {
                Some(idx) => ModeResult::Consumed(vec![ModeAction::YankIndex(
                    ActionPosition::cursor(),
                    *idx,
                )]),
                None => ModeResult::Consumed(vec![ModeAction::Yank(ActionPosition::cursor())]),
            },
            KeyAction::ForceIndent => ModeResult::Ignored,
            KeyAction::Tab => ModeResult::Ignored,
            KeyAction::Delete => {
                ModeResult::Consumed(vec![ModeAction::DeleteText(ActionPosition::cursor(), 1)])
            }
            KeyAction::Backspace => {
                ModeResult::Consumed(vec![ModeAction::DeleteText(ActionPosition::cursor(), -1)])
            }
            KeyAction::Enter => ModeResult::Consumed(vec![ModeAction::InsertText(
                ActionPosition::cursor(),
                "\n".to_string(),
            )]),
            KeyAction::Escape => ModeResult::Ignored,
            KeyAction::DeleteWord => ModeResult::Ignored,
            KeyAction::ToggleCapsLock => ModeResult::Ignored,
            KeyAction::ToggleScrollLock => ModeResult::Ignored,
            KeyAction::BackspaceWord => ModeResult::Ignored,
            KeyAction::AlphaNumeric(x) => ModeResult::Annotated(vec![ModeAction::InsertText(
                ActionPosition::cursor(),
                x.to_string(),
            )]),
            KeyAction::ChordNext => ModeResult::Ignored,
            KeyAction::CommandMode => ModeResult::Ignored,
            KeyAction::Save => ModeResult::Consumed(vec![ModeAction::Save]),
            KeyAction::Quit => ModeResult::Ignored,
            KeyAction::FindFile => ModeResult::Ignored,
            KeyAction::SplitHorizontal => ModeResult::Ignored,
            KeyAction::SplitVertical => ModeResult::Ignored,
            KeyAction::SwitchWindow => ModeResult::Ignored,
            KeyAction::DeleteWindow => ModeResult::Ignored,
            KeyAction::DeleteOtherWindows => ModeResult::Ignored,
            KeyAction::Cancel => ModeResult::Consumed(vec![ModeAction::ClearMark]),
            KeyAction::SwitchBuffer => ModeResult::Ignored,
            KeyAction::KillBuffer => ModeResult::Ignored,
            KeyAction::Unbound => ModeResult::Ignored,
        }
    }

    fn available_commands(&self) -> Vec<Command> {
        use crate::editor::ChromeAction;

        vec![
            Command::new(
                "save-buffer",
                "Save current buffer to file",
                CommandCategory::Mode("file".to_string()),
                Box::new(|_context| Ok(vec![ChromeAction::Echo("Saving file...".to_string())])),
            ),
            Command::new(
                "revert-buffer",
                "Reload buffer from file, discarding changes",
                CommandCategory::Mode("file".to_string()),
                Box::new(|_context| {
                    Ok(vec![ChromeAction::Echo("Reverting buffer...".to_string())])
                }),
            ),
            Command::new(
                "write-file",
                "Write buffer to a new file",
                CommandCategory::Mode("file".to_string()),
                Box::new(|_context| {
                    Ok(vec![ChromeAction::Echo(
                        "Write file not implemented yet".to_string(),
                    )])
                }),
            ),
        ]
    }
}

/// A read-only mode for displaying messages (echo events, logs, etc.)
pub struct MessagesMode {}

impl Mode for MessagesMode {
    fn name(&self) -> &str {
        "messages"
    }

    fn perform(&mut self, action: &KeyAction) -> ModeResult {
        match action {
            // Allow cursor movement for navigation
            KeyAction::Cursor(_) => ModeResult::Ignored,
            // Allow marks for copying messages
            KeyAction::MarkStart => ModeResult::Consumed(vec![ModeAction::SetMark]),
            KeyAction::KillRegion(destructive) => {
                if *destructive {
                    // C-w - don't allow destructive operations in messages buffer
                    ModeResult::Ignored
                } else {
                    // M-w - copy region to kill-ring without deleting
                    ModeResult::Consumed(vec![ModeAction::CopyRegion])
                }
            }
            KeyAction::Yank(index) => match index {
                Some(idx) => ModeResult::Consumed(vec![ModeAction::YankIndex(
                    ActionPosition::cursor(),
                    *idx,
                )]),
                None => ModeResult::Consumed(vec![ModeAction::Yank(ActionPosition::cursor())]),
            },
            KeyAction::Cancel => ModeResult::Consumed(vec![ModeAction::ClearMark]),
            // Block all editing operations - messages buffer is read-only
            KeyAction::AlphaNumeric(_) => ModeResult::Ignored,
            KeyAction::Enter => ModeResult::Ignored,
            KeyAction::Backspace => ModeResult::Ignored,
            KeyAction::Delete => ModeResult::Ignored,
            KeyAction::Tab => ModeResult::Ignored,
            KeyAction::KillLine(_) => ModeResult::Ignored,
            // All other operations are ignored
            _ => ModeResult::Ignored,
        }
    }

    fn available_commands(&self) -> Vec<Command> {
        use crate::editor::ChromeAction;

        vec![Command::new(
            "clear-messages",
            "Clear all messages from the messages buffer",
            CommandCategory::Mode("messages".to_string()),
            Box::new(|_context| Ok(vec![ChromeAction::Echo("Messages cleared".to_string())])),
        )]
    }
}
