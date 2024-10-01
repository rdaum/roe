use crate::buffer::Buffer;
use crate::keys::KeyAction::ChordNext;
use crate::keys::{Bindings, CursorDirection, KeyAction, KeyState, LogicalKey};
use crate::mode::{ActionPosition, Mode, ModeAction};
use crate::{echo, BufferId, ModeId, Modeline, WindowId, ECHO_AREA_HEIGHT};
use slotmap::SlotMap;

/// A "window" in the emacs sense, not the OS sense.
/// Represents a subsection of the "frame" (OS window or screen)
pub struct Window {
    /// What percent of the frame's available width this window takes up.
    /// The sum of all windows' widths should be 100.
    pub width: f32,
    /// What percent of the frame's available height this window takes up.
    /// The sum of all windows' heights should be 100.
    pub height: f32,
    pub active_buffer: BufferId,
    /// What line is the top left corner of the window in the buffer at?
    pub start_line: u16,
    /// Cursor offset
    /// The position of the cursor inside the buffer for this window.
    /// The actual physical cursor position on the screen is calculated from this and the window's
    /// position in the frame.
    pub cursor: usize,
}

/// A "frame" in the emacs sense, not the OS sense.
/// Represents the entire screen or window, including the modeline and echo area.
pub struct Frame {
    pub columns: u16,
    pub rows: u16,
    pub available_columns: u16,
    pub available_lines: u16,
}

impl Frame {
    pub fn new(columns: u16, rows: u16) -> Self {
        Frame {
            columns,
            rows,
            available_columns: columns,
            available_lines: rows - ECHO_AREA_HEIGHT - 1, /* modeline */
        }
    }
}

pub struct Editor {
    pub frame: Frame,
    pub buffers: SlotMap<BufferId, Buffer>,
    pub windows: SlotMap<WindowId, Window>,
    pub modes: SlotMap<ModeId, Box<dyn Mode>>,
    pub active_window: WindowId,
    pub key_state: KeyState,
    pub bindings: Box<dyn Bindings>,
    pub modeline: Modeline,
}

/// The main event loop, which receives keystrokes and dispatches them to the mode in the buffer
/// in the active window.
impl Editor {}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ChromeAction {
    FileOpen,
    CommandMode,
    CursorMove((u16, u16)),
    Huh,
    Echo(String),
    Refresh(WindowId),
    Quit,
}

impl Editor {
    pub fn key_event(
        &mut self,
        keys: Vec<LogicalKey>,
    ) -> Result<Vec<ChromeAction>, std::io::Error> {
        for key in keys {
            self.key_state.press(key);
        }

        // Send pressed keys through to the bindings.
        // If responds with ChordNext, we keep.
        // Otherwise, we take() and pass that to the mode for execution.
        // If the mode returns an action, we execute that action.
        let pressed = self.key_state.pressed();
        let key_action = self
            .bindings
            .keystroke(pressed.iter().map(|k| k.key).collect());
        if key_action == ChordNext {
            return Ok(vec![]);
        }

        let _ = self.key_state.take();

        echo(&mut std::io::stdout(), self, &format!("{:?}", key_action))?;
        let window = &mut self.windows.get_mut(self.active_window).unwrap();
        let buffer = &self.buffers.get_mut(window.active_buffer).unwrap();

        // Some actions like save, quit, etc. are out of the control of the mode.
        match &key_action {
            KeyAction::CommandMode => {
                return Ok(vec![ChromeAction::CommandMode]);
            }
            KeyAction::Quit => {
                return Ok(vec![
                    ChromeAction::Echo("Quitting".to_string()),
                    ChromeAction::Quit,
                ]);
            }
            KeyAction::FindFile => {
                return Ok(vec![ChromeAction::FileOpen]);
            }

            KeyAction::Cursor(CursorDirection::LineEnd) => {
                let eol_pos = buffer.eol_pos(window.cursor);
                window.cursor = eol_pos;

                let (col, line) = buffer.to_column_line(eol_pos);
                return Ok(vec![ChromeAction::CursorMove(
                    window.cursor_position(col, line),
                )]);
            }

            KeyAction::Cursor(CursorDirection::LineStart) => {
                let eol_pos = buffer.bol_pos(window.cursor);
                window.cursor = eol_pos;

                let (col, line) = buffer.to_column_line(eol_pos);
                return Ok(vec![ChromeAction::CursorMove(
                    window.cursor_position(col, line),
                )]);
            }
            KeyAction::Cursor(cd) => {
                // First move in the window and buffer and then return the action to move the
                // physical cursor on the screen.
                let (offset_col, offset_row) = match cd {
                    CursorDirection::Up => (0isize, -1isize),
                    CursorDirection::Down => (0, 1),
                    CursorDirection::Left => (-1, 0),
                    CursorDirection::Right => (1, 0),
                    _ => {
                        // TODO: page/end/home, etc.
                        (0, 0)
                    }
                };

                // Take the current offset in the window and ask the buffer where that is in the
                // buffer.
                let new_pos =
                    buffer.char_index_relative_offset(window.cursor, offset_col, offset_row);
                window.cursor = new_pos;

                // Now compute the physical position of the cursor in the window.
                let (col, line) = buffer.to_column_line(new_pos);

                return Ok(vec![ChromeAction::CursorMove(
                    window.cursor_position(col, line),
                )]);
            }
            KeyAction::Unbound => return Ok(vec![ChromeAction::Huh]),
            _ => {}
        }

        // Dispatch the key to the modes of the active-buffer in the active-window

        let mut chrome_actions = vec![];
        for mode_id in buffer.modes.clone() {
            let mode = self.modes.get_mut(mode_id).unwrap();
            let actions = mode.perform(&key_action);
            for action in actions {
                match &action {
                    ModeAction::InsertText(p, t) => {
                        chrome_actions.extend(self.insert_text(t.clone(), p));
                    }
                    ModeAction::DeleteText(p, c) => {
                        chrome_actions.extend(self.delete_text(p, *c));
                    }
                    ModeAction::CursorUp => {}
                    ModeAction::CursorDown => {}
                    ModeAction::CursorLeft => {}
                    ModeAction::CursorRight => {}
                    ModeAction::NextLine => {}
                }
                chrome_actions.push(ChromeAction::Echo(format!("{:?}", action)));
            }
        }

        crate::echo(
            &mut std::io::stdout(),
            self,
            &format!("{:?}", chrome_actions),
        )?;
        Ok(chrome_actions)
    }

