// Copyright (C) 2025 Ryan Daum <ryan.daum@gmail.com> This program is free
// software: you can redistribute it and/or modify it under the terms of the GNU
// General Public License as published by the Free Software Foundation, version
// 3.

//! Transport-neutral editor session and presentation protocol.
//!
//! Frontends normalize platform events into [`InputEvent`] and consume
//! [`SessionOutput`]. Editor policy and native-resource authority remain behind
//! this boundary. The current endpoint is an in-process direct call with no
//! mailbox; every envelope is owned and serde-compatible for a later process
//! transport.

use crate::command_mode::CommandMode;
use crate::editor::{ChromeAction, WindowType};
use crate::keys::LogicalKey;
use crate::native_kernel::{
    Capability, CapabilityGrants, KernelError, NativeKernel, NativeOperation, NativeResult,
    ResourceId, TextSelection, ViewId,
};
use crate::syntax::{Color, face_registry};
use crate::{BufferId, Editor, WindowId};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};

pub const SESSION_PROTOCOL_VERSION: u16 = 1;
pub const MAX_KEYS_PER_INPUT: usize = 64;
pub const MAX_TEXT_CHARS_PER_INPUT: usize = 65_536;
pub const MAX_PRESENTATION_CHARS: usize = 1_000_000;
pub const MAX_FRAME_COLUMNS: u16 = 1_000;
pub const MAX_FRAME_ROWS: u16 = 1_000;

