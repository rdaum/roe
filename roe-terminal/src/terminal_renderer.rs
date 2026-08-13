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

use compio::time::interval;
use crossterm::event::{
    Event, KeyCode, KeyModifiers, ModifierKeyCode, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::style::{Color, Print, Stylize};
use crossterm::terminal::{Clear, ClearType};
use crossterm::{cursor, queue};
use roe_core::gutter::{GutterConfig, calculate_gutter_width, format_line_number};
use roe_core::keys::{KeyModifier, LogicalKey, Side};
use roe_core::renderer::PresentationStreamState;
use roe_core::session::{
    HostSession, InputEvent, LifecycleEvent, PointerButton, PointerEvent, PointerKind,
    PresentationColor, PresentationUpdate, PresentedView, SessionOutput, StyleDefinition,
};
use std::borrow::Cow;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub const ECHO_AREA_HEIGHT: u16 = 1;
pub const BG_COLOR: Color = Color::Black;
pub const FG_COLOR: Color = Color::White;
pub const MODE_LINE_BG_COLOR: Color = Color::Blue;
pub const INACTIVE_MODE_LINE_BG_COLOR: Color = Color::DarkGrey;
pub const RUNE_COLOR: Color = Color::Yellow;
pub const BORDER_COLOR: Color = Color::DarkGrey;
pub const ACTIVE_BORDER_COLOR: Color = Color::Cyan;
// Unicode box drawing characters
pub const BORDER_HORIZONTAL: &str = "─";
pub const BORDER_VERTICAL: &str = "│";
pub const BORDER_TOP_LEFT: &str = "┌";
pub const BORDER_TOP_RIGHT: &str = "┐";
pub const BORDER_BOTTOM_LEFT: &str = "└";
pub const BORDER_BOTTOM_RIGHT: &str = "┘";
pub const _BORDER_CROSS: &str = "┼";
pub const _BORDER_T_DOWN: &str = "┬";
pub const _BORDER_T_UP: &str = "┴";
pub const BORDER_T_RIGHT: &str = "├";
pub const BORDER_T_LEFT: &str = "┤";

fn truncate_echo(message: &str, available_width: usize) -> Cow<'_, str> {
    if message.chars().count() <= available_width {
        return Cow::Borrowed(message);
    }

    if available_width <= 3 {
        return Cow::Owned(".".repeat(available_width));
    }

    let mut truncated: String = message.chars().take(available_width - 3).collect();
    truncated.push_str("...");
    Cow::Owned(truncated)
}

fn cursor_in_visible_slice(view: &PresentedView) -> (u16, u16) {
    let target = view
        .cursor
        .saturating_sub(view.visible_start_char)
        .min(view.visible_text.chars().count());
    let mut column = 0u16;
    let mut line = 0u16;
    for character in view.visible_text.chars().take(target) {
        if character == '\n' {
            line = line.saturating_add(1);
            column = 0;
        } else {
            column = column.saturating_add(1);
        }
    }
    (column, line)
}

fn session_style(
    position: usize,
    ranges: &[roe_core::session::StyledRange],
    styles: &[StyleDefinition],
    theme: &CachedTheme,
) -> (Color, Color) {
    let style_id = ranges
        .iter()
        .rev()
        .find(|range| position >= range.start && position < range.end)
        .map(|range| range.style);
    let Some(style) = style_id.and_then(|id| styles.iter().find(|style| style.id == id)) else {
        return (theme.fg_color, theme.bg_color);
    };
    (
        style
            .foreground
            .as_ref()
            .map(|color| session_color(color, theme.fg_color))
            .unwrap_or(theme.fg_color),
        style
            .background
            .as_ref()
            .map(|color| session_color(color, theme.bg_color))
            .unwrap_or(theme.bg_color),
    )
}

