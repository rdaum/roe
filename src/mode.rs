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

pub trait Mode {
    fn name(&self) -> &str;
    fn perform(&mut self, action: &KeyAction) -> Vec<ModeAction>;
}

pub struct ScratchMode {}

impl Mode for ScratchMode {
    fn name(&self) -> &str {
        "scratch"
    }

    fn perform(&mut self, action: &KeyAction) -> Vec<ModeAction> {
        match action {
            KeyAction::Cursor(_) => {}
            KeyAction::InsertModeToggle => {}
            KeyAction::Undo => {}
            KeyAction::Redo => {}
            KeyAction::MarkStart => {
                return vec![ModeAction::SetMark];
            }
            KeyAction::MarkEnd => {}
            KeyAction::KillRegion(_destructive) => {
                // TODO: Implement region killing when mark is implemented
                return vec![ModeAction::KillRegion];
            }
            KeyAction::KillLine(_whole_line) => {
                return vec![ModeAction::KillLine];
            }
            KeyAction::Yank(index) => match index {
                Some(idx) => return vec![ModeAction::YankIndex(ActionPosition::cursor(), *idx)],
                None => return vec![ModeAction::Yank(ActionPosition::cursor())],
            },
            KeyAction::ForceIndent => {}
            KeyAction::Tab => {}
            KeyAction::Delete => return vec![ModeAction::DeleteText(ActionPosition::cursor(), 1)],
            KeyAction::Backspace => {
                return vec![ModeAction::DeleteText(ActionPosition::cursor(), -1)]
            }
            KeyAction::Enter => {
                return vec![ModeAction::InsertText(
                    ActionPosition::cursor(),
                    "\n".to_string(),
                )]
            }
            KeyAction::Escape => {}
            KeyAction::DeleteWord => {}
            KeyAction::ToggleCapsLock => {}
            KeyAction::ToggleScrollLock => {}
            KeyAction::BackspaceWord => {}
            KeyAction::AlphaNumeric(x) => {
                return vec![ModeAction::InsertText(
                    ActionPosition::cursor(),
                    x.to_string(),
                )]
            }
            KeyAction::ChordNext => {}
            KeyAction::CommandMode => {}
            KeyAction::Save => {}
            KeyAction::Quit => {}
            KeyAction::FindFile => {}
            KeyAction::SplitHorizontal => {}
            KeyAction::SplitVertical => {}
            KeyAction::SwitchWindow => {}
            KeyAction::DeleteWindow => {}
            KeyAction::DeleteOtherWindows => {}
            KeyAction::Cancel => {
                return vec![ModeAction::ClearMark];
            }
            KeyAction::Unbound => {}
        }
        vec![]
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

    fn perform(&mut self, action: &KeyAction) -> Vec<ModeAction> {
        match action {
            KeyAction::Cursor(_) => {}
            KeyAction::InsertModeToggle => {}
            KeyAction::Undo => {}
            KeyAction::Redo => {}
            KeyAction::MarkStart => {
                return vec![ModeAction::SetMark];
            }
            KeyAction::MarkEnd => {}
            KeyAction::KillRegion(_destructive) => {
                return vec![ModeAction::KillRegion];
            }
            KeyAction::KillLine(_whole_line) => {
                return vec![ModeAction::KillLine];
            }
            KeyAction::Yank(index) => match index {
                Some(idx) => return vec![ModeAction::YankIndex(ActionPosition::cursor(), *idx)],
                None => return vec![ModeAction::Yank(ActionPosition::cursor())],
            },
            KeyAction::ForceIndent => {}
            KeyAction::Tab => {}
            KeyAction::Delete => return vec![ModeAction::DeleteText(ActionPosition::cursor(), 1)],
            KeyAction::Backspace => {
                return vec![ModeAction::DeleteText(ActionPosition::cursor(), -1)]
            }
            KeyAction::Enter => {
                return vec![ModeAction::InsertText(
                    ActionPosition::cursor(),
                    "\n".to_string(),
                )]
            }
            KeyAction::Escape => {}
            KeyAction::DeleteWord => {}
            KeyAction::ToggleCapsLock => {}
            KeyAction::ToggleScrollLock => {}
            KeyAction::BackspaceWord => {}
            KeyAction::AlphaNumeric(x) => {
                return vec![ModeAction::InsertText(
                    ActionPosition::cursor(),
                    x.to_string(),
                )]
            }
            KeyAction::ChordNext => {}
            KeyAction::CommandMode => {}
            KeyAction::Save => {
                return vec![ModeAction::Save];
            }
            KeyAction::Quit => {}
            KeyAction::FindFile => {}
            KeyAction::SplitHorizontal => {}
            KeyAction::SplitVertical => {}
            KeyAction::SwitchWindow => {}
            KeyAction::DeleteWindow => {}
            KeyAction::DeleteOtherWindows => {}
            KeyAction::Cancel => {
                return vec![ModeAction::ClearMark];
            }
            KeyAction::Unbound => {}
        }
        vec![]
    }
}
