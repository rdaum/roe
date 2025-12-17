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
pub use theme::VelloTheme;

use roe_core::editor::ChromeAction;
use roe_core::Editor;
use std::sync::Arc;
use text::TextRenderer;
use vello::kurbo::{Affine, Rect};
use vello::util::{RenderContext, RenderSurface};
use vello::wgpu;
use vello::{AaConfig, RenderParams, RendererOptions, Scene};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::ModifiersState;
use winit::window::{Window, WindowId};

/// Default window dimensions
const DEFAULT_WIDTH: u32 = 1200;
const DEFAULT_HEIGHT: u32 = 800;

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
        }
    }

    fn create_window(&mut self, event_loop: &ActiveEventLoop) -> Arc<Window> {
        let attrs = Window::default_attributes()
            .with_title("Roe - Ryan's Own Emacs")
            .with_inner_size(LogicalSize::new(DEFAULT_WIDTH, DEFAULT_HEIGHT));

        Arc::new(event_loop.create_window(attrs).expect("Failed to create window"))
    }

    fn render(&mut self) {
        // Extract surface info first to avoid borrow conflicts
        let (width, height, dev_id, format) = {
            let Some(ref state) = self.state else {
                return;
            };
            (
                state.surface.config.width,
                state.surface.config.height,
                state.surface.dev_id,
                state.surface.format,
            )
        };

        // Get dimensions from text renderer
        let char_width = self.text_renderer.char_width();
        let line_height = self.text_renderer.line_height();

        // Update editor frame dimensions
        let cols = (width as f32 / char_width).floor() as u16;
        let lines = (height as f32 / line_height).floor() as u16;
        self.editor.handle_resize(cols.max(1), lines.saturating_sub(1).max(1)); // -1 for echo area

        // Build the scene
        self.scene.reset();
        self.build_scene(width, height);

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

        // Get buffer info
        let buffer = &self.editor.buffers[window.active_buffer];
        let content_x = x + char_width;
        let content_y = y + line_height;
        let content_height = window.height_chars.saturating_sub(2) as usize;
        let start_line = window.start_line as usize;
        let content_width = (window.width_chars.saturating_sub(2) as f64 * char_width) as f32;

        // Get selection region (only for active window)
        let region_bounds = if is_active {
            buffer.get_region(window.cursor)
        } else {
            None
        };

        // Collect lines to render with their buffer positions
        let lines_to_render: Vec<(usize, usize, String)> = buffer
            .buffer_lines()
            .into_iter()
            .enumerate()
            .filter(|(idx, _)| *idx >= start_line && (*idx - start_line) < content_height)
            .map(|(idx, text)| {
                let line_start_pos = buffer.to_char_index(0, idx as u16);
                (idx - start_line, line_start_pos, text.trim_end_matches('\n').to_string())
            })
            .collect();

        // Draw selection highlights first (behind text)
        if let Some((region_start, region_end)) = region_bounds {
            let selection_color = self.theme.selection_color;
            for (visual_line, line_start_pos, line_text) in &lines_to_render {
                let line_end_pos = line_start_pos + line_text.len();

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
                        line_text.len()
                    };

                    // Draw selection rectangle
                    let sel_x = content_x + (sel_start_in_line as f64 * char_width);
                    let sel_y = content_y + (*visual_line as f64 * line_height);
                    let sel_width = (sel_end_in_line - sel_start_in_line) as f64 * char_width;

                    // Extend selection to end of line if region extends past line content
                    let sel_width = if region_end > line_end_pos && sel_end_in_line == line_text.len() {
                        // Extend to fill remaining visible width
                        (content_width as f64) - (sel_start_in_line as f64 * char_width)
                    } else {
                        sel_width
                    };

                    let sel_rect = Rect::new(sel_x, sel_y, sel_x + sel_width, sel_y + line_height);
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

        // Render each line of text
        let fg_color = self.theme.fg_color;
        for (visual_line, _line_start_pos, line_text) in lines_to_render {
            let text_x = content_x as f32;
            let text_y = content_y as f32 + (visual_line as f32) * line_height as f32;

            self.text_renderer.render_line(
                &mut self.scene,
                &line_text,
                text_x,
                text_y,
                fg_color,
                Some(content_width),
            );
        }

        // Draw modeline text
        let buffer_name = buffer.object();
        let (col, line) = buffer.to_column_line(window.cursor);
        let modeline_text = if is_active {
            format!(" ᚱᛟ {} {}:{}", buffer_name, line + 1, col + 1)
        } else {
            format!("    {} {}:{}", buffer_name, line + 1, col + 1)
        };

        self.text_renderer.render_line(
            &mut self.scene,
            &modeline_text,
            (x + char_width) as f32,
            modeline_y as f32,
            self.theme.fg_color,
            Some(content_width),
        );

        // Draw cursor
        if is_active {
            let (col, line) = buffer.to_column_line(window.cursor);
            let line = line as usize;
            let col = col as usize;
            if line >= start_line {
                let cursor_visual_line = line - start_line;
                if cursor_visual_line < content_height {
                    let cursor_x = content_x + (col as f64 * char_width);
                    let cursor_y = content_y + (cursor_visual_line as f64) * line_height;

                    let cursor_rect = Rect::new(cursor_x, cursor_y, cursor_x + 2.0, cursor_y + line_height);
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

    async fn handle_key_event(
        &mut self,
        event: winit::event::KeyEvent,
    ) -> Vec<ChromeAction> {
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

        // Convert to buffer position
        let buffer_line = relative_y as usize + window.start_line as usize;
        let buffer_col = relative_x as usize;

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
        let line_text = buffer.buffer_lines().into_iter().nth(clamped_line).unwrap_or_default();
        let line_len = line_text.trim_end_matches('\n').len();
        let clamped_col = buffer_col.min(line_len);

        // Get the new cursor position using clamped values
        let new_cursor = buffer.to_char_index(clamped_col as u16, clamped_line as u16);

        // Final safety clamp to buffer length
        let buffer_len = buffer.buffer_len_chars();
        let clamped_cursor = if buffer_len == 0 { 0 } else { new_cursor.min(buffer_len - 1) };

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
        _window_id: WindowId,
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
                    self.render_cx.resize_surface(&mut state.surface, size.width, size.height);
                    state.window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                self.render();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let actions = pollster::block_on(self.handle_key_event(event));

                for action in actions {
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
                        _ => {}
                    }
                }

                // Request redraw after key events
                if let Some(ref state) = self.state {
                    state.window.request_redraw();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_position = Some((position.x, position.y));
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if state == ElementState::Pressed && button == MouseButton::Left {
                    if let Some((x, y)) = self.cursor_position {
                        pollster::block_on(self.handle_mouse_click(x, y));
                        // Request redraw after mouse click
                        if let Some(ref render_state) = self.state {
                            render_state.window.request_redraw();
                        }
                    }
                }
            }
            _ => {}
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
            _ => runtime.get_config(&format!("colors.{}", key)).await.ok().flatten(),
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