fn session_color(color: &PresentationColor, default: Color) -> Color {
    match color {
        PresentationColor::Rgb { r, g, b } => Color::Rgb {
            r: *r,
            g: *g,
            b: *b,
        },
        PresentationColor::Named(name) => match name.as_str() {
            "black" => Color::Black,
            "white" => Color::White,
            "red" => Color::Red,
            "green" => Color::Green,
            "blue" => Color::Blue,
            "yellow" => Color::Yellow,
            "cyan" => Color::Cyan,
            "magenta" => Color::Magenta,
            _ => default,
        },
        PresentationColor::Inherit => default,
    }
}

// Gutter colors
pub const GUTTER_BG_COLOR: Color = Color::Rgb {
    r: 20,
    g: 20,
    b: 20,
}; // Slightly darker than BG
pub const GUTTER_FG_COLOR: Color = Color::DarkGrey; // Dimmed line numbers
pub const GUTTER_SEPARATOR_COLOR: Color = Color::DarkGrey;
pub const GUTTER_MODIFIED_COLOR: Color = Color::Yellow;
pub const GUTTER_SAVED_COLOR: Color = Color::Green;
pub const GUTTER_CONFLICT_COLOR: Color = Color::Red;

/// Cached theme colors used by the terminal renderer
#[derive(Clone)]
pub struct CachedTheme {
    pub bg_color: Color,
    pub fg_color: Color,
    pub selection_color: Color,
    pub mode_line_bg_color: Color,
    pub inactive_mode_line_bg_color: Color,
    pub rune_color: Color,
    pub border_color: Color,
    pub active_border_color: Color,
}

impl Default for CachedTheme {
    fn default() -> Self {
        Self {
            bg_color: BG_COLOR,
            fg_color: FG_COLOR,
            selection_color: Color::Yellow,
            mode_line_bg_color: MODE_LINE_BG_COLOR,
            inactive_mode_line_bg_color: INACTIVE_MODE_LINE_BG_COLOR,
            rune_color: RUNE_COLOR,
            border_color: BORDER_COLOR,
            active_border_color: ACTIVE_BORDER_COLOR,
        }
    }
}

/// Terminal-specific renderer using crossterm
pub struct TerminalRenderer<W: Write> {
    device: W,
    theme: CachedTheme,
    session_presentation: PresentationStreamState,
}

impl<W: Write> TerminalRenderer<W> {
    pub fn new(device: W) -> Self {
        Self {
            device,
            theme: CachedTheme::default(),
            session_presentation: PresentationStreamState::default(),
        }
    }

    pub fn new_with_theme(device: W, theme: CachedTheme) -> Self {
        Self {
            device,
            theme,
            session_presentation: PresentationStreamState::default(),
        }
    }

    pub fn apply_session_presentation(
        &mut self,
        update: &PresentationUpdate,
    ) -> Result<(), std::io::Error> {
        self.session_presentation
            .apply(update)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    }

    pub fn session_presentation(&self) -> &PresentationStreamState {
        &self.session_presentation
    }

    /// Realize the authoritative transport-neutral presentation.
    pub fn render_session(&mut self) -> Result<(), std::io::Error> {
        let snapshot = self
            .session_presentation
            .current()
            .cloned()
            .ok_or_else(|| std::io::Error::other("session has no presentation snapshot"))?;
        queue!(&mut self.device, cursor::Hide, Clear(ClearType::All))?;

        for view in &snapshot.views {
            self.draw_session_view(view, &snapshot.styles)?;
        }
        if !snapshot.echo_area.is_empty() {
            let message = truncate_echo(&snapshot.echo_area, snapshot.columns as usize);
            queue!(
                &mut self.device,
                cursor::MoveTo(0, snapshot.rows),
                Clear(ClearType::CurrentLine),
                Print(
                    message
                        .as_ref()
                        .with(self.theme.fg_color)
                        .on(self.theme.bg_color)
                )
            )?;
        }

        if let Some(view) = snapshot.views.iter().find(|view| view.active) {
            let (column, line) = cursor_in_visible_slice(view);
            let gutter = if view.show_gutter {
                calculate_gutter_width(
                    view.visible_text.lines().count().max(1),
                    &GutterConfig::default(),
                ) as u16
            } else {
                0
            };
            let x = view
                .geometry
                .x
                .saturating_add(1)
                .saturating_add(gutter)
                .saturating_add(column.saturating_sub(view.scroll.start_column));
            let y = view.geometry.y.saturating_add(1).saturating_add(line);
            queue!(&mut self.device, cursor::MoveTo(x, y))?;
            if view.command_view {
                queue!(&mut self.device, cursor::Hide)?;
            } else {
                queue!(&mut self.device, cursor::Show)?;
            }
        }
        self.device.flush()
    }

