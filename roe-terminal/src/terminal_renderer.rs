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

use crate::text_cells::{CellRow, fitted, start_for_cursor};
use compio::time::interval;
use crossterm::event::{
    Event, KeyCode, KeyModifiers, ModifierKeyCode, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::style::{Color, Print, Stylize};
use crossterm::terminal::{Clear, ClearType};
use crossterm::{cursor, queue};
use roe_core::frontend::{LocalFrontendServices, PresentationConsumer};
use roe_core::gutter::{GutterConfig, calculate_gutter_width, format_line_number};
use roe_core::keys::{KeyModifier, LogicalKey, Side};
use roe_core::renderer::PresentationStreamState;
use roe_core::session::{
    DirectSessionClient, InputEvent, PointerButton, PointerEvent, PointerKind, PointerTextHit,
    PresentationColor, PresentationSnapshot, PresentationUpdate, PresentedView, SessionClient,
    SessionOutput, StyleDefinition,
};
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

fn gutter_width(view: &PresentedView) -> usize {
    if !view.show_gutter {
        return 0;
    }
    calculate_gutter_width(view.total_lines, &GutterConfig::default())
        .min(usize::from(view.geometry.columns.saturating_sub(2)))
}

fn text_width(view: &PresentedView) -> usize {
    usize::from(view.geometry.columns.saturating_sub(2)).saturating_sub(gutter_width(view))
}

fn realized_start_column(view: &PresentedView) -> usize {
    if !view.active {
        return view.scroll.start_column;
    }
    let (column, row) = cursor_in_visible_slice(view);
    start_for_cursor(
        session_view_line(view, row).trim_end_matches('\n'),
        view.scroll.start_column,
        text_width(view),
        column,
    )
}

fn cursor_in_visible_slice(view: &PresentedView) -> (usize, usize) {
    let target = view
        .cursor
        .saturating_sub(view.visible_start_char)
        .min(view.visible_text.chars().count());
    let mut column = 0usize;
    let mut line = 0usize;
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
    default_foreground: Color,
    default_background: Color,
) -> (Color, Color) {
    let style_id = ranges
        .iter()
        .rev()
        .find(|range| position >= range.start && position < range.end)
        .map(|range| range.style);
    let Some(style) = style_id.and_then(|id| styles.iter().find(|style| style.id == id)) else {
        return (default_foreground, default_background);
    };
    (
        style
            .foreground
            .as_ref()
            .map(|color| session_color(color, default_foreground))
            .unwrap_or(default_foreground),
        style
            .background
            .as_ref()
            .map(|color| session_color(color, default_background))
            .unwrap_or(default_background),
    )
}

fn session_line_style(
    line: usize,
    view: &PresentedView,
    styles: &[StyleDefinition],
    theme: &CachedTheme,
) -> (Color, Color) {
    let style = view
        .styled_lines
        .iter()
        .rev()
        .find(|styled| styled.line == line)
        .and_then(|styled| styles.iter().find(|style| style.id == styled.style));
    let Some(style) = style else {
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
    rendered_presentation: Option<PresentationSnapshot>,
    force_full_render: bool,
    force_clear_render: bool,
}

impl<W: Write> TerminalRenderer<W> {
    pub fn new(device: W) -> Self {
        Self {
            device,
            theme: CachedTheme::default(),
            session_presentation: PresentationStreamState::default(),
            rendered_presentation: None,
            force_full_render: false,
            force_clear_render: false,
        }
    }

    pub fn new_with_theme(device: W, theme: CachedTheme) -> Self {
        Self {
            device,
            theme,
            session_presentation: PresentationStreamState::default(),
            rendered_presentation: None,
            force_full_render: false,
            force_clear_render: false,
        }
    }

    pub fn apply_session_presentation(
        &mut self,
        update: &PresentationUpdate,
    ) -> Result<(), std::io::Error> {
        self.accept_presentation(update)
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
        let previous = self.rendered_presentation.take();
        let layout_changed = previous
            .as_ref()
            .is_some_and(|previous| !session_layout_matches(previous, &snapshot));
        let result = if self.force_clear_render || previous.is_none() || layout_changed {
            self.render_full_session(&snapshot)
        } else if self.force_full_render {
            self.render_complete_session(&snapshot)
        } else if let Some(previous) = previous.as_ref() {
            self.render_incremental_session(previous, &snapshot)
        } else {
            self.render_full_session(&snapshot)
        };
        if result.is_ok() {
            self.rendered_presentation = Some(snapshot);
            self.force_full_render = false;
            self.force_clear_render = false;
        } else {
            self.force_full_render = true;
            self.force_clear_render = true;
        }
        result
    }

    fn render_full_session(
        &mut self,
        snapshot: &PresentationSnapshot,
    ) -> Result<(), std::io::Error> {
        queue!(&mut self.device, cursor::Hide, Clear(ClearType::All))?;
        self.render_complete_session(snapshot)
    }

    fn render_complete_session(
        &mut self,
        snapshot: &PresentationSnapshot,
    ) -> Result<(), std::io::Error> {
        queue!(&mut self.device, cursor::Hide)?;
        for view in &snapshot.views {
            self.draw_session_view(view, &snapshot.styles)?;
        }
        self.draw_echo_area(snapshot)?;
        self.position_session_cursor(snapshot)?;
        self.device.flush()
    }

    fn render_incremental_session(
        &mut self,
        previous: &PresentationSnapshot,
        snapshot: &PresentationSnapshot,
    ) -> Result<(), std::io::Error> {
        let styles_changed = previous.styles != snapshot.styles;
        let mut drew = false;
        for view in &snapshot.views {
            let old = previous
                .views
                .iter()
                .find(|candidate| candidate.id == view.id)
                .expect("matching session layout must contain every view");

            if old.active != view.active {
                self.hide_cursor_once(&mut drew)?;
                self.draw_session_view_border(view)?;
            }

            let redraw_all_content = styles_changed
                || old.resource != view.resource
                || old.visible_start_char != view.visible_start_char
                || old.scroll != view.scroll
                || realized_start_column(old) != realized_start_column(view)
                || old.selection != view.selection
                || old.styled_ranges != view.styled_ranges
                || old.styled_lines != view.styled_lines
                || old.typeout != view.typeout
                || old.show_gutter != view.show_gutter
                || old.visible_text.lines().count() != view.visible_text.lines().count()
                || (view.typeout.is_some() && old.visible_text != view.visible_text);
            if redraw_all_content {
                self.hide_cursor_once(&mut drew)?;
                self.draw_session_view_content(view, &snapshot.styles)?;
                self.draw_session_typeout(view)?;
            } else if old.visible_text != view.visible_text {
                for row in 0..usize::from(view.geometry.rows.saturating_sub(2)) {
                    if session_view_line(old, row) != session_view_line(view, row) {
                        self.hide_cursor_once(&mut drew)?;
                        self.draw_session_view_content_row(view, &snapshot.styles, row)?;
                    }
                }
            }

            if old.modeline != view.modeline || old.active != view.active {
                self.hide_cursor_once(&mut drew)?;
                self.draw_session_view_modeline(view)?;
            }
        }

        if previous.echo_area != snapshot.echo_area || previous.rows != snapshot.rows {
            self.hide_cursor_once(&mut drew)?;
            self.draw_echo_area(snapshot)?;
        }
        self.position_session_cursor(snapshot)?;
        self.device.flush()
    }

    fn hide_cursor_once(&mut self, drew: &mut bool) -> Result<(), std::io::Error> {
        if !*drew {
            queue!(&mut self.device, cursor::Hide)?;
            *drew = true;
        }
        Ok(())
    }

    fn pointer_text_hit(&self, pointer: &PointerEvent) -> Option<PointerTextHit> {
        let snapshot = self.session_presentation.current()?;
        let dragging =
            pointer.kind == PointerKind::Move && pointer.button == PointerButton::Primary;
        let view = snapshot.views.iter().find(|view| {
            if dragging {
                return view.active;
            }
            let geometry = view.geometry;
            pointer.column > geometry.x
                && pointer.column
                    < geometry
                        .x
                        .saturating_add(geometry.columns.saturating_sub(1))
                && pointer.row > geometry.y
                && pointer.row < geometry.y.saturating_add(geometry.rows.saturating_sub(1))
        })?;
        if view.command_view || view.typeout.is_some() {
            return None;
        }
        let row = usize::from(
            pointer
                .row
                .saturating_sub(view.geometry.y.saturating_add(1)),
        )
        .min(usize::from(view.geometry.rows.saturating_sub(3)));
        let cell = usize::from(
            pointer
                .column
                .saturating_sub(view.geometry.x.saturating_add(1)),
        )
        .saturating_sub(gutter_width(view));
        let line = session_view_line(view, row).trim_end_matches('\n');
        let cells = CellRow::new(line, realized_start_column(view), text_width(view));
        let column = cells.character_at_cell(cell).min(line.chars().count());
        let preceding: usize = view
            .visible_text
            .split_inclusive('\n')
            .take(row)
            .map(|line| line.chars().count())
            .sum();
        Some(PointerTextHit {
            view: view.id,
            resource: view.resource,
            text_revision: view.text_revision,
            position: view
                .visible_start_char
                .saturating_add(preceding)
                .saturating_add(column)
                .min(view.visible_end_char),
        })
    }

    fn draw_echo_area(&mut self, snapshot: &PresentationSnapshot) -> Result<(), std::io::Error> {
        let message = fitted(&snapshot.echo_area, usize::from(snapshot.columns), ' ');
        queue!(
            &mut self.device,
            cursor::MoveTo(0, snapshot.rows),
            Print(message.with(self.theme.fg_color).on(self.theme.bg_color))
        )
    }

    fn position_session_cursor(
        &mut self,
        snapshot: &PresentationSnapshot,
    ) -> Result<(), std::io::Error> {
        let Some(view) = snapshot.views.iter().find(|view| view.active) else {
            return Ok(());
        };
        let (column, line) = cursor_in_visible_slice(view);
        let gutter = gutter_width(view);
        let cells = CellRow::new(
            session_view_line(view, line).trim_end_matches('\n'),
            realized_start_column(view),
            text_width(view),
        );
        let Some(column) = cells
            .cell_at_character(column)
            .filter(|column| *column < text_width(view))
        else {
            return queue!(&mut self.device, cursor::Hide);
        };
        if line >= usize::from(view.geometry.rows.saturating_sub(2)) {
            return queue!(&mut self.device, cursor::Hide);
        }
        let x = view
            .geometry
            .x
            .saturating_add(1)
            .saturating_add(gutter as u16)
            .saturating_add(column as u16);
        let y = view
            .geometry
            .y
            .saturating_add(1)
            .saturating_add(line as u16);
        queue!(&mut self.device, cursor::MoveTo(x, y))?;
        if view.command_view || view.typeout.is_some() {
            queue!(&mut self.device, cursor::Hide)
        } else {
            queue!(&mut self.device, cursor::Show)
        }
    }

    fn draw_session_view(
        &mut self,
        view: &PresentedView,
        styles: &[StyleDefinition],
    ) -> Result<(), std::io::Error> {
        self.draw_session_view_border(view)?;
        self.draw_session_view_content(view, styles)?;
        self.draw_session_typeout(view)?;
        self.draw_session_view_modeline(view)
    }

    fn draw_session_view_border(&mut self, view: &PresentedView) -> Result<(), std::io::Error> {
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
        Ok(())
    }

    fn draw_session_view_content(
        &mut self,
        view: &PresentedView,
        styles: &[StyleDefinition],
    ) -> Result<(), std::io::Error> {
        for row in 0..usize::from(view.geometry.rows.saturating_sub(2)) {
            self.draw_session_view_content_row(view, styles, row)?;
        }
        Ok(())
    }

    fn draw_session_view_content_row(
        &mut self,
        view: &PresentedView,
        styles: &[StyleDefinition],
        row: usize,
    ) -> Result<(), std::io::Error> {
        let geometry = view.geometry;
        if geometry.columns < 2 || row >= usize::from(geometry.rows.saturating_sub(2)) {
            return Ok(());
        }
        let total_width = geometry.columns.saturating_sub(2) as usize;
        let gutter_width = gutter_width(view);
        let text_width = total_width.saturating_sub(gutter_width);
        let lines: Vec<&str> = view.visible_text.split_inclusive('\n').collect();
        let absolute = view.visible_start_char
            + lines.iter().take(row).fold(0usize, |offset, line| {
                offset.saturating_add(line.chars().count())
            });
        let y = geometry.y + 1 + row as u16;
        let logical_line = view.scroll.start_line + row;
        let (line_foreground, line_background) =
            session_line_style(logical_line, view, styles, &self.theme);
        queue!(
            &mut self.device,
            cursor::MoveTo(geometry.x + 1, y),
            Print(
                " ".repeat(total_width)
                    .with(line_foreground)
                    .on(line_background)
            )
        )?;
        let line = lines.get(row).copied().unwrap_or("").trim_end_matches('\n');
        if view.show_gutter {
            let number = view.scroll.start_line + row + 1;
            let digits = gutter_width.saturating_sub(2);
            let gutter = format!(" {}│", format_line_number(number, digits));
            queue!(
                &mut self.device,
                cursor::MoveTo(geometry.x + 1, y),
                Print(
                    fitted(&gutter, gutter_width, ' ')
                        .with(GUTTER_FG_COLOR)
                        .on(GUTTER_BG_COLOR)
                )
            )?;
        }
        queue!(
            &mut self.device,
            cursor::MoveTo(geometry.x + 1 + gutter_width as u16, y)
        )?;
        let cells = CellRow::new(line, realized_start_column(view), text_width);
        for glyph in cells.glyphs {
            let position = absolute + glyph.characters.start;
            let selected = view.selection.is_some_and(|selection| {
                let start = selection.anchor.min(selection.active);
                let end = selection.anchor.max(selection.active);
                position < end && absolute + glyph.characters.end > start
            });
            let (foreground, background) = if selected {
                (Color::Black, self.theme.selection_color)
            } else {
                session_style(
                    position,
                    &view.styled_ranges,
                    styles,
                    line_foreground,
                    line_background,
                )
            };
            queue!(
                &mut self.device,
                Print(glyph.text.with(foreground).on(background))
            )?;
        }
        Ok(())
    }

    fn draw_session_view_modeline(&mut self, view: &PresentedView) -> Result<(), std::io::Error> {
        let geometry = view.geometry;
        if geometry.columns < 2 || geometry.rows < 2 {
            return Ok(());
        }
        let total_width = geometry.columns.saturating_sub(2) as usize;
        let bottom = geometry.y + geometry.rows - 1;
        let modeline_bg = if view.active {
            self.theme.mode_line_bg_color
        } else {
            self.theme.inactive_mode_line_bg_color
        };
        let padded = fitted(&view.modeline, total_width, ' ');
        queue!(
            &mut self.device,
            cursor::MoveTo(geometry.x + 1, bottom),
            Print(padded.with(self.theme.fg_color).on(modeline_bg))
        )?;
        Ok(())
    }

    fn draw_session_typeout(&mut self, view: &PresentedView) -> Result<(), std::io::Error> {
        let Some(typeout) = view.typeout.as_ref() else {
            return Ok(());
        };
        let geometry = view.geometry;
        let content_rows = usize::from(geometry.rows.saturating_sub(2));
        let available_columns = usize::from(geometry.columns.saturating_sub(2));
        if content_rows == 0 || available_columns == 0 {
            return Ok(());
        }
        let inset = usize::from(geometry.columns >= 8);
        let width = available_columns.saturating_sub(inset * 2).max(1);
        let x = geometry.x.saturating_add(1 + inset as u16);
        let y = geometry.y.saturating_add(1);
        let body: Vec<_> = if typeout.visible_text.is_empty() {
            vec![""]
        } else {
            typeout.visible_text.lines().collect()
        };
        let framed = width >= 4 && content_rows >= 3;
        let height = if framed {
            body.len().saturating_add(2).min(content_rows)
        } else {
            body.len().max(1).min(content_rows)
        };
        let foreground = if typeout.kind == "diagnostic" {
            Color::Yellow
        } else {
            self.theme.fg_color
        };
        for row in 0..height {
            let line = if framed && row == 0 {
                typeout_frame_line('┌', &typeout.title, '┐', width, '─')
            } else if framed && row + 1 == height {
                let status = if typeout.more_after {
                    "More — Space"
                } else if typeout.more_before {
                    "End — Backspace"
                } else {
                    "End"
                };
                typeout_frame_line('└', status, '┘', width, '─')
            } else {
                let body_row = row.saturating_sub(usize::from(framed));
                let text = body.get(body_row).copied().unwrap_or("");
                if framed {
                    typeout_frame_line('│', text, '│', width, ' ')
                } else {
                    fitted(text, width, ' ')
                }
            };
            queue!(
                &mut self.device,
                cursor::MoveTo(x, y.saturating_add(row as u16)),
                Print(line.with(foreground).on(Color::DarkGrey))
            )?;
        }
        Ok(())
    }
}

fn typeout_frame_line(left: char, text: &str, right: char, width: usize, fill: char) -> String {
    if width < 2 {
        return fill.to_string().repeat(width);
    }
    format!("{left}{}{right}", fitted(text, width - 2, fill))
}

fn session_layout_matches(
    previous: &PresentationSnapshot,
    snapshot: &PresentationSnapshot,
) -> bool {
    previous.columns == snapshot.columns
        && previous.rows == snapshot.rows
        && previous.views.len() == snapshot.views.len()
        && snapshot.views.iter().all(|view| {
            previous
                .views
                .iter()
                .any(|old| old.id == view.id && old.geometry == view.geometry)
        })
}

fn session_view_line(view: &PresentedView, row: usize) -> &str {
    view.visible_text
        .split_inclusive('\n')
        .nth(row)
        .unwrap_or("")
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
    session: &mut DirectSessionClient,
    shutdown_requested: &AtomicBool,
    frontend_services: &mut LocalFrontendServices,
) -> Result<(), std::io::Error> {
    let mut event_tick = interval(Duration::from_millis(20));

    loop {
        event_tick.tick().await;
        if let Some(output) = session.poll_output().await.map_err(session_io_error)?
            && apply_session_output(renderer, session, frontend_services, output).await?
        {
            return Ok(());
        }

        if shutdown_requested.load(Ordering::Acquire) {
            tracing::info!("terminal shutdown requested");
            return Ok(());
        }
        if !crossterm::event::poll(Duration::ZERO)? {
            continue;
        }

        let Some(mut input) = normalize_terminal_event(crossterm::event::read()?) else {
            continue;
        };
        if let InputEvent::Pointer(pointer) = &mut input {
            pointer.text_hit = renderer.pointer_text_hit(pointer);
        }
        let envelope = session.envelope(input);
        let output = session.dispatch(envelope).await.map_err(session_io_error)?;
        if apply_session_output(renderer, session, frontend_services, output).await? {
            return Ok(());
        }
    }
}

fn normalize_terminal_event(event: Event) -> Option<InputEvent> {
    match event {
        Event::Key(keystroke) => {
            let logical_key = crossterm_key_translate(&keystroke.code, keystroke.modifiers);
            if matches!(logical_key, LogicalKey::Unmapped | LogicalKey::Modifier(_)) {
                return None;
            }
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
            keys.push(logical_key);
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
        text_hit: None,
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

async fn apply_session_output<W: Write>(
    renderer: &mut TerminalRenderer<W>,
    session: &mut DirectSessionClient,
    frontend_services: &mut LocalFrontendServices,
    output: SessionOutput,
) -> Result<bool, std::io::Error> {
    roe_core::frontend::consume_output(session, frontend_services, renderer, output)
        .await
        .map_err(std::io::Error::other)
}

fn session_io_error(error: roe_core::session::SessionError) -> std::io::Error {
    std::io::Error::other(error)
}

impl<W: Write> PresentationConsumer for TerminalRenderer<W> {
    fn accept_presentation(
        &mut self,
        update: &PresentationUpdate,
    ) -> Result<(), roe_core::renderer::PresentationStreamError> {
        self.session_presentation.apply(update)?;
        match update {
            PresentationUpdate::Full(_) => self.force_clear_render = true,
            PresentationUpdate::Delta(delta) => {
                self.force_full_render |= delta
                    .invalidations
                    .contains(&roe_core::session::Invalidation::Full);
            }
        }
        Ok(())
    }
    fn redraw_presentation(&mut self) -> std::io::Result<()> {
        self.render_session()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use roe_core::native_kernel::{ResourceId, ViewId};
    use roe_core::session::{
        Invalidation, PresentationColor, PresentationDelta, Revision, SessionEpoch, StyleRef,
        StyledLine, TypeoutId, ViewGeometry, ViewScroll,
    };

    fn test_snapshot(revision: u64, text: &str) -> PresentationSnapshot {
        PresentationSnapshot {
            epoch: SessionEpoch(1),
            revision: Revision(revision),
            columns: 20,
            rows: 5,
            active_view: ViewId(1),
            views: vec![PresentedView {
                id: ViewId(1),
                resource: ResourceId {
                    slot: 0,
                    generation: 1,
                },
                name: "*test*".to_owned(),
                buffer_kind: "ordinary".to_owned(),
                visited_file: None,
                text_revision: 0,
                last_saved_revision: 0,
                modified: false,
                read_only: false,
                visible_text: text.into(),
                visible_start_char: 0,
                visible_end_char: text.chars().count(),
                total_lines: text.lines().count().max(1),
                max_line_chars: text
                    .lines()
                    .map(|line| line.chars().count())
                    .max()
                    .unwrap_or(0),
                cursor: text.chars().count(),
                selection: None,
                geometry: ViewGeometry {
                    x: 0,
                    y: 0,
                    columns: 20,
                    rows: 5,
                },
                scroll: ViewScroll {
                    start_line: 0,
                    start_column: 0,
                },
                active: true,
                command_view: false,
                show_gutter: false,
                modeline: "*test* 1:1".to_owned(),
                styled_ranges: Vec::new(),
                styled_lines: Vec::new(),
                typeout: None,
            }],
            styles: Vec::new(),
            echo_area: String::new(),
        }
    }

    fn apply_delta(renderer: &mut TerminalRenderer<Vec<u8>>, snapshot: PresentationSnapshot) {
        let update = PresentationUpdate::Delta(PresentationDelta {
            epoch: snapshot.epoch,
            base_revision: Revision(snapshot.revision.0 - 1),
            revision: snapshot.revision,
            invalidations: vec![Invalidation::View(ViewId(1))],
            snapshot,
        });
        renderer.apply_session_presentation(&update).unwrap();
        renderer.render_session().unwrap();
    }

    #[test]
    fn test_terminal_renderer_creation() {
        let output = Vec::new();
        let renderer = TerminalRenderer::new(output);
        assert!(renderer.session_presentation().current().is_none());
    }

    #[test]
    fn unicode_echo_truncation_preserves_character_boundaries() {
        assert_eq!(fitted("λé猫abc", 5, ' '), "λé...");
        assert_eq!(fitted("λé", 5, ' '), "λé   ");
        assert_eq!(fitted("λé", 1, ' '), ".");
    }

    #[test]
    fn standalone_modifier_keys_do_not_become_text() {
        let event = crossterm::event::KeyEvent::new(
            KeyCode::Modifier(ModifierKeyCode::LeftAlt),
            KeyModifiers::ALT,
        );
        assert_eq!(normalize_terminal_event(Event::Key(event)), None);
    }

    #[test]
    fn session_line_style_realizes_a_full_row_background() {
        let mut snapshot = test_snapshot(1, "M-x \ncommand\n");
        snapshot.styles.push(StyleDefinition {
            id: StyleRef(1),
            name: "completion-selection".to_owned(),
            foreground: None,
            background: Some(PresentationColor::Rgb {
                r: 0x3a,
                g: 0x3a,
                b: 0x3a,
            }),
            bold: false,
            italic: false,
            underline: false,
            strikethrough: false,
        });
        snapshot.views[0].styled_lines.push(StyledLine {
            line: 1,
            style: StyleRef(1),
        });
        let (_, background) = session_line_style(
            1,
            &snapshot.views[0],
            &snapshot.styles,
            &CachedTheme::default(),
        );
        assert_eq!(
            background,
            Color::Rgb {
                r: 0x3a,
                g: 0x3a,
                b: 0x3a
            }
        );
    }

    #[test]
    fn session_typeout_is_drawn_as_an_inset_child_surface() {
        let mut snapshot = test_snapshot(1, "underlying text\n");
        snapshot.views[0].typeout = Some(roe_core::session::PresentedTypeout {
            id: TypeoutId(1),
            kind: "information".to_owned(),
            title: "Evaluation".to_owned(),
            visible_text: "Mica => 3\nnext line".to_owned(),
            first_visible_line: 0,
            total_lines: 4,
            more_before: false,
            more_after: true,
            complete: true,
        });
        let mut renderer = TerminalRenderer::new(Vec::new());
        renderer
            .apply_session_presentation(&PresentationUpdate::Full(snapshot))
            .unwrap();
        renderer.render_session().unwrap();
        let rendered = String::from_utf8_lossy(&renderer.device);
        assert!(rendered.contains("Evaluation"));
        assert!(rendered.contains("Mica => 3"));
        assert!(rendered.contains("More — Space"));
    }

    #[test]
    fn session_delta_redraws_changed_lines_without_clearing_the_screen() {
        let mut renderer = TerminalRenderer::new(Vec::new());
        let first = test_snapshot(1, "alpha\nbeta\n");
        renderer
            .apply_session_presentation(&PresentationUpdate::Full(first))
            .unwrap();
        renderer.render_session().unwrap();
        let full_bytes = renderer.device.len();
        assert!(renderer.device.windows(4).any(|bytes| bytes == b"\x1b[2J"));

        renderer.device.clear();
        let second = test_snapshot(2, "alpha!\nbeta\n");
        apply_delta(&mut renderer, second);
        assert!(!renderer.device.windows(4).any(|bytes| bytes == b"\x1b[2J"));
        assert!(!String::from_utf8_lossy(&renderer.device).contains(BORDER_TOP_LEFT));
        assert!(renderer.device.len() < full_bytes / 2);

        renderer.device.clear();
        let mut cursor_only = test_snapshot(3, "alpha!\nbeta\n");
        cursor_only.views[0].cursor = 1;
        apply_delta(&mut renderer, cursor_only);
        assert!(!String::from_utf8_lossy(&renderer.device).contains("alpha"));
        assert!(!renderer.device.windows(4).any(|bytes| bytes == b"\x1b[2J"));
    }

    #[test]
    fn session_layout_change_still_performs_a_full_clear() {
        let mut renderer = TerminalRenderer::new(Vec::new());
        let first = test_snapshot(1, "alpha\n");
        renderer
            .apply_session_presentation(&PresentationUpdate::Full(first))
            .unwrap();
        renderer.render_session().unwrap();
        renderer.device.clear();

        let mut resized = test_snapshot(2, "alpha\n");
        resized.columns = 21;
        resized.views[0].geometry.columns = 21;
        apply_delta(&mut renderer, resized);
        assert!(renderer.device.windows(4).any(|bytes| bytes == b"\x1b[2J"));
    }

    #[test]
    fn explicit_full_invalidation_repaints_without_blank_screen_clear() {
        let mut renderer = TerminalRenderer::new(Vec::new());
        let first = test_snapshot(1, "alpha\n");
        renderer
            .apply_session_presentation(&PresentationUpdate::Full(first))
            .unwrap();
        renderer.render_session().unwrap();
        renderer.device.clear();

        let snapshot = test_snapshot(2, "alpha\n");
        let update = PresentationUpdate::Delta(PresentationDelta {
            epoch: snapshot.epoch,
            base_revision: Revision(1),
            revision: snapshot.revision,
            invalidations: vec![Invalidation::Full],
            snapshot,
        });
        renderer.apply_session_presentation(&update).unwrap();
        renderer.render_session().unwrap();
        assert!(!renderer.device.windows(4).any(|bytes| bytes == b"\x1b[2J"));
        assert!(String::from_utf8_lossy(&renderer.device).contains(BORDER_TOP_LEFT));
    }
    #[test]
    fn terminal_cursor_and_pointer_share_display_cell_mapping() {
        let mut renderer = TerminalRenderer::new(Vec::new());
        let snapshot = test_snapshot(1, "a\t界e\u{301}");
        renderer
            .apply_session_presentation(&PresentationUpdate::Full(snapshot))
            .unwrap();
        renderer.render_session().unwrap();
        let output = String::from_utf8_lossy(&renderer.device);
        assert!(!output.contains('\t'));
        assert!(output.contains("\x1b[2;13H"), "{output:?}");
        let hit = renderer
            .pointer_text_hit(&PointerEvent {
                text_hit: None,
                column: 10,
                row: 1,
                kind: PointerKind::Down,
                button: PointerButton::Primary,
            })
            .unwrap();
        assert_eq!(hit.position, 2); // Second cell of the wide character.
        let hit = renderer
            .pointer_text_hit(&PointerEvent {
                text_hit: None,
                column: 5,
                row: 1,
                kind: PointerKind::Down,
                button: PointerButton::Primary,
            })
            .unwrap();
        assert_eq!(hit.position, 1); // Interior of the expanded tab.
    }

    #[test]
    fn every_terminal_text_surface_escapes_terminal_controls() {
        let attack = "\x1b]52;x\x07";
        let mut renderer = TerminalRenderer::new(Vec::new());
        let mut snapshot = test_snapshot(1, attack);
        snapshot.echo_area = attack.into();
        snapshot.views[0].modeline = attack.into();
        renderer
            .draw_session_view_content_row(&snapshot.views[0], &[], 0)
            .unwrap();
        renderer
            .draw_session_view_modeline(&snapshot.views[0])
            .unwrap();
        renderer.draw_echo_area(&snapshot).unwrap();
        let output = String::from_utf8_lossy(&renderer.device);
        assert!(!output.contains("\x1b]52"));
        assert!(!output.contains('\x07'));
        assert!(output.matches("^[").count() >= 3, "{output:?}");
        let frame = typeout_frame_line('│', attack, '│', 18, ' ');
        assert!(!frame.contains('\x1b'));
        assert!(!frame.contains('\x07'));
        assert!(frame.contains("^[]52;x^G"));
    }
}
