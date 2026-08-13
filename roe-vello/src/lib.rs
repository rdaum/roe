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

use roe_core::Editor;
use roe_core::gutter::{GutterConfig, calculate_gutter_width, format_line_number};
use roe_core::native_kernel::{CapabilityGrants, ViewId};
use roe_core::native_services::FrontendWake;
use roe_core::renderer::DirtyRegion;
use roe_core::session::{
    HostSession, InputEvent, LifecycleEvent, PointerButton, PointerEvent, PointerKind,
    PresentationColor, PresentedView, SessionOutput, StyleDefinition,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use text::TextRenderer;
use thiserror::Error;
use vello::kurbo::{Affine, Rect};
use vello::peniko::{Color, Fill};
use vello::util::{RenderContext, RenderSurface};
use vello::wgpu;
use vello::{AaConfig, RenderParams, RendererOptions, Scene};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::ModifiersState;
use winit::window::{CursorIcon, Window};

/// Default window dimensions
const DEFAULT_WIDTH: u32 = 1200;
const DEFAULT_HEIGHT: u32 = 800;

#[derive(Debug, Error)]
pub enum FrontendError {
    #[error("failed to create Vello window: {0}")]
    Window(#[source] winit::error::OsError),
    #[error("failed to create Vello render surface: {0}")]
    Surface(#[source] vello::Error),
    #[error("Vello renderer failed: {0}")]
    Renderer(#[source] vello::Error),
    #[error("failed to capture logical presentation: {0}")]
    Presentation(#[source] std::io::Error),
    #[error("editor session failed: {0}")]
    Session(#[source] roe_core::session::SessionError),
    #[error("failed to start Mica editor host: {0}")]
    MicaHost(#[source] roe_core::mica_host::MicaHostError),
    #[error("Vello renderer state is inconsistent: {0}")]
    InvalidState(&'static str),
    #[error("Vello event loop failed: {0}")]
    EventLoop(#[source] winit::error::EventLoopError),
}

#[derive(Debug, Clone, Copy)]
enum HostEvent {
    Wake,
}

#[derive(Default)]
struct WakeState {
    pending: AtomicBool,
}

impl WakeState {
    fn request(&self, send: impl FnOnce() -> bool) {
        if self
            .pending
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
            && !send()
        {
            self.pending.store(false, Ordering::Release);
        }
    }

    fn acknowledge(&self) {
        self.pending.store(false, Ordering::Release);
    }
}

struct WinitWake {
    proxy: EventLoopProxy<HostEvent>,
    state: Arc<WakeState>,
}

impl FrontendWake for WinitWake {
    fn wake(&self) {
        self.state.request(|| {
            let sent = self.proxy.send_event(HostEvent::Wake).is_ok();
            if !sent {
                tracing::debug!("Vello event loop already closed");
            }
            sent
        });
    }
}

fn pump_runtime(runtime: &compio::runtime::Runtime) {
    runtime.enter(|| {
        runtime.poll_with(Some(Duration::ZERO));
        runtime.run();
    });
}

fn session_vello_color(color: &PresentationColor, default: Color) -> Color {
    match color {
        PresentationColor::Rgb { r, g, b } => Color::from_rgb8(*r, *g, *b),
        PresentationColor::Named(name) => match name.as_str() {
            "black" => Color::BLACK,
            "white" => Color::WHITE,
            "red" => Color::from_rgb8(255, 0, 0),
            "green" => Color::from_rgb8(0, 255, 0),
            "blue" => Color::from_rgb8(0, 0, 255),
            "yellow" => Color::from_rgb8(255, 255, 0),
            "cyan" => Color::from_rgb8(0, 255, 255),
            "magenta" => Color::from_rgb8(255, 0, 255),
            _ => default,
        },
        PresentationColor::Inherit => default,
    }
}

/// Scrollbar width in logical pixels
const SCROLLBAR_WIDTH: f64 = 14.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SessionViewMetrics {
    content_width_chars: usize,
    content_rows: usize,
    horizontal_overflow: bool,
}

fn session_view_metrics(view: &PresentedView, char_width: f64) -> SessionViewMetrics {
    let gutter_chars = if view.show_gutter {
        calculate_gutter_width(view.total_lines, &GutterConfig::default())
    } else {
        0
    };
    let width = f64::from(view.geometry.columns) * char_width;
    let gutter_width = gutter_chars as f64 * char_width;
    let content_width =
        (width - (2.0 * char_width) - SCROLLBAR_WIDTH - 4.0 - gutter_width).max(0.0);
    let content_width_chars = (content_width / char_width).floor() as usize;
    let content_rows = view.geometry.rows.saturating_sub(3) as usize;
    SessionViewMetrics {
        content_width_chars,
        content_rows,
        horizontal_overflow: view.max_line_chars > content_width_chars,
    }
}

/// Gutter colors
const GUTTER_FG_COLOR: Color = Color::from_rgba8(0x60, 0x60, 0x60, 0xFF); // Dimmed line numbers

/// Application state for the Vello renderer
pub struct RoeVelloApp<'a> {
    /// The shared editor host/session boundary.
    session: HostSession,
    /// The compio runtime driving buffer host tasks
    runtime: compio::runtime::Runtime,
    /// Vello render context
    render_cx: RenderContext,
    /// The renderer
    renderers: Vec<Option<vello::Renderer>>,
    redraw_state: VelloRenderer,
    /// Current render state (window + surface)
    state: Option<RenderState<'a>>,
    /// Coalesces host wakeups to at most one queued Winit user event.
    wake_state: Arc<WakeState>,
    fatal_error: Option<FrontendError>,
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
    /// Whether vertical scrollbar is being dragged
    scrollbar_dragging: Option<ViewId>,
    /// Whether horizontal scrollbar is being dragged
    hscrollbar_dragging: Option<ViewId>,
    border_dragging: Option<bool>,
}

struct RenderState<'s> {
    surface: RenderSurface<'s>,
    window: Arc<Window>,
}

impl<'a> RoeVelloApp<'a> {
    fn new(
        editor: Editor,
        theme: VelloTheme,
        runtime: compio::runtime::Runtime,
        wake_state: Arc<WakeState>,
    ) -> Result<Self, FrontendError> {
        let font_size = theme.font_size;
        let font_family = if theme.font_family.is_empty() {
            None
        } else {
            Some(theme.font_family.clone())
        };

        let mut session = HostSession::open_with_mica(editor, CapabilityGrants::editor_default())
            .map_err(FrontendError::MicaHost)?;
        let initial = runtime.block_on(session.initial_output());
        let mut redraw_state = VelloRenderer::with_theme(theme.clone());
        if let Some(update) = initial.presentation.as_ref() {
            redraw_state
                .apply_session_presentation(update)
                .expect("initial session snapshot must be valid");
        }

        Ok(Self {
            session,
            runtime,
            render_cx: RenderContext::new(),
            renderers: vec![],
            redraw_state,
            state: None,
            wake_state,
            fatal_error: None,
            scene: Scene::new(),
            text_renderer: TextRenderer::new(font_size, font_family),
            theme,
            quit_requested: false,
            modifiers: ModifiersState::empty(),
            cursor_position: None,
            mouse_dragging: false,
            scrollbar_dragging: None,
            hscrollbar_dragging: None,
            border_dragging: None,
        })
    }

    fn request_redraw(&mut self, region: DirtyRegion) {
        tracing::trace!(?region, "Vello redraw requested");
        self.redraw_state.invalidate(region);
        if let Some(ref state) = self.state {
            state.window.request_redraw();
        }
    }

    fn drive_background(&mut self) {
        pump_runtime(&self.runtime);
        let envelope = self.session.envelope(InputEvent::Timer { token: 0 });
        match self.runtime.block_on(self.session.dispatch(envelope)) {
            Ok(output) => self.apply_session_output(output),
            Err(error) => {
                self.fatal_error = Some(FrontendError::Session(error));
                self.quit_requested = true;
            }
        }

        if self.redraw_state.needs_redraw()
            && let Some(state) = self.state.as_ref()
        {
            state.window.request_redraw();
        }
    }

    fn apply_session_output(&mut self, output: SessionOutput) {
        for event in output.lifecycle {
            match event {
                LifecycleEvent::QuitRequested | LifecycleEvent::EndpointClosed => {
                    self.quit_requested = true;
                }
                LifecycleEvent::Warning(message) => tracing::warn!(%message, "session warning"),
                LifecycleEvent::Error(message) => tracing::error!(%message, "session error"),
                LifecycleEvent::Fatal(message) => {
                    tracing::error!(%message, "fatal session error");
                    self.quit_requested = true;
                }
                LifecycleEvent::Overloaded { detail } => {
                    tracing::warn!(%detail, "session overload")
                }
                LifecycleEvent::RecoveryResult { operation, result } => match result {
                    Ok(_) => tracing::info!(%operation, "Mica recovery operation completed"),
                    Err(error) => {
                        tracing::error!(%operation, %error, "Mica recovery operation failed")
                    }
                },
                LifecycleEvent::Ready { .. }
                | LifecycleEvent::Heartbeat
                | LifecycleEvent::MicaTaskCancelled { .. }
                | LifecycleEvent::MicaSubscriptionReady { .. }
                | LifecycleEvent::RequestCancelled { .. }
                | LifecycleEvent::ResourceChanged { .. }
                | LifecycleEvent::ResourceInvalidated { .. } => {}
            }
        }
        if let Some(update) = output.presentation {
            if let Err(error) = self.redraw_state.apply_session_presentation(&update) {
                self.fatal_error = Some(FrontendError::Presentation(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    error,
                )));
                self.quit_requested = true;
                return;
            }
            self.redraw_state.invalidate(DirtyRegion::FullScreen);
        }
    }

    fn create_window(
        &mut self,
        event_loop: &ActiveEventLoop,
    ) -> Result<Arc<Window>, winit::error::OsError> {
        let attrs = Window::default_attributes()
            .with_title("Roe - Ryan's Own Emacs")
            .with_inner_size(LogicalSize::new(DEFAULT_WIDTH, DEFAULT_HEIGHT));

        event_loop.create_window(attrs).map(Arc::new)
    }

    fn render(&mut self) -> Result<(), FrontendError> {
        // Extract surface info first to avoid borrow conflicts
        let (width, height, dev_id, scale_factor) = {
            let Some(ref state) = self.state else {
                return Ok(());
            };
            (
                state.surface.config.width,
                state.surface.config.height,
                state.surface.dev_id,
                state.window.scale_factor(),
            )
        };

        // Convert to logical dimensions for layout calculations
        let logical_width = (width as f64 / scale_factor) as u32;
        let logical_height = (height as f64 / scale_factor) as u32;

        // Build the scene in logical coordinates, then scale for physical rendering
        self.scene.reset();
        self.build_session_scene(logical_width, logical_height)?;

        // Apply scale factor transform to the scene
        if scale_factor != 1.0 {
            let mut scaled_scene = Scene::new();
            scaled_scene.append(&self.scene, Some(Affine::scale(scale_factor)));
            self.scene = scaled_scene;
        }

        // Now get the surface texture
        let Some(ref mut state) = self.state else {
            return Ok(());
        };
        let surface_texture = match state.surface.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture)
            | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => texture,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Outdated
            | wgpu::CurrentSurfaceTexture::Lost
            | wgpu::CurrentSurfaceTexture::Validation => {
                self.redraw_state.invalidate(DirtyRegion::FullScreen);
                return Ok(());
            }
        };

        let device_handle = &self.render_cx.devices[dev_id];

        // Ensure we have a renderer for this device
        if self.renderers.len() <= dev_id {
            self.renderers.resize_with(dev_id + 1, || None);
        }
        if self.renderers[dev_id].is_none() {
            let renderer = vello::Renderer::new(
                &device_handle.device,
                RendererOptions {
                    use_cpu: false,
                    antialiasing_support: vello::AaSupport::all(),
                    num_init_threads: None,
                    pipeline_cache: None,
                },
            )
            .map_err(FrontendError::Renderer)?;
            self.renderers[dev_id] = Some(renderer);
        }

        let Some(renderer) = self.renderers[dev_id].as_mut() else {
            return Err(FrontendError::InvalidState("renderer slot is empty"));
        };

        renderer
            .render_to_texture(
                &device_handle.device,
                &device_handle.queue,
                &self.scene,
                &state.surface.target_view,
                &RenderParams {
                    base_color: self.theme.bg_color,
                    width,
                    height,
                    antialiasing_method: AaConfig::Msaa16,
                },
            )
            .map_err(FrontendError::Renderer)?;

        self.redraw_state.redraw_complete();

        let surface_view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder =
            device_handle
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("vello_blit"),
                });
        state.surface.blitter.copy(
            &device_handle.device,
            &mut encoder,
            &state.surface.target_view,
            &surface_view,
        );
        device_handle.queue.submit(Some(encoder.finish()));
        surface_texture.present();
        Ok(())
    }

    fn build_session_scene(&mut self, width: u32, height: u32) -> Result<(), FrontendError> {
        let snapshot = self
            .redraw_state
            .session_presentation()
            .current()
            .cloned()
            .ok_or(FrontendError::InvalidState(
                "session has no logical presentation",
            ))?;
        let background = Rect::new(0.0, 0.0, width as f64, height as f64);
        self.scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            self.theme.bg_color,
            None,
            &background,
        );
        for view in &snapshot.views {
            self.draw_session_view(view, &snapshot.styles);
        }

        let line_height = f64::from(self.text_renderer.line_height());
        let echo_y = height as f64 - line_height;
        let echo_rect = Rect::new(0.0, echo_y, width as f64, height as f64);
        self.scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            self.theme.bg_color,
            None,
            &echo_rect,
        );
        if !snapshot.echo_area.is_empty() {
            self.text_renderer.render_line(
                &mut self.scene,
                &snapshot.echo_area,
                4.0,
                echo_y as f32,
                self.theme.fg_color,
                Some(width as f32 - 8.0),
            );
        }
        Ok(())
    }

    fn draw_session_view(&mut self, view: &PresentedView, styles: &[StyleDefinition]) {
        let char_width = f64::from(self.text_renderer.char_width());
        let line_height = f64::from(self.text_renderer.line_height());
        let x = f64::from(view.geometry.x) * char_width;
        let y = f64::from(view.geometry.y) * line_height;
        let width = f64::from(view.geometry.columns) * char_width;
        let height = f64::from(view.geometry.rows) * line_height;
        let border = if view.active {
            self.theme.active_border_color
        } else {
            self.theme.border_color
        };
        self.scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            self.theme.bg_color,
            None,
            &Rect::new(x, y, x + width, y + height),
        );
        for rect in [
            Rect::new(x, y, x + width, y + 2.0),
            Rect::new(x, y, x + 2.0, y + height),
            Rect::new(x + width - 2.0, y, x + width, y + height),
        ] {
            self.scene
                .fill(Fill::NonZero, Affine::IDENTITY, border, None, &rect);
        }
        let modeline_y = y + height - line_height;
        let modeline_color = if view.active {
            self.theme.mode_line_bg_color
        } else {
            self.theme.inactive_mode_line_bg_color
        };
        self.scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            modeline_color,
            None,
            &Rect::new(x, modeline_y, x + width, modeline_y + line_height),
        );
        self.text_renderer.render_line(
            &mut self.scene,
            &view.modeline,
            (x + 4.0) as f32,
            modeline_y as f32,
            self.theme.fg_color,
            Some((width - 8.0) as f32),
        );

        let gutter_chars = if view.show_gutter {
            calculate_gutter_width(view.total_lines, &GutterConfig::default())
        } else {
            0
        };
        let content_x = x + char_width * (1 + gutter_chars) as f64;
        // Vello reserves a right-hand lane for its vertical scrollbar and one
        // row above the modeline for horizontal scrolling. Text must not be
        // realized underneath renderer-owned chrome.
        let metrics = session_view_metrics(view, char_width);
        let content_width_chars = metrics.content_width_chars;
        let content_rows = metrics.content_rows;
        let mut absolute = view.visible_start_char;
        let mut lines: Vec<&str> = view.visible_text.split_inclusive('\n').collect();
        if lines.is_empty() {
            lines.push("");
        }
        for (row, raw_line) in lines.into_iter().take(content_rows).enumerate() {
            let line = raw_line.trim_end_matches('\n');
            let displayed: String = line
                .chars()
                .skip(usize::from(view.scroll.start_column))
                .take(content_width_chars)
                .collect();
            let line_y = y + line_height * (row + 1) as f64;
            if view.show_gutter {
                let line_number = usize::from(view.scroll.start_line) + row + 1;
                let label = format_line_number(line_number, gutter_chars.saturating_sub(2));
                self.text_renderer.render_line(
                    &mut self.scene,
                    &format!(" {label}│"),
                    (x + char_width) as f32,
                    line_y as f32,
                    GUTTER_FG_COLOR,
                    None,
                );
            }
            let visible_column = usize::from(view.scroll.start_column);
            let display_start = absolute + visible_column;
            let display_end = display_start + displayed.chars().count();
            if let Some(selection) = view.selection {
                let start = selection.anchor.min(selection.active).max(display_start);
                let end = selection.anchor.max(selection.active).min(display_end);
                if start < end {
                    let left = content_x + (start - display_start) as f64 * char_width;
                    let right = content_x + (end - display_start) as f64 * char_width;
                    self.scene.fill(
                        Fill::NonZero,
                        Affine::IDENTITY,
                        self.theme.selection_color,
                        None,
                        &Rect::new(left, line_y, right, line_y + line_height),
                    );
                }
            }
            if view.active && view.cursor >= display_start && view.cursor <= display_end {
                let cursor_x = content_x + (view.cursor - display_start) as f64 * char_width;
                self.scene.fill(
                    Fill::NonZero,
                    Affine::IDENTITY,
                    self.theme.cursor_color,
                    None,
                    &Rect::new(cursor_x, line_y, cursor_x + 2.0, line_y + line_height),
                );
            }
            let spans: Vec<StyledSpan> = view
                .styled_ranges
                .iter()
                .filter_map(|range| {
                    let start = range.start.max(absolute + visible_column);
                    let end = range
                        .end
                        .min(absolute + visible_column + displayed.chars().count());
                    if start >= end {
                        return None;
                    }
                    let style = styles.iter().find(|style| style.id == range.style)?;
                    let color = style
                        .foreground
                        .as_ref()
                        .map(|color| session_vello_color(color, self.theme.fg_color))
                        .unwrap_or(self.theme.fg_color);
                    Some(
                        StyledSpan::new(
                            start - absolute - visible_column,
                            end - absolute - visible_column,
                            color,
                        )
                        .with_bold(style.bold)
                        .with_italic(style.italic),
                    )
                })
                .collect();
            self.text_renderer.render_line_with_styles(
                &mut self.scene,
                &displayed,
                content_x as f32,
                line_y as f32,
                self.theme.fg_color,
                &spans,
            );
            absolute += line.chars().count() + usize::from(raw_line.ends_with('\n'));
        }

        let scrollbar_top = y + 2.0;
        let scrollbar_extent = (height - line_height - 4.0).max(1.0);
        let scrollbar_x = x + width - SCROLLBAR_WIDTH - 2.0;
        self.scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            Color::from_rgba8(0x40, 0x40, 0x40, 0x80),
            None,
            &Rect::new(
                scrollbar_x,
                scrollbar_top,
                scrollbar_x + SCROLLBAR_WIDTH,
                scrollbar_top + scrollbar_extent,
            ),
        );
        let visible_lines = content_rows.max(1);
        let vertical_fraction = (visible_lines as f64 / view.total_lines.max(1) as f64).min(1.0);
        let thumb_height = (scrollbar_extent * vertical_fraction)
            .max(20.0)
            .min(scrollbar_extent);
        let max_line = view.total_lines.saturating_sub(visible_lines);
        let vertical_position = if max_line == 0 {
            0.0
        } else {
            f64::from(view.scroll.start_line) / max_line as f64
        };
        let thumb_y = scrollbar_top + vertical_position * (scrollbar_extent - thumb_height);
        self.scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            if view.active {
                self.theme.active_border_color
            } else {
                self.theme.border_color
            },
            None,
            &Rect::new(
                scrollbar_x + 2.0,
                thumb_y,
                scrollbar_x + SCROLLBAR_WIDTH - 2.0,
                thumb_y + thumb_height,
            ),
        );

        if metrics.horizontal_overflow {
            let horizontal_x = x + 2.0;
            let horizontal_y = y + height - line_height - SCROLLBAR_WIDTH - 2.0;
            let horizontal_extent = (width - SCROLLBAR_WIDTH - 6.0).max(1.0);
            self.scene.fill(
                Fill::NonZero,
                Affine::IDENTITY,
                Color::from_rgba8(0x40, 0x40, 0x40, 0x80),
                None,
                &Rect::new(
                    horizontal_x,
                    horizontal_y,
                    horizontal_x + horizontal_extent,
                    horizontal_y + SCROLLBAR_WIDTH,
                ),
            );
            let visible_columns = content_width_chars.max(1);
            let horizontal_fraction =
                (visible_columns as f64 / view.max_line_chars.max(1) as f64).min(1.0);
            let thumb_width = (horizontal_extent * horizontal_fraction)
                .max(20.0)
                .min(horizontal_extent);
            let max_column = view.max_line_chars.saturating_sub(visible_columns);
            let horizontal_position = if max_column == 0 {
                0.0
            } else {
                f64::from(view.scroll.start_column) / max_column as f64
            };
            let thumb_x = horizontal_x + horizontal_position * (horizontal_extent - thumb_width);
            self.scene.fill(
                Fill::NonZero,
                Affine::IDENTITY,
                if view.active {
                    self.theme.active_border_color
                } else {
                    self.theme.border_color
                },
                None,
                &Rect::new(
                    thumb_x,
                    horizontal_y + 2.0,
                    thumb_x + thumb_width,
                    horizontal_y + SCROLLBAR_WIDTH - 2.0,
                ),
            );
        }
    }

    async fn handle_key_event(&mut self, event: winit::event::KeyEvent) {
        if event.state != ElementState::Pressed {
            return;
        }

        let keys = key_translate::translate_key_event(&event, self.modifiers);
        if keys.is_empty() {
            return;
        }

        let envelope = self.session.envelope(InputEvent::Keys(keys));
        match self.session.dispatch(envelope).await {
            Ok(output) => self.apply_session_output(output),
            Err(error) => {
                self.fatal_error = Some(FrontendError::Session(error));
                self.quit_requested = true;
            }
        }
    }

    /// Handle mouse click at the given pixel position
    async fn handle_mouse_click(&mut self, x: f64, y: f64) {
        let column = (x / f64::from(self.text_renderer.char_width())) as u16;
        let row = (y / f64::from(self.text_renderer.line_height())) as u16;
        let envelope = self.session.envelope(InputEvent::Pointer(PointerEvent {
            column,
            row,
            kind: PointerKind::Down,
            button: PointerButton::Primary,
        }));
        match self.session.dispatch(envelope).await {
            Ok(output) => self.apply_session_output(output),
            Err(error) => {
                self.fatal_error = Some(FrontendError::Session(error));
                self.quit_requested = true;
            }
        }
    }

    fn presented_view(&self, id: ViewId) -> Option<&PresentedView> {
        self.redraw_state
            .session_presentation()
            .current()?
            .views
            .iter()
            .find(|view| view.id == id)
    }

    fn check_scrollbar_hit(&self, px: f64, py: f64) -> Option<(ViewId, f64)> {
        let char_width = f64::from(self.text_renderer.char_width());
        let line_height = f64::from(self.text_renderer.line_height());
        for view in &self.redraw_state.session_presentation().current()?.views {
            let x = f64::from(view.geometry.x) * char_width;
            let y = f64::from(view.geometry.y) * line_height;
            let width = f64::from(view.geometry.columns) * char_width;
            let height = f64::from(view.geometry.rows) * line_height;
            let scrollbar_x = x + width - SCROLLBAR_WIDTH - 2.0;
            let top = y + 2.0;
            let extent = height - line_height - 4.0;
            if px >= scrollbar_x
                && px <= scrollbar_x + SCROLLBAR_WIDTH
                && py >= top
                && py <= top + extent
            {
                return Some((view.id, ((py - top) / extent).clamp(0.0, 1.0)));
            }
        }
        None
    }

    async fn handle_scrollbar_click(&mut self, view_id: ViewId, ratio: f64) {
        let Some(view) = self.presented_view(view_id).cloned() else {
            return;
        };
        let visible =
            session_view_metrics(&view, f64::from(self.text_renderer.char_width())).content_rows;
        if view.total_lines <= visible {
            return;
        }
        let max_start = view.total_lines.saturating_sub(visible);
        let start = ((max_start as f64) * ratio).round() as usize;
        self.set_view_scroll(view_id, Some(start.min(u16::MAX as usize) as u16), None)
            .await;
    }

    async fn handle_scrollbar_drag(&mut self, py: f64) {
        let Some(view_id) = self.scrollbar_dragging else {
            return;
        };
        let Some(view) = self.presented_view(view_id).cloned() else {
            return;
        };
        let line_height = f64::from(self.text_renderer.line_height());
        let top = f64::from(view.geometry.y) * line_height + 2.0;
        let extent = f64::from(view.geometry.rows) * line_height - line_height - 4.0;
        self.handle_scrollbar_click(view_id, ((py - top) / extent).clamp(0.0, 1.0))
            .await;
    }

    fn check_hscrollbar_hit(&self, px: f64, py: f64) -> Option<(ViewId, f64)> {
        let char_width = f64::from(self.text_renderer.char_width());
        let line_height = f64::from(self.text_renderer.line_height());
        for view in &self.redraw_state.session_presentation().current()?.views {
            if !session_view_metrics(view, char_width).horizontal_overflow {
                continue;
            }
            let x = f64::from(view.geometry.x) * char_width;
            let y = f64::from(view.geometry.y) * line_height;
            let width = f64::from(view.geometry.columns) * char_width;
            let height = f64::from(view.geometry.rows) * line_height;
            let bar_y = y + height - line_height - SCROLLBAR_WIDTH - 2.0;
            let bar_x = x + 2.0;
            let extent = width - SCROLLBAR_WIDTH - 6.0;
            if px >= bar_x && px <= bar_x + extent && py >= bar_y && py <= bar_y + SCROLLBAR_WIDTH {
                return Some((view.id, ((px - bar_x) / extent).clamp(0.0, 1.0)));
            }
        }
        None
    }

    async fn handle_hscrollbar_click(&mut self, view_id: ViewId, ratio: f64) {
        let Some(view) = self.presented_view(view_id).cloned() else {
            return;
        };
        let char_width = f64::from(self.text_renderer.char_width());
        let visible = session_view_metrics(&view, char_width).content_width_chars;
        if view.max_line_chars <= visible {
            return;
        }
        let max_start = view.max_line_chars.saturating_sub(visible);
        let start = ((max_start as f64) * ratio).round() as usize;
        self.set_view_scroll(view_id, None, Some(start.min(u16::MAX as usize) as u16))
            .await;
    }

    async fn handle_hscrollbar_drag(&mut self, px: f64) {
        let Some(view_id) = self.hscrollbar_dragging else {
            return;
        };
        let Some(view) = self.presented_view(view_id).cloned() else {
            return;
        };
        let char_width = f64::from(self.text_renderer.char_width());
        let left = f64::from(view.geometry.x) * char_width + 2.0;
        let extent = f64::from(view.geometry.columns) * char_width - SCROLLBAR_WIDTH - 6.0;
        self.handle_hscrollbar_click(view_id, ((px - left) / extent).clamp(0.0, 1.0))
            .await;
    }

    async fn set_view_scroll(
        &mut self,
        view: ViewId,
        start_line: Option<u16>,
        start_column: Option<u16>,
    ) {
        let envelope = self.session.envelope(InputEvent::SetViewScroll {
            view,
            start_line,
            start_column,
        });
        match self.session.dispatch(envelope).await {
            Ok(output) => self.apply_session_output(output),
            Err(error) => {
                self.fatal_error = Some(FrontendError::Session(error));
                self.quit_requested = true;
            }
        }
    }

    /// Return the orientation of a shared logical border under the pointer.
    fn check_border_hit(&self, px: f64, py: f64) -> Option<bool> {
        let char_width = f64::from(self.text_renderer.char_width());
        let line_height = f64::from(self.text_renderer.line_height());
        let column = (px / char_width) as u16;
        let row = (py / line_height) as u16;
        let views = &self.redraw_state.session_presentation().current()?.views;
        for view in views {
            let right = view.geometry.x + view.geometry.columns.saturating_sub(1);
            let bottom = view.geometry.y + view.geometry.rows.saturating_sub(1);
            let vertical = (column == view.geometry.x || column == right)
                && row >= view.geometry.y
                && row <= bottom;
            if vertical && views.len() > 1 {
                return Some(true);
            }
            let horizontal = (row == view.geometry.y || row == bottom)
                && column >= view.geometry.x
                && column <= right;
            if horizontal && views.len() > 1 {
                return Some(false);
            }
        }
        None
    }
}