    fn draw_session_view(
        &mut self,
        view: &PresentedView,
        styles: &[StyleDefinition],
    ) -> Result<(), std::io::Error> {
        let geometry = view.geometry;
        if geometry.columns < 2 || geometry.rows < 2 {
            return Ok(());
        }
        let border = if view.active {
            self.theme.active_border_color
        } else {
            self.theme.border_color
        };
        let right = geometry.x + geometry.columns - 1;
        let bottom = geometry.y + geometry.rows - 1;
        queue!(
            &mut self.device,
            cursor::MoveTo(geometry.x, geometry.y),
            Print(BORDER_TOP_LEFT.with(border)),
            cursor::MoveTo(right, geometry.y),
            Print(BORDER_TOP_RIGHT.with(border)),
            cursor::MoveTo(geometry.x, bottom),
            Print(BORDER_BOTTOM_LEFT.with(border)),
            cursor::MoveTo(right, bottom),
            Print(BORDER_BOTTOM_RIGHT.with(border))
        )?;
        if geometry.columns > 2 {
            queue!(
                &mut self.device,
                cursor::MoveTo(geometry.x + 1, geometry.y),
                Print(
                    BORDER_HORIZONTAL
                        .repeat((geometry.columns - 2) as usize)
                        .with(border)
                )
            )?;
        }
        for row in geometry.y + 1..bottom {
            queue!(
                &mut self.device,
                cursor::MoveTo(geometry.x, row),
                Print(BORDER_VERTICAL.with(border)),
                cursor::MoveTo(right, row),
                Print(BORDER_VERTICAL.with(border))
            )?;
        }

        let content_rows = geometry.rows.saturating_sub(2) as usize;
        let total_width = geometry.columns.saturating_sub(2) as usize;
        let visible_line_count = view.visible_text.lines().count().max(1);
        let gutter_width = if view.show_gutter {
            calculate_gutter_width(visible_line_count, &GutterConfig::default())
        } else {
            0
        };
        let text_width = total_width.saturating_sub(gutter_width);
        let mut absolute = view.visible_start_char;
        let lines: Vec<&str> = view.visible_text.split_inclusive('\n').collect();
        for row in 0..content_rows {
            let y = geometry.y + 1 + row as u16;
            queue!(
                &mut self.device,
                cursor::MoveTo(geometry.x + 1, y),
                Print(
                    " ".repeat(total_width)
                        .with(self.theme.fg_color)
                        .on(self.theme.bg_color)
                )
            )?;
            let line = lines.get(row).copied().unwrap_or("");
            let line = line.trim_end_matches('\n');
            if view.show_gutter {
                let number = usize::from(view.scroll.start_line) + row + 1;
                let digits = gutter_width.saturating_sub(2);
                let gutter = format!(" {}│", format_line_number(number, digits));
                queue!(
                    &mut self.device,
                    cursor::MoveTo(geometry.x + 1, y),
                    Print(gutter.with(GUTTER_FG_COLOR).on(GUTTER_BG_COLOR))
                )?;
            }
            queue!(
                &mut self.device,
                cursor::MoveTo(geometry.x + 1 + gutter_width as u16, y)
            )?;
            for (offset, character) in line
                .chars()
                .skip(usize::from(view.scroll.start_column))
                .take(text_width)
                .enumerate()
            {
                let position = absolute + usize::from(view.scroll.start_column) + offset;
                let selected = view.selection.is_some_and(|selection| {
                    let start = selection.anchor.min(selection.active);
                    let end = selection.anchor.max(selection.active);
                    position >= start && position < end
                });
                let (foreground, background) = if selected {
                    (Color::Black, self.theme.selection_color)
                } else {
                    session_style(position, &view.styled_ranges, styles, &self.theme)
                };
                queue!(
                    &mut self.device,
                    Print(character.to_string().with(foreground).on(background))
                )?;
            }
            absolute += line.chars().count()
                + usize::from(lines.get(row).is_some_and(|l| l.ends_with('\n')));
        }

        let modeline_bg = if view.active {
            self.theme.mode_line_bg_color
        } else {
            self.theme.inactive_mode_line_bg_color
        };
        let modeline = truncate_echo(&view.modeline, total_width);
        let padded = format!("{:<width$}", modeline, width = total_width);
        queue!(
            &mut self.device,
            cursor::MoveTo(geometry.x + 1, bottom),
            Print(padded.with(self.theme.fg_color).on(modeline_bg))
        )?;
        Ok(())
    }
}

