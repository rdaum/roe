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

//! Vello-based GPU renderer for Roe editor.
//!
//! This crate provides a graphical rendering backend using Vello (GPU 2D rendering)
//! as an alternative to the terminal-based renderer.

mod key_translate;
mod renderer;
mod text;
mod theme;

pub use renderer::VelloRenderer;
pub use text::StyledSpan;
pub use theme::VelloTheme;

use roe_core::editor::{
    BorderInfo, ChromeAction, DragType, MouseDragState, SplitDirection, WindowNode,
};
use roe_core::gutter::{
    calculate_gutter_width, format_line_number, get_line_status, GutterConfig, LineStatus,
};
use roe_core::julia_runtime::face_registry;
use roe_core::syntax::Color as SyntaxColor;
use roe_core::{Editor, WindowId};
use std::collections::HashSet;
use std::sync::Arc;
use text::TextRenderer;
use vello::kurbo::{Affine, Rect};
use vello::peniko::Color;
use vello::util::{RenderContext, RenderSurface};
use vello::wgpu;
use vello::{AaConfig, RenderParams, RendererOptions, Scene};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::ModifiersState;
use winit::window::{CursorIcon, Window};

/// Default window dimensions
const DEFAULT_WIDTH: u32 = 1200;
const DEFAULT_HEIGHT: u32 = 800;

/// Convert a syntax color to Vello Color
fn syntax_color_to_vello(color: &SyntaxColor, default: Color) -> Color {
    match color {
        SyntaxColor::Rgb { r, g, b } => Color::rgba8(*r, *g, *b, 255),
        SyntaxColor::Named(name) => match name.to_lowercase().as_str() {
            "black" => Color::BLACK,
            "red" => Color::rgba8(255, 0, 0, 255),
            "green" => Color::rgba8(0, 255, 0, 255),
            "yellow" => Color::rgba8(255, 255, 0, 255),
            "blue" => Color::rgba8(0, 0, 255, 255),
            "magenta" => Color::rgba8(255, 0, 255, 255),
            "cyan" => Color::rgba8(0, 255, 255, 255),
            "white" => Color::WHITE,
            _ => default,
        },
        SyntaxColor::Inherit => default,
    }
}

/// Convert a character position to byte position in a string
fn char_to_byte(s: &str, char_pos: usize) -> usize {
    s.char_indices()
        .nth(char_pos)
        .map(|(byte_idx, _)| byte_idx)
        .unwrap_or(s.len())
}

/// Convert a byte position to character position in a string
fn byte_to_char(s: &str, byte_pos: usize) -> usize {
    s[..byte_pos.min(s.len())].chars().count()
}

/// Scrollbar width in logical pixels
const SCROLLBAR_WIDTH: f64 = 14.0;

/// Gutter colors
const GUTTER_BG_COLOR: Color = Color::rgba8(0x14, 0x14, 0x14, 0xFF); // Slightly darker than bg
const GUTTER_FG_COLOR: Color = Color::rgba8(0x60, 0x60, 0x60, 0xFF); // Dimmed line numbers
const GUTTER_SEPARATOR_COLOR: Color = Color::rgba8(0x40, 0x40, 0x40, 0xFF);
const GUTTER_MODIFIED_COLOR: Color = Color::rgba8(0xFF, 0xD7, 0x00, 0xFF); // Yellow
const GUTTER_SAVED_COLOR: Color = Color::rgba8(0x00, 0xC8, 0x00, 0xFF); // Green
const GUTTER_CONFLICT_COLOR: Color = Color::rgba8(0xFF, 0x40, 0x40, 0xFF); // Red

/// Application state for the Vello renderer
pub struct RoeVelloApp<'a> {
    /// The editor state
    editor: &'a mut Editor,
    /// Vello render context
    render_cx: RenderContext,
    /// The renderer
    renderers: Vec<Option<vello::Renderer>>,
    /// Current render state (window + surface)
    state: Option<RenderState<'a>>,
    /// The scene to render
    scene: Scene,
    /// The theme
    theme: VelloTheme,
    /// Text renderer
    text_renderer: TextRenderer,
    /// Whether we need to quit
    quit_requested: bool,
    /// Current modifier state
    modifiers: ModifiersState,
    /// Current cursor position in pixels
    cursor_position: Option<(f64, f64)>,
    /// Whether mouse is being dragged for selection
    mouse_dragging: bool,
    /// Position where mouse drag started (to set mark on first movement)
    drag_start_cursor: Option<usize>,
    /// Whether vertical scrollbar is being dragged
    scrollbar_dragging: Option<roe_core::WindowId>,
    /// Whether horizontal scrollbar is being dragged
    hscrollbar_dragging: Option<roe_core::WindowId>,
}

struct RenderState<'s> {
    surface: RenderSurface<'s>,
    window: Arc<Window>,
}

impl<'a> RoeVelloApp<'a> {
    pub fn new(editor: &'a mut Editor, theme: VelloTheme) -> Self {
        let font_size = theme.font_size;
        let font_family = if theme.font_family.is_empty() {
            None
        } else {
            Some(theme.font_family.clone())
        };

        Self {
            editor,
            render_cx: RenderContext::new(),
            renderers: vec![],
            state: None,
            scene: Scene::new(),
            text_renderer: TextRenderer::new(font_size, font_family),
            theme,
            quit_requested: false,
            modifiers: ModifiersState::empty(),
            cursor_position: None,
            mouse_dragging: false,
            drag_start_cursor: None,
            scrollbar_dragging: None,
            hscrollbar_dragging: None,
        }
    }

    fn create_window(&mut self, event_loop: &ActiveEventLoop) -> Arc<Window> {
        let attrs = Window::default_attributes()
            .with_title("Roe - Ryan's Own Emacs")
            .with_inner_size(LogicalSize::new(DEFAULT_WIDTH, DEFAULT_HEIGHT));

        Arc::new(
            event_loop
                .create_window(attrs)
                .expect("Failed to create window"),
        )
    }

    fn render(&mut self) {
        // Extract surface info first to avoid borrow conflicts
        let (width, height, dev_id, format, scale_factor) = {
            let Some(ref state) = self.state else {
                return;
            };
            (
                state.surface.config.width,
                state.surface.config.height,
                state.surface.dev_id,
                state.surface.format,
                state.window.scale_factor(),
            )
        };

        // Convert to logical dimensions for layout calculations
        let logical_width = (width as f64 / scale_factor) as u32;
        let logical_height = (height as f64 / scale_factor) as u32;

        // Get dimensions from text renderer
        let char_width = self.text_renderer.char_width();
        let line_height = self.text_renderer.line_height();

        // Update editor frame dimensions (using logical dimensions)
        let cols = (logical_width as f32 / char_width).floor() as u16;
        let lines = (logical_height as f32 / line_height).floor() as u16;
        self.editor
            .handle_resize(cols.max(1), lines.saturating_sub(1).max(1)); // -1 for echo area

        // Build the scene in logical coordinates, then scale for physical rendering
        self.scene.reset();
        self.build_scene(logical_width, logical_height);

        // Apply scale factor transform to the scene
        if scale_factor != 1.0 {
            let mut scaled_scene = Scene::new();
            scaled_scene.append(&self.scene, Some(Affine::scale(scale_factor)));
            self.scene = scaled_scene;
        }

        // Now get the surface texture
        let Some(ref mut state) = self.state else {
            return;
        };
        let surface_texture = state
            .surface
            .surface
            .get_current_texture()
            .expect("Failed to get surface texture");

        let device_handle = &self.render_cx.devices[dev_id];

        // Ensure we have a renderer for this device
        if self.renderers.len() <= dev_id {
            self.renderers.resize_with(dev_id + 1, || None);
        }
        if self.renderers[dev_id].is_none() {
            let renderer = vello::Renderer::new(
                &device_handle.device,
                RendererOptions {
                    surface_format: Some(format),
                    use_cpu: false,
                    antialiasing_support: vello::AaSupport::all(),
                    num_init_threads: None,
                },
            )
            .expect("Failed to create Vello renderer");
            self.renderers[dev_id] = Some(renderer);
        }

        let renderer = self.renderers[dev_id].as_mut().unwrap();

        renderer
            .render_to_surface(
                &device_handle.device,
                &device_handle.queue,
                &self.scene,
                &surface_texture,
                &RenderParams {
                    base_color: self.theme.bg_color,
                    width,
                    height,
                    antialiasing_method: AaConfig::Msaa16,
                },
            )
            .expect("Failed to render to surface");

        surface_texture.present();
    }