impl<'a> ApplicationHandler<HostEvent> for RoeVelloApp<'a> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }

        let window = match self.create_window(event_loop) {
            Ok(window) => window,
            Err(error) => {
                self.fatal_error = Some(FrontendError::Window(error));
                event_loop.exit();
                return;
            }
        };
        let size = window.inner_size();
        let surface = match pollster::block_on(self.render_cx.create_surface(
            window.clone(),
            size.width,
            size.height,
            wgpu::PresentMode::AutoVsync,
        )) {
            Ok(surface) => surface,
            Err(error) => {
                self.fatal_error = Some(FrontendError::Surface(error));
                event_loop.exit();
                return;
            }
        };

        self.state = Some(RenderState { window, surface });
        self.request_redraw(DirtyRegion::FullScreen);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: winit::window::WindowId,
        event: WindowEvent,
    ) {
        // Editor mutations must run inside the compio runtime context: they may
        // lazily spawn buffer hosts, which requires an active runtime.
        let runtime = self.runtime.clone();
        runtime.block_on(async {
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
                    }
                    let scale_factor = self
                        .state
                        .as_ref()
                        .map(|state| state.window.scale_factor())
                        .unwrap_or(1.0);
                    let columns = ((size.width as f64 / scale_factor)
                        / f64::from(self.text_renderer.char_width()))
                    .floor() as u16;
                    let rows = ((size.height as f64 / scale_factor)
                        / f64::from(self.text_renderer.line_height()))
                    .floor() as u16;
                    let envelope = self.session.envelope(InputEvent::Resize {
                        columns: columns.max(1),
                        rows: rows.saturating_sub(1).max(1),
                    });
                    match self.session.dispatch(envelope).await {
                        Ok(output) => self.apply_session_output(output),
                        Err(error) => {
                            self.fatal_error = Some(FrontendError::Session(error));
                            event_loop.exit();
                        }
                    }
                    self.request_redraw(DirtyRegion::FullScreen);
                }
                WindowEvent::RedrawRequested => {
                    if self.redraw_state.needs_redraw()
                        && let Err(error) = self.render()
                    {
                        self.fatal_error = Some(error);
                        event_loop.exit();
                    }
                }
                WindowEvent::KeyboardInput { event, .. } => {
                    self.handle_key_event(event).await;
                    if self.quit_requested {
                        event_loop.exit();
                    } else if self.redraw_state.needs_redraw() {
                        self.request_redraw(DirtyRegion::FullScreen);
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
                    if self.border_dragging.is_some() {
                        let column =
                            (logical_x / f64::from(self.text_renderer.char_width())) as u16;
                        let row = (logical_y / f64::from(self.text_renderer.line_height())) as u16;
                        let envelope = self.session.envelope(InputEvent::Pointer(PointerEvent {
                            column,
                            row,
                            kind: PointerKind::Move,
                            button: PointerButton::Primary,
                        }));
                        match self.session.dispatch(envelope).await {
                            Ok(output) => self.apply_session_output(output),
                            Err(error) => {
                                self.fatal_error = Some(FrontendError::Session(error));
                                event_loop.exit();
                            }
                        }
                        self.request_redraw(DirtyRegion::FullScreen);
                    }
                    // Handle vertical scrollbar dragging
                    else if self.scrollbar_dragging.is_some() {
                        self.handle_scrollbar_drag(logical_y).await;
                        self.request_redraw(DirtyRegion::FullScreen);
                    }
                    // Handle horizontal scrollbar dragging
                    else if self.hscrollbar_dragging.is_some() {
                        self.handle_hscrollbar_drag(logical_x).await;
                        self.request_redraw(DirtyRegion::FullScreen);
                    }
                    // Handle text selection drag
                    else if self.mouse_dragging {
                        let column =
                            (logical_x / f64::from(self.text_renderer.char_width())) as u16;
                        let row = (logical_y / f64::from(self.text_renderer.line_height())) as u16;
                        let envelope = self.session.envelope(InputEvent::Pointer(PointerEvent {
                            column,
                            row,
                            kind: PointerKind::Move,
                            button: PointerButton::Primary,
                        }));
                        match self.session.dispatch(envelope).await {
                            Ok(output) => self.apply_session_output(output),
                            Err(error) => {
                                self.fatal_error = Some(FrontendError::Session(error));
                                event_loop.exit();
                            }
                        }
                        self.request_redraw(DirtyRegion::FullScreen);
                    }

                    // Update cursor icon based on hover state
                    if let Some(ref state) = self.state {
                        let cursor = if let Some(is_vertical) = self.border_dragging {
                            if is_vertical {
                                CursorIcon::ColResize
                            } else {
                                CursorIcon::RowResize
                            }
                        } else if self.scrollbar_dragging.is_some()
                            || self.hscrollbar_dragging.is_some()
                        {
                            CursorIcon::Grabbing
                        } else if let Some(is_vertical) =
                            self.check_border_hit(logical_x, logical_y)
                        {
                            // Show resize cursor when hovering over draggable borders
                            if is_vertical {
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
                WindowEvent::MouseInput {
                    state,
                    button: MouseButton::Left,
                    ..
                } => {
                    match state {
                        ElementState::Pressed => {
                            if let Some((x, y)) = self.cursor_position {
                                // Check if click is on a window border (for resizing splits)
                                if let Some(is_vertical) = self.check_border_hit(x, y) {
                                    self.handle_mouse_click(x, y).await;
                                    self.border_dragging = Some(is_vertical);
                                    if let Some(ref state) = self.state {
                                        let cursor = if is_vertical {
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
                                    self.handle_scrollbar_click(window_id, ratio).await;
                                    self.scrollbar_dragging = Some(window_id);
                                    if let Some(ref state) = self.state {
                                        state.window.set_cursor(CursorIcon::Grabbing);
                                    }
                                }
                                // Check horizontal scrollbar
                                else if let Some((window_id, ratio)) =
                                    self.check_hscrollbar_hit(x, y)
                                {
                                    self.handle_hscrollbar_click(window_id, ratio).await;
                                    self.hscrollbar_dragging = Some(window_id);
                                    if let Some(ref state) = self.state {
                                        state.window.set_cursor(CursorIcon::Grabbing);
                                    }
                                } else {
                                    // Normal text click
                                    self.handle_mouse_click(x, y).await;
                                    self.mouse_dragging = true;
                                }
                                self.request_redraw(DirtyRegion::FullScreen);
                            }
                        }
                        ElementState::Released => {
                            if let Some((x, y)) = self.cursor_position {
                                let column =
                                    (x / f64::from(self.text_renderer.char_width())) as u16;
                                let row = (y / f64::from(self.text_renderer.line_height())) as u16;
                                let envelope =
                                    self.session.envelope(InputEvent::Pointer(PointerEvent {
                                        column,
                                        row,
                                        kind: PointerKind::Up,
                                        button: PointerButton::Primary,
                                    }));
                                match self.session.dispatch(envelope).await {
                                    Ok(output) => self.apply_session_output(output),
                                    Err(error) => {
                                        self.fatal_error = Some(FrontendError::Session(error));
                                        event_loop.exit();
                                    }
                                }
                            }
                            self.mouse_dragging = false;
                            self.scrollbar_dragging = None;
                            self.hscrollbar_dragging = None;
                            self.border_dragging = None;
                        }
                    }
                }
                _ => {}
            }
        });
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, _event: HostEvent) {
        self.wake_state.acknowledge();
        self.drive_background();
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.drive_background();
        event_loop.set_control_flow(ControlFlow::WaitUntil(
            Instant::now() + Duration::from_millis(20),
        ));
    }
}

/// Run the editor with the Vello renderer
pub fn run_vello(
    mut editor: Editor,
    runtime: compio::runtime::Runtime,
) -> Result<(), FrontendError> {
    // Mica owns face/configuration description; Vello retains native font,
    // scene, device, and surface realization.
    let theme = VelloTheme::default();

    let event_loop = EventLoop::<HostEvent>::with_user_event()
        .build()
        .map_err(FrontendError::EventLoop)?;
    let wake_proxy = event_loop.create_proxy();
    let wake_state = Arc::new(WakeState::default());
    editor.file_watcher.set_wake_handler(Arc::new(WinitWake {
        proxy: wake_proxy,
        state: wake_state.clone(),
    }));
    event_loop.set_control_flow(ControlFlow::WaitUntil(
        Instant::now() + Duration::from_millis(20),
    ));

    let mut app = RoeVelloApp::new(editor, theme, runtime, wake_state)?;
    let event_loop_result = event_loop.run_app(&mut app);
    let fatal_error = app.fatal_error.take();
    let close = app.session.envelope(InputEvent::Close);
    if let Ok(output) = app.runtime.block_on(app.session.dispatch(close)) {
        for event in output.lifecycle {
            if let LifecycleEvent::Warning(error) = event {
                tracing::warn!(%error, "editor shutdown warning");
            }
        }
    }
    if let Some(error) = fatal_error {
        return Err(error);
    }
    event_loop_result.map_err(FrontendError::EventLoop)?;

    Ok(())
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use roe_core::editor::{WindowNode, WindowType};
    use roe_core::file_watcher::FileWatcher;
    use roe_core::kill_ring::KillRing;
    use roe_core::native_kernel::ResourceId;
    use roe_core::native_services::SystemClock;
    use roe_core::session::{ViewGeometry, ViewScroll};
    use roe_core::{Buffer, BufferId, Frame, Window as EditorWindow, WindowId};
    use slotmap::SlotMap;
    use std::cell::Cell;
    use std::rc::Rc;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn wake_requests_are_coalesced_until_the_ui_acknowledges() {
        let state = WakeState::default();
        let sends = AtomicUsize::new(0);

        for _ in 0..10_000 {
            state.request(|| {
                sends.fetch_add(1, Ordering::Relaxed);
                true
            });
        }
        assert_eq!(sends.load(Ordering::Relaxed), 1);

        state.acknowledge();
        state.request(|| {
            sends.fetch_add(1, Ordering::Relaxed);
            true
        });
        assert_eq!(sends.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn failed_wake_send_can_be_retried() {
        let state = WakeState::default();
        state.request(|| false);

        let sent = AtomicBool::new(false);
        state.request(|| {
            sent.store(true, Ordering::Relaxed);
            true
        });
        assert!(sent.load(Ordering::Relaxed));
    }

    #[test]
    fn runtime_pump_completes_ready_work_without_window_input() {
        let runtime = compio::runtime::Runtime::new().unwrap();
        let completed = Rc::new(Cell::new(false));
        let task_completed = completed.clone();
        let task = runtime.enter(|| {
            runtime.spawn(async move {
                compio::time::sleep(Duration::from_millis(1)).await;
                task_completed.set(true);
            })
        });

        let deadline = Instant::now() + Duration::from_secs(1);
        while !completed.get() && Instant::now() < deadline {
            pump_runtime(&runtime);
            std::thread::sleep(Duration::from_millis(1));
        }

        assert!(completed.get(), "periodic runtime pump stranded ready work");
        drop(task);
    }

    fn session_editor() -> Editor {
        let mut buffers: SlotMap<BufferId, Buffer> = SlotMap::default();
        let buffer = Buffer::new();
        buffer.set_object("*vello-session*".to_owned());
        buffer.load_str("headless scene λ");
        let buffer_id = buffers.insert(buffer);
        let mut windows: SlotMap<WindowId, EditorWindow> = SlotMap::default();
        let window_id = windows.insert(EditorWindow {
            x: 0,
            y: 0,
            width_chars: 80,
            height_chars: 23,
            active_buffer: buffer_id,
            start_line: 0,
            start_column: 0,
            cursor: 16,
            window_type: WindowType::Normal,
        });
        Editor {
            frame: Frame::new(80, 23),
            buffers,
            windows,
            active_window: window_id,
            window_tree: WindowNode::new_leaf(window_id),
            kill_ring: KillRing::without_clipboard(60),
            previous_active_window: None,
            buffer_history: vec![buffer_id],
            echo_message: String::new(),
            echo_message_time: None,
            clock: Arc::new(SystemClock),
            mouse_drag_state: None,
            messages_buffer_id: None,
            file_watcher: FileWatcher::new(),
        }
    }

    #[test]
    fn production_mica_session_builds_a_vello_scene_without_a_display() {
        let runtime = compio::runtime::Runtime::new().unwrap();
        let mut app = RoeVelloApp::new(
            session_editor(),
            VelloTheme::default(),
            runtime,
            Arc::new(WakeState::default()),
        )
        .unwrap();
        app.build_session_scene(DEFAULT_WIDTH, DEFAULT_HEIGHT)
            .unwrap();

        let output = app.runtime.block_on(async {
            let envelope = app.session.envelope(InputEvent::Text("x".to_owned()));
            app.session.dispatch(envelope).await
        });
        app.apply_session_output(output.unwrap());
        app.scene.reset();
        app.build_session_scene(DEFAULT_WIDTH, DEFAULT_HEIGHT)
            .unwrap();
        assert_eq!(
            app.redraw_state
                .session_presentation()
                .current()
                .unwrap()
                .views[0]
                .visible_text,
            "headless scene λx"
        );
    }

    fn presented_view(columns: u16, rows: u16, max_line_chars: usize) -> PresentedView {
        PresentedView {
            id: ViewId(1),
            resource: ResourceId {
                slot: 0,
                generation: 1,
            },
            name: "test".to_owned(),
            visible_text: String::new(),
            visible_start_char: 0,
            visible_end_char: 0,
            total_lines: 20,
            max_line_chars,
            cursor: 0,
            selection: None,
            geometry: ViewGeometry {
                x: 0,
                y: 0,
                columns,
                rows,
            },
            scroll: ViewScroll {
                start_line: 0,
                start_column: 0,
            },
            active: true,
            command_view: false,
            show_gutter: false,
            modeline: String::new(),
            styled_ranges: Vec::new(),
        }
    }

    #[test]
    fn session_scene_reserves_scrollbar_lanes_and_only_shows_overflow() {
        let char_width = 8.0;
        let fits = session_view_metrics(&presented_view(80, 24, 70), char_width);
        assert_eq!(fits.content_rows, 21);
        assert_eq!(fits.content_width_chars, 75);
        assert!(!fits.horizontal_overflow);

        let overflow = session_view_metrics(&presented_view(80, 24, 76), char_width);
        assert!(overflow.horizontal_overflow);
    }
}
