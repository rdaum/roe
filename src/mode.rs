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
    Absolute(u16, u16),
    /// Action should happen at the very end of the buffer, appending or removing etc.
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
            KeyAction::MarkStart => {}
            KeyAction::MarkEnd => {}
            KeyAction::KillRegion(_) => {}
            KeyAction::KillLine(_) => {}
            KeyAction::Yank(_) => {}
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
            KeyAction::Unbound => {}
        }
        vec![]
    }
}
