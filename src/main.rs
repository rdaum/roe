use buffer::Buffer;
use crossterm::event::{
    Event, KeyCode, KeyModifiers, KeyboardEnhancementFlags, ModifierKeyCode,
    PushKeyboardEnhancementFlags,
};
use crossterm::style::{Color, Print, Stylize};
use crossterm::terminal::{Clear, ClearType};
use crossterm::{cursor, execute, queue};
use editor::{ChromeAction, Editor, Frame, Window};
use keys::{KeyModifier, KeyState, LogicalKey, Side};
use mode::{Mode, ScratchMode};
use slotmap::{new_key_type, Key, SlotMap};
use std::io::Write;

mod buffer;
mod editor;
mod keys;
mod mode;
mod window;

pub const ECHO_AREA_HEIGHT: u16 = 1;

new_key_type! {
    pub struct WindowId;
}

new_key_type! {
    pub struct BufferId;
}

new_key_type! {
    pub struct ModeId;
}

pub const BG_COLOR: Color = Color::Black;
pub const FG_COLOR: Color = Color::White;
pub const MODE_LINE_BG_COLOR: Color = Color::Blue;
pub const MAJOR_MODE_TEXT_COLOR: Color = Color::Yellow;
pub const OBJECT_TEXT_COLOR: Color = Color::Green;
pub const LINE_COL_TEXT_COLOR: Color = Color::Red;
pub const MINOR_MODES_TEXT_COLOR: Color = Color::White;

pub fn echo_area_position(frame: &Frame) -> (u16, u16) {
    (0, frame.rows - ECHO_AREA_HEIGHT)
}

pub enum ModelinePartType {
    Mode,
    Object,
    LineCol,
    MinorModes,
    Separator,
}

pub struct ModelinePart {
    part: ModelinePartType,
    width: usize,
    fg_color: Color,
}

pub const MODELINE_PARTS_PADDING: usize = 2;
pub const MODELINE_MAJOR_MODE_WIDTH: usize = 20;
pub const MODELINE_OBJECT_WIDTH: usize = 32;
pub const MODELINE_LINE_COL_WIDTH: usize = 10;

pub struct Modeline {
    parts: Vec<ModelinePart>,
    columns: Vec<usize>,
}

impl Modeline {
    /// Build all the parts of the modeline.
    fn new(modes: Vec<&dyn Mode>) -> Self {
        let mut columns = vec![];

        // Initial separator padding
        columns.push(ModelinePart {
            part: ModelinePartType::Separator,
            width: MODELINE_PARTS_PADDING,
            fg_color: MODE_LINE_BG_COLOR,
        });
        // Major mode
        columns.push(ModelinePart {
            part: ModelinePartType::Mode,
            width: MODELINE_MAJOR_MODE_WIDTH,
            fg_color: MAJOR_MODE_TEXT_COLOR,
        });
        // Separator
        columns.push(ModelinePart {
            part: ModelinePartType::Separator,
            width: MODELINE_PARTS_PADDING,
            fg_color: MODE_LINE_BG_COLOR,
        });
        // Object
        columns.push(ModelinePart {
            part: ModelinePartType::Object,
            width: MODELINE_OBJECT_WIDTH,
            fg_color: OBJECT_TEXT_COLOR,
        });
        // Separator
        columns.push(ModelinePart {
            part: ModelinePartType::Separator,
            width: MODELINE_PARTS_PADDING,
            fg_color: MODE_LINE_BG_COLOR,
        });
        // Line:Col
        columns.push(ModelinePart {
            part: ModelinePartType::LineCol,
            width: MODELINE_LINE_COL_WIDTH,
            fg_color: LINE_COL_TEXT_COLOR,
        });
        // Separator
        columns.push(ModelinePart {
            part: ModelinePartType::Separator,
            width: MODELINE_PARTS_PADDING,
            fg_color: MODE_LINE_BG_COLOR,
        });
        // Minor modes
        for mode in modes {
            let name = mode.name();
            columns.push(ModelinePart {
                part: ModelinePartType::MinorModes,
                width: name.len(),
                fg_color: MINOR_MODES_TEXT_COLOR,
            });
            // Separator
            columns.push(ModelinePart {
                part: ModelinePartType::Separator,
                width: MODELINE_PARTS_PADDING,
                fg_color: MODE_LINE_BG_COLOR,
            });
        }

        // Now compute column positions from the widths
        let mut column = 0;
        let mut columns_out = vec![];
        for part in &columns {
            columns_out.push(column);
            column += part.width;
        }

        Modeline {
            parts: columns,
            columns: columns_out,
        }
    }