fn crossterm_modifier_translate(modifier: &ModifierKeyCode) -> KeyModifier {
    match modifier {
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
        ModifierKeyCode::IsoLevel3Shift | ModifierKeyCode::IsoLevel5Shift => KeyModifier::Unmapped,
    }
}

fn crossterm_key_translate(key: &KeyCode, modifiers: KeyModifiers) -> LogicalKey {
    match key {
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
        KeyCode::Delete => LogicalKey::Delete,
        KeyCode::Insert => LogicalKey::Insert,
        KeyCode::F(number) => LogicalKey::Function(*number),
        KeyCode::Char(character) if modifiers.contains(KeyModifiers::CONTROL) => match character {
            '\x1f' => LogicalKey::AlphaNumeric('/'),
            '\x00' => LogicalKey::AlphaNumeric(' '),
            character => LogicalKey::AlphaNumeric(*character),
        },
        KeyCode::Char(character) => LogicalKey::AlphaNumeric(*character),
        KeyCode::Esc => LogicalKey::Esc,
        KeyCode::CapsLock => LogicalKey::CapsLock,
        KeyCode::ScrollLock => LogicalKey::ScrollLock,
        KeyCode::Modifier(modifier) => LogicalKey::Modifier(crossterm_modifier_translate(modifier)),
        _ => LogicalKey::Unmapped,
    }
}

pub async fn session_event_loop_with_renderer<W: Write>(
    renderer: &mut TerminalRenderer<W>,
    session: &mut HostSession,
    shutdown_requested: &AtomicBool,
) -> Result<(), std::io::Error> {
    let mut event_tick = interval(Duration::from_millis(20));

    loop {
        event_tick.tick().await;
        let tick = session.envelope(InputEvent::Timer { token: 0 });
        let output = session.dispatch(tick).await.map_err(session_io_error)?;
        if apply_session_output(renderer, output)? {
            return Ok(());
        }

        if shutdown_requested.load(Ordering::Acquire) {
            tracing::info!("terminal shutdown requested");
            return Ok(());
        }
        if !crossterm::event::poll(Duration::ZERO)? {
            continue;
        }

        let Some(input) = normalize_terminal_event(crossterm::event::read()?) else {
            continue;
        };
        let envelope = session.envelope(input);
        let output = session.dispatch(envelope).await.map_err(session_io_error)?;
        if apply_session_output(renderer, output)? {
            return Ok(());
        }
    }
}