    fn build_scene(&mut self, width: u32, height: u32) {
        // Draw background
        let bg_rect = Rect::new(0.0, 0.0, width as f64, height as f64);
        self.scene.fill(
            vello::peniko::Fill::NonZero,
            Affine::IDENTITY,
            self.theme.bg_color,
            None,
            &bg_rect,
        );

        // Draw each window
        for window_id in self.editor.windows.keys().collect::<Vec<_>>() {
            self.draw_window(window_id);
        }

        // Draw echo area at bottom
        self.draw_echo_area(width, height);
    }

    fn draw_window(&mut self, window_id: roe_core::WindowId) {
        let char_width = self.text_renderer.char_width() as f64;
        let line_height = self.text_renderer.line_height() as f64;

        let window = &self.editor.windows[window_id];
        let is_active = window_id == self.editor.active_window;

        // Calculate window bounds in pixels
        let x = window.x as f64 * char_width;
        let y = window.y as f64 * line_height;
        let w = window.width_chars as f64 * char_width;
        let h = window.height_chars as f64 * line_height;

        // Draw window background
        let window_rect = Rect::new(x, y, x + w, y + h);
        self.scene.fill(
            vello::peniko::Fill::NonZero,
            Affine::IDENTITY,
            self.theme.bg_color,
            None,
            &window_rect,
        );

        // Draw border
        let border_color = if is_active {
            self.theme.active_border_color
        } else {
            self.theme.border_color
        };

        // Top border
        let top_border = Rect::new(x, y, x + w, y + 2.0);
        self.scene.fill(
            vello::peniko::Fill::NonZero,
            Affine::IDENTITY,
            border_color,
            None,
            &top_border,
        );

        // Bottom border / modeline background
        let modeline_y = y + h - line_height;
        let modeline_rect = Rect::new(x, modeline_y, x + w, modeline_y + line_height);
        let modeline_color = if is_active {
            self.theme.mode_line_bg_color
        } else {
            self.theme.inactive_mode_line_bg_color
        };
        self.scene.fill(
            vello::peniko::Fill::NonZero,
            Affine::IDENTITY,
            modeline_color,
            None,
            &modeline_rect,
        );

        // Left border
        let left_border = Rect::new(x, y, x + 2.0, y + h);
        self.scene.fill(
            vello::peniko::Fill::NonZero,
            Affine::IDENTITY,
            border_color,
            None,
            &left_border,
        );

        // Right border
        let right_border = Rect::new(x + w - 2.0, y, x + w, y + h);
        self.scene.fill(
            vello::peniko::Fill::NonZero,
            Affine::IDENTITY,
            border_color,
            None,
            &right_border,
        );

        // Get buffer info - guard against stale buffer IDs
        let Some(buffer) = self.editor.buffers.get(window.active_buffer) else {
            // Buffer no longer exists (likely replaced by visit-file), skip rendering
            return;
        };
        let base_content_x = x + char_width;
        let content_y = y + line_height;
        // Reserve space for horizontal scrollbar at bottom
        let content_height = window.height_chars.saturating_sub(3) as usize; // -3 for top border, modeline, h-scrollbar
        let start_line = window.start_line as usize;
        let start_column = window.start_column as usize;

        // Check if gutter should be shown (controlled by major mode / Julia)
        let show_gutter = buffer.show_gutter();

        // Calculate gutter width and get modified lines
        let (gutter_width_chars, modified_lines): (usize, HashSet<usize>) = if show_gutter {
            let total_lines = buffer.buffer_len_lines();
            let config = GutterConfig::default();
            let width = calculate_gutter_width(total_lines, &config);
            let buffer_content = buffer.content();
            let modified = self
                .editor
                .file_watcher
                .get_modified_lines(window.active_buffer, &buffer_content);
            (width, modified)
        } else {
            (0, HashSet::new())
        };

        let gutter_width_px = gutter_width_chars as f64 * char_width;
        let content_x = base_content_x + gutter_width_px;

        // Account for scrollbar width and gutter in content area
        let content_width_px = w - (2.0 * char_width) - SCROLLBAR_WIDTH - 4.0 - gutter_width_px;
        let content_width = content_width_px as f32;
        let content_width_chars = (content_width_px / char_width) as usize;

        // Calculate line number width for formatting
        let line_number_width = gutter_width_chars.saturating_sub(2); // Subtract status indicator and separator

        // For tracking merged lines (TODO: track separately)
        let merged_lines: HashSet<usize> = HashSet::new();

        // Draw gutter background and content (outside clip region)
        if show_gutter {
            // Gutter background
            let gutter_rect = Rect::new(
                base_content_x,
                content_y,
                base_content_x + gutter_width_px,
                content_y + (content_height as f64 * line_height),
            );
            self.scene.fill(
                vello::peniko::Fill::NonZero,
                Affine::IDENTITY,
                GUTTER_BG_COLOR,
                None,
                &gutter_rect,
            );

            // Gutter separator line
            let separator_x = base_content_x + gutter_width_px - 1.0;
            let separator_rect = Rect::new(
                separator_x,
                content_y,
                separator_x + 1.0,
                content_y + (content_height as f64 * line_height),
            );
            self.scene.fill(
                vello::peniko::Fill::NonZero,
                Affine::IDENTITY,
                GUTTER_SEPARATOR_COLOR,
                None,
                &separator_rect,
            );

            // Draw line numbers and status indicators for visible lines
            let total_buffer_lines = buffer.buffer_len_lines();
            for visual_row in 0..content_height {
                let buffer_line = start_line + visual_row;
                let gutter_y = content_y + (visual_row as f64 * line_height);

                if buffer_line < total_buffer_lines {
                    // Get line content for status check
                    let line_text = buffer.buffer_line(buffer_line);
                    let line_status =
                        get_line_status(&line_text, buffer_line, &modified_lines, &merged_lines);

                    // Draw status indicator bar
                    let status_color = match line_status {
                        LineStatus::Clean => None,
                        LineStatus::Modified => Some(GUTTER_MODIFIED_COLOR),
                        LineStatus::ModifiedSaved => Some(GUTTER_SAVED_COLOR),
                        LineStatus::Conflict => Some(GUTTER_CONFLICT_COLOR),
                    };

                    if let Some(color) = status_color {
                        let status_rect = Rect::new(
                            base_content_x,
                            gutter_y,
                            base_content_x + 3.0, // 3px wide bar
                            gutter_y + line_height,
                        );
                        self.scene.fill(
                            vello::peniko::Fill::NonZero,
                            Affine::IDENTITY,
                            color,
                            None,
                            &status_rect,
                        );
                    }

                    // Draw line number (right-aligned)
                    let line_num_str = format_line_number(buffer_line + 1, line_number_width);
                    let line_num_x = base_content_x + char_width; // After status indicator
                    self.text_renderer.render_line(
                        &mut self.scene,
                        &line_num_str,
                        line_num_x as f32,
                        gutter_y as f32,
                        GUTTER_FG_COLOR,
                        None,
                    );
                } else {
                    // Empty line (past end of buffer) - show tilde
                    let tilde_str = format!("{:>width$}", "~", width = line_number_width);
                    let line_num_x = base_content_x + char_width;
                    self.text_renderer.render_line(
                        &mut self.scene,
                        &tilde_str,
                        line_num_x as f32,
                        gutter_y as f32,
                        GUTTER_FG_COLOR,
                        None,
                    );
                }
            }
        }

        // Set up clipping region for content area (prevents text overflow)
        let clip_rect = Rect::new(
            content_x,
            content_y,
            content_x + content_width_px,
            content_y + (content_height as f64 * line_height),
        );
        self.scene.push_layer(
            vello::peniko::BlendMode::default(),
            1.0,
            Affine::IDENTITY,
            &clip_rect,
        );

        // Get selection region (only for active window)
        let region_bounds = if is_active {
            buffer.get_region(window.cursor)
        } else {
            None
        };

        // Collect lines to render with their buffer positions, track max width
        let mut max_line_len: usize = 0;
        let lines_to_render: Vec<(usize, usize, String)> = buffer
            .buffer_lines()
            .into_iter()
            .enumerate()
            .inspect(|(_, text)| {
                let len = text.trim_end_matches('\n').chars().count();
                if len > max_line_len {
                    max_line_len = len;
                }
            })
            .filter(|(idx, _)| *idx >= start_line && (*idx - start_line) < content_height)
            .map(|(idx, text)| {
                let line_start_pos = buffer.to_char_index(0, idx as u16);
                (
                    idx - start_line,
                    line_start_pos,
                    text.trim_end_matches('\n').to_string(),
                )
            })
            .collect();

        // Draw selection highlights first (behind text), accounting for horizontal scroll
        if let Some((region_start, region_end)) = region_bounds {
            let selection_color = self.theme.selection_color;
            for (visual_line, line_start_pos, line_text) in &lines_to_render {
                let line_char_len = line_text.chars().count();
                let line_end_pos = line_start_pos + line_char_len;

                // Check if this line intersects with selection
                if *line_start_pos < region_end && line_end_pos > region_start {
                    // Calculate selection bounds within this line
                    let sel_start_in_line = if region_start > *line_start_pos {
                        region_start - line_start_pos
                    } else {
                        0
                    };
                    let sel_end_in_line = if region_end < line_end_pos {
                        region_end - line_start_pos
                    } else {
                        line_char_len
                    };

                    // Adjust for horizontal scroll
                    let visible_sel_start = sel_start_in_line.saturating_sub(start_column);
                    let visible_sel_end = sel_end_in_line.saturating_sub(start_column);

                    if visible_sel_end > 0 && visible_sel_start < content_width_chars {
                        let sel_x = content_x + (visible_sel_start as f64 * char_width);
                        let sel_y = content_y + (*visual_line as f64 * line_height);
                        let sel_width = (visible_sel_end - visible_sel_start) as f64 * char_width;

                        let sel_rect =
                            Rect::new(sel_x, sel_y, sel_x + sel_width, sel_y + line_height);
                        self.scene.fill(
                            vello::peniko::Fill::NonZero,
                            Affine::IDENTITY,
                            selection_color,
                            None,
                            &sel_rect,
                        );
                    }
                }
            }
        }

        // Render each line of text with horizontal scroll offset and syntax highlighting
        let fg_color = self.theme.fg_color;
        let face_registry_guard = face_registry().lock().ok();

        // Get full buffer content for byte<->char conversion
        let buffer_content = buffer.content();

        for (visual_line, line_start_char, line_text) in lines_to_render {
            // Apply horizontal scroll - skip start_column characters
            let visible_text: String = line_text.chars().skip(start_column).collect();
            if visible_text.is_empty() {
                continue;
            }

            let text_x = content_x as f32;
            let text_y = content_y as f32 + (visual_line as f32) * line_height as f32;

            // Convert char positions to byte positions for span query
            // (spans use byte positions for tree-sitter/Julia compatibility)
            let line_start_byte = char_to_byte(&buffer_content, line_start_char);
            let line_end_byte =
                char_to_byte(&buffer_content, line_start_char + line_text.chars().count());
            let syntax_spans = buffer.spans_in_range(line_start_byte..line_end_byte);

            // Draw background rectangles for spans with background colors
            let line_char_count = line_text.chars().count();
            let visible_char_count = visible_text.chars().count();
            if let Some(ref registry) = face_registry_guard {
                for span in &syntax_spans {
                    if let Some(face) = registry.get(span.face_id) {
                        if let Some(ref bg_color) = face.background {
                            // Convert span byte positions to char positions within line
                            let span_byte_start_in_line =
                                span.start.saturating_sub(line_start_byte);
                            let span_byte_end_in_line = span.end.saturating_sub(line_start_byte);
                            let span_start_in_line =
                                byte_to_char(&line_text, span_byte_start_in_line);
                            let span_end_in_line = byte_to_char(&line_text, span_byte_end_in_line)
                                .min(line_char_count);

                            // Adjust for horizontal scroll
                            if span_end_in_line <= start_column
                                || span_start_in_line >= start_column + visible_char_count
                            {
                                continue; // Span is not visible
                            }

                            let visible_start = span_start_in_line.saturating_sub(start_column);
                            let visible_end = span_end_in_line
                                .saturating_sub(start_column)
                                .min(visible_char_count);

                            if visible_start >= visible_end {
                                continue;
                            }

                            // Draw background rectangle
                            let bg_x = text_x + (visible_start as f32 * char_width as f32);
                            let bg_w = (visible_end - visible_start) as f32 * char_width as f32;
                            let bg_rect = Rect::new(
                                bg_x as f64,
                                text_y as f64,
                                (bg_x + bg_w) as f64,
                                (text_y + line_height as f32) as f64,
                            );
                            let vello_bg = syntax_color_to_vello(bg_color, self.theme.bg_color);
                            self.scene.fill(
                                vello::peniko::Fill::NonZero,
                                Affine::IDENTITY,
                                vello_bg,
                                None,
                                &bg_rect,
                            );
                        }
                    }
                }
            }

            // Convert buffer spans to StyledSpans for rendering
            let styled_spans: Vec<StyledSpan> = if let Some(ref registry) = face_registry_guard {
                syntax_spans
                    .iter()
                    .filter_map(|span| {
                        let face = registry.get(span.face_id)?;
                        // Convert span byte positions to char positions within line
                        let span_byte_start_in_line = span.start.saturating_sub(line_start_byte);
                        let span_byte_end_in_line = span.end.saturating_sub(line_start_byte);
                        let span_start_in_line = byte_to_char(&line_text, span_byte_start_in_line);
                        let span_end_in_line =
                            byte_to_char(&line_text, span_byte_end_in_line).min(line_char_count);

                        // Adjust for horizontal scroll
                        if span_end_in_line <= start_column
                            || span_start_in_line >= start_column + visible_char_count
                        {
                            return None; // Span is not visible
                        }

                        let visible_start = span_start_in_line.saturating_sub(start_column);
                        let visible_end = span_end_in_line
                            .saturating_sub(start_column)
                            .min(visible_char_count);

                        if visible_start >= visible_end {
                            return None;
                        }

                        let color = face
                            .foreground
                            .as_ref()
                            .map(|c| syntax_color_to_vello(c, fg_color))
                            .unwrap_or(fg_color);

                        Some(
                            StyledSpan::new(visible_start, visible_end, color)
                                .with_bold(face.bold)
                                .with_italic(face.italic),
                        )
                    })
                    .collect()
            } else {
                Vec::new()
            };

            // Use styled rendering if we have spans, otherwise plain rendering
            if styled_spans.is_empty() {
                self.text_renderer.render_line(
                    &mut self.scene,
                    &visible_text,
                    text_x,
                    text_y,
                    fg_color,
                    Some(content_width),
                );
            } else {
                self.text_renderer.render_line_with_styles(
                    &mut self.scene,
                    &visible_text,
                    text_x,
                    text_y,
                    fg_color,
                    &styled_spans,
                    Some(content_width),
                );
            }
        }

        // Draw cursor (inside clipping region), accounting for horizontal scroll
        if is_active {
            let (col, line) = buffer.to_column_line(window.cursor);
            let line = line as usize;
            let col = col as usize;
            if line >= start_line {
                let cursor_visual_line = line - start_line;
                // Check if cursor is horizontally visible
                if cursor_visual_line < content_height
                    && col >= start_column
                    && col < start_column + content_width_chars
                {
                    let visual_col = col - start_column;
                    let cursor_x = content_x + (visual_col as f64 * char_width);
                    let cursor_y = content_y + (cursor_visual_line as f64) * line_height;

                    let cursor_rect =
                        Rect::new(cursor_x, cursor_y, cursor_x + 2.0, cursor_y + line_height);
                    self.scene.fill(
                        vello::peniko::Fill::NonZero,
                        Affine::IDENTITY,
                        self.theme.cursor_color,
                        None,
                        &cursor_rect,
                    );
                }
            }
        }

        // Pop the clipping layer (content area done)
        self.scene.pop_layer();

        // Draw modeline text (outside clip)
        let buffer_name = buffer.object();
        let (col, line) = buffer.to_column_line(window.cursor);
        let major_mode_str = buffer
            .major_mode()
            .map(|m| format!("({}) ", m))
            .unwrap_or_default();
        let modeline_text = if is_active {
            format!(
                " ᚱᛟ {} {}{}:{}",
                buffer_name,
                major_mode_str,
                line + 1,
                col + 1
            )
        } else {
            format!(
                "    {} {}{}:{}",
                buffer_name,
                major_mode_str,
                line + 1,
                col + 1
            )
        };

        self.text_renderer.render_line(
            &mut self.scene,
            &modeline_text,
            (x + char_width) as f32,
            modeline_y as f32,
            self.theme.fg_color,
            Some(content_width),
        );

        // Draw scrollbar
        let total_lines = buffer.buffer_len_lines().max(1);
        let scrollbar_x = x + w - SCROLLBAR_WIDTH - 2.0; // Inside right border
        let scrollbar_top = y + 2.0; // Below top border
        let scrollbar_height = h - line_height - 4.0; // Above modeline

        // Draw scrollbar track (subtle background)
        let track_rect = Rect::new(
            scrollbar_x,
            scrollbar_top,
            scrollbar_x + SCROLLBAR_WIDTH,
            scrollbar_top + scrollbar_height,
        );
        self.scene.fill(
            vello::peniko::Fill::NonZero,
            Affine::IDENTITY,
            Color::rgba8(0x40, 0x40, 0x40, 0x80),
            None,
            &track_rect,
        );

        // Calculate thumb position and size
        let visible_ratio = (content_height as f64 / total_lines as f64).min(1.0);
        let thumb_height = (scrollbar_height * visible_ratio).max(20.0); // Minimum thumb size
        let scroll_ratio = if total_lines > content_height {
            start_line as f64 / (total_lines - content_height) as f64
        } else {
            0.0
        };
        let thumb_y = scrollbar_top + scroll_ratio * (scrollbar_height - thumb_height);

        // Draw thumb
        let thumb_rect = Rect::new(
            scrollbar_x + 2.0,
            thumb_y,
            scrollbar_x + SCROLLBAR_WIDTH - 2.0,
            thumb_y + thumb_height,
        );
        let thumb_color = if is_active {
            Color::rgba8(0x80, 0x80, 0x80, 0xC0)
        } else {
            Color::rgba8(0x60, 0x60, 0x60, 0xA0)
        };
        self.scene.fill(
            vello::peniko::Fill::NonZero,
            Affine::IDENTITY,
            thumb_color,
            None,
            &thumb_rect,
        );

        // Draw horizontal scrollbar (only if content exceeds visible width)
        if max_line_len > content_width_chars {
            let hscroll_y = y + h - line_height - SCROLLBAR_WIDTH - 2.0; // Above modeline
            let hscroll_x = x + 2.0; // After left border
            let hscroll_width = w - SCROLLBAR_WIDTH - 6.0; // Before vertical scrollbar

            // Draw horizontal scrollbar track
            let htrack_rect = Rect::new(
                hscroll_x,
                hscroll_y,
                hscroll_x + hscroll_width,
                hscroll_y + SCROLLBAR_WIDTH,
            );
            self.scene.fill(
                vello::peniko::Fill::NonZero,
                Affine::IDENTITY,
                Color::rgba8(0x40, 0x40, 0x40, 0x80),
                None,
                &htrack_rect,
            );

            // Calculate horizontal thumb position and size
            let h_visible_ratio = (content_width_chars as f64 / max_line_len as f64).min(1.0);
            let hthumb_width = (hscroll_width * h_visible_ratio).max(20.0);
            let h_scroll_ratio = if max_line_len > content_width_chars {
                start_column as f64 / (max_line_len - content_width_chars) as f64
            } else {
                0.0
            };
            let hthumb_x = hscroll_x + h_scroll_ratio * (hscroll_width - hthumb_width);

            // Draw horizontal thumb
            let hthumb_rect = Rect::new(
                hthumb_x,
                hscroll_y + 2.0,
                hthumb_x + hthumb_width,
                hscroll_y + SCROLLBAR_WIDTH - 2.0,
            );
            self.scene.fill(
                vello::peniko::Fill::NonZero,
                Affine::IDENTITY,
                thumb_color,
                None,
                &hthumb_rect,
            );
        }
    }