static NEXT_EPOCH: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionEpoch(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RequestId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Revision(pub u64);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputEnvelope {
    pub protocol_version: u16,
    pub epoch: SessionEpoch,
    pub sequence: u64,
    pub event: InputEvent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum InputEvent {
    Keys(Vec<LogicalKey>),
    Text(String),
    Pointer(PointerEvent),
    Resize {
        columns: u16,
        rows: u16,
    },
    Focus(bool),
    Timer {
        token: u64,
    },
    NativeNotification(NativeNotification),
    NativeRequest {
        request_id: RequestId,
        operation: NativeOperation,
    },
    Cancel {
        request_id: RequestId,
    },
    Heartbeat,
    RequestSnapshot {
        after: Option<Revision>,
    },
    Close,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PointerEvent {
    pub column: u16,
    pub row: u16,
    pub kind: PointerKind,
    pub button: PointerButton,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PointerKind {
    Down,
    Move,
    Up,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PointerButton {
    Primary,
    Secondary,
    Middle,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NativeNotification {
    FilesChanged,
    Wake,
    PlatformWarning(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionOutput {
    pub protocol_version: u16,
    pub epoch: SessionEpoch,
    pub input_sequence: u64,
    pub presentation: Option<PresentationUpdate>,
    pub native_completions: Vec<NativeCompletion>,
    pub lifecycle: Vec<LifecycleEvent>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeCompletion {
    pub request_id: RequestId,
    pub result: Result<NativeResult, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LifecycleEvent {
    Ready {
        protocol_version: u16,
        capabilities: Vec<Capability>,
    },
    Warning(String),
    Error(String),
    QuitRequested,
    EndpointClosed,
    Heartbeat,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PresentationUpdate {
    Full(PresentationSnapshot),
    Delta(PresentationDelta),
}

impl PresentationUpdate {
    pub fn revision(&self) -> Revision {
        match self {
            Self::Full(snapshot) => snapshot.revision,
            Self::Delta(delta) => delta.revision,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PresentationDelta {
    pub epoch: SessionEpoch,
    pub base_revision: Revision,
    pub revision: Revision,
    pub invalidations: Vec<Invalidation>,
    pub snapshot: PresentationSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Invalidation {
    Full,
    View(ViewId),
    Resource(ResourceId),
    EchoArea,
    Cursor(ViewId),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PresentationSnapshot {
    pub epoch: SessionEpoch,
    pub revision: Revision,
    pub columns: u16,
    pub rows: u16,
    pub active_view: ViewId,
    pub views: Vec<PresentedView>,
    pub styles: Vec<StyleDefinition>,
    pub echo_area: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PresentedView {
    pub id: ViewId,
    pub resource: ResourceId,
    pub name: String,
    /// Renderer-neutral visible slice, bounded by the logical view height.
    pub visible_text: String,
    pub visible_start_char: usize,
    pub visible_end_char: usize,
    pub cursor: usize,
    pub selection: Option<TextSelection>,
    pub geometry: ViewGeometry,
    pub scroll: ViewScroll,
    pub active: bool,
    pub command_view: bool,
    pub show_gutter: bool,
    pub modeline: String,
    pub styled_ranges: Vec<StyledRange>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewGeometry {
    pub x: u16,
    pub y: u16,
    pub columns: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewScroll {
    pub start_line: u16,
    pub start_column: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StyleRef(pub u32);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StyledRange {
    pub start: usize,
    pub end: usize,
    pub style: StyleRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StyleDefinition {
    pub id: StyleRef,
    pub name: String,
    pub foreground: Option<PresentationColor>,
    pub background: Option<PresentationColor>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PresentationColor {
    Rgb { r: u8, g: u8, b: u8 },
    Named(String),
    Inherit,
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("session protocol version {received} is unsupported; expected {expected}")]
    ProtocolVersion { received: u16, expected: u16 },
    #[error("input belongs to stale session epoch {received:?}; active epoch is {expected:?}")]
    StaleEpoch {
        received: SessionEpoch,
        expected: SessionEpoch,
    },
    #[error("input sequence {received} is invalid; expected {expected}")]
    Sequence { received: u64, expected: u64 },
    #[error("session input exceeds its bound: {0}")]
    InputTooLarge(String),
    #[error("the session endpoint is closed")]
    Closed,
    #[error("editor input failed: {0}")]
    Editor(#[from] std::io::Error),
    #[error("native kernel failed: {0}")]
    Kernel(#[from] KernelError),
}

/// Direct, ordered in-process session endpoint.
pub struct HostSession {
    editor: Editor,
    kernel: NativeKernel,
    epoch: SessionEpoch,
    next_sequence: u64,
    revision: Revision,
    buffer_resources: HashMap<BufferId, ResourceId>,
    view_ids: HashMap<WindowId, ViewId>,
    next_view_id: u64,
    closed: bool,
}

impl HostSession {
    pub fn open(editor: Editor, grants: CapabilityGrants) -> Self {
        let epoch = SessionEpoch(NEXT_EPOCH.fetch_add(1, Ordering::Relaxed));
        let mut session = Self {
            editor,
            kernel: NativeKernel::new(grants),
            epoch,
            next_sequence: 0,
            revision: Revision(0),
            buffer_resources: HashMap::new(),
            view_ids: HashMap::new(),
            next_view_id: 1,
            closed: false,
        };
        session.synchronize_identities();
        session
    }

    pub fn epoch(&self) -> SessionEpoch {
        self.epoch
    }

    pub fn next_sequence(&self) -> u64 {
        self.next_sequence
    }

    pub fn envelope(&self, event: InputEvent) -> InputEnvelope {
        InputEnvelope {
            protocol_version: SESSION_PROTOCOL_VERSION,
            epoch: self.epoch,
            sequence: self.next_sequence,
            event,
        }
    }

    /// Compatibility realization view. Frontends may pass this read-only state
    /// to existing Rust renderers, but may not execute policy through it. Phase
    /// 2 presentation updates are the authoritative logical view contract.
    pub fn realization_state(&self) -> &Editor {
        &self.editor
    }

    pub fn initial_output(&mut self) -> SessionOutput {
        self.revision.0 += 1;
        SessionOutput {
            protocol_version: SESSION_PROTOCOL_VERSION,
            epoch: self.epoch,
            input_sequence: self.next_sequence,
            presentation: Some(PresentationUpdate::Full(self.capture_snapshot())),
            native_completions: Vec::new(),
            lifecycle: vec![LifecycleEvent::Ready {
                protocol_version: SESSION_PROTOCOL_VERSION,
                capabilities: capability_list(self.kernel.grants()),
            }],
        }
    }

    pub async fn dispatch(
        &mut self,
        envelope: InputEnvelope,
    ) -> Result<SessionOutput, SessionError> {
        self.validate_envelope(&envelope)?;
        self.validate_event_size(&envelope.event)?;
        self.next_sequence += 1;

        let mut lifecycle = Vec::new();
        let mut completions = Vec::new();
        let mut invalidations = Vec::new();
        let force_full = matches!(envelope.event, InputEvent::RequestSnapshot { .. });

        match envelope.event {
            InputEvent::Keys(keys) => {
                let actions = self.editor.key_event(keys).await?;
                self.resolve_actions(actions, &mut lifecycle, &mut invalidations)
                    .await;
            }
            InputEvent::Text(text) => {
                for character in text.chars() {
                    let actions = self
                        .editor
                        .key_event(vec![LogicalKey::AlphaNumeric(character)])
                        .await?;
                    self.resolve_actions(actions, &mut lifecycle, &mut invalidations)
                        .await;
                }
            }
            InputEvent::Pointer(pointer) => {
                self.apply_pointer(pointer);
                invalidations.push(Invalidation::Full);
            }
            InputEvent::Resize { columns, rows } => {
                self.editor.handle_resize(columns, rows);
                invalidations.push(Invalidation::Full);
            }
            InputEvent::Timer { .. }
            | InputEvent::NativeNotification(NativeNotification::FilesChanged)
            | InputEvent::NativeNotification(NativeNotification::Wake) => {
                if self.editor.check_and_clear_expired_echo() {
                    invalidations.push(Invalidation::EchoArea);
                }
                let actions = self.editor.poll_file_changes();
                self.resolve_actions(actions, &mut lifecycle, &mut invalidations)
                    .await;
            }
            InputEvent::NativeNotification(NativeNotification::PlatformWarning(warning)) => {
                lifecycle.push(LifecycleEvent::Warning(warning));
            }
            InputEvent::NativeRequest {
                request_id,
                operation,
            } => {
                let result = if matches!(
                    operation,
                    NativeOperation::CloseResource { resource }
                        if self.buffer_resources.values().any(|current| *current == resource)
                ) {
                    Err("cannot close a text resource while a logical buffer owns it".to_string())
                } else {
                    self.kernel
                        .execute(operation)
                        .map_err(|error| error.to_string())
                };
                if matches!(result, Ok(NativeResult::TextChanged(_))) {
                    invalidations.push(Invalidation::Full);
                }
                completions.push(NativeCompletion { request_id, result });
            }
            InputEvent::Cancel { request_id } => lifecycle.push(LifecycleEvent::Warning(format!(
                "request {} was not pending",
                request_id.0
            ))),
            InputEvent::Heartbeat => lifecycle.push(LifecycleEvent::Heartbeat),
            InputEvent::RequestSnapshot { .. } => {}
            InputEvent::Focus(_) => {}
            InputEvent::Close => {
                self.closed = true;
                for warning in self.editor.shutdown_native_work() {
                    lifecycle.push(LifecycleEvent::Warning(warning));
                }
                lifecycle.push(LifecycleEvent::EndpointClosed);
            }
        }

        self.synchronize_identities();
        let presentation = if self.closed {
            None
        } else {
            self.revision.0 += 1;
            let snapshot = self.capture_snapshot();
            if force_full || self.revision.0 == 1 {
                Some(PresentationUpdate::Full(snapshot))
            } else {
                if invalidations.is_empty() {
                    invalidations.push(Invalidation::Full);
                }
                Some(PresentationUpdate::Delta(PresentationDelta {
                    epoch: self.epoch,
                    base_revision: Revision(self.revision.0 - 1),
                    revision: self.revision,
                    invalidations,
                    snapshot,
                }))
            }
        };

        Ok(SessionOutput {
            protocol_version: SESSION_PROTOCOL_VERSION,
            epoch: self.epoch,
            input_sequence: envelope.sequence,
            presentation,
            native_completions: completions,
            lifecycle,
        })
    }

    fn validate_envelope(&self, envelope: &InputEnvelope) -> Result<(), SessionError> {
        if self.closed {
            return Err(SessionError::Closed);
        }
        if envelope.protocol_version != SESSION_PROTOCOL_VERSION {
            return Err(SessionError::ProtocolVersion {
                received: envelope.protocol_version,
                expected: SESSION_PROTOCOL_VERSION,
            });
        }
        if envelope.epoch != self.epoch {
            return Err(SessionError::StaleEpoch {
                received: envelope.epoch,
                expected: self.epoch,
            });
        }
        if envelope.sequence != self.next_sequence {
            return Err(SessionError::Sequence {
                received: envelope.sequence,
                expected: self.next_sequence,
            });
        }
        Ok(())
    }

    fn validate_event_size(&self, event: &InputEvent) -> Result<(), SessionError> {
        match event {
            InputEvent::Keys(keys) if keys.len() > MAX_KEYS_PER_INPUT => {
                Err(SessionError::InputTooLarge(format!(
                    "{} keys exceeds {MAX_KEYS_PER_INPUT}",
                    keys.len()
                )))
            }
            InputEvent::Text(text) if text.chars().count() > MAX_TEXT_CHARS_PER_INPUT => {
                Err(SessionError::InputTooLarge(format!(
                    "{} text characters exceeds {MAX_TEXT_CHARS_PER_INPUT}",
                    text.chars().count()
                )))
            }
            InputEvent::Resize { columns, rows }
                if *columns == 0
                    || *rows == 0
                    || *columns > MAX_FRAME_COLUMNS
                    || *rows > MAX_FRAME_ROWS =>
            {
                Err(SessionError::InputTooLarge(format!(
                    "frame {columns}x{rows} outside 1..={MAX_FRAME_COLUMNS} by 1..={MAX_FRAME_ROWS}"
                )))
            }
            InputEvent::NativeRequest { operation, .. }
                if native_operation_text_size(operation) > MAX_TEXT_CHARS_PER_INPUT =>
            {
                Err(SessionError::InputTooLarge(format!(
                    "native text payload exceeds {MAX_TEXT_CHARS_PER_INPUT} characters"
                )))
            }
            _ => Ok(()),
        }
    }

    async fn resolve_actions(
        &mut self,
        actions: Vec<ChromeAction>,
        lifecycle: &mut Vec<LifecycleEvent>,
        invalidations: &mut Vec<Invalidation>,
    ) {
        let mut pending: VecDeque<_> = self.editor.process_chrome_actions(actions).await.into();
        while let Some(action) = pending.pop_front() {
            match action {
                ChromeAction::Quit => lifecycle.push(LifecycleEvent::QuitRequested),
                ChromeAction::Echo(message) => {
                    self.editor.set_echo_message(message);
                    invalidations.push(Invalidation::EchoArea);
                }
                ChromeAction::MarkDirty(_) => invalidations.push(Invalidation::Full),
                ChromeAction::SplitHorizontal => {
                    self.editor.split_horizontal();
                    invalidations.push(Invalidation::Full);
                }
                ChromeAction::SplitVertical => {
                    self.editor.split_vertical();
                    invalidations.push(Invalidation::Full);
                }
                ChromeAction::SwitchWindow => {
                    self.editor.switch_window();
                    invalidations.push(Invalidation::Full);
                }
                ChromeAction::DeleteWindow => {
                    if self.editor.delete_window() {
                        invalidations.push(Invalidation::Full);
                    }
                }
                ChromeAction::DeleteOtherWindows => {
                    if self.editor.delete_other_windows() {
                        invalidations.push(Invalidation::Full);
                    }
                }
                ChromeAction::ShowMessages => {
                    let buffer = self.editor.get_messages_buffer();
                    if let Some(window) = self.editor.windows.get_mut(self.editor.active_window) {
                        window.active_buffer = buffer;
                        window.cursor = 0;
                    }
                    invalidations.push(Invalidation::Full);
                }
                ChromeAction::NewBufferWithMode {
                    buffer_name,
                    mode_name,
                    initial_content,
                } => {
                    let cursor = initial_content.chars().count();
                    if let Some(buffer) =
                        self.editor
                            .create_buffer_with_mode(buffer_name, mode_name, initial_content)
                        && let Some(window) = self.editor.windows.get_mut(self.editor.active_window)
                    {
                        window.active_buffer = buffer;
                        window.cursor = cursor;
                    }
                    invalidations.push(Invalidation::Full);
                }
                ChromeAction::ExecuteCommand(command_name) => {
                    let context = self.editor.create_command_context();
                    match CommandMode::execute_command(
                        &command_name,
                        &self.editor.command_registry,
                        context,
                    )
                    .await
                    {
                        Ok(actions) => {
                            let processed = self.editor.process_chrome_actions(actions).await;
                            pending.extend(processed);
                        }
                        Err(error) => lifecycle.push(LifecycleEvent::Error(format!(
                            "command {command_name}: {error}"
                        ))),
                    }
                }
                ChromeAction::FileWatcherStatus => {
                    self.editor
                        .set_echo_message(self.editor.file_watcher.status());
                    invalidations.push(Invalidation::EchoArea);
                }
                ChromeAction::CursorMove(_) | ChromeAction::BufferChanged { .. } => {}
                // These are consumed by Editor::process_chrome_actions. If one
                // survives, report it rather than letting a frontend interpret policy.
                unexpected => lifecycle.push(LifecycleEvent::Error(format!(
                    "unresolved editor action: {unexpected:?}"
                ))),
            }
        }
    }

    fn apply_pointer(&mut self, pointer: PointerEvent) {
        if pointer.kind != PointerKind::Down || pointer.button != PointerButton::Primary {
            return;
        }
        let selected = self
            .editor
            .windows
            .iter()
            .find(|(_, window)| {
                pointer.column >= window.x.saturating_add(1)
                    && pointer.column
                        < window
                            .x
                            .saturating_add(window.width_chars.saturating_sub(1))
                    && pointer.row >= window.y.saturating_add(1)
                    && pointer.row
                        < window
                            .y
                            .saturating_add(window.height_chars.saturating_sub(1))
            })
            .map(|(id, _)| id);
        let Some(window_id) = selected else {
            return;
        };
        if self.editor.active_window != window_id {
            self.editor.previous_active_window = Some(self.editor.active_window);
            self.editor.active_window = window_id;
        }
        let window = &self.editor.windows[window_id];
        let buffer = &self.editor.buffers[window.active_buffer];
        let line = pointer
            .row
            .saturating_sub(window.y.saturating_add(1))
            .saturating_add(window.start_line);
        let column = pointer
            .column
            .saturating_sub(window.x.saturating_add(1))
            .saturating_add(window.start_column);
        let cursor = buffer.to_char_index(column, line);
        self.editor.windows[window_id].cursor = cursor;
    }

    fn synchronize_identities(&mut self) {
        let live_buffers: HashSet<_> = self.editor.buffers.keys().collect();
        self.buffer_resources.retain(|buffer, resource| {
            if live_buffers.contains(buffer) {
                true
            } else {
                let _ = self.kernel.execute(NativeOperation::CloseResource {
                    resource: *resource,
                });
                false
            }
        });
        for (buffer_id, buffer) in &self.editor.buffers {
            self.buffer_resources
                .entry(buffer_id)
                .or_insert_with(|| self.kernel.register_buffer(buffer.clone()));
        }

        let live_windows: HashSet<_> = self.editor.windows.keys().collect();
        self.view_ids
            .retain(|window, _| live_windows.contains(window));
        for window_id in self.editor.windows.keys() {
            self.view_ids.entry(window_id).or_insert_with(|| {
                let id = ViewId(self.next_view_id);
                self.next_view_id += 1;
                id
            });
        }
    }

    fn capture_snapshot(&self) -> PresentationSnapshot {
        let mut styles = Vec::new();
        let mut style_by_name = HashMap::new();
        let face_registry = face_registry().lock().ok();
        let mut views = Vec::new();

        for (window_id, window) in &self.editor.windows {
            let Some(buffer) = self.editor.buffers.get(window.active_buffer) else {
                continue;
            };
            let resource = self.buffer_resources[&window.active_buffer];
            let id = self.view_ids[&window_id];
            let total_lines = buffer.buffer_len_lines();
            let start_line = usize::from(window.start_line).min(total_lines.saturating_sub(1));
            let visible_lines = usize::from(window.height_chars.saturating_sub(2)).max(1);
            let end_line = (start_line + visible_lines).min(total_lines);
            let visible_start_char = buffer.buffer_line_to_char(start_line);
            let calculated_end_char = if end_line < total_lines {
                buffer.buffer_line_to_char(end_line)
            } else {
                buffer.buffer_len_chars()
            };
            let visible_end_char =
                calculated_end_char.min(visible_start_char.saturating_add(MAX_PRESENTATION_CHARS));
            let text = buffer.content();
            let visible_text: String = text
                .chars()
                .skip(visible_start_char)
                .take(visible_end_char.saturating_sub(visible_start_char))
                .collect();

            let styled_ranges = buffer
                .spans_in_range(visible_start_char..visible_end_char)
                .into_iter()
                .filter_map(|span| {
                    let face = face_registry.as_ref()?.get(span.face_id)?;
                    let style = *style_by_name.entry(face.name.clone()).or_insert_with(|| {
                        let id = StyleRef(styles.len() as u32 + 1);
                        styles.push(StyleDefinition {
                            id,
                            name: face.name.clone(),
                            foreground: face.foreground.as_ref().map(presentation_color),
                            background: face.background.as_ref().map(presentation_color),
                            bold: face.bold,
                            italic: face.italic,
                            underline: face.underline,
                            strikethrough: face.strikethrough,
                        });
                        id
                    });
                    Some(StyledRange {
                        start: span.start,
                        end: span.end,
                        style,
                    })
                })
                .collect();

            let selection = buffer
                .get_region(window.cursor)
                .map(|(anchor, active)| TextSelection { anchor, active });
            let (column, line) = buffer.to_column_line(window.cursor);
            let mode = buffer
                .major_mode()
                .unwrap_or_else(|| "fundamental".to_string());
            let modeline = format!(
                "{} ({mode}) {}:{}",
                buffer.object(),
                line.saturating_add(1),
                column.saturating_add(1)
            );
            views.push(PresentedView {
                id,
                resource,
                name: buffer.object(),
                visible_text,
                visible_start_char,
                visible_end_char,
                cursor: window.cursor,
                selection,
                geometry: ViewGeometry {
                    x: window.x,
                    y: window.y,
                    columns: window.width_chars,
                    rows: window.height_chars,
                },
                scroll: ViewScroll {
                    start_line: window.start_line,
                    start_column: window.start_column,
                },
                active: window_id == self.editor.active_window,
                command_view: matches!(window.window_type, WindowType::Command { .. }),
                show_gutter: buffer.show_gutter(),
                modeline,
                styled_ranges,
            });
        }
        views.sort_by_key(|view| view.id.0);

        PresentationSnapshot {
            epoch: self.epoch,
            revision: self.revision,
            columns: self.editor.frame.columns,
            rows: self.editor.frame.rows,
            active_view: self.view_ids[&self.editor.active_window],
            views,
            styles,
            echo_area: self.editor.echo_message.clone(),
        }
    }
}

fn capability_list(grants: &CapabilityGrants) -> Vec<Capability> {
    [
        Capability::TextRead,
        Capability::TextWrite,
        Capability::Layout,
        Capability::FileRead,
        Capability::FileWrite,
        Capability::ClipboardRead,
        Capability::ClipboardWrite,
        Capability::ClockRead,
        Capability::ProcessSpawn,
        Capability::Watch,
    ]
    .into_iter()
    .filter(|capability| grants.contains(*capability))
    .collect()
}

fn native_operation_text_size(operation: &NativeOperation) -> usize {
    match operation {
        NativeOperation::CreateText { name, initial } => {
            name.chars().count().saturating_add(initial.chars().count())
        }
        NativeOperation::Insert { text, .. } | NativeOperation::Replace { text, .. } => {
            text.chars().count()
        }
        NativeOperation::WriteFile { contents, .. }
        | NativeOperation::WriteClipboard { contents } => contents.chars().count(),
        NativeOperation::SpawnProcess { program, args } => {
            args.iter().fold(program.chars().count(), |size, arg| {
                size.saturating_add(arg.chars().count())
            })
        }
        _ => 0,
    }
}

fn presentation_color(color: &Color) -> PresentationColor {
    match color {
        Color::Rgb { r, g, b } => PresentationColor::Rgb {
            r: *r,
            g: *g,
            b: *b,
        },
        Color::Named(name) => PresentationColor::Named(name.clone()),
        Color::Inherit => PresentationColor::Inherit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer_host;
    use crate::command_registry;
    use crate::editor::{Frame, Window, WindowNode};
    use crate::keys::{ConfigurableBindings, KeyState};
    use crate::kill_ring::KillRing;
    use crate::native_services::SystemClock;
    use crate::{Buffer, BufferId, Mode, ModeId};
    use slotmap::SlotMap;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn test_session() -> HostSession {
        let mut buffers: SlotMap<BufferId, Buffer> = SlotMap::default();
        let buffer = Buffer::new(&[]);
        buffer.set_object("*test*".to_string());
        buffer.load_str("hello");
        let buffer_id = buffers.insert(buffer.clone());
        let mut modes: SlotMap<ModeId, Box<dyn Mode>> = SlotMap::default();
        let mode_id = modes.insert(Box::new(crate::mode::FileMode {
            file_path: "*test*".to_string(),
        }));
        let mode = modes.remove(mode_id).unwrap();
        let mut hosts = HashMap::new();
        hosts.insert(
            buffer_id,
            buffer_host::create_buffer_host(
                buffer,
                vec![(mode_id, "file".to_string(), mode)],
                buffer_id,
            ),
        );
        let mut windows = SlotMap::default();
        let window_id = windows.insert(Window {
            x: 0,
            y: 0,
            width_chars: 80,
            height_chars: 23,
            active_buffer: buffer_id,
            start_line: 0,
            start_column: 0,
            cursor: 5,
            window_type: WindowType::Normal,
        });
        let editor = Editor {
            frame: Frame::new(80, 23),
            buffers,
            buffer_hosts: hosts,
            windows,
            modes,
            active_window: window_id,
            previous_active_window: None,
            key_state: KeyState::new(),
            bindings: Box::new(ConfigurableBindings::new()),
            window_tree: WindowNode::new_leaf(window_id),
            kill_ring: KillRing::without_clipboard(60),
            command_registry: command_registry::create_default_registry(),
            buffer_history: vec![buffer_id],
            echo_message: String::new(),
            echo_message_time: None,
            clock: Arc::new(SystemClock),
            current_key_chord: Vec::new(),
            mouse_drag_state: None,
            messages_buffer_id: None,
            file_watcher: crate::file_watcher::FileWatcher::new(),
            last_search_term: String::new(),
        };
        HostSession::open(editor, CapabilityGrants::editor_default())
    }

    fn snapshot(output: &SessionOutput) -> &PresentationSnapshot {
        match output.presentation.as_ref().unwrap() {
            PresentationUpdate::Full(snapshot) => snapshot,
            PresentationUpdate::Delta(delta) => &delta.snapshot,
        }
    }

    #[test]
    fn ordered_input_produces_monotonic_revisions() {
        compio::runtime::Runtime::new().unwrap().block_on(async {
            let mut session = test_session();
            let initial = session.initial_output();
            let first_revision = snapshot(&initial).revision;
            let envelope = session.envelope(InputEvent::Text("!".to_string()));
            let output = session.dispatch(envelope).await.unwrap();
            assert!(snapshot(&output).revision.0 > first_revision.0);
            assert_eq!(snapshot(&output).views[0].visible_text, "hello!");
        });
    }

    #[test]
    fn duplicate_and_gapped_sequences_are_rejected_without_mutation() {
        compio::runtime::Runtime::new().unwrap().block_on(async {
            let mut session = test_session();
            let mut envelope = session.envelope(InputEvent::Heartbeat);
            envelope.sequence += 1;
            assert!(matches!(
                session.dispatch(envelope).await,
                Err(SessionError::Sequence { .. })
            ));
            assert_eq!(session.next_sequence(), 0);
        });
    }

    #[test]
    fn full_resync_snapshot_is_self_contained_and_stable() {
        compio::runtime::Runtime::new().unwrap().block_on(async {
            let mut session = test_session();
            let first = session
                .dispatch(session.envelope(InputEvent::RequestSnapshot { after: None }))
                .await
                .unwrap();
            let first_snapshot = snapshot(&first).clone();
            let second = session
                .dispatch(session.envelope(InputEvent::RequestSnapshot {
                    after: Some(first_snapshot.revision),
                }))
                .await
                .unwrap();
            assert_eq!(snapshot(&second).views, first_snapshot.views);
            assert!(snapshot(&second).revision.0 > first_snapshot.revision.0);
        });
    }

    #[test]
    fn native_capability_denial_is_a_typed_completion_not_endpoint_failure() {
        compio::runtime::Runtime::new().unwrap().block_on(async {
            let mut session = test_session();
            let output = session
                .dispatch(session.envelope(InputEvent::NativeRequest {
                    request_id: RequestId(7),
                    operation: NativeOperation::Snapshot {
                        resource: ResourceId {
                            slot: u32::MAX,
                            generation: 1,
                        },
                    },
                }))
                .await
                .unwrap();
            assert_eq!(output.native_completions[0].request_id, RequestId(7));
            assert!(output.native_completions[0].result.is_err());
            assert!(output.lifecycle.is_empty());
        });
    }

    #[test]
    fn close_is_idempotently_terminal() {
        compio::runtime::Runtime::new().unwrap().block_on(async {
            let mut session = test_session();
            let close = session
                .dispatch(session.envelope(InputEvent::Close))
                .await
                .unwrap();
            assert!(close.presentation.is_none());
            assert!(close.lifecycle.contains(&LifecycleEvent::EndpointClosed));
            assert!(matches!(
                session
                    .dispatch(session.envelope(InputEvent::Heartbeat))
                    .await,
                Err(SessionError::Closed)
            ));
        });
    }
}
