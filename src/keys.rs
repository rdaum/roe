use std::time::Instant;

pub trait Bindings {
    fn keystroke(&self, keys: Vec<LogicalKey>) -> KeyAction;
}

/// An enumeration of our logical actions caused by keystrokes.
/// E.g. Save, CursorLeft, CursorRight, AlphaNumeric('a'), InsertModeToggle, etc.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum KeyAction {
    /// Move the cursor in a direction
    Cursor(CursorDirection),
    /// Toggle insert mode
    InsertModeToggle,
    /// Undo the last action
    Undo,
    /// Redo the last undone action
    Redo,
    /// Begin a selection
    MarkStart,
    /// End a selection
    MarkEnd,
    /// Add to kill-ring. If true, the selection is deleted, otherwise left present.
    KillRegion(bool),
    /// Kill line (whole or rest)
    KillLine(bool),
    /// Yank from kill-ring. If Some, yank that index, otherwise yank the last kill.
    Yank(Option<usize>),
    /// Force the current line to obey the indent rules
    ForceIndent,
    /// Move cursor over one tab-stop
    Tab,
    /// Delete the character under the cursor
    Delete,
    /// Backspace-delete the character before the cursor
    Backspace,
    /// Insert a newline or receive command etc (maybe split those two up?)
    Enter,
    ///
    Escape,
    /// Delete the "word" under the cursor
    DeleteWord,
    ///
    ToggleCapsLock,
    ///
    ToggleScrollLock,
    /// Backspace-delete the "word" before the cursor
    BackspaceWord,
    AlphaNumeric(char),
    /// Wait for the next key, to form a chord
    /// e.g. for C-x C-c etc.
    /// The next non-Chord key will pull the sequence out of the KeyState
    ChordNext,
    /// Enter command mode in `echo` (M-x)
    CommandMode,
    /// Save
    Save,
    /// Quit
    Quit,
    /// Find file
    FindFile,
    ///
    Unbound,
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
    pub fn to_display(&self) -> String {
        // emacs-like short form.  i.e. x, m,. C-, S- M- etc.
        let s = match self {
            LogicalKey::Left => "←",
            LogicalKey::Right => "→",
            LogicalKey::Up => "↑",
            LogicalKey::Down => "↓",
            LogicalKey::PageUp => "PgUp",
            LogicalKey::PageDown => "PgDn",
            LogicalKey::Function(f) => &format!("F{}", f),
            LogicalKey::AlphaNumeric(a) => &format!("{}", a),
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
                (
                    LogicalKey::Modifier(KeyModifier::Control(_) | KeyModifier::Shift(_)),
                    LogicalKey::AlphaNumeric(a),
                ) if a == 'c' || a == 'x' => return KeyAction::ChordNext,
                // M-x command mode
                (LogicalKey::Modifier(KeyModifier::Meta(_)), LogicalKey::AlphaNumeric(a))
                    if a == 'x' =>
                {
                    return KeyAction::CommandMode
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
                (LogicalKey::Modifier(KeyModifier::Control(_)), LogicalKey::AlphaNumeric(a))
                    if a == 'p' =>
                {
                    return KeyAction::Cursor(CursorDirection::Up)
                }
                // Ctrl-N
                (LogicalKey::Modifier(KeyModifier::Control(_)), LogicalKey::AlphaNumeric(a))
                    if a == 'n' =>
                {
                    return KeyAction::Cursor(CursorDirection::Down)
                }
                // Ctrl-F
                (LogicalKey::Modifier(KeyModifier::Control(_)), LogicalKey::AlphaNumeric(a))
                    if a == 'f' =>
                {
                    return KeyAction::Cursor(CursorDirection::Right)
                }
                // Ctrl-B
                (LogicalKey::Modifier(KeyModifier::Control(_)), LogicalKey::AlphaNumeric(a))
                    if a == 'b' =>
                {
                    return KeyAction::Cursor(CursorDirection::Left)
                }
                // Ctrl-V is page-down
                (LogicalKey::Modifier(KeyModifier::Control(_)), LogicalKey::AlphaNumeric(a))
                    if a == 'v' =>
                {
                    return KeyAction::Cursor(CursorDirection::PageDown)
                }
                // Ctrl-A is start of line
                (LogicalKey::Modifier(KeyModifier::Control(_)), LogicalKey::AlphaNumeric(a))
                    if a == 'a' =>
                {
                    return KeyAction::Cursor(CursorDirection::LineStart)
                }
                // Ctrl-E is end of line
                (LogicalKey::Modifier(KeyModifier::Control(_)), LogicalKey::AlphaNumeric(a))
                    if a == 'e' =>
                {
                    return KeyAction::Cursor(CursorDirection::LineEnd)
                }
                // Ctrl-K is kill-line
                (LogicalKey::Modifier(KeyModifier::Control(_)), LogicalKey::AlphaNumeric(a))
                    if a == 'k' =>
                {
                    return KeyAction::KillLine(false)
                }
                // Ctrl-Y is yank
                (LogicalKey::Modifier(KeyModifier::Control(_)), LogicalKey::AlphaNumeric(a))
                    if a == 'y' =>
                {
                    return KeyAction::Yank(None)
                }
                // Ctrl-W is kill-region
                (LogicalKey::Modifier(KeyModifier::Control(_)), LogicalKey::AlphaNumeric(a))
                    if a == 'w' =>
                {
                    return KeyAction::KillRegion(true)
                }
                // Ctrl-/ is undo
                (LogicalKey::Modifier(KeyModifier::Control(_)), LogicalKey::AlphaNumeric(a))
                    if a == '/' =>
                {
                    return KeyAction::Undo
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
                ) if *a == 'x' && *b == 'c' => return KeyAction::Quit,
                // C-x C-s save
                (
                    LogicalKey::Modifier(KeyModifier::Control(_)),
                    LogicalKey::AlphaNumeric(a),
                    LogicalKey::AlphaNumeric(b),
                ) if *a == 'x' && *b == 's' => return KeyAction::Save,
                // C-x C-f find-file
                (
                    LogicalKey::Modifier(KeyModifier::Control(_)),
                    LogicalKey::AlphaNumeric(a),
                    LogicalKey::AlphaNumeric(b),
                ) if *a == 'x' && *b == 'f' => return KeyAction::FindFile,
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
        return KeyAction::Unbound;
    }
}