    fn draw_echo_area(&mut self, width: u32, height: u32) {
        let line_height = self.text_renderer.line_height() as f64;
        let echo_y = height as f64 - line_height;

        // Echo area background
        let echo_rect = Rect::new(0.0, echo_y, width as f64, height as f64);
        self.scene.fill(
            vello::peniko::Fill::NonZero,
            Affine::IDENTITY,
            self.theme.bg_color,
            None,
            &echo_rect,
        );

        // Draw echo message text
        if !self.editor.echo_message.is_empty() {
            let message = self.editor.echo_message.clone();
            let fg_color = self.theme.fg_color;
            self.text_renderer.render_line(
                &mut self.scene,
                &message,
                4.0, // Small left padding
                echo_y as f32,
                fg_color,
                Some(width as f32 - 8.0),
            );
        }
    }

    async fn handle_key_event(&mut self, event: winit::event::KeyEvent) -> Vec<ChromeAction> {
        if event.state != ElementState::Pressed {
            return vec![];
        }

        let keys = key_translate::translate_key_event(&event, self.modifiers);
        if keys.is_empty() {
            return vec![];
        }

        self.editor.key_event(keys).await.unwrap_or_default()
    }

    /// Handle mouse click at the given pixel position
    async fn handle_mouse_click(&mut self, x: f64, y: f64) {
        let char_width = self.text_renderer.char_width() as f64;
        let line_height = self.text_renderer.line_height() as f64;

        // Convert pixel position to character grid position
        let grid_x = (x / char_width) as u16;
        let grid_y = (y / line_height) as u16;

        // Find which window was clicked
        let clicked_window = self.find_window_at_position(grid_x, grid_y);

        let Some(window_id) = clicked_window else {
            return;
        };

        // Switch to clicked window if different from active
        if self.editor.active_window != window_id {
            self.editor.previous_active_window = Some(self.editor.active_window);
            self.editor.active_window = window_id;
        }

        // Calculate cursor position within the buffer
        let window = &self.editor.windows[window_id];
        let buffer = &self.editor.buffers[window.active_buffer];

        // Position relative to window content area (+1 for border)
        let relative_x = grid_x.saturating_sub(window.x + 1);
        let relative_y = grid_y.saturating_sub(window.y + 1);

        // Convert to buffer position (account for scroll offsets)
        let buffer_line = relative_y as usize + window.start_line as usize;
        let buffer_col = relative_x as usize + window.start_column as usize;

        // Clamp line to valid range
        let total_lines = buffer.buffer_len_lines();
        if total_lines == 0 {
            // Empty buffer - set cursor to 0
            let window = self.editor.windows.get_mut(window_id).unwrap();
            window.cursor = 0;
            return;
        }
        let clamped_line = buffer_line.min(total_lines - 1);

        // Get line length to clamp column
        let line_text = buffer
            .buffer_lines()
            .into_iter()
            .nth(clamped_line)
            .unwrap_or_default();
        let line_len = line_text.trim_end_matches('\n').len();
        let clamped_col = buffer_col.min(line_len);

        // Get the new cursor position using clamped values
        let new_cursor = buffer.to_char_index(clamped_col as u16, clamped_line as u16);

        // Final safety clamp to buffer length
        let buffer_len = buffer.buffer_len_chars();
        let clamped_cursor = if buffer_len == 0 {
            0
        } else {
            new_cursor.min(buffer_len - 1)
        };

        // Update cursor in window
        let window = self.editor.windows.get_mut(window_id).unwrap();
        window.cursor = clamped_cursor;

        // Clear any existing mark (simple click shouldn't start selection)
        buffer.clear_mark();
    }

