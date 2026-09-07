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
mod scene;
use scene::{SCROLLBAR_WIDTH, session_view_metrics};
mod text;
mod theme;

pub use renderer::VelloRenderer;
pub use text::StyledSpan;
pub use theme::VelloTheme;

use roe_core::Editor;
use roe_core::frontend::LocalFrontendServices;
use roe_core::native_kernel::ViewId;
use roe_core::native_services::FrontendWake;
use roe_core::session::{
    AttachmentConfiguration, DirectSessionClient, InputEvent, LifecycleEvent, PointerButton,
    PointerEvent, PointerKind, PresentedView, SessionClient, SessionOutput,
    StartupRecoveryOperation,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use text::TextRenderer;
use thiserror::Error;
use vello::kurbo::Affine;

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
    #[error("{0}")]
    Startup(#[from] roe_core::startup::StartupError),
    #[error("{0}")]
    Output(#[from] roe_core::frontend::FrontendOutputError),
    #[error("Mica recovery failed: {0}")]
    Recovery(String),
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

/// Application state for the Vello renderer
pub struct RoeVelloApp<'a> {
    /// The direct attachment to the embedded workspace.
    session: DirectSessionClient,
    frontend_services: LocalFrontendServices,
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
        frontend_wake: Arc<dyn FrontendWake>,
        recovery: &[StartupRecoveryOperation],
    ) -> Result<Self, FrontendError> {
        let font_size = theme.font_size;
        let font_family = if theme.font_family.is_empty() {
            None
        } else {
            Some(theme.font_family.clone())
        };

        let attachment = AttachmentConfiguration::local_frontend(
            editor.frame().available_columns,
            editor.frame().available_lines,
        );
        let mut session = runtime
            .block_on(roe_core::startup::attach_editor(
                editor,
                attachment,
                recovery,
                Some(frontend_wake),
            ))
            .map_err(FrontendError::Startup)?;
        let initial = runtime.block_on(session.initial_output());
        let mut redraw_state = VelloRenderer::with_theme(theme.clone());
        let mut frontend_services = LocalFrontendServices::new();
        let quit_requested = runtime
            .block_on(roe_core::frontend::consume_output(
                &mut session,
                &mut frontend_services,
                &mut redraw_state,
                initial,
            ))
            .map_err(FrontendError::Output)?;

        Ok(Self {
            session,
            frontend_services,
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
            quit_requested,
            modifiers: ModifiersState::empty(),
            cursor_position: None,
            mouse_dragging: false,
            scrollbar_dragging: None,
            hscrollbar_dragging: None,
            border_dragging: None,
        })
    }

    fn request_redraw(&mut self) {
        tracing::trace!("Vello redraw requested");
        self.redraw_state.invalidate();
        if let Some(ref state) = self.state {
            state.window.request_redraw();
        }
    }

    fn drive_background(&mut self) {
        pump_runtime(&self.runtime);
        match self.runtime.block_on(self.session.poll_output()) {
            Ok(Some(output)) => self.apply_session_output(output),
            Ok(None) => {}
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
        match self.runtime.block_on(roe_core::frontend::consume_output(
            &mut self.session,
            &mut self.frontend_services,
            &mut self.redraw_state,
            output,
        )) {
            Ok(quit) => self.quit_requested |= quit,
            Err(error) => {
                self.fatal_error = Some(FrontendError::Output(error));
                self.quit_requested = true;
            }
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
                self.redraw_state.invalidate();
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
        let snapshot = self.redraw_state.session_presentation().current().ok_or(
            FrontendError::InvalidState("session has no logical presentation"),
        )?;
        scene::SceneBuilder {
            scene: &mut self.scene,
            text_renderer: &mut self.text_renderer,
            theme: &self.theme,
        }
        .build(snapshot, width, height);
        Ok(())
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
            text_hit: None,
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
            if view.command_view {
                continue;
            }
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
        self.set_view_scroll(view_id, Some(start), None).await;
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
        self.set_view_scroll(view_id, None, Some(start)).await;
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
        start_line: Option<usize>,
        start_column: Option<usize>,
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
        self.request_redraw();
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
                    self.request_redraw();
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
                        self.request_redraw();
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
                            text_hit: None,
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
                        self.request_redraw();
                    }
                    // Handle vertical scrollbar dragging
                    else if self.scrollbar_dragging.is_some() {
                        self.handle_scrollbar_drag(logical_y).await;
                        self.request_redraw();
                    }
                    // Handle horizontal scrollbar dragging
                    else if self.hscrollbar_dragging.is_some() {
                        self.handle_hscrollbar_drag(logical_x).await;
                        self.request_redraw();
                    }
                    // Handle text selection drag
                    else if self.mouse_dragging {
                        let column =
                            (logical_x / f64::from(self.text_renderer.char_width())) as u16;
                        let row = (logical_y / f64::from(self.text_renderer.line_height())) as u16;
                        let envelope = self.session.envelope(InputEvent::Pointer(PointerEvent {
                            text_hit: None,
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
                        self.request_redraw();
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
                                self.request_redraw();
                            }
                        }
                        ElementState::Released => {
                            if let Some((x, y)) = self.cursor_position {
                                let column =
                                    (x / f64::from(self.text_renderer.char_width())) as u16;
                                let row = (y / f64::from(self.text_renderer.line_height())) as u16;
                                let envelope =
                                    self.session.envelope(InputEvent::Pointer(PointerEvent {
                                        text_hit: None,
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
pub fn run_vello(editor: Editor, runtime: compio::runtime::Runtime) -> Result<(), FrontendError> {
    run_vello_with_recovery(editor, runtime, Vec::new())
}

pub fn run_vello_with_recovery(
    editor: Editor,
    runtime: compio::runtime::Runtime,
    recovery: Vec<StartupRecoveryOperation>,
) -> Result<(), FrontendError> {
    // Mica owns face/configuration description; Vello retains native font,
    // scene, device, and surface realization.
    let theme = VelloTheme::default();

    let event_loop = EventLoop::<HostEvent>::with_user_event()
        .build()
        .map_err(FrontendError::EventLoop)?;
    let wake_proxy = event_loop.create_proxy();
    let wake_state = Arc::new(WakeState::default());
    let frontend_wake: Arc<dyn FrontendWake> = Arc::new(WinitWake {
        proxy: wake_proxy,
        state: wake_state.clone(),
    });
    event_loop.set_control_flow(ControlFlow::WaitUntil(
        Instant::now() + Duration::from_millis(20),
    ));

    let mut app = RoeVelloApp::new(editor, theme, runtime, wake_state, frontend_wake, &recovery)?;
    let event_loop_result = event_loop.run_app(&mut app);
    let fatal_error = app.fatal_error.take();
    if let Ok(output) = app.runtime.block_on(app.session.terminate_workspace()) {
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
    use crate::scene::{session_vello_line_style, typeout_body_capacity};
    use roe_core::session::{PresentationColor, StyleDefinition};
    use vello::peniko::Color;

    struct NoopWake;

    impl FrontendWake for NoopWake {
        fn wake(&self) {}
    }
    use roe_core::keys::{KeyModifier, LogicalKey, Side};
    use roe_core::native_kernel::ResourceId;
    use roe_core::session::{StyleRef, StyledLine, ViewGeometry, ViewScroll};
    use roe_core::{Buffer, Frame};
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
        let buffer = Buffer::named("*vello-session*", roe_core::buffer::BufferKind::Ordinary);
        buffer.load_str("headless scene λ");
        let mut editor = Editor::new(buffer, Frame::new(80, 23));
        editor.move_cursor_to(16, false);
        editor
    }

    #[test]
    fn production_mica_session_builds_a_vello_scene_without_a_display() {
        let runtime = compio::runtime::Runtime::new().unwrap();
        let mut app = RoeVelloApp::new(
            session_editor(),
            VelloTheme::default(),
            runtime,
            Arc::new(WakeState::default()),
            Arc::new(NoopWake),
            &[StartupRecoveryOperation::Inspect],
        )
        .unwrap();
        app.build_session_scene(DEFAULT_WIDTH, DEFAULT_HEIGHT)
            .unwrap();
        assert!(
            app.redraw_state
                .session_presentation()
                .current()
                .unwrap()
                .echo_area
                .contains("Mica recovery diagnostics")
        );

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

        let meta = LogicalKey::Modifier(KeyModifier::Meta(Side::Left));
        let output = app.runtime.block_on(async {
            let envelope = app
                .session
                .envelope(InputEvent::Keys(vec![meta, LogicalKey::AlphaNumeric('x')]));
            app.session.dispatch(envelope).await
        });
        app.apply_session_output(output.unwrap());
        for _ in 0..5 {
            let output = app.runtime.block_on(async {
                let envelope = app
                    .session
                    .envelope(InputEvent::Keys(vec![LogicalKey::Down]));
                app.session.dispatch(envelope).await
            });
            app.apply_session_output(output.unwrap());
        }
        app.scene.reset();
        app.build_session_scene(DEFAULT_WIDTH, DEFAULT_HEIGHT)
            .unwrap();
        let snapshot = app.redraw_state.session_presentation().current().unwrap();
        let prompt = snapshot
            .views
            .iter()
            .find(|view| view.command_view)
            .unwrap();
        assert_eq!(prompt.geometry.rows, 10);
        assert_eq!(prompt.styled_lines.len(), 1);
        assert!(
            snapshot
                .styles
                .iter()
                .any(|style| style.id == prompt.styled_lines[0].style
                    && style.name == "completion-selection")
        );
    }

    #[test]
    fn production_rust_mode_builds_a_vello_scene_without_a_display() {
        assert_mode_scene(
            "scene.rs",
            "fn scene() {\nlet λ = 1;\n}\n",
            13,
            "fn scene() {\n    let λ = 1;\n}\n",
            "syntax-keyword",
        );
    }

    #[test]
    fn production_markdown_mode_builds_a_vello_scene_without_a_display() {
        assert_mode_scene(
            "scene.md",
            "# λ **bold**\n",
            0,
            "  # λ **bold**\n",
            "markdown-strong",
        );
    }

    fn assert_mode_scene(file: &str, source: &str, cursor: usize, expected: &str, face: &str) {
        let buffer = Buffer::named(file, roe_core::buffer::BufferKind::File);
        buffer.set_visited_file(Some(file.into()));
        buffer.load_str(source);
        let mut editor = Editor::new(buffer, Frame::new(80, 23));
        editor.move_cursor_to(cursor, false);
        let runtime = compio::runtime::Runtime::new().unwrap();
        let mut app = RoeVelloApp::new(
            editor,
            VelloTheme::default(),
            runtime,
            Arc::new(WakeState::default()),
            Arc::new(NoopWake),
            &[],
        )
        .unwrap();
        app.build_session_scene(DEFAULT_WIDTH, DEFAULT_HEIGHT)
            .unwrap();
        let output = app
            .runtime
            .block_on(async {
                app.session
                    .dispatch(
                        app.session
                            .envelope(InputEvent::Keys(vec![LogicalKey::Tab])),
                    )
                    .await
            })
            .unwrap();
        assert!(
            !output
                .lifecycle
                .iter()
                .any(|event| matches!(event, LifecycleEvent::Error(_))),
            "{output:#?}"
        );
        app.apply_session_output(output);
        app.scene.reset();
        app.build_session_scene(DEFAULT_WIDTH, DEFAULT_HEIGHT)
            .unwrap();
        let snapshot = app.redraw_state.session_presentation().current().unwrap();
        assert_eq!(snapshot.views[0].visible_text, expected);
        assert!(snapshot.styles.iter().any(|style| style.name == face));
        assert!(!app.scene.encoding().path_tags.is_empty());
    }

    #[test]
    fn production_mica_typeout_builds_a_vello_scene_without_a_display() {
        let runtime = compio::runtime::Runtime::new().unwrap();
        let mut editor = session_editor();
        editor.active_buffer().load_str("1 + 2");
        editor.active_buffer().set_mark(0);
        editor.move_cursor_to(5, false);
        let mut app = RoeVelloApp::new(
            editor,
            VelloTheme::default(),
            runtime,
            Arc::new(WakeState::default()),
            Arc::new(NoopWake),
            &[],
        )
        .unwrap();
        let control = LogicalKey::Modifier(KeyModifier::Control(Side::Left));
        let output = app.runtime.block_on(async {
            let envelope = app.session.envelope(InputEvent::Keys(vec![
                control,
                LogicalKey::AlphaNumeric('c'),
                control,
                LogicalKey::AlphaNumeric('r'),
            ]));
            app.session.dispatch(envelope).await
        });
        app.apply_session_output(output.unwrap());
        let snapshot = app.redraw_state.session_presentation().current().unwrap();
        assert_eq!(
            snapshot.views[0]
                .typeout
                .as_ref()
                .map(|typeout| typeout.visible_text.as_str()),
            Some("Mica => 3")
        );
        app.scene.reset();
        app.build_session_scene(DEFAULT_WIDTH, DEFAULT_HEIGHT)
            .unwrap();
    }

    fn presented_view(columns: u16, rows: u16, max_line_chars: usize) -> PresentedView {
        PresentedView {
            id: ViewId(1),
            resource: ResourceId {
                slot: 0,
                generation: 1,
            },
            name: "test".to_owned(),
            buffer_kind: "ordinary".to_owned(),
            visited_file: None,
            text_revision: 0,
            last_saved_revision: 0,
            modified: false,
            read_only: false,
            visible_text: String::new().into(),
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
            styled_lines: Vec::new(),
            typeout: None,
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

        let mut command = presented_view(80, 10, 200);
        command.command_view = true;
        let command = session_view_metrics(&command, char_width);
        assert_eq!(command.content_rows, 8);
        assert_eq!(command.content_width_chars, 78);
        assert!(!command.horizontal_overflow);
    }

    #[test]
    fn typeout_layout_retains_a_single_body_row_between_header_and_footer() {
        assert_eq!(typeout_body_capacity(92.0, 110.0, 18.0), 1);
        assert_eq!(typeout_body_capacity(92.0, 91.0, 18.0), 0);
    }

    #[test]
    fn session_line_style_realizes_a_full_row_background() {
        let mut view = presented_view(80, 10, 20);
        view.styled_lines.push(StyledLine {
            line: 4,
            style: StyleRef(1),
        });
        let styles = vec![StyleDefinition {
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
        }];
        let (_, background) =
            session_vello_line_style(4, &view, &styles, Color::WHITE, Color::BLACK);
        assert_eq!(background, Color::from_rgb8(0x3a, 0x3a, 0x3a));
    }
}