    pub fn draw(
        &self,
        device: &mut impl Write,
        editor: &Editor,
        clear: bool,
    ) -> Result<(), std::io::Error> {
        // Mode line is always one up from echo area.
        queue!(
            device,
            cursor::MoveTo(0, editor.frame.rows - ECHO_AREA_HEIGHT - 1)
        )?;

        if clear {
            queue!(device, Clear(ClearType::CurrentLine))?;
        }

        // Draw the background
        let fill = " ".repeat(editor.frame.columns as usize);
        queue!(device, Print(fill.on(MODE_LINE_BG_COLOR)))?;

        let mut column = 0;
        for part in &self.parts {
            draw_modeline_part(device, editor, column, &part);
            column += part.width;
        }

        Ok(())
    }
}

fn draw_modeline_part(
    device: &mut impl Write,
    editor: &Editor,
    column: usize,
    part: &ModelinePart,
) {
    // Move cursor to column
    queue!(
        device,
        cursor::MoveTo(column as u16, editor.frame.rows - ECHO_AREA_HEIGHT - 1)
    )
    .unwrap();

    match part.part {
        ModelinePartType::Mode => {
            let mode = &editor.modes
                [editor.buffers[editor.windows[editor.active_window].active_buffer].modes[0]];
            queue!(
                device,
                Print(mode.name().with(part.fg_color).on(MODE_LINE_BG_COLOR))
            )
            .unwrap();
        }
        ModelinePartType::Object => {
            let object = editor.buffers[editor.windows[editor.active_window].active_buffer]
                .object
                .clone();
            queue!(
                device,
                Print(object.with(part.fg_color).on(MODE_LINE_BG_COLOR))
            )
            .unwrap();
        }
        ModelinePartType::LineCol => {
            let (col, line) = editor.buffers[editor.windows[editor.active_window].active_buffer]
                .to_column_line(editor.windows[editor.active_window].cursor);
            queue!(
                device,
                Print(
                    format!("{}:{}", line, col)
                        .with(part.fg_color)
                        .on(MODE_LINE_BG_COLOR)
                )
            )
            .unwrap();
        }
        ModelinePartType::MinorModes => {
            let minor_modes = editor.buffers[editor.windows[editor.active_window].active_buffer]
                .modes
                .iter()
                .skip(1)
                .map(|mode_id| {
                    let mode = &editor.modes[*mode_id];
                    mode.name()
                });
            for mode in minor_modes {
                queue!(
                    device,
                    Print(mode.with(part.fg_color).on(MODE_LINE_BG_COLOR))
                )
                .unwrap();
                queue!(
                    device,
                    Print(":".with(part.fg_color).on(MODE_LINE_BG_COLOR))
                )
                .unwrap();
            }
        }
        ModelinePartType::Separator => {
            let fill = " ".repeat(part.width);
            queue!(device, Print(fill.on(MODE_LINE_BG_COLOR))).unwrap();
        }
    }
}

/// Redraw the entire buffer in a window.
pub fn draw_window(
    device: &mut impl Write,
    editor: &Editor,
    window: &Window,
) -> Result<(), std::io::Error> {
    // Draw the buffer in the window
    let buffer = &editor.buffers[window.active_buffer];
    for (line, line_text) in buffer.buffer.lines().enumerate() {
        // Draw the chunk
        queue!(
            device,
            cursor::MoveTo(0, line as u16),
            Print(line_text.to_string().as_str())
        )?;
        if line as u16 >= editor.frame.available_lines {
            break;
        }
    }
    Ok(())
}

/// Redraw the entire screen.
pub fn draw_screen(device: &mut impl Write, editor: &Editor) -> Result<(), std::io::Error> {
    queue!(device, cursor::Hide)?;

    // Clear the screen
    queue!(device, Clear(ClearType::All))?;

    editor.modeline.draw(device, editor, false)?;

    // Draw the echo area
    // TODO

    // Draw the windows
    // We have to partition the frame into windows, then draw each window's buffer.
    // For now we only have one window that takes the whole frame's window area.
    let window = &editor.windows[editor.active_window];
    draw_window(device, editor, window)?;

    // Now move the physical cursor to the window's cursor position.
    let (col, line) = editor.buffers[window.active_buffer].to_column_line(window.cursor);
    let (x, y) = window.cursor_position(col, line);
    queue!(device, cursor::MoveTo(x, y))?;
    queue!(device, cursor::Show)?;

    // fin.
    device.flush()?;

    Ok(())
}