    /// Handle mouse drag to update selection
    fn handle_mouse_drag(&mut self, x: f64, y: f64) {
        let char_width = self.text_renderer.char_width() as f64;
        let line_height = self.text_renderer.line_height() as f64;

        // Convert pixel position to character grid position
        let grid_x = (x / char_width) as u16;
        let grid_y = (y / line_height) as u16;

        // Only update cursor in the active window during drag
        let window_id = self.editor.active_window;
        let window = &self.editor.windows[window_id];
        let buffer = &self.editor.buffers[window.active_buffer];

        // Position relative to window content area (+1 for border)
        let relative_x = grid_x.saturating_sub(window.x + 1);
        let relative_y = grid_y.saturating_sub(window.y + 1);

        // Convert to buffer position (account for scroll offsets)
        let buffer_line = relative_y as usize + window.start_line as usize;
        let buffer_col = relative_x as usize + window.start_column as usize;

        // Clamp line to valid range
        let total_lines = buffer.buffer_len_lines();
        if total_lines == 0 {
            return;
        }
        let clamped_line = buffer_line.min(total_lines - 1);

        // Get line length to clamp column
        let line_text = buffer
            .buffer_lines()
            .into_iter()
            .nth(clamped_line)
            .unwrap_or_default();
        let line_len = line_text.trim_end_matches('\n').len();
        let clamped_col = buffer_col.min(line_len);

        // Get the new cursor position using clamped values
        let new_cursor = buffer.to_char_index(clamped_col as u16, clamped_line as u16);

        // Final safety clamp to buffer length
        let buffer_len = buffer.buffer_len_chars();
        let clamped_cursor = if buffer_len == 0 {
            0
        } else {
            new_cursor.min(buffer_len - 1)
        };

        // On first drag movement, set the mark at the starting position
        if let Some(start_cursor) = self.drag_start_cursor.take() {
            buffer.set_mark(start_cursor);
        }

        // Update cursor in window
        let window = self.editor.windows.get_mut(window_id).unwrap();
        window.cursor = clamped_cursor;
    }

