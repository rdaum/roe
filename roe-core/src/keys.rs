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

use crate::command_registry::*;
use std::time::Instant;

pub trait Bindings {
    fn keystroke(&self, keys: Vec<LogicalKey>) -> KeyAction;
}

/// An enumeration of our logical actions caused by keystrokes.
/// Direct text manipulation and meta-actions stay as KeyActions.
/// Complex UI/system actions become commands.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum KeyAction {
    // Direct text manipulation (fast path)
    /// Type a character
    AlphaNumeric(char),
    /// Move the cursor in a direction
    Cursor(CursorDirection),
    /// Delete the character under the cursor
    Delete,
    /// Backspace-delete the character before the cursor
    Backspace,
    /// Insert a newline
    Enter,
    /// Tab character or indentation
    Tab,

    // Basic editing operations (still direct for performance)
    /// Begin a selection (set mark)
    MarkStart,
    /// Add to kill-ring. If true, the selection is deleted, otherwise left present.
    KillRegion(bool),
    /// Kill line (whole or rest)
    KillLine(bool),
    /// Yank from kill-ring. If Some, yank that index, otherwise yank the last kill.
    Yank(Option<usize>),

    // All complex actions become commands
    /// Execute a named command
    Command(String),

    // Special meta-actions
    /// Wait for the next key, to form a chord
    ChordNext,
    /// Escape key
    Escape,
    /// Cancel current operation
    Cancel,
    /// Unbound/unmapped key
    Unbound,

    // Additional action types that are still direct for performance/simplicity
    /// Toggle insert mode
    InsertModeToggle,
    /// Undo last operation
    Undo,
    /// Redo last undone operation
    Redo,
    /// Delete word forward
    DeleteWord,
    /// Backspace word backward
    BackspaceWord,
    /// Toggle caps lock
    ToggleCapsLock,
    /// Toggle scroll lock
    ToggleScrollLock,
    /// Force indentation
    ForceIndent,
    /// Mark end (unused but referenced)
    MarkEnd,

    // TEMPORARY: Keep these during transition
    CommandMode,
    Save,
    Quit,
    FindFile,
    SplitHorizontal,
    SplitVertical,
    SwitchWindow,
    DeleteWindow,
    DeleteOtherWindows,
    SwitchBuffer,
    KillBuffer,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum CursorDirection {
    Left,
    Right,
    Up,
    Down,
    LineEnd,
    LineStart,
    BufferStart,
    BufferEnd,
    PageUp,
    PageDown,
    WordForward,
    WordBackward,
    ParagraphForward,
    ParagraphBackward,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum Side {
    Left,
    Right,
}

/// The set of emacs-ish keys we care about, that we map the physical system keycodes to.
/// Series of these then get mapped to actions.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum LogicalKey {
    Left,
    Right,
    Up,
    Down,
    PageUp,
    PageDown,
    Function(u8),
    AlphaNumeric(char),
    Backspace,
    Enter,
    Home,
    End,
    Insert,
    Tab,
    Delete,
    Unmapped,
    CapsLock,
    ScrollLock,
    Esc,
    /// A modifier pressed without an underlying non-modifier key.
    Modifier(KeyModifier),
}