fn crossterm_modifier_translate(mk: &ModifierKeyCode) -> KeyModifier {
    match mk {
        ModifierKeyCode::LeftAlt => KeyModifier::Alt(Side::Left),
        ModifierKeyCode::RightAlt => KeyModifier::Alt(Side::Right),
        ModifierKeyCode::LeftControl => KeyModifier::Control(Side::Left),
        ModifierKeyCode::RightControl => KeyModifier::Control(Side::Right),
        ModifierKeyCode::LeftShift => KeyModifier::Shift(Side::Left),
        ModifierKeyCode::RightShift => KeyModifier::Shift(Side::Right),
        ModifierKeyCode::LeftSuper => KeyModifier::Super(Side::Left),
        ModifierKeyCode::RightSuper => KeyModifier::Super(Side::Right),
        ModifierKeyCode::LeftHyper => KeyModifier::Hyper(Side::Left),
        ModifierKeyCode::RightHyper => KeyModifier::Hyper(Side::Right),
        ModifierKeyCode::LeftMeta => KeyModifier::Meta(Side::Left),
        ModifierKeyCode::RightMeta => KeyModifier::Meta(Side::Right),
        ModifierKeyCode::IsoLevel3Shift => KeyModifier::Unmapped,
        ModifierKeyCode::IsoLevel5Shift => KeyModifier::Unmapped,
    }
}

fn crossterm_key_translate(ck: &KeyCode) -> LogicalKey {
    match &ck {
        KeyCode::Backspace => LogicalKey::Backspace,
        KeyCode::Enter => LogicalKey::Enter,
        KeyCode::Left => LogicalKey::Left,
        KeyCode::Right => LogicalKey::Right,
        KeyCode::Up => LogicalKey::Up,
        KeyCode::Down => LogicalKey::Down,
        KeyCode::Home => LogicalKey::Home,
        KeyCode::End => LogicalKey::End,
        KeyCode::PageUp => LogicalKey::PageUp,
        KeyCode::PageDown => LogicalKey::PageDown,
        KeyCode::Tab => LogicalKey::Tab,
        KeyCode::BackTab => LogicalKey::Unmapped,
        KeyCode::Delete => LogicalKey::Delete,
        KeyCode::Insert => LogicalKey::Insert,
        KeyCode::F(f) => LogicalKey::Function(*f),
        KeyCode::Char(c) => LogicalKey::AlphaNumeric(*c),
        KeyCode::Null => LogicalKey::Unmapped,
        KeyCode::Esc => LogicalKey::Esc,
        KeyCode::CapsLock => LogicalKey::CapsLock,
        KeyCode::ScrollLock => LogicalKey::ScrollLock,
        KeyCode::NumLock => LogicalKey::Unmapped,
        KeyCode::PrintScreen => LogicalKey::Unmapped,
        KeyCode::Pause => LogicalKey::Unmapped,
        KeyCode::Menu => LogicalKey::Unmapped,
        KeyCode::KeypadBegin => LogicalKey::Unmapped,
        KeyCode::Media(_) => LogicalKey::Unmapped,
        KeyCode::Modifier(m) => LogicalKey::Modifier(crossterm_modifier_translate(m)),
    }
}

pub fn echo(
    device: &mut impl Write,
    editor: &mut Editor,
    message: &str,
) -> Result<(), std::io::Error> {
    let (x, y) = echo_area_position(&editor.frame);

    // Stash the cursor position
    let cursor_pos = crossterm::cursor::position()?;

    queue!(device, cursor::MoveTo(x, y), Clear(ClearType::CurrentLine))?;
    queue!(
        device,
        cursor::MoveTo(x, y),
        Print(message.with(FG_COLOR).on(BG_COLOR))
    )?;
    // Restore the cursor position
    queue!(device, cursor::MoveTo(cursor_pos.0, cursor_pos.1))?;

    device.flush()?;
    Ok(())
}