    /// Find which window contains the given grid position
    fn find_window_at_position(&self, x: u16, y: u16) -> Option<roe_core::WindowId> {
        for (window_id, window) in &self.editor.windows {
            // Check if position is within window content area
            let content_left = window.x + 1; // +1 for left border
            let content_right = window.x + window.width_chars - 1;
            let content_top = window.y;
            let content_bottom = window.y + window.height_chars - 1;

            if x >= content_left && x < content_right && y >= content_top && y <= content_bottom {
                return Some(window_id);
            }
        }
        None
    }

    /// Check if a pixel position is in a window's scrollbar, returns (window_id, relative_y_ratio)
    fn check_scrollbar_hit(&self, px: f64, py: f64) -> Option<(roe_core::WindowId, f64)> {
        let char_width = self.text_renderer.char_width() as f64;
        let line_height = self.text_renderer.line_height() as f64;

        for (window_id, window) in &self.editor.windows {
            let x = window.x as f64 * char_width;
            let y = window.y as f64 * line_height;
            let w = window.width_chars as f64 * char_width;
            let h = window.height_chars as f64 * line_height;

            let scrollbar_x = x + w - SCROLLBAR_WIDTH - 2.0;
            let scrollbar_top = y + 2.0;
            let scrollbar_height = h - line_height - 4.0;

            // Check if position is in scrollbar area
            if px >= scrollbar_x
                && px <= scrollbar_x + SCROLLBAR_WIDTH
                && py >= scrollbar_top
                && py <= scrollbar_top + scrollbar_height
            {
                // Return ratio of position within scrollbar
                let ratio = (py - scrollbar_top) / scrollbar_height;
                return Some((window_id, ratio.clamp(0.0, 1.0)));
            }
        }
        None
    }

    /// Handle scrollbar click - scroll to position
    fn handle_scrollbar_click(&mut self, window_id: roe_core::WindowId, ratio: f64) {
        let window = &self.editor.windows[window_id];
        let buffer = &self.editor.buffers[window.active_buffer];
        let total_lines = buffer.buffer_len_lines();
        let content_height = window.height_chars.saturating_sub(2) as usize;

        if total_lines <= content_height {
            return; // No scrolling needed
        }

        // Calculate new start line based on click ratio
        let max_start = total_lines.saturating_sub(content_height);
        let new_start = ((max_start as f64) * ratio).round() as usize;

        let window = self.editor.windows.get_mut(window_id).unwrap();
        window.start_line = new_start as u16;
    }

    /// Handle scrollbar drag
    fn handle_scrollbar_drag(&mut self, py: f64) {
        let Some(window_id) = self.scrollbar_dragging else {
            return;
        };

        let line_height = self.text_renderer.line_height() as f64;

        let window = &self.editor.windows[window_id];
        let y = window.y as f64 * line_height;
        let h = window.height_chars as f64 * line_height;

        let scrollbar_top = y + 2.0;
        let scrollbar_height = h - line_height - 4.0;

        // Calculate ratio from pixel position
        let ratio = ((py - scrollbar_top) / scrollbar_height).clamp(0.0, 1.0);

        // Scroll to that position
        let buffer = &self.editor.buffers[window.active_buffer];
        let total_lines = buffer.buffer_len_lines();
        let content_height = window.height_chars.saturating_sub(2) as usize;

        if total_lines <= content_height {
            return;
        }

        let max_start = total_lines.saturating_sub(content_height);
        let new_start = ((max_start as f64) * ratio).round() as usize;

        let window = self.editor.windows.get_mut(window_id).unwrap();
        window.start_line = new_start as u16;
    }

