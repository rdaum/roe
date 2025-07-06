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
use slotmap::{new_key_type, SlotMap};
use std::io::Write;

mod buffer;
mod editor;
mod keys;
mod kill_ring;
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
pub const BORDER_COLOR: Color = Color::DarkGrey;
pub const ACTIVE_BORDER_COLOR: Color = Color::Cyan;

// Unicode box drawing characters
pub const BORDER_HORIZONTAL: &str = "─";
pub const BORDER_VERTICAL: &str = "│";
pub const BORDER_TOP_LEFT: &str = "┌";
pub const BORDER_TOP_RIGHT: &str = "┐";
pub const BORDER_BOTTOM_LEFT: &str = "└";
pub const BORDER_BOTTOM_RIGHT: &str = "┘";
pub const BORDER_CROSS: &str = "┼";
pub const BORDER_T_DOWN: &str = "┬";
pub const BORDER_T_UP: &str = "┴";
pub const BORDER_T_RIGHT: &str = "├";
pub const BORDER_T_LEFT: &str = "┤";

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
}

impl Modeline {
    /// Build all the parts of the modeline.
    #[allow(dead_code, clippy::vec_init_then_push)]
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

        Modeline {
            parts: columns,
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
            draw_modeline_part(device, editor, column, part);
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
                    format!("{line}:{col}")
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

/// Draw borders around all windows in a more sophisticated way that handles adjacency
pub fn draw_all_window_borders(
    device: &mut impl Write,
    editor: &Editor,
) -> Result<(), std::io::Error> {
    // Create a grid to track what's already drawn to avoid conflicts
    let mut border_grid = vec![vec![' '; editor.frame.available_columns as usize]; editor.frame.available_lines as usize];
    
    // First pass: mark all window areas and determine border positions
    for window_id in editor.windows.keys() {
        let window = &editor.windows[window_id];
        let is_active = window_id == editor.active_window;
        let border_char = if is_active { 'A' } else { 'I' }; // Active/Inactive marker
        
        if window.width_chars < 2 || window.height_chars < 2 {
            continue;
        }
        
        let right = window.x + window.width_chars - 1;
        let bottom = window.y + window.height_chars - 1;
        
        // Mark corners
        if window.x < editor.frame.available_columns && window.y < editor.frame.available_lines {
            border_grid[window.y as usize][window.x as usize] = border_char;
        }
        if right < editor.frame.available_columns && window.y < editor.frame.available_lines {
            border_grid[window.y as usize][right as usize] = border_char;
        }
        if window.x < editor.frame.available_columns && bottom < editor.frame.available_lines {
            border_grid[bottom as usize][window.x as usize] = border_char;
        }
        if right < editor.frame.available_columns && bottom < editor.frame.available_lines {
            border_grid[bottom as usize][right as usize] = border_char;
        }
        
        // Mark horizontal borders
        for x in window.x + 1..right {
            if x < editor.frame.available_columns {
                if window.y < editor.frame.available_lines {
                    border_grid[window.y as usize][x as usize] = border_char;
                }
                if bottom < editor.frame.available_lines {
                    border_grid[bottom as usize][x as usize] = border_char;
                }
            }
        }
        
        // Mark vertical borders
        for y in window.y + 1..bottom {
            if y < editor.frame.available_lines {
                if window.x < editor.frame.available_columns {
                    border_grid[y as usize][window.x as usize] = border_char;
                }
                if right < editor.frame.available_columns {
                    border_grid[y as usize][right as usize] = border_char;
                }
            }
        }
    }
    
    // Second pass: actually draw the borders
    for window_id in editor.windows.keys() {
        draw_single_window_border(device, editor, window_id, &border_grid)?;
    }
    
    Ok(())
}

/// Draw borders for a single window
fn draw_single_window_border(
    device: &mut impl Write,
    editor: &Editor,
    window_id: WindowId,
    _border_grid: &[Vec<char>],
) -> Result<(), std::io::Error> {
    let window = &editor.windows[window_id];
    let is_active = window_id == editor.active_window;
    let border_color = if is_active { ACTIVE_BORDER_COLOR } else { BORDER_COLOR };
    
    // Only draw borders if the window has space for them
    if window.width_chars < 2 || window.height_chars < 2 {
        return Ok(());
    }
    
    let right = window.x + window.width_chars - 1;
    let bottom = window.y + window.height_chars - 1;
    let modeline_row = bottom - 1; // Modeline is one row above the bottom border
    
    // Draw corners
    queue!(device, cursor::MoveTo(window.x, window.y), Print(BORDER_TOP_LEFT.with(border_color)))?;
    queue!(device, cursor::MoveTo(right, window.y), Print(BORDER_TOP_RIGHT.with(border_color)))?;
    queue!(device, cursor::MoveTo(window.x, bottom), Print(BORDER_BOTTOM_LEFT.with(border_color)))?;
    queue!(device, cursor::MoveTo(right, bottom), Print(BORDER_BOTTOM_RIGHT.with(border_color)))?;
    
    // Draw top horizontal border
    if window.x + 1 < right {
        let horizontal_line = BORDER_HORIZONTAL.repeat((right - window.x - 1) as usize);
        queue!(device, cursor::MoveTo(window.x + 1, window.y), Print(horizontal_line.with(border_color)))?;
    }
    
    // Draw modeline separator (horizontal line above bottom border)
    if window.x + 1 < right {
        queue!(device, cursor::MoveTo(window.x, modeline_row), Print(BORDER_T_RIGHT.with(border_color)))?;
        let horizontal_line = BORDER_HORIZONTAL.repeat((right - window.x - 1) as usize);
        queue!(device, cursor::MoveTo(window.x + 1, modeline_row), Print(horizontal_line.with(border_color)))?;
        queue!(device, cursor::MoveTo(right, modeline_row), Print(BORDER_T_LEFT.with(border_color)))?;
    }
    
    // Draw bottom horizontal border
    if window.x + 1 < right {
        let horizontal_line = BORDER_HORIZONTAL.repeat((right - window.x - 1) as usize);
        queue!(device, cursor::MoveTo(window.x + 1, bottom), Print(horizontal_line.with(border_color)))?;
    }
    
    // Draw vertical borders (excluding modeline row)
    for y in window.y + 1..modeline_row {
        queue!(device, cursor::MoveTo(window.x, y), Print(BORDER_VERTICAL.with(border_color)))?;
        queue!(device, cursor::MoveTo(right, y), Print(BORDER_VERTICAL.with(border_color)))?;
    }
    
    // Draw vertical borders for modeline row to bottom
    for y in (modeline_row + 1)..bottom {
        queue!(device, cursor::MoveTo(window.x, y), Print(BORDER_VERTICAL.with(border_color)))?;
        queue!(device, cursor::MoveTo(right, y), Print(BORDER_VERTICAL.with(border_color)))?;
    }
    
    // Draw the actual modeline content
    draw_window_modeline(device, editor, window_id)?;
    
    Ok(())
}

/// Draw the modeline for a specific window
fn draw_window_modeline(
    device: &mut impl Write,
    editor: &Editor,
    window_id: WindowId,
) -> Result<(), std::io::Error> {
    let window = &editor.windows[window_id];
    let buffer = &editor.buffers[window.active_buffer];
    
    // Calculate modeline position and width
    let modeline_y = window.y + window.height_chars - 2; // One row above bottom border
    let modeline_x = window.x + 1; // Inside left border
    let modeline_width = window.width_chars.saturating_sub(2) as usize; // Inside both borders
    
    if modeline_width == 0 {
        return Ok(());
    }
    
    // Move to modeline position
    queue!(device, cursor::MoveTo(modeline_x, modeline_y))?;
    
    // Create modeline content
    let mut modeline_content = String::new();
    
    // Add buffer object name
    let object_part = format!(" {} ", buffer.object);
    modeline_content.push_str(&object_part);
    
    // Add mode name
    if let Some(mode_id) = buffer.modes.first() {
        if let Some(mode) = editor.modes.get(*mode_id) {
            let mode_part = format!("({}) ", mode.name());
            modeline_content.push_str(&mode_part);
        }
    }
    
    // Add cursor position
    let (col, line) = buffer.to_column_line(window.cursor);
    let position_part = format!("{}:{} ", line + 1, col + 1); // 1-based for display
    
    // Calculate remaining space for position (right-aligned)
    let used_space = modeline_content.len() + position_part.len();
    let remaining_space = modeline_width.saturating_sub(used_space);
    
    // Fill with spaces to right-align position
    modeline_content.push_str(&" ".repeat(remaining_space));
    modeline_content.push_str(&position_part);
    
    // Truncate if too long
    if modeline_content.len() > modeline_width {
        modeline_content.truncate(modeline_width);
    } else if modeline_content.len() < modeline_width {
        // Pad with spaces to fill the entire modeline
        modeline_content.push_str(&" ".repeat(modeline_width - modeline_content.len()));
    }
    
    // Draw with modeline colors
    queue!(device, Print(modeline_content.on(MODE_LINE_BG_COLOR).with(FG_COLOR)))?;
    
    Ok(())
}

/// Redraw the entire buffer in a window.
pub fn draw_window(
    device: &mut impl Write,
    editor: &Editor,
    window: &Window,
) -> Result<(), std::io::Error> {
    // Draw the buffer in the window
    let buffer = &editor.buffers[window.active_buffer];
    
    // Calculate content area (inside the border, above the modeline)
    let content_x = window.x + 1;
    let content_y = window.y + 1;
    let content_width = window.width_chars.saturating_sub(2);
    let content_height = window.height_chars.saturating_sub(3); // Reserve space for modeline
    
    // Clear the content area first (only the content, not the whole line)
    for row in 0..content_height {
        let spaces = " ".repeat(content_width as usize);
        queue!(
            device,
            cursor::MoveTo(content_x, content_y + row),
            Print(spaces)
        )?;
    }
    
    // Draw the buffer content within the content bounds
    for (line_idx, line_text) in buffer.buffer.lines().enumerate() {
        let screen_line = line_idx as u16;
        
        // Skip lines that are scrolled out of view
        if screen_line < window.start_line {
            continue;
        }
        
        let content_line = screen_line - window.start_line;
        
        // Stop if we've reached the bottom of the content area
        if content_line >= content_height {
            break;
        }
        
        // Truncate line if it's too long for the content area
        let line_str = line_text.to_string();
        let truncated_line = if line_str.len() > content_width as usize {
            &line_str[..content_width as usize]
        } else {
            &line_str
        };
        
        // Draw the line at the correct position within the content area
        queue!(
            device,
            cursor::MoveTo(content_x, content_y + content_line),
            Print(truncated_line)
        )?;
    }
    
    Ok(())
}

/// Redraw the entire screen.
pub fn draw_screen(device: &mut impl Write, editor: &Editor) -> Result<(), std::io::Error> {
    queue!(device, cursor::Hide)?;

    // Clear the screen
    queue!(device, Clear(ClearType::All))?;

    // Draw the echo area
    // TODO

    // Draw all windows and their borders (including per-window modelines)
    for window_id in editor.windows.keys() {
        let window = &editor.windows[window_id];
        draw_window(device, editor, window)?;
    }
    
    // Draw all borders with modelines in a coordinated way
    draw_all_window_borders(device, editor)?;

    // Now move the physical cursor to the active window's cursor position.
    let active_window = &editor.windows[editor.active_window];
    let (col, line) = editor.buffers[active_window.active_buffer].to_column_line(active_window.cursor);
    let (x, y) = active_window.absolute_cursor_position(col, line);
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

        // Display the keys pressed in echo with - between, using as_display_string, but only if there's
        // modifiers in play
        if keys.iter().any(|k| matches!(k, LogicalKey::Modifier(_))) {
            let keys_display = keys
                .iter()
                .map(|k| k.as_display_string())
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
                    echo(device, editor, &format!("CursorMove: {col}, {line}"))?;
                    // Move the physical cursor on the screen relative to the window
                    queue!(device, cursor::MoveTo(col, line))?;
                    device.flush()?;
                }
                ChromeAction::Refresh(_) => {
                    // just redraw the screen for now
                    draw_screen(device, editor)?;
                }
                ChromeAction::SplitHorizontal => {
                    editor.split_horizontal();
                    draw_screen(device, editor)?;
                    echo(device, editor, "Window split horizontally")?;
                }
                ChromeAction::SplitVertical => {
                    editor.split_vertical();
                    draw_screen(device, editor)?;
                    echo(device, editor, "Window split vertically")?;
                }
                ChromeAction::SwitchWindow => {
                    editor.switch_window();
                    draw_screen(device, editor)?;
                    echo(device, editor, "Switched to next window")?;
                }
                ChromeAction::DeleteWindow => {
                    if editor.delete_window() {
                        draw_screen(device, editor)?;
                        echo(device, editor, "Window deleted")?;
                    } else {
                        echo(device, editor, "Cannot delete only window")?;
                    }
                }
                ChromeAction::DeleteOtherWindows => {
                    if editor.delete_other_windows() {
                        draw_screen(device, editor)?;
                        echo(device, editor, "Deleted other windows")?;
                    } else {
                        echo(device, editor, "Only one window")?;
                    }
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
        mark: None,
    };
    let scratch_buffer_id = buffers.insert(scratch_buffer);
    let initial_window = Window {
        x: 0,
        y: 0,
        width_chars: tsize.0,
        height_chars: tsize.1 - ECHO_AREA_HEIGHT,
        active_buffer: scratch_buffer_id,
        start_line: 0,
        cursor: buffers[scratch_buffer_id].buffer.len_chars(),
    };
    let mut windows: SlotMap<WindowId, Window> = SlotMap::default();
    let initial_window_id = windows.insert(initial_window);

    let mut editor = Editor {
        frame: Frame::new(tsize.0, tsize.1),
        buffers,
        windows,
        modes,
        active_window: initial_window_id,
        key_state: KeyState::new(),
        bindings: Box::new(keys::DefaultBindings {}),
        window_tree: editor::WindowNode::new_leaf(initial_window_id),
        kill_ring: kill_ring::KillRing::new(),
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
        eprintln!("Error: {e}");
        return Err(e);
    }

    exit_state(&mut stdout)?;

    Ok(())
}