fn normalize_terminal_event(event: Event) -> Option<InputEvent> {
    match event {
        Event::Key(keystroke) => {
            let mut keys = Vec::new();
            if keystroke.modifiers.contains(KeyModifiers::CONTROL) {
                keys.push(LogicalKey::Modifier(KeyModifier::Control(Side::Left)));
            }
            if keystroke.modifiers.contains(KeyModifiers::ALT) {
                keys.push(LogicalKey::Modifier(KeyModifier::Meta(Side::Left)));
            }
            if keystroke.modifiers.contains(KeyModifiers::SHIFT) {
                keys.push(LogicalKey::Modifier(KeyModifier::Shift(Side::Left)));
            }
            if keystroke.modifiers.contains(KeyModifiers::SUPER) {
                keys.push(LogicalKey::Modifier(KeyModifier::Super(Side::Left)));
            }
            keys.push(crossterm_key_translate(
                &keystroke.code,
                keystroke.modifiers,
            ));
            Some(InputEvent::Keys(keys))
        }
        Event::Resize(columns, rows) => Some(InputEvent::Resize {
            columns,
            rows: rows.saturating_sub(ECHO_AREA_HEIGHT),
        }),
        Event::Mouse(mouse) => normalize_terminal_pointer(mouse),
        Event::FocusGained => Some(InputEvent::Focus(true)),
        Event::FocusLost => Some(InputEvent::Focus(false)),
        Event::Paste(text) => Some(InputEvent::Text(text)),
    }
}

fn normalize_terminal_pointer(mouse: MouseEvent) -> Option<InputEvent> {
    let (kind, button) = match mouse.kind {
        MouseEventKind::Down(button) => (PointerKind::Down, pointer_button(button)),
        MouseEventKind::Drag(button) => (PointerKind::Move, pointer_button(button)),
        MouseEventKind::Up(button) => (PointerKind::Up, pointer_button(button)),
        MouseEventKind::Moved => (PointerKind::Move, PointerButton::None),
        MouseEventKind::ScrollDown
        | MouseEventKind::ScrollUp
        | MouseEventKind::ScrollLeft
        | MouseEventKind::ScrollRight => return None,
    };
    Some(InputEvent::Pointer(PointerEvent {
        column: mouse.column,
        row: mouse.row,
        kind,
        button,
    }))
}

fn pointer_button(button: MouseButton) -> PointerButton {
    match button {
        MouseButton::Left => PointerButton::Primary,
        MouseButton::Right => PointerButton::Secondary,
        MouseButton::Middle => PointerButton::Middle,
    }
}

fn apply_session_output<W: Write>(
    renderer: &mut TerminalRenderer<W>,
    output: SessionOutput,
) -> Result<bool, std::io::Error> {
    let quit = output.lifecycle.iter().any(|event| {
        matches!(
            event,
            LifecycleEvent::QuitRequested
                | LifecycleEvent::EndpointClosed
                | LifecycleEvent::Fatal(_)
        )
    });
    for event in &output.lifecycle {
        match event {
            LifecycleEvent::Warning(message) => tracing::warn!(%message, "session warning"),
            LifecycleEvent::Error(message) => tracing::error!(%message, "session error"),
            LifecycleEvent::Fatal(message) => tracing::error!(%message, "fatal session error"),
            LifecycleEvent::Overloaded { detail } => {
                tracing::warn!(%detail, "session overload")
            }
            _ => {}
        }
    }
    if let Some(update) = output.presentation.as_ref() {
        renderer.apply_session_presentation(update)?;
        renderer.render_session()?;
    }
    Ok(quit)
}

fn session_io_error(error: roe_core::session::SessionError) -> std::io::Error {
    std::io::Error::other(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_terminal_renderer_creation() {
        let output = Vec::new();
        let renderer = TerminalRenderer::new(output);
        assert!(renderer.session_presentation().current().is_none());
    }

    #[test]
    fn unicode_echo_truncation_preserves_character_boundaries() {
        assert_eq!(truncate_echo("λé猫abc", 5), "λé...");
        assert_eq!(truncate_echo("λé", 5), "λé");
        assert_eq!(truncate_echo("λé", 1), ".");
    }
}