pub fn event_loop(device: &mut impl Write, editor: &mut Editor) -> Result<(), std::io::Error> {
    loop {
        // Get the next event
        let event = crossterm::event::read()?;
        let keys = match event {
            Event::Key(keystroke) => {
                let key = crossterm_key_translate(&keystroke.code);

                let mut keys = vec![];

                // Modifiers first
                if keystroke.modifiers.contains(KeyModifiers::CONTROL) {
                    keys.push(LogicalKey::Modifier(KeyModifier::Control(Side::Left)));
                }
                if keystroke.modifiers.contains(KeyModifiers::ALT) {
                    keys.push(LogicalKey::Modifier(KeyModifier::Alt(Side::Left)));
                }
                if keystroke.modifiers.contains(KeyModifiers::SHIFT) {
                    keys.push(LogicalKey::Modifier(KeyModifier::Shift(Side::Left)));
                }
                if keystroke.modifiers.contains(KeyModifiers::SUPER) {
                    keys.push(LogicalKey::Modifier(KeyModifier::Super(Side::Left)));
                }

                // Then key.
                keys.push(key);
                keys
            }
            _ => vec![],
        };

        // Display the keys pressed in echo with - between, using to_display, but only if there's
        // modifiers in play
        if keys.iter().any(|k| matches!(k, LogicalKey::Modifier(_))) {
            let keys_display = keys
                .iter()
                .map(|k| k.to_display())
                .collect::<Vec<String>>()
                .join("-");
            echo(device, editor, &keys_display)?;
        }

        let actions = editor.key_event(keys)?;

        for action in actions {
            match action {
                ChromeAction::Echo(message) => {
                    echo(device, editor, &message)?;
                }

                ChromeAction::FileOpen => {
                    // TODO: open file selection mode
                    echo(device, editor, "File Open")?;
                }
                ChromeAction::CommandMode => {
                    echo(device, editor, "Command Mode")?;
                }
                ChromeAction::Huh => {}
                ChromeAction::Quit => {
                    // TODO: confirm quit
                    return Ok(());
                }
                ChromeAction::CursorMove((col, line)) => {
                    echo(device, editor, &format!("CursorMove: {}, {}", col, line))?;
                    // Modeline update
                    editor.modeline.draw(device, editor, true)?;
                    // Move the physical cursor on the screen relative to the window
                    queue!(device, cursor::MoveTo(col, line))?;
                    device.flush()?;
                }
                ChromeAction::Refresh(_) => {
                    // just redraw the screen for now
                    draw_screen(device, editor)?;
                }
            }
        }
    }
}

// Everything to run in raw_mode
fn terminal_main(stdout: &mut impl Write) -> Result<(), std::io::Error> {
    assert!(crossterm::terminal::is_raw_mode_enabled()?);
    let ws = crossterm::terminal::window_size()?;

    // Set the size of the screen
    execute!(stdout, crossterm::terminal::SetSize(ws.columns, ws.rows))?;
    assert!(crossterm::terminal::size()? != (0, 0));

    let tsize = crossterm::terminal::size()?;

    let scratch_mode = Box::new(ScratchMode {});

    let mut modes: SlotMap<ModeId, Box<dyn Mode>> = SlotMap::default();
    let scratch_mode_id = modes.insert(scratch_mode);

    let mut buffers: SlotMap<BufferId, Buffer> = SlotMap::default();
    let scratch_buffer = Buffer {
        object: "** scratch **".to_string(),
        modes: vec![scratch_mode_id],
        buffer: ropey::Rope::from_str("scratch content"),
    };
    let scratch_buffer_id = buffers.insert(scratch_buffer);
    let initial_window = Window {
        width: 100.0,
        height: 100.0,
        active_buffer: scratch_buffer_id,
        start_line: 0,
        cursor: buffers[scratch_buffer_id].buffer.len_chars(),
    };
    let mut windows: SlotMap<WindowId, Window> = SlotMap::default();
    let initial_window_id = windows.insert(initial_window);

    let mode = modes.get_mut(scratch_mode_id).unwrap();
    let modeline_modes = vec![mode.as_ref()];
    let mut modeline = Modeline::new(modeline_modes);

    let mut editor = Editor {
        frame: Frame::new(tsize.0, tsize.1),
        buffers,
        windows,
        modes,
        active_window: initial_window_id,
        key_state: KeyState::new(),
        bindings: Box::new(keys::DefaultBindings {}),
        modeline,
    };

    draw_screen(stdout, &editor)?;

    // Event loop
    event_loop(stdout, &mut editor)?;

    Ok(())
}

fn exit_state(device: &mut impl Write) -> Result<(), std::io::Error> {
    execute!(
        device,
        crossterm::terminal::Clear(crossterm::terminal::ClearType::All)
    )?;
    execute!(device, crossterm::cursor::Show)?;
    crossterm::terminal::disable_raw_mode()?;
    Ok(())
}

fn main() -> Result<(), std::io::Error> {
    let mut stdout = std::io::stdout();

    crossterm::terminal::enable_raw_mode()?;
    // Disambiguate keyboard modifier codes
    execute!(
        stdout,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    )?;
    execute!(stdout, crossterm::cursor::EnableBlinking)?;
    if let Err(e) = terminal_main(&mut stdout) {
        exit_state(&mut stdout)?;
        eprintln!("Error: {}", e);
        return Err(e);
    }

    exit_state(&mut stdout)?;

    Ok(())
}