    /// Check if a pixel position is in a window's horizontal scrollbar
    fn check_hscrollbar_hit(&self, px: f64, py: f64) -> Option<(roe_core::WindowId, f64)> {
        let char_width = self.text_renderer.char_width() as f64;
        let line_height = self.text_renderer.line_height() as f64;

        for (window_id, window) in &self.editor.windows {
            let x = window.x as f64 * char_width;
            let y = window.y as f64 * line_height;
            let w = window.width_chars as f64 * char_width;
            let h = window.height_chars as f64 * line_height;

            let hscroll_y = y + h - line_height - SCROLLBAR_WIDTH - 2.0;
            let hscroll_x = x + 2.0;
            let hscroll_width = w - SCROLLBAR_WIDTH - 6.0;

            // Check if position is in horizontal scrollbar area
            if px >= hscroll_x
                && px <= hscroll_x + hscroll_width
                && py >= hscroll_y
                && py <= hscroll_y + SCROLLBAR_WIDTH
            {
                let ratio = (px - hscroll_x) / hscroll_width;
                return Some((window_id, ratio.clamp(0.0, 1.0)));
            }
        }
        None
    }

    /// Get max line length for a buffer
    fn get_max_line_len(&self, window_id: roe_core::WindowId) -> usize {
        let window = &self.editor.windows[window_id];
        let buffer = &self.editor.buffers[window.active_buffer];
        buffer
            .buffer_lines()
            .into_iter()
            .map(|line| line.trim_end_matches('\n').chars().count())
            .max()
            .unwrap_or(0)
    }

    /// Handle horizontal scrollbar click
    fn handle_hscrollbar_click(&mut self, window_id: roe_core::WindowId, ratio: f64) {
        let char_width = self.text_renderer.char_width() as f64;
        let window = &self.editor.windows[window_id];
        let w = window.width_chars as f64 * char_width;
        let content_width_px = w - (2.0 * char_width) - SCROLLBAR_WIDTH - 4.0;
        let content_width_chars = (content_width_px / char_width) as usize;

        let max_line_len = self.get_max_line_len(window_id);
        if max_line_len <= content_width_chars {
            return; // No horizontal scrolling needed
        }

        let max_start = max_line_len.saturating_sub(content_width_chars);
        let new_start = ((max_start as f64) * ratio).round() as usize;

        let window = self.editor.windows.get_mut(window_id).unwrap();
        window.start_column = new_start as u16;
    }

    /// Handle horizontal scrollbar drag
    fn handle_hscrollbar_drag(&mut self, px: f64) {
        let Some(window_id) = self.hscrollbar_dragging else {
            return;
        };

        let char_width = self.text_renderer.char_width() as f64;
        let window = &self.editor.windows[window_id];
        let x = window.x as f64 * char_width;
        let w = window.width_chars as f64 * char_width;

        let hscroll_x = x + 2.0;
        let hscroll_width = w - SCROLLBAR_WIDTH - 6.0;

        let ratio = ((px - hscroll_x) / hscroll_width).clamp(0.0, 1.0);

        let content_width_px = w - (2.0 * char_width) - SCROLLBAR_WIDTH - 4.0;
        let content_width_chars = (content_width_px / char_width) as usize;

        let max_line_len = self.get_max_line_len(window_id);
        if max_line_len <= content_width_chars {
            return;
        }

        let max_start = max_line_len.saturating_sub(content_width_chars);
        let new_start = ((max_start as f64) * ratio).round() as usize;

        let window = self.editor.windows.get_mut(window_id).unwrap();
        window.start_column = new_start as u16;
    }

    /// Check if a pixel position is on a window border that can be dragged to resize
    fn check_border_hit(&self, px: f64, py: f64) -> Option<(BorderInfo, WindowId)> {
        let char_width = self.text_renderer.char_width() as f64;
        let line_height = self.text_renderer.line_height() as f64;

        // Convert to grid coordinates
        let grid_x = (px / char_width) as u16;
        let grid_y = (py / line_height) as u16;

        // Check all windows to see if the click is on a border
        for (window_id, window) in &self.editor.windows {
            let left_border = window.x;
            let right_border = window.x + window.width_chars - 1;
            let top_border = window.y;
            let bottom_border = window.y + window.height_chars - 1;

            // Check vertical borders (left and right sides)
            if (grid_x == left_border || grid_x == right_border)
                && grid_y >= top_border
                && grid_y <= bottom_border
            {
                // This is a vertical border
                if let Some(split_info) = self.find_split_for_border(window_id, true) {
                    return Some((
                        BorderInfo {
                            is_vertical: true,
                            split_node_path: split_info.0,
                            original_ratio: split_info.1,
                        },
                        window_id,
                    ));
                }
            }

            // Check horizontal borders (top and bottom sides)
            if (grid_y == top_border || grid_y == bottom_border)
                && grid_x >= left_border
                && grid_x <= right_border
            {
                // This is a horizontal border
                if let Some(split_info) = self.find_split_for_border(window_id, false) {
                    return Some((
                        BorderInfo {
                            is_vertical: false,
                            split_node_path: split_info.0,
                            original_ratio: split_info.1,
                        },
                        window_id,
                    ));
                }
            }
        }

        None
    }

    /// Find the split node that controls the given border
    fn find_split_for_border(
        &self,
        window_id: WindowId,
        is_vertical_border: bool,
    ) -> Option<(Vec<usize>, f32)> {
        // Find if this window has a sibling that shares the border
        for (other_window_id, other_window) in &self.editor.windows {
            if other_window_id == window_id {
                continue;
            }

            let window = &self.editor.windows[window_id];

            if is_vertical_border {
                // Check if windows are horizontally adjacent
                if (window.x + window.width_chars == other_window.x
                    || other_window.x + other_window.width_chars == window.x)
                    && window.y < other_window.y + other_window.height_chars
                    && other_window.y < window.y + window.height_chars
                {
                    return Some((vec![0], 0.5)); // Simplified path and ratio
                }
            } else {
                // Check if windows are vertically adjacent
                if (window.y + window.height_chars == other_window.y
                    || other_window.y + other_window.height_chars == window.y)
                    && window.x < other_window.x + other_window.width_chars
                    && other_window.x < window.x + window.width_chars
                {
                    return Some((vec![0], 0.5)); // Simplified path and ratio
                }
            }
        }

        None
    }

    /// Handle border drag for window resizing
    fn handle_border_drag(&mut self, px: f64, py: f64) {
        let Some(ref drag_state) = self.editor.mouse_drag_state else {
            return;
        };

        let char_width = self.text_renderer.char_width() as f64;
        let line_height = self.text_renderer.line_height() as f64;

        // Convert to grid coordinates
        let grid_x = (px / char_width) as u16;
        let grid_y = (py / line_height) as u16;

        let new_pos = (grid_x, grid_y);
        let dx = new_pos.0 as i32 - drag_state.last_pos.0 as i32;
        let dy = new_pos.1 as i32 - drag_state.last_pos.1 as i32;

        if dx == 0 && dy == 0 {
            return;
        }

        let border_info = drag_state.border_info.clone();
        let target_window = drag_state.target_window;

        // Update drag state positions
        if let Some(ref mut drag_state_mut) = self.editor.mouse_drag_state {
            drag_state_mut.current_pos = new_pos;
            drag_state_mut.last_pos = new_pos;
        }

        let Some(border_info) = border_info else {
            return;
        };

        // Apply the resize
        update_window_resize_incremental(
            &mut self.editor.window_tree,
            &mut self.editor.windows,
            &self.editor.frame,
            target_window,
            &border_info,
            dx,
            dy,
        );

        // Recalculate window layout
        self.editor.calculate_window_layout();
    }
}