    /// Perform insert action, based on the position passed and taking into account the window's
    /// cursor position.
    pub fn insert_text(&mut self, text: String, position: &ActionPosition) -> Vec<ChromeAction> {
        let window = &mut self.windows.get_mut(self.active_window).unwrap();
        let buffer = &mut self.buffers.get_mut(window.active_buffer).unwrap();
        match position {
            ActionPosition::Cursor => {
                let length = text.len();
                buffer.insert_pos(text, window.cursor);

                // Advance the cursor
                window.cursor += length;

                let new_cursor = buffer.to_column_line(window.cursor);
                let window_cursor = window.cursor_position(new_cursor.0, new_cursor.1);

                // Refresh the window
                // TODO: actually just print the portion rather than the whole thing
                return vec![
                    ChromeAction::Echo("Inserted text".to_string()),
                    ChromeAction::Refresh(self.active_window),
                    ChromeAction::CursorMove(window_cursor),
                ];
            }
            ActionPosition::Absolute(l, c) => {
                buffer.insert_col_line(text, (*l, *c));

                let new_cursor = buffer.to_column_line(window.cursor);
                let window_cursor = window.cursor_position(new_cursor.0, new_cursor.1);
                return vec![
                    ChromeAction::Echo("Inserted text".to_string()),
                    ChromeAction::Refresh(self.active_window),
                    ChromeAction::CursorMove(window_cursor),
                ];
            }
            ActionPosition::End => {
                return vec![ChromeAction::Echo("End insert not implemented".to_string())];
            }
        }
    }

    pub fn delete_text(&mut self, position: &ActionPosition, count: isize) -> Vec<ChromeAction> {
        let window = &mut self.windows.get_mut(self.active_window).unwrap();
        let buffer = &mut self.buffers.get_mut(window.active_buffer).unwrap();

        match position {
            ActionPosition::Cursor => {
                let Some(deleted) = buffer.delete_pos(window.cursor, count) else {
                    return vec![];
                };
                if deleted.is_empty() {
                    return vec![];
                }
                // If the count was negative, then we need to adjust the cursor back by the size
                // of the deleted fragment.
                if count < 0 {
                    let length = deleted.len();
                    window.cursor -= length;
                }
                let new_cursor = buffer.to_column_line(window.cursor);
                let window_cursor = window.cursor_position(new_cursor.0, new_cursor.1);
                return vec![
                    ChromeAction::Echo("Deleted text".to_string()),
                    ChromeAction::Refresh(self.active_window),
                    ChromeAction::CursorMove(window_cursor),
                ];
            }
            ActionPosition::Absolute(l, c) => {
                buffer.delete_col_line((*l, *c), count);
                let new_cursor = buffer.to_column_line(window.cursor);
                let window_cursor = window.cursor_position(new_cursor.0, new_cursor.1);
                return vec![
                    ChromeAction::Echo("Deleted text".to_string()),
                    ChromeAction::Refresh(self.active_window),
                    ChromeAction::CursorMove(window_cursor),
                ];
            }
            ActionPosition::End => {
                return vec![ChromeAction::Echo("End delete not implemented".to_string())];
            }
        }
    }
}