impl LogicalKey {
    pub fn as_display_string(&self) -> String {
        // emacs-like short form.  i.e. x, m,. C-, S- M- etc.
        let s = match self {
            LogicalKey::Left => "←",
            LogicalKey::Right => "→",
            LogicalKey::Up => "↑",
            LogicalKey::Down => "↓",
            LogicalKey::PageUp => "PgUp",
            LogicalKey::PageDown => "PgDn",
            LogicalKey::Function(f) => &format!("F{f}"),
            LogicalKey::AlphaNumeric(a) => &format!("{a}"),
            LogicalKey::Backspace => "⌫",
            LogicalKey::Enter => "⏎",
            LogicalKey::Home => "Home",
            LogicalKey::End => "End",
            LogicalKey::Insert => "Ins",
            LogicalKey::Tab => "Tab",
            LogicalKey::Delete => "Del",
            LogicalKey::Unmapped => "Unmapped",
            LogicalKey::CapsLock => "Caps",
            LogicalKey::ScrollLock => "Scroll",
            LogicalKey::Esc => "Esc",
            LogicalKey::Modifier(KeyModifier::Hyper(_)) => "H",
            LogicalKey::Modifier(KeyModifier::Super(_)) => "S",
            LogicalKey::Modifier(KeyModifier::Meta(_)) => "M",
            LogicalKey::Modifier(KeyModifier::Control(_)) => "C",
            LogicalKey::Modifier(KeyModifier::Shift(_)) => "S",
            LogicalKey::Modifier(KeyModifier::Alt(_)) => "A",
            LogicalKey::Modifier(KeyModifier::Unmapped) => "?",
        };
        s.to_string()
    }
}
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum KeyModifier {
    Hyper(Side),
    Super(Side),
    Meta(Side),
    Control(Side),
    Shift(Side),
    Alt(Side),
    Unmapped,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct KeyPress {
    /// What key
    pub key: LogicalKey,
    /// When pressed
    pub when: Instant,
}

/// The key-state machine, tracking the entry and exit of modifiers and keys.
pub struct KeyState {
    // Active keys
    // TODO: this could be a bitset, with some fandangling for modifiers
    keys: Vec<KeyPress>,
}

impl Default for KeyState {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyState {
    pub fn new() -> Self {
        KeyState { keys: Vec::new() }
    }

    pub fn press(&mut self, key_code: LogicalKey) {
        // If we already have this key pressed, ignore it.
        if self.keys.iter().any(|kp| kp.key == key_code) {
            return;
        }
        self.keys.push(KeyPress {
            key: key_code,
            when: Instant::now(),
        });
    }

    pub fn release(&mut self, key_code: LogicalKey) {
        // Remove the key from the list of active keys
        self.keys.retain(|kp| kp.key != key_code);
    }

    /// Return what is currently pressed and in what order.
    pub fn pressed(&self) -> Vec<KeyPress> {
        let mut keys = self.keys.clone();
        keys.sort_by(|a, b| a.when.cmp(&b.when));
        keys
    }

    pub fn take(&mut self) -> Vec<KeyPress> {
        let keys = self.keys.clone();
        self.keys.clear();
        keys
    }
}

pub struct DefaultBindings {}

impl Bindings for DefaultBindings {
    fn keystroke(&self, keys: Vec<LogicalKey>) -> KeyAction {
        // Translate emacs-style keys to actions
        // Handle single-key cases first.
        if keys.len() == 1 {
            match &keys[0] {
                LogicalKey::Left => {
                    return KeyAction::Cursor(CursorDirection::Left);
                }
                LogicalKey::Right => {
                    return KeyAction::Cursor(CursorDirection::Right);
                }
                LogicalKey::Up => {
                    return KeyAction::Cursor(CursorDirection::Up);
                }
                LogicalKey::Down => {
                    return KeyAction::Cursor(CursorDirection::Down);
                }
                LogicalKey::PageUp => {
                    return KeyAction::Cursor(CursorDirection::PageUp);
                }
                LogicalKey::PageDown => {
                    return KeyAction::Cursor(CursorDirection::PageDown);
                }
                LogicalKey::Function(_) => {
                    return KeyAction::Unbound;
                }
                LogicalKey::AlphaNumeric(a) => {
                    return KeyAction::AlphaNumeric(*a);
                }
                LogicalKey::Backspace => {
                    return KeyAction::Backspace;
                }
                LogicalKey::Enter => {
                    return KeyAction::Enter;
                }
                LogicalKey::Home => {
                    return KeyAction::Cursor(CursorDirection::LineStart);
                }
                LogicalKey::End => {
                    return KeyAction::Cursor(CursorDirection::LineEnd);
                }
                LogicalKey::Insert => {
                    return KeyAction::InsertModeToggle;
                }
                LogicalKey::Tab => {
                    // Could also be ForceIndent, or the mode could decide. Hm.
                    return KeyAction::Tab;
                }
                LogicalKey::Delete => {
                    return KeyAction::Delete;
                }
                LogicalKey::Unmapped => {
                    return KeyAction::Unbound;
                }
                LogicalKey::CapsLock => {
                    return KeyAction::ToggleCapsLock;
                }
                LogicalKey::ScrollLock => {
                    return KeyAction::ToggleScrollLock;
                }
                LogicalKey::Esc => {
                    return KeyAction::Escape;
                }
                LogicalKey::Modifier(_) => {
                    // Begin chord
                    return KeyAction::ChordNext;
                }
            }
        }

        // Otherwise, what we have is a chord of some kind.
        // There are some special two-key cases, like Shift-Alpha which we can handle right away
        if keys.len() == 2 {
            // shift-alpha
            match (&keys[0], keys[1]) {
                // Shift-alpha
                (LogicalKey::Modifier(KeyModifier::Shift(_)), LogicalKey::AlphaNumeric(a)) => {
                    return KeyAction::AlphaNumeric(a.to_ascii_uppercase());
                }
                // C-c C-x continue a chord
                (LogicalKey::Modifier(KeyModifier::Control(_)), LogicalKey::AlphaNumeric(a))
                    if a == 'c' || a == 'x' =>
                {
                    return KeyAction::ChordNext
                }
                // M-x command mode
                (LogicalKey::Modifier(KeyModifier::Meta(_)), LogicalKey::AlphaNumeric('x')) => {
                    return KeyAction::Command(CMD_COMMAND_MODE.to_string())
                }
                // M-w copy region (like C-w but without deleting)
                (LogicalKey::Modifier(KeyModifier::Meta(_)), LogicalKey::AlphaNumeric('w')) => {
                    return KeyAction::KillRegion(false)
                }
                // M-f forward word
                (LogicalKey::Modifier(KeyModifier::Meta(_)), LogicalKey::AlphaNumeric('f')) => {
                    return KeyAction::Cursor(CursorDirection::WordForward)
                }
                // M-b backward word
                (LogicalKey::Modifier(KeyModifier::Meta(_)), LogicalKey::AlphaNumeric('b')) => {
                    return KeyAction::Cursor(CursorDirection::WordBackward)
                }
                // C-left backward word
                (LogicalKey::Modifier(KeyModifier::Control(_)), LogicalKey::Left) => {
                    return KeyAction::Cursor(CursorDirection::WordBackward)
                }
                // C-right forward word
                (LogicalKey::Modifier(KeyModifier::Control(_)), LogicalKey::Right) => {
                    return KeyAction::Cursor(CursorDirection::WordForward)
                }
                // M-v page up
                (LogicalKey::Modifier(KeyModifier::Meta(_)), LogicalKey::AlphaNumeric('v')) => {
                    return KeyAction::Cursor(CursorDirection::PageUp)
                }
                // M-up page up
                (LogicalKey::Modifier(KeyModifier::Meta(_)), LogicalKey::Up) => {
                    return KeyAction::Cursor(CursorDirection::PageUp)
                }
                // M-down page down
                (LogicalKey::Modifier(KeyModifier::Meta(_)), LogicalKey::Down) => {
                    return KeyAction::Cursor(CursorDirection::PageDown)
                }
                // M-{ backward paragraph
                (LogicalKey::Modifier(KeyModifier::Meta(_)), LogicalKey::AlphaNumeric('{')) => {
                    return KeyAction::Cursor(CursorDirection::ParagraphBackward)
                }
                // M-} forward paragraph
                (LogicalKey::Modifier(KeyModifier::Meta(_)), LogicalKey::AlphaNumeric('}')) => {
                    return KeyAction::Cursor(CursorDirection::ParagraphForward)
                }
                // Ctrl-End is buffer-end
                (LogicalKey::Modifier(KeyModifier::Control(_)), LogicalKey::End) => {
                    return KeyAction::Cursor(CursorDirection::BufferEnd)
                }
                // Ctrl-Home is buffer-start
                (LogicalKey::Modifier(KeyModifier::Control(_)), LogicalKey::Home) => {
                    return KeyAction::Cursor(CursorDirection::BufferStart)
                }
                // Ctrl-P
                (LogicalKey::Modifier(KeyModifier::Control(_)), LogicalKey::AlphaNumeric('p')) => {
                    return KeyAction::Cursor(CursorDirection::Up)
                }
                // Ctrl-N
                (LogicalKey::Modifier(KeyModifier::Control(_)), LogicalKey::AlphaNumeric('n')) => {
                    return KeyAction::Cursor(CursorDirection::Down)
                }
                // Ctrl-F
                (LogicalKey::Modifier(KeyModifier::Control(_)), LogicalKey::AlphaNumeric('f')) => {
                    return KeyAction::Cursor(CursorDirection::Right)
                }
                // Ctrl-B
                (LogicalKey::Modifier(KeyModifier::Control(_)), LogicalKey::AlphaNumeric('b')) => {
                    return KeyAction::Cursor(CursorDirection::Left)
                }
                // Ctrl-V is page-down
                (LogicalKey::Modifier(KeyModifier::Control(_)), LogicalKey::AlphaNumeric('v')) => {
                    return KeyAction::Cursor(CursorDirection::PageDown)
                }
                // Ctrl-A is start of line
                (LogicalKey::Modifier(KeyModifier::Control(_)), LogicalKey::AlphaNumeric('a')) => {
                    return KeyAction::Cursor(CursorDirection::LineStart)
                }
                // Ctrl-E is end of line
                (LogicalKey::Modifier(KeyModifier::Control(_)), LogicalKey::AlphaNumeric('e')) => {
                    return KeyAction::Cursor(CursorDirection::LineEnd)
                }
                // Ctrl-K is kill-line
                (LogicalKey::Modifier(KeyModifier::Control(_)), LogicalKey::AlphaNumeric('k')) => {
                    return KeyAction::KillLine(false)
                }
                // Ctrl-Y is yank
                (LogicalKey::Modifier(KeyModifier::Control(_)), LogicalKey::AlphaNumeric('y')) => {
                    return KeyAction::Yank(None)
                }
                // Ctrl-W is kill-region
                (LogicalKey::Modifier(KeyModifier::Control(_)), LogicalKey::AlphaNumeric('w')) => {
                    return KeyAction::KillRegion(true)
                }
                // Ctrl-/ is undo
                (LogicalKey::Modifier(KeyModifier::Control(_)), LogicalKey::AlphaNumeric('/')) => {
                    return KeyAction::Undo
                }
                // Ctrl-Space is set mark (C-SPC)
                (LogicalKey::Modifier(KeyModifier::Control(_)), LogicalKey::AlphaNumeric(' ')) => {
                    return KeyAction::MarkStart
                }
                // Ctrl-G is cancel
                (LogicalKey::Modifier(KeyModifier::Control(_)), LogicalKey::AlphaNumeric('g')) => {
                    return KeyAction::Cancel
                }
                //
                (_, _) => {}
            }
        }

        // Three chords and the truth
        // C-x C-c is exit
        if keys.len() == 3 {
            match (&keys[0], &keys[1], &keys[2]) {
                // C-x C-c quit
                (
                    LogicalKey::Modifier(KeyModifier::Control(_)),
                    LogicalKey::AlphaNumeric(a),
                    LogicalKey::AlphaNumeric(b),
                ) if *a == 'x' && *b == 'c' => return KeyAction::Command(CMD_QUIT.to_string()),
                // C-x C-s save
                (
                    LogicalKey::Modifier(KeyModifier::Control(_)),
                    LogicalKey::AlphaNumeric(a),
                    LogicalKey::AlphaNumeric(b),
                ) if *a == 'x' && *b == 's' => {
                    return KeyAction::Command(CMD_SAVE_BUFFER.to_string())
                }
                // C-x C-f find-file
                (
                    LogicalKey::Modifier(KeyModifier::Control(_)),
                    LogicalKey::AlphaNumeric(a),
                    LogicalKey::AlphaNumeric(b),
                ) if *a == 'x' && *b == 'f' => {
                    return KeyAction::Command(CMD_FIND_FILE.to_string())
                }
                // C-x 2 split horizontally
                (
                    LogicalKey::Modifier(KeyModifier::Control(_)),
                    LogicalKey::AlphaNumeric(a),
                    LogicalKey::AlphaNumeric(b),
                ) if *a == 'x' && *b == '2' => {
                    return KeyAction::Command(CMD_SPLIT_HORIZONTAL.to_string())
                }
                // C-x 3 split vertically
                (
                    LogicalKey::Modifier(KeyModifier::Control(_)),
                    LogicalKey::AlphaNumeric(a),
                    LogicalKey::AlphaNumeric(b),
                ) if *a == 'x' && *b == '3' => {
                    return KeyAction::Command(CMD_SPLIT_VERTICAL.to_string())
                }
                // C-x o switch window
                (
                    LogicalKey::Modifier(KeyModifier::Control(_)),
                    LogicalKey::AlphaNumeric(a),
                    LogicalKey::AlphaNumeric(b),
                ) if *a == 'x' && *b == 'o' => {
                    return KeyAction::Command(CMD_OTHER_WINDOW.to_string())
                }
                // C-x 0 delete window
                (
                    LogicalKey::Modifier(KeyModifier::Control(_)),
                    LogicalKey::AlphaNumeric(a),
                    LogicalKey::AlphaNumeric(b),
                ) if *a == 'x' && *b == '0' => {
                    return KeyAction::Command(CMD_DELETE_WINDOW.to_string())
                }
                // C-x 1 delete other windows
                (
                    LogicalKey::Modifier(KeyModifier::Control(_)),
                    LogicalKey::AlphaNumeric(a),
                    LogicalKey::AlphaNumeric(b),
                ) if *a == 'x' && *b == '1' => {
                    return KeyAction::Command(CMD_DELETE_OTHER_WINDOWS.to_string())
                }
                // C-x b switch buffer
                (
                    LogicalKey::Modifier(KeyModifier::Control(_)),
                    LogicalKey::AlphaNumeric(a),
                    LogicalKey::AlphaNumeric(b),
                ) if *a == 'x' && *b == 'b' => {
                    return KeyAction::Command(CMD_SWITCH_BUFFER.to_string())
                }
                // C-x k kill buffer
                (
                    LogicalKey::Modifier(KeyModifier::Control(_)),
                    LogicalKey::AlphaNumeric(a),
                    LogicalKey::AlphaNumeric(b),
                ) if *a == 'x' && *b == 'k' => {
                    return KeyAction::Command(CMD_KILL_BUFFER.to_string())
                }
                // Ctrl-Shift-W is kill-region non-destructive
                (
                    LogicalKey::Modifier(KeyModifier::Control(_)),
                    LogicalKey::Modifier(KeyModifier::Shift(_)),
                    LogicalKey::AlphaNumeric(a),
                ) if *a == 'w' => return KeyAction::KillRegion(false),
                // Ctrl-Shift-Y is yank-index-start
                (
                    LogicalKey::Modifier(KeyModifier::Control(_)),
                    LogicalKey::Modifier(KeyModifier::Shift(_)),
                    LogicalKey::AlphaNumeric(a),
                ) if *a == 'y' => return KeyAction::Yank(Some(0)),
                // Redo is Ctrl-Shift-/
                (
                    LogicalKey::Modifier(KeyModifier::Control(_)),
                    LogicalKey::Modifier(KeyModifier::Shift(_)),
                    LogicalKey::AlphaNumeric(a),
                ) if *a == '/' => return KeyAction::Redo,
                // TODO: others
                (_, _, _) => {}
            }
        }
        KeyAction::Unbound
    }
}