impl<'a> ApplicationHandler for RoeVelloApp<'a> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }

        let window = self.create_window(event_loop);
        let size = window.inner_size();
        let surface = pollster::block_on(self.render_cx.create_surface(
            window.clone(),
            size.width,
            size.height,
            wgpu::PresentMode::AutoVsync,
        ))
        .expect("Failed to create surface");

        self.state = Some(RenderState { window, surface });
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: winit::window::WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }
            WindowEvent::ModifiersChanged(new_modifiers) => {
                self.modifiers = new_modifiers.state();
            }
            WindowEvent::Resized(size) => {
                if let Some(ref mut state) = self.state {
                    self.render_cx
                        .resize_surface(&mut state.surface, size.width, size.height);
                    state.window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                // Poll for external file changes
                let file_change_actions = self.editor.poll_file_changes();
                for action in file_change_actions {
                    match action {
                        ChromeAction::Echo(msg) => {
                            self.editor.set_echo_message(msg);
                        }
                        ChromeAction::MarkDirty(_) => {
                            // Will be redrawn anyway
                        }
                        ChromeAction::BufferChanged {
                            buffer_id,
                            start,
                            old_end,
                            new_end,
                        } => {
                            // Call major mode after-change hook for syntax highlighting
                            let Some(buffer) = self.editor.buffers.get(buffer_id) else {
                                continue;
                            };
                            let Some(major_mode) = buffer.major_mode() else {
                                continue;
                            };
                            let Some(ref julia_runtime) = self.editor.julia_runtime else {
                                continue;
                            };

                            roe_core::julia_runtime::set_current_buffer(buffer.clone());
                            let runtime = pollster::block_on(julia_runtime.lock());
                            let _ = pollster::block_on(runtime.call_major_mode_after_change(
                                &major_mode,
                                start as i64,
                                old_end as i64,
                                new_end as i64,
                            ));
                            roe_core::julia_runtime::clear_current_buffer();
                        }
                        _ => {}
                    }
                }

                self.render();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let mut actions: std::collections::VecDeque<_> =
                    pollster::block_on(self.handle_key_event(event)).into();

                while let Some(action) = actions.pop_front() {
                    match action {
                        ChromeAction::Quit => {
                            self.quit_requested = true;
                            event_loop.exit();
                        }
                        ChromeAction::SplitHorizontal => {
                            self.editor.split_horizontal();
                        }
                        ChromeAction::SplitVertical => {
                            self.editor.split_vertical();
                        }
                        ChromeAction::SwitchWindow => {
                            self.editor.switch_window();
                        }
                        ChromeAction::DeleteWindow => {
                            self.editor.delete_window();
                        }
                        ChromeAction::DeleteOtherWindows => {
                            self.editor.delete_other_windows();
                        }
                        ChromeAction::Echo(msg) => {
                            self.editor.set_echo_message(msg);
                        }
                        ChromeAction::NewBufferWithMode {
                            buffer_name,
                            mode_name,
                            initial_content,
                        } => {
                            // Create a new buffer with the specified mode (e.g., Julia REPL)
                            let cursor_pos = initial_content.len();
                            if let Some(buffer_id) = self.editor.create_buffer_with_mode(
                                buffer_name,
                                mode_name,
                                initial_content,
                            ) {
                                // Switch current window to the new buffer
                                if let Some(current_window) =
                                    self.editor.windows.get_mut(self.editor.active_window)
                                {
                                    current_window.active_buffer = buffer_id;
                                    current_window.cursor = cursor_pos;
                                }
                            }
                        }
                        ChromeAction::ShowMessages => {
                            // Create or show messages buffer
                            let messages_buffer_id = self.editor.get_messages_buffer();
                            if let Some(current_window) =
                                self.editor.windows.get_mut(self.editor.active_window)
                            {
                                current_window.active_buffer = messages_buffer_id;
                                current_window.cursor = 0;
                            }
                        }
                        ChromeAction::BufferChanged {
                            buffer_id,
                            start,
                            old_end,
                            new_end,
                        } => {
                            // Call major mode after-change hook for syntax highlighting
                            let Some(buffer) = self.editor.buffers.get(buffer_id) else {
                                continue;
                            };
                            let Some(major_mode) = buffer.major_mode() else {
                                continue;
                            };
                            let Some(ref julia_runtime) = self.editor.julia_runtime else {
                                continue;
                            };

                            roe_core::julia_runtime::set_current_buffer(buffer.clone());
                            let runtime = pollster::block_on(julia_runtime.lock());
                            let _ = pollster::block_on(runtime.call_major_mode_after_change(
                                &major_mode,
                                start as i64,
                                old_end as i64,
                                new_end as i64,
                            ));
                            roe_core::julia_runtime::clear_current_buffer();
                        }
                        ChromeAction::ExecuteCommand(command_name) => {
                            // Execute another command via the command registry
                            let context = self.editor.create_command_context();
                            if self.editor.julia_runtime.is_some() {
                                match pollster::block_on(
                                    roe_core::command_mode::CommandMode::execute_command(
                                        &command_name,
                                        &self.editor.command_registry,
                                        context,
                                    ),
                                ) {
                                    Ok(command_actions) => {
                                        // Process through editor to handle BufferOps etc.
                                        let processed =
                                            self.editor.process_chrome_actions(command_actions);
                                        for a in processed {
                                            actions.push_back(a);
                                        }
                                    }
                                    Err(error_msg) => {
                                        self.editor.set_echo_message(format!(
                                            "Command error: {error_msg}"
                                        ));
                                    }
                                }
                            }
                        }
                        ChromeAction::FileWatcherStatus => {
                            let status = self.editor.file_watcher.status();
                            self.editor.set_echo_message(status);
                        }
                        _ => {}
                    }
                }

                // Request redraw after key events
                if let Some(ref state) = self.state {
                    state.window.request_redraw();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                // Convert physical to logical coordinates
                let scale_factor = self
                    .state
                    .as_ref()
                    .map(|s| s.window.scale_factor())
                    .unwrap_or(1.0);
                let logical_x = position.x / scale_factor;
                let logical_y = position.y / scale_factor;

                self.cursor_position = Some((logical_x, logical_y));

                // Handle window border dragging (for resizing splits)
                if self.editor.mouse_drag_state.is_some() {
                    self.handle_border_drag(logical_x, logical_y);
                    if let Some(ref render_state) = self.state {
                        render_state.window.request_redraw();
                    }
                }
                // Handle vertical scrollbar dragging
                else if self.scrollbar_dragging.is_some() {
                    self.handle_scrollbar_drag(logical_y);
                    if let Some(ref render_state) = self.state {
                        render_state.window.request_redraw();
                    }
                }
                // Handle horizontal scrollbar dragging
                else if self.hscrollbar_dragging.is_some() {
                    self.handle_hscrollbar_drag(logical_x);
                    if let Some(ref render_state) = self.state {
                        render_state.window.request_redraw();
                    }
                }
                // Handle text selection drag
                else if self.mouse_dragging {
                    self.handle_mouse_drag(logical_x, logical_y);
                    if let Some(ref render_state) = self.state {
                        render_state.window.request_redraw();
                    }
                }

                // Update cursor icon based on hover state
                if let Some(ref state) = self.state {
                    let cursor = if self.editor.mouse_drag_state.is_some() {
                        // Check if dragging vertical or horizontal border
                        if let Some(ref drag_state) = self.editor.mouse_drag_state {
                            if let Some(ref border_info) = drag_state.border_info {
                                if border_info.is_vertical {
                                    CursorIcon::ColResize
                                } else {
                                    CursorIcon::RowResize
                                }
                            } else {
                                CursorIcon::Default
                            }
                        } else {
                            CursorIcon::Default
                        }
                    } else if self.scrollbar_dragging.is_some()
                        || self.hscrollbar_dragging.is_some()
                    {
                        CursorIcon::Grabbing
                    } else if let Some((border_info, _)) =
                        self.check_border_hit(logical_x, logical_y)
                    {
                        // Show resize cursor when hovering over draggable borders
                        if border_info.is_vertical {
                            CursorIcon::ColResize
                        } else {
                            CursorIcon::RowResize
                        }
                    } else if self.check_scrollbar_hit(logical_x, logical_y).is_some()
                        || self.check_hscrollbar_hit(logical_x, logical_y).is_some()
                    {
                        CursorIcon::Grab
                    } else {
                        CursorIcon::Text
                    };
                    state.window.set_cursor(cursor);
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if button == MouseButton::Left {
                    match state {
                        ElementState::Pressed => {
                            if let Some((x, y)) = self.cursor_position {
                                // Check if click is on a window border (for resizing splits)
                                if let Some((border_info, target_window)) =
                                    self.check_border_hit(x, y)
                                {
                                    let char_width = self.text_renderer.char_width() as f64;
                                    let line_height = self.text_renderer.line_height() as f64;
                                    let grid_x = (x / char_width) as u16;
                                    let grid_y = (y / line_height) as u16;

                                    self.editor.mouse_drag_state = Some(MouseDragState {
                                        drag_type: DragType::WindowBorder,
                                        start_pos: (grid_x, grid_y),
                                        last_pos: (grid_x, grid_y),
                                        current_pos: (grid_x, grid_y),
                                        target_window: Some(target_window),
                                        border_info: Some(border_info.clone()),
                                    });
                                    if let Some(ref state) = self.state {
                                        let cursor = if border_info.is_vertical {
                                            CursorIcon::ColResize
                                        } else {
                                            CursorIcon::RowResize
                                        };
                                        state.window.set_cursor(cursor);
                                    }
                                }
                                // Check if click is on vertical scrollbar
                                else if let Some((window_id, ratio)) =
                                    self.check_scrollbar_hit(x, y)
                                {
                                    self.handle_scrollbar_click(window_id, ratio);
                                    self.scrollbar_dragging = Some(window_id);
                                    if let Some(ref state) = self.state {
                                        state.window.set_cursor(CursorIcon::Grabbing);
                                    }
                                }
                                // Check horizontal scrollbar
                                else if let Some((window_id, ratio)) =
                                    self.check_hscrollbar_hit(x, y)
                                {
                                    self.handle_hscrollbar_click(window_id, ratio);
                                    self.hscrollbar_dragging = Some(window_id);
                                    if let Some(ref state) = self.state {
                                        state.window.set_cursor(CursorIcon::Grabbing);
                                    }
                                } else {
                                    // Normal text click
                                    pollster::block_on(self.handle_mouse_click(x, y));
                                    // Save cursor position for potential drag selection
                                    let cursor =
                                        self.editor.windows[self.editor.active_window].cursor;
                                    self.drag_start_cursor = Some(cursor);
                                    self.mouse_dragging = true;
                                }
                                if let Some(ref render_state) = self.state {
                                    render_state.window.request_redraw();
                                }
                            }
                        }
                        ElementState::Released => {
                            self.mouse_dragging = false;
                            self.drag_start_cursor = None;
                            self.scrollbar_dragging = None;
                            self.hscrollbar_dragging = None;
                            // Clear border drag state
                            if self.editor.mouse_drag_state.is_some() {
                                self.editor.mouse_drag_state = None;
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// Update window layout based on incremental mouse drag
fn update_window_resize_incremental(
    window_tree: &mut WindowNode,
    _windows: &mut slotmap::SlotMap<WindowId, roe_core::editor::Window>,
    _frame: &roe_core::editor::Frame,
    _target_window_id: Option<WindowId>,
    border_info: &BorderInfo,
    dx: i32,
    dy: i32,
) {
    // Use a sensitivity factor to make resizing smoother
    // Each pixel of mouse movement = 0.5% ratio change
    const SENSITIVITY: f32 = 0.005;

    // Calculate the incremental ratio change
    if border_info.is_vertical && dx != 0 {
        // For vertical borders, adjust the split ratio based on horizontal movement
        let ratio_change = dx as f32 * SENSITIVITY;
        adjust_window_tree_ratio_incremental(window_tree, ratio_change, true);
    } else if !border_info.is_vertical && dy != 0 {
        // For horizontal borders, adjust the split ratio based on vertical movement
        let ratio_change = dy as f32 * SENSITIVITY;
        adjust_window_tree_ratio_incremental(window_tree, ratio_change, false);
    }
}

/// Recursively adjust window tree ratios for incremental resizing
fn adjust_window_tree_ratio_incremental(
    node: &mut WindowNode,
    ratio_change: f32,
    is_vertical: bool,
) {
    match node {
        WindowNode::Leaf { .. } => {
            // Nothing to adjust for leaf nodes
        }
        WindowNode::Split {
            direction,
            ratio,
            first,
            second,
        } => {
            // Only adjust if the split direction matches the resize direction
            let should_adjust = match direction {
                SplitDirection::Vertical => is_vertical,
                SplitDirection::Horizontal => !is_vertical,
            };

            if should_adjust {
                // Adjust the ratio incrementally, keeping it within bounds
                // Use tighter bounds to prevent extreme layouts
                *ratio = (*ratio + ratio_change).clamp(0.15, 0.85);
            } else {
                // Recurse into child nodes
                adjust_window_tree_ratio_incremental(first, ratio_change, is_vertical);
                adjust_window_tree_ratio_incremental(second, ratio_change, is_vertical);
            }
        }
    }
}

/// Load theme settings from Julia runtime
async fn load_theme_from_julia(editor: &Editor) -> VelloTheme {
    let mut theme = VelloTheme::default();

    let Some(ref julia_runtime) = editor.julia_runtime else {
        return theme;
    };

    let runtime = julia_runtime.lock().await;

    // Color keys to load (supports both "colours" and "colors" spelling)
    let color_keys = [
        ("background", "bg"),
        ("foreground", "fg"),
        ("selection", "sel"),
        ("modeline", "mode-line"),
        ("modeline_inactive", "mode-line-inactive"),
        ("border", "border"),
        ("border_active", "active-border"),
        ("cursor", "cursor"),
        ("rune", "rune"),
    ];

    for (key, alias) in color_keys {
        // Try "colours.key" first, then "colors.key"
        let value = match runtime.get_config(&format!("colours.{}", key)).await {
            Ok(Some(v)) => Some(v),
            _ => runtime
                .get_config(&format!("colors.{}", key))
                .await
                .ok()
                .flatten(),
        };

        if let Some(config_value) = value {
            if let Some(color_str) = config_value.as_string() {
                theme.set_color(alias, &color_str);
            }
        }
    }

    // Load font settings - try "font.family" or "font.name"
    let font_family = match runtime.get_config("font.family").await {
        Ok(Some(v)) => v.as_string(),
        _ => runtime
            .get_config("font.name")
            .await
            .ok()
            .flatten()
            .and_then(|v| v.as_string()),
    };
    if let Some(family) = font_family {
        theme.set_font_family(&family);
    }

    // Load font size
    let font_size = match runtime.get_config("font.size").await {
        Ok(Some(v)) => v.as_integer().map(|i| i as f32),
        _ => None,
    };
    if let Some(size) = font_size {
        theme.set_font_size(size);
    }

    theme
}

/// Run the editor with the Vello renderer
pub fn run_vello(editor: &mut Editor) -> Result<(), Box<dyn std::error::Error>> {
    // Load theme from Julia config
    let theme = pollster::block_on(load_theme_from_julia(editor));

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Wait);

    let mut app = RoeVelloApp::new(editor, theme);
    event_loop.run_app(&mut app)?;

    Ok(())
}
