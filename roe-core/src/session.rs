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

use crate::editor::{
    BorderInfo, ChromeAction, CommandType, CommandWindowPosition, DragType, MouseDragState,
    SplitDirection, WindowNode, WindowType,
};
use crate::keys::{CursorDirection, KeyAction, LogicalKey, NoBindings};
use crate::mica_host::{
    MicaEventBatch, MicaHost, MicaHostError, MicaKeyResult, MicaPromptTarget, MicaPromptUpdate,
    normalized_key_sequence,
};
use crate::native_kernel::{
    Capability, CapabilityGrants, KernelError, NativeClock, NativeKernel, NativeOperation,
    NativeResult, ResourceId, TextSelection, ViewId,
};
use crate::syntax::{Color, face_registry};
use crate::{BufferId, Editor, WindowId};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub const SESSION_PROTOCOL_VERSION: u16 = 1;
pub const MAX_KEYS_PER_INPUT: usize = 64;
pub const MAX_TEXT_CHARS_PER_INPUT: usize = 65_536;
pub const MAX_PRESENTATION_CHARS: usize = 1_000_000;
pub const MAX_NATIVE_RESULT_BYTES: usize = 1_048_576;
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
pub struct SessionTranscript {
    pub events: Vec<InputEvent>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum InputEvent {
    Keys(Vec<LogicalKey>),
    Text(String),
    Pointer(PointerEvent),
    SetViewScroll {
        view: ViewId,
        start_line: Option<u16>,
        start_column: Option<u16>,
    },
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
    Fatal(String),
    Overloaded {
        detail: String,
    },
    RequestCancelled {
        request_id: RequestId,
        was_pending: bool,
    },
    ResourceChanged {
        resource: ResourceId,
        path: std::path::PathBuf,
    },
    ResourceInvalidated {
        resource: ResourceId,
    },
    MicaTaskCancelled {
        task_id: u64,
    },
    MicaSubscriptionReady {
        mailbox: u64,
    },
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
    pub total_lines: usize,
    pub max_line_chars: usize,
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
    kernel: Arc<Mutex<NativeKernel>>,
    epoch: SessionEpoch,
    next_sequence: u64,
    revision: Revision,
    buffer_resources: HashMap<BufferId, ResourceId>,
    view_ids: HashMap<WindowId, ViewId>,
    next_view_id: u64,
    pointer_selection: Option<(WindowId, usize)>,
    mica: Option<MicaHost>,
    mica_palette_actions: HashMap<String, String>,
    closed: bool,
}

impl HostSession {
    pub fn open(editor: Editor, grants: CapabilityGrants) -> Self {
        Self::open_with_kernel(editor, Arc::new(Mutex::new(NativeKernel::new(grants))))
    }

    fn open_with_kernel(editor: Editor, kernel: Arc<Mutex<NativeKernel>>) -> Self {
        let epoch = SessionEpoch(NEXT_EPOCH.fetch_add(1, Ordering::Relaxed));
        let mut session = Self {
            editor,
            kernel,
            epoch,
            next_sequence: 0,
            revision: Revision(0),
            buffer_resources: HashMap::new(),
            view_ids: HashMap::new(),
            next_view_id: 1,
            pointer_selection: None,
            mica: None,
            mica_palette_actions: HashMap::new(),
            closed: false,
        };
        let _ = session.synchronize_identities();
        session
    }

    /// Open the public-driver Mica endpoint used by the first integration
    /// wave. The ordinary constructor remains available for headless and
    /// frontend-conformance tests that deliberately exercise only Rust policy.
    pub fn open_with_mica(editor: Editor, grants: CapabilityGrants) -> Result<Self, MicaHostError> {
        let mut editor = editor;
        editor.bindings = Box::new(NoBindings);
        let mut session = Self::open(editor, grants);
        session.mica = Some(MicaHost::open(
            &session.editor,
            Arc::clone(&session.kernel),
            &session.buffer_resources,
        )?);
        Ok(session)
    }

    pub fn open_with_mica_clock(
        editor: Editor,
        grants: CapabilityGrants,
        clock: Arc<dyn NativeClock>,
    ) -> Result<Self, MicaHostError> {
        let mut editor = editor;
        editor.bindings = Box::new(NoBindings);
        let mut session = Self::open_with_kernel(
            editor,
            Arc::new(Mutex::new(NativeKernel::with_clock(grants, clock))),
        );
        session.mica = Some(MicaHost::open(
            &session.editor,
            Arc::clone(&session.kernel),
            &session.buffer_resources,
        )?);
        Ok(session)
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
                capabilities: capability_list(self.kernel.lock().unwrap().grants()),
            }],
        }
    }

    pub async fn dispatch(
        &mut self,
        envelope: InputEnvelope,
    ) -> Result<SessionOutput, SessionError> {
        self.validate_envelope(&envelope)?;
        if let Err(SessionError::InputTooLarge(detail)) = self.validate_event_size(&envelope.event)
        {
            self.next_sequence += 1;
            return Ok(SessionOutput {
                protocol_version: SESSION_PROTOCOL_VERSION,
                epoch: self.epoch,
                input_sequence: envelope.sequence,
                presentation: None,
                native_completions: Vec::new(),
                lifecycle: vec![LifecycleEvent::Overloaded { detail }],
            });
        }
        self.next_sequence += 1;

        let mut lifecycle = Vec::new();
        let mut completions = Vec::new();
        let mut invalidations = Vec::new();
        let force_full = matches!(envelope.event, InputEvent::RequestSnapshot { .. });
        if let Some(mut mica) = self.mica.take() {
            let events = mica.drain_background_events();
            self.mica = Some(mica);
            self.apply_mica_events(events, &mut lifecycle, &mut invalidations)
                .await;
        }

        match envelope.event {
            InputEvent::Keys(keys) => {
                let mica_result = if let Some(mut mica) = self.mica.take() {
                    let result = mica
                        .dispatch_key(
                            &self.editor,
                            &self.buffer_resources,
                            normalized_key_sequence(&keys),
                        )
                        .await;
                    self.mica = Some(mica);
                    match result {
                        Ok(dispatch) => {
                            self.apply_mica_events(
                                dispatch.events,
                                &mut lifecycle,
                                &mut invalidations,
                            )
                            .await;
                            Some(Ok(dispatch.key))
                        }
                        Err(error) => Some(Err(error)),
                    }
                } else {
                    None
                };
                match mica_result {
                    Some(Ok(MicaKeyResult::Handled)) => {}
                    Some(Ok(MicaKeyResult::Prefix)) => {
                        self.editor.set_echo_message(normalized_key_sequence(&keys));
                        invalidations.push(Invalidation::EchoArea);
                    }
                    Some(Ok(MicaKeyResult::Failed(message))) => {
                        self.editor.set_echo_message(message.clone());
                        invalidations.push(Invalidation::EchoArea);
                        lifecycle.push(LifecycleEvent::Error(message));
                    }
                    Some(Err(error)) => {
                        let message = error.to_string();
                        self.editor.set_echo_message(message.clone());
                        invalidations.push(Invalidation::EchoArea);
                        lifecycle.push(LifecycleEvent::Error(message));
                    }
                    Some(Ok(MicaKeyResult::Unbound)) | None => {
                        let direct = match text_character_from_keys(&keys) {
                            Some(character) => {
                                self.editor
                                    .perform_native_action(KeyAction::AlphaNumeric(character))
                                    .await
                            }
                            _ => self.editor.key_event(keys).await,
                        };
                        match direct {
                            Ok(actions) => {
                                self.resolve_actions(actions, &mut lifecycle, &mut invalidations)
                                    .await;
                            }
                            Err(error) => self.fail_endpoint(error, &mut lifecycle),
                        }
                    }
                }
            }
            InputEvent::Text(text) => {
                for character in text.chars() {
                    let mica_result = if let Some(mut mica) = self.mica.take() {
                        let result = mica
                            .dispatch_key(
                                &self.editor,
                                &self.buffer_resources,
                                character.to_string(),
                            )
                            .await;
                        self.mica = Some(mica);
                        match result {
                            Ok(dispatch) => {
                                self.apply_mica_events(
                                    dispatch.events,
                                    &mut lifecycle,
                                    &mut invalidations,
                                )
                                .await;
                                Some(dispatch.key)
                            }
                            Err(error) => {
                                lifecycle.push(LifecycleEvent::Error(error.to_string()));
                                None
                            }
                        }
                    } else {
                        None
                    };
                    if matches!(mica_result, Some(MicaKeyResult::Handled)) {
                        continue;
                    }
                    match self
                        .editor
                        .perform_native_action(KeyAction::AlphaNumeric(character))
                        .await
                    {
                        Ok(actions) => {
                            self.resolve_actions(actions, &mut lifecycle, &mut invalidations)
                                .await;
                        }
                        Err(error) => {
                            self.fail_endpoint(error, &mut lifecycle);
                            break;
                        }
                    }
                }
            }
            InputEvent::Pointer(pointer) => {
                self.apply_pointer(pointer);
                invalidations.push(Invalidation::Full);
            }
            InputEvent::SetViewScroll {
                view,
                start_line,
                start_column,
            } => {
                let window_id = self
                    .view_ids
                    .iter()
                    .find_map(|(window, id)| (*id == view).then_some(*window));
                if let Some(window_id) = window_id {
                    let buffer_id = self.editor.windows[window_id].active_buffer;
                    let buffer = &self.editor.buffers[buffer_id];
                    let max_line = buffer
                        .buffer_len_lines()
                        .saturating_sub(1)
                        .min(u16::MAX as usize) as u16;
                    let max_column = buffer
                        .buffer_lines()
                        .into_iter()
                        .map(|line| line.trim_end_matches('\n').chars().count())
                        .max()
                        .unwrap_or(0)
                        .min(u16::MAX as usize) as u16;
                    let window = &mut self.editor.windows[window_id];
                    if let Some(line) = start_line {
                        window.start_line = line.min(max_line);
                    }
                    if let Some(column) = start_column {
                        window.start_column = column.min(max_column);
                    }
                    invalidations.push(Invalidation::View(view));
                } else {
                    lifecycle.push(LifecycleEvent::Warning(format!(
                        "view {} is no longer live",
                        view.0
                    )));
                }
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
                let kernel = self.kernel.lock().unwrap();
                for notification in kernel.poll_watch_notifications() {
                    invalidations.push(Invalidation::Resource(notification.resource));
                    lifecycle.push(LifecycleEvent::ResourceChanged {
                        resource: notification.resource,
                        path: notification.path,
                    });
                }
                if let Some(error) = kernel.take_watch_error() {
                    lifecycle.push(LifecycleEvent::Error(format!(
                        "native watcher backend: {error}"
                    )));
                }
            }
            InputEvent::NativeNotification(NativeNotification::PlatformWarning(warning)) => {
                lifecycle.push(LifecycleEvent::Warning(warning));
            }
            InputEvent::NativeRequest {
                request_id,
                operation,
            } => {
                let mut result = if matches!(
                    operation,
                    NativeOperation::CloseResource { resource }
                        if self.buffer_resources.values().any(|current| *current == resource)
                ) {
                    Err("cannot close a text resource while a logical buffer owns it".to_string())
                } else {
                    self.kernel
                        .lock()
                        .unwrap()
                        .execute(operation)
                        .map_err(|error| error.to_string())
                };
                if result
                    .as_ref()
                    .is_ok_and(|result| native_result_size(result) > MAX_NATIVE_RESULT_BYTES)
                {
                    lifecycle.push(LifecycleEvent::Overloaded {
                        detail: format!(
                            "native completion exceeds {MAX_NATIVE_RESULT_BYTES} bytes"
                        ),
                    });
                    result = Err(format!(
                        "native completion exceeds {MAX_NATIVE_RESULT_BYTES} bytes"
                    ));
                }
                if matches!(result, Ok(NativeResult::TextChanged { .. })) {
                    invalidations.push(Invalidation::Full);
                }
                completions.push(NativeCompletion { request_id, result });
            }
            InputEvent::Cancel { request_id } => {
                lifecycle.push(LifecycleEvent::RequestCancelled {
                    request_id,
                    was_pending: false,
                });
            }
            InputEvent::Heartbeat => lifecycle.push(LifecycleEvent::Heartbeat),
            InputEvent::RequestSnapshot { .. } => {}
            InputEvent::Focus(_) => {}
            InputEvent::Close => {
                if let Some(mut mica) = self.mica.take() {
                    match mica.close().await {
                        Ok(events) => {
                            self.apply_mica_events(events, &mut lifecycle, &mut invalidations)
                                .await
                        }
                        Err(error) => lifecycle.push(LifecycleEvent::Warning(format!(
                            "Mica endpoint shutdown: {error}"
                        ))),
                    }
                }
                self.closed = true;
                for warning in self.editor.shutdown_native_work() {
                    lifecycle.push(LifecycleEvent::Warning(warning));
                }
                let (resources, cleanup_warnings) = self.invalidate_all_resources();
                for warning in cleanup_warnings {
                    lifecycle.push(LifecycleEvent::Warning(warning));
                }
                for resource in resources {
                    lifecycle.push(LifecycleEvent::ResourceInvalidated { resource });
                }
                lifecycle.push(LifecycleEvent::EndpointClosed);
            }
        }

        if !self.closed {
            let (resources, cleanup_warnings) = self.synchronize_identities();
            for warning in cleanup_warnings {
                lifecycle.push(LifecycleEvent::Warning(warning));
            }
            for resource in resources {
                lifecycle.push(LifecycleEvent::ResourceInvalidated { resource });
            }
        }
        let presentation = if self.closed || (!force_full && invalidations.is_empty()) {
            None
        } else {
            self.revision.0 += 1;
            let snapshot = self.capture_snapshot();
            if force_full {
                Some(PresentationUpdate::Full(snapshot))
            } else {
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

    /// Replay a deterministic list of normalized inputs through the same
    /// ordered endpoint used by interactive frontends.
    pub async fn replay(
        &mut self,
        transcript: &SessionTranscript,
    ) -> Result<Vec<SessionOutput>, SessionError> {
        let mut outputs = Vec::with_capacity(transcript.events.len());
        for event in transcript.events.iter().cloned() {
            let envelope = self.envelope(event);
            outputs.push(self.dispatch(envelope).await?);
        }
        Ok(outputs)
    }

    pub async fn replace_mica_first_wave(&mut self, source: String) -> Result<(), MicaHostError> {
        self.mica
            .as_ref()
            .ok_or(MicaHostError::Closed)?
            .replace_first_wave(source)
            .await
    }

    async fn apply_mica_events(
        &mut self,
        events: MicaEventBatch,
        lifecycle: &mut Vec<LifecycleEvent>,
        invalidations: &mut Vec<Invalidation>,
    ) {
        let buffer_candidates = events.buffer_candidates;
        if !events.command_candidates.is_empty() {
            self.mica_palette_actions = events.command_candidates.into_iter().collect();
        }
        for effect in events.effects {
            if let Some(window) = self.editor.windows.get_mut(effect.view) {
                if window.active_buffer == effect.buffer {
                    window.cursor = effect.cursor;
                    if let Some(view) = self.view_ids.get(&effect.view).copied() {
                        invalidations.push(Invalidation::View(view));
                    } else {
                        invalidations.push(Invalidation::Full);
                    }
                } else {
                    lifecycle.push(LifecycleEvent::Warning(
                        "Mica effect referred to a stale view/buffer association".to_owned(),
                    ));
                }
            }
        }
        if events.prompt_close
            && let Some(window) = self.editor.find_command_window()
        {
            self.editor.close_command_window(window);
            invalidations.push(Invalidation::Full);
        }
        for update in events.prompt_updates {
            let (content, cursor) = mica_prompt_content(&update);
            if !self.editor.update_mica_prompt_window(&content, cursor) {
                let command_type = match update.kind.as_str() {
                    "command" => CommandType::Execute,
                    "switch_buffer" => CommandType::BufferSwitch,
                    "kill_buffer" => CommandType::KillBuffer,
                    "find_file" => CommandType::OpenFile(crate::editor::OpenType::New),
                    "visit_file" => CommandType::OpenFile(crate::editor::OpenType::Visit),
                    "isearch_forward" => CommandType::ISearch { forward: true },
                    "isearch_backward" => CommandType::ISearch { forward: false },
                    _ => {
                        lifecycle.push(LifecycleEvent::Error(format!(
                            "unknown Mica prompt kind: {}",
                            update.kind
                        )));
                        continue;
                    }
                };
                self.editor
                    .create_mica_prompt_window(command_type, 10, content, cursor);
            }
            invalidations.push(Invalidation::Full);
        }
        for update in events.search_updates {
            let actions =
                self.editor
                    .apply_mica_search(update.view, &update.matches, update.selected);
            self.resolve_actions(actions, lifecycle, invalidations)
                .await;
        }
        for finish in events.search_finishes {
            let actions = self.editor.finish_mica_search(
                finish.view,
                finish.original_cursor,
                finish.accepted,
            );
            self.resolve_actions(actions, lifecycle, invalidations)
                .await;
        }
        for message in events.errors {
            self.editor.set_echo_message(message.clone());
            invalidations.push(Invalidation::EchoArea);
            lifecycle.push(LifecycleEvent::Error(message));
        }
        for action in events.native_actions {
            let Some(action) = mica_native_action(&action) else {
                lifecycle.push(LifecycleEvent::Error(format!(
                    "unknown Mica native action: {action}"
                )));
                continue;
            };
            match self.editor.perform_native_action(action).await {
                Ok(actions) => {
                    self.resolve_actions(actions, lifecycle, invalidations)
                        .await
                }
                Err(error) => self.fail_endpoint(error, lifecycle),
            }
        }
        for action in events.host_actions {
            match action.name.as_str() {
                "quit" => lifecycle.push(LifecycleEvent::QuitRequested),
                "redraw" => invalidations.push(Invalidation::Full),
                "split_horizontal" => {
                    if let Some(view) = action.view {
                        self.editor.active_window = view;
                    }
                    self.editor.split_horizontal();
                    invalidations.push(Invalidation::Full);
                }
                "split_vertical" => {
                    if let Some(view) = action.view {
                        self.editor.active_window = view;
                    }
                    self.editor.split_vertical();
                    invalidations.push(Invalidation::Full);
                }
                "other_window" => {
                    if let Some(view) = action.view {
                        self.editor.active_window = view;
                    } else {
                        lifecycle.push(LifecycleEvent::Error(
                            "Mica window selection lost its logical view".to_owned(),
                        ));
                    }
                    invalidations.push(Invalidation::Full);
                }
                "delete_window" => {
                    if let Some(view) = action.view {
                        self.editor.active_window = view;
                    }
                    if self.editor.delete_window() {
                        invalidations.push(Invalidation::Full);
                    }
                }
                "delete_other_windows" => {
                    if let Some(view) = action.view {
                        self.editor.active_window = view;
                    }
                    if self.editor.delete_other_windows() {
                        invalidations.push(Invalidation::Full);
                    }
                }
                "save_buffer" => {
                    self.resolve_actions(vec![ChromeAction::Save], lifecycle, invalidations)
                        .await;
                }
                "find_file" => {
                    self.resolve_actions(
                        vec![ChromeAction::OpenFile(crate::editor::OpenType::New)],
                        lifecycle,
                        invalidations,
                    )
                    .await;
                }
                "visit_file" => {
                    self.resolve_actions(
                        vec![ChromeAction::OpenFile(crate::editor::OpenType::Visit)],
                        lifecycle,
                        invalidations,
                    )
                    .await;
                }
                "switch_buffer" => {
                    if let Some(existing) = self.editor.find_command_window() {
                        self.editor.close_command_window(existing);
                    }
                    self.editor.create_command_window(
                        CommandType::BufferSwitch,
                        CommandWindowPosition::Bottom,
                        10,
                        None,
                        Some(buffer_candidates.clone()),
                    );
                    self.editor
                        .set_echo_message("Mica buffer selection".to_owned());
                    invalidations.push(Invalidation::Full);
                }
                "kill_buffer" => {
                    if let Some(existing) = self.editor.find_command_window() {
                        self.editor.close_command_window(existing);
                    }
                    self.editor.create_command_window(
                        CommandType::KillBuffer,
                        CommandWindowPosition::Bottom,
                        10,
                        None,
                        Some(buffer_candidates.clone()),
                    );
                    self.editor
                        .set_echo_message("Mica kill-buffer selection".to_owned());
                    invalidations.push(Invalidation::Full);
                }
                "execute_command" => {
                    let mut candidates: Vec<_> =
                        self.mica_palette_actions.keys().cloned().collect();
                    candidates.sort();
                    if let Some(existing) = self.editor.find_command_window() {
                        self.editor.close_command_window(existing);
                    }
                    self.editor.create_command_window(
                        CommandType::Execute,
                        CommandWindowPosition::Bottom,
                        10,
                        Some(candidates),
                        None,
                    );
                    self.editor
                        .set_echo_message("Mica command selection".to_owned());
                    invalidations.push(Invalidation::Full);
                }
                "switch_buffer_selected" => {
                    if let Some(buffer) = action.buffer {
                        let actions = self.editor.select_mica_buffer(buffer, false);
                        self.resolve_actions(actions, lifecycle, invalidations)
                            .await;
                    } else {
                        lifecycle.push(LifecycleEvent::Error(
                            "Mica switch-buffer result lost its buffer identity".to_owned(),
                        ));
                    }
                }
                "kill_buffer_selected" => {
                    if let Some(buffer) = action.buffer {
                        let actions = self.editor.select_mica_buffer(buffer, true);
                        self.resolve_actions(actions, lifecycle, invalidations)
                            .await;
                    } else {
                        lifecycle.push(LifecycleEvent::Error(
                            "Mica kill-buffer result lost its buffer identity".to_owned(),
                        ));
                    }
                }
                "find_file_selected" | "visit_file_selected" => {
                    if let Some(path) = action.path {
                        let open_type = if action.name == "find_file_selected" {
                            crate::editor::OpenType::New
                        } else {
                            crate::editor::OpenType::Visit
                        };
                        let actions = self.editor.open_mica_file(path.into(), open_type).await;
                        self.resolve_actions(actions, lifecycle, invalidations)
                            .await;
                    } else {
                        lifecycle.push(LifecycleEvent::Error(
                            "Mica file prompt result lost its path".to_owned(),
                        ));
                    }
                }
                "isearch_forward" => {
                    self.resolve_actions(
                        vec![ChromeAction::ISearchForward],
                        lifecycle,
                        invalidations,
                    )
                    .await;
                }
                "isearch_backward" => {
                    self.resolve_actions(
                        vec![ChromeAction::ISearchBackward],
                        lifecycle,
                        invalidations,
                    )
                    .await;
                }
                unknown => lifecycle.push(LifecycleEvent::Error(format!(
                    "unknown Mica host action: {unknown}"
                ))),
            }
        }
        lifecycle.extend(
            events
                .cancelled_tasks
                .into_iter()
                .map(|task_id| LifecycleEvent::MicaTaskCancelled { task_id }),
        );
        lifecycle.extend(
            events
                .ready_subscriptions
                .into_iter()
                .map(|mailbox| LifecycleEvent::MicaSubscriptionReady { mailbox }),
        );
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
                    let Some(selector) = self.mica_palette_actions.get(&command_name).cloned()
                    else {
                        lifecycle.push(LifecycleEvent::Error(format!(
                            "Mica command is no longer discoverable: {command_name}"
                        )));
                        continue;
                    };
                    let Some(mut mica) = self.mica.take() else {
                        lifecycle.push(LifecycleEvent::Error(
                            "Mica command selected without a live Mica host".to_owned(),
                        ));
                        continue;
                    };
                    let invoked = mica
                        .invoke_selector(&self.editor, &self.buffer_resources, &selector)
                        .await;
                    self.mica = Some(mica);
                    match invoked {
                        Ok(events) => {
                            Box::pin(self.apply_mica_events(events, lifecycle, invalidations)).await
                        }
                        Err(error) => lifecycle.push(LifecycleEvent::Error(error.to_string())),
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

    fn fail_endpoint(&mut self, error: std::io::Error, lifecycle: &mut Vec<LifecycleEvent>) {
        self.closed = true;
        lifecycle.push(LifecycleEvent::Fatal(format!(
            "editor input failed: {error}"
        )));
        for warning in self.editor.shutdown_native_work() {
            lifecycle.push(LifecycleEvent::Warning(warning));
        }
        let (resources, cleanup_warnings) = self.invalidate_all_resources();
        for warning in cleanup_warnings {
            lifecycle.push(LifecycleEvent::Warning(warning));
        }
        for resource in resources {
            lifecycle.push(LifecycleEvent::ResourceInvalidated { resource });
        }
        lifecycle.push(LifecycleEvent::EndpointClosed);
    }

    fn apply_pointer(&mut self, pointer: PointerEvent) {
        if pointer.button != PointerButton::Primary && pointer.kind != PointerKind::Move {
            return;
        }
        if pointer.kind == PointerKind::Up {
            self.editor.mouse_drag_state = None;
            self.pointer_selection = None;
            return;
        }

        if pointer.kind == PointerKind::Move {
            if let Some(drag_state) = self.editor.mouse_drag_state.clone() {
                let position = (pointer.column, pointer.row);
                let dx = i32::from(position.0) - i32::from(drag_state.last_pos.0);
                let dy = i32::from(position.1) - i32::from(drag_state.last_pos.1);
                if let Some(state) = self.editor.mouse_drag_state.as_mut() {
                    state.last_pos = position;
                    state.current_pos = position;
                }
                if let Some(border) = drag_state.border_info.as_ref() {
                    update_layout_drag(&mut self.editor, border, dx, dy);
                }
                return;
            }
            if let Some((window_id, anchor)) = self.pointer_selection {
                let cursor = cursor_at(&self.editor, window_id, pointer.column, pointer.row);
                let buffer_id = self.editor.windows[window_id].active_buffer;
                self.editor.buffers[buffer_id].set_mark(anchor);
                self.editor.windows[window_id].cursor = cursor;
            }
            return;
        }

        if let Some((border_info, target_window)) =
            detect_border(&self.editor, pointer.column, pointer.row)
        {
            self.editor.mouse_drag_state = Some(MouseDragState {
                drag_type: DragType::WindowBorder,
                start_pos: (pointer.column, pointer.row),
                last_pos: (pointer.column, pointer.row),
                current_pos: (pointer.column, pointer.row),
                target_window: Some(target_window),
                border_info: Some(border_info),
            });
            self.pointer_selection = None;
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
        let cursor = cursor_at(&self.editor, window_id, pointer.column, pointer.row);
        let buffer_id = self.editor.windows[window_id].active_buffer;
        self.editor.buffers[buffer_id].clear_mark();
        self.editor.windows[window_id].cursor = cursor;
        self.pointer_selection = Some((window_id, cursor));
    }

    fn synchronize_identities(&mut self) -> (Vec<ResourceId>, Vec<String>) {
        let mut invalidated = Vec::new();
        let mut cleanup_warnings = Vec::new();
        let live_buffers: HashSet<_> = self.editor.buffers.keys().collect();
        self.buffer_resources.retain(|buffer, resource| {
            if live_buffers.contains(buffer) {
                true
            } else {
                match self.kernel.lock().unwrap().invalidate_resource(*resource) {
                    Ok(cleanup_error) => {
                        invalidated.push(*resource);
                        if let Some(error) = cleanup_error {
                            cleanup_warnings.push(format!(
                                "resource {resource:?} was revoked after cleanup failed: {error}"
                            ));
                        }
                    }
                    Err(error) => cleanup_warnings.push(format!(
                        "resource {resource:?} invalidation failed: {error}"
                    )),
                }
                false
            }
        });
        for (buffer_id, buffer) in &self.editor.buffers {
            self.buffer_resources
                .entry(buffer_id)
                .or_insert_with(|| self.kernel.lock().unwrap().register_buffer(buffer.clone()));
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
        (invalidated, cleanup_warnings)
    }

    fn invalidate_all_resources(&mut self) -> (Vec<ResourceId>, Vec<String>) {
        let resources: Vec<_> = self
            .buffer_resources
            .drain()
            .map(|(_, resource)| resource)
            .collect();
        let mut invalidated = Vec::with_capacity(resources.len());
        let mut cleanup_warnings = Vec::new();
        for resource in &resources {
            match self.kernel.lock().unwrap().invalidate_resource(*resource) {
                Ok(cleanup_error) => {
                    invalidated.push(*resource);
                    if let Some(error) = cleanup_error {
                        cleanup_warnings.push(format!(
                            "resource {resource:?} was revoked after cleanup failed: {error}"
                        ));
                    }
                }
                Err(error) => cleanup_warnings.push(format!(
                    "resource {resource:?} invalidation failed: {error}"
                )),
            }
        }
        (invalidated, cleanup_warnings)
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
                total_lines,
                max_line_chars: buffer
                    .buffer_lines()
                    .into_iter()
                    .map(|line| line.trim_end_matches('\n').chars().count())
                    .max()
                    .unwrap_or(0),
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

fn mica_native_action(action: &str) -> Option<KeyAction> {
    let cursor = |direction| Some(KeyAction::Cursor(direction));
    let select = |direction| Some(KeyAction::CursorSelect(direction));
    match action {
        "cursor_left" => cursor(CursorDirection::Left),
        "cursor_right" => cursor(CursorDirection::Right),
        "cursor_up" => cursor(CursorDirection::Up),
        "cursor_down" => cursor(CursorDirection::Down),
        "cursor_line_start" => cursor(CursorDirection::LineStart),
        "cursor_line_end" => cursor(CursorDirection::LineEnd),
        "cursor_buffer_start" => cursor(CursorDirection::BufferStart),
        "cursor_buffer_end" => cursor(CursorDirection::BufferEnd),
        "cursor_page_up" => cursor(CursorDirection::PageUp),
        "cursor_page_down" => cursor(CursorDirection::PageDown),
        "cursor_word_forward" => cursor(CursorDirection::WordForward),
        "cursor_word_backward" => cursor(CursorDirection::WordBackward),
        "cursor_paragraph_forward" => cursor(CursorDirection::ParagraphForward),
        "cursor_paragraph_backward" => cursor(CursorDirection::ParagraphBackward),
        "cursor_left_select" => select(CursorDirection::Left),
        "cursor_right_select" => select(CursorDirection::Right),
        "cursor_up_select" => select(CursorDirection::Up),
        "cursor_down_select" => select(CursorDirection::Down),
        "cursor_line_start_select" => select(CursorDirection::LineStart),
        "cursor_line_end_select" => select(CursorDirection::LineEnd),
        "cursor_buffer_start_select" => select(CursorDirection::BufferStart),
        "cursor_buffer_end_select" => select(CursorDirection::BufferEnd),
        "cursor_page_up_select" => select(CursorDirection::PageUp),
        "cursor_page_down_select" => select(CursorDirection::PageDown),
        "cursor_word_forward_select" => select(CursorDirection::WordForward),
        "cursor_word_backward_select" => select(CursorDirection::WordBackward),
        "backspace" => Some(KeyAction::Backspace),
        "delete" => Some(KeyAction::Delete),
        "enter" => Some(KeyAction::Enter),
        "tab" => Some(KeyAction::Tab),
        "kill_line" => Some(KeyAction::KillLine(false)),
        "kill_region" => Some(KeyAction::KillRegion(true)),
        "copy_region" => Some(KeyAction::KillRegion(false)),
        "yank" => Some(KeyAction::Yank(None)),
        "kill_word" => Some(KeyAction::DeleteWord),
        "backward_kill_word" => Some(KeyAction::BackspaceWord),
        "set_mark" => Some(KeyAction::MarkStart),
        "cancel" => Some(KeyAction::Cancel),
        "escape" => Some(KeyAction::Escape),
        "undo" => Some(KeyAction::Undo),
        "redo" => Some(KeyAction::Redo),
        _ => None,
    }
}

fn text_character_from_keys(keys: &[LogicalKey]) -> Option<char> {
    match keys {
        [LogicalKey::AlphaNumeric(character)] => Some(*character),
        [
            LogicalKey::Modifier(crate::keys::KeyModifier::Shift(_)),
            LogicalKey::AlphaNumeric(character),
        ] => character.to_uppercase().next(),
        _ => None,
    }
}

fn mica_prompt_content(update: &MicaPromptUpdate) -> (String, usize) {
    let prefix = match update.kind.as_str() {
        "command" => "M-x ",
        "switch_buffer" => "Switch to buffer: ",
        "kill_buffer" => "Kill buffer: ",
        "find_file" => "Find file: ",
        "visit_file" => "Visit file: ",
        "isearch_forward" => "I-search: ",
        "isearch_backward" => "I-search backward: ",
        _ => "Prompt: ",
    };
    let mut content = format!("{prefix}{}", update.query);
    for (index, (name, target)) in update.candidates.iter().take(8).enumerate() {
        debug_assert!(matches!(
            target,
            MicaPromptTarget::Selector(_) | MicaPromptTarget::Buffer(_) | MicaPromptTarget::Path(_)
        ));
        content.push('\n');
        content.push_str(if index == update.selected { "> " } else { "  " });
        content.push_str(name);
    }
    (
        content,
        prefix.chars().count() + update.query.chars().count(),
    )
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

fn cursor_at(editor: &Editor, window_id: WindowId, column: u16, row: u16) -> usize {
    let window = &editor.windows[window_id];
    let buffer = &editor.buffers[window.active_buffer];
    let line = row
        .saturating_sub(window.y.saturating_add(1))
        .saturating_add(window.start_line);
    let column = column
        .saturating_sub(window.x.saturating_add(1))
        .saturating_add(window.start_column);
    buffer.to_char_index(column, line)
}

fn detect_border(editor: &Editor, x: u16, y: u16) -> Option<(BorderInfo, WindowId)> {
    for (window_id, window) in &editor.windows {
        let right = window
            .x
            .saturating_add(window.width_chars.saturating_sub(1));
        let bottom = window
            .y
            .saturating_add(window.height_chars.saturating_sub(1));
        if (x == window.x || x == right)
            && y >= window.y
            && y <= bottom
            && let Some((path, ratio)) = find_split_for_border(editor, window_id, x, true)
        {
            return Some((
                BorderInfo {
                    is_vertical: true,
                    split_node_path: path,
                    original_ratio: ratio,
                },
                window_id,
            ));
        }
        if (y == window.y || y == bottom)
            && x >= window.x
            && x <= right
            && let Some((path, ratio)) = find_split_for_border(editor, window_id, y, false)
        {
            return Some((
                BorderInfo {
                    is_vertical: false,
                    split_node_path: path,
                    original_ratio: ratio,
                },
                window_id,
            ));
        }
    }
    None
}

fn find_split_for_border(
    editor: &Editor,
    window_id: WindowId,
    coordinate: u16,
    vertical: bool,
) -> Option<(Vec<usize>, f32)> {
    let window = editor.windows.get(window_id)?;
    let (leading, trailing) = if vertical {
        (
            window.x,
            window
                .x
                .saturating_add(window.width_chars.saturating_sub(1)),
        )
    } else {
        (
            window.y,
            window
                .y
                .saturating_add(window.height_chars.saturating_sub(1)),
        )
    };
    let required_branch = if coordinate == leading {
        1
    } else if coordinate == trailing {
        0
    } else {
        return None;
    };
    let direction = if vertical {
        SplitDirection::Vertical
    } else {
        SplitDirection::Horizontal
    };
    find_split_path(&editor.window_tree, window_id, direction, required_branch)
}

fn find_split_path(
    tree: &WindowNode,
    window_id: WindowId,
    direction: SplitDirection,
    required_branch: usize,
) -> Option<(Vec<usize>, f32)> {
    fn leaf_path(node: &WindowNode, target: WindowId, path: &mut Vec<usize>) -> bool {
        match node {
            WindowNode::Leaf { window_id } => *window_id == target,
            WindowNode::Split { first, second, .. } => {
                path.push(0);
                if leaf_path(first, target, path) {
                    return true;
                }
                path.pop();
                path.push(1);
                if leaf_path(second, target, path) {
                    return true;
                }
                path.pop();
                false
            }
        }
    }

    let mut leaf = Vec::new();
    if !leaf_path(tree, window_id, &mut leaf) {
        return None;
    }
    let mut node = tree;
    let mut node_path = Vec::new();
    let mut candidate = None;
    for branch in leaf {
        let WindowNode::Split {
            direction: node_direction,
            ratio,
            first,
            second,
        } = node
        else {
            return None;
        };
        if *node_direction == direction && branch == required_branch {
            candidate = Some((node_path.clone(), *ratio));
        }
        node = if branch == 0 { first } else { second };
        node_path.push(branch);
    }
    candidate
}

fn update_layout_drag(editor: &mut Editor, border: &BorderInfo, dx: i32, dy: i32) {
    const SENSITIVITY: f32 = 0.005;
    let change = if border.is_vertical {
        dx as f32 * SENSITIVITY
    } else {
        dy as f32 * SENSITIVITY
    };
    if change == 0.0 {
        return;
    }
    adjust_ratio_at_path(&mut editor.window_tree, &border.split_node_path, change);
    editor.calculate_window_layout();
}

fn adjust_ratio_at_path(node: &mut WindowNode, path: &[usize], change: f32) {
    if path.is_empty() {
        if let WindowNode::Split { ratio, .. } = node {
            *ratio = (*ratio + change).clamp(0.15, 0.85);
        }
        return;
    }
    match node {
        WindowNode::Leaf { .. } => {}
        WindowNode::Split { first, second, .. } => match path[0] {
            0 => adjust_ratio_at_path(first, &path[1..], change),
            1 => adjust_ratio_at_path(second, &path[1..], change),
            _ => {}
        },
    }
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

fn native_result_size(result: &NativeResult) -> usize {
    match result {
        NativeResult::Snapshot(snapshot) => snapshot.name.len().saturating_add(snapshot.text.len()),
        NativeResult::FileContents(contents) | NativeResult::ClipboardContents(contents) => {
            contents.len()
        }
        NativeResult::DirectoryEntries(entries) => entries.iter().fold(0usize, |size, path| {
            size.saturating_add(path.as_os_str().len())
        }),
        NativeResult::ProcessOutput { stdout, stderr, .. } => {
            stdout.len().saturating_add(stderr.len())
        }
        NativeResult::ResourceCreated(_)
        | NativeResult::ResourceClosed
        | NativeResult::TextChanged { .. }
        | NativeResult::LayoutValidated
        | NativeResult::FileWritten
        | NativeResult::ClipboardWritten
        | NativeResult::ClockMillis(_)
        | NativeResult::WatchRegistered
        | NativeResult::WatchUnregistered => 0,
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
    use std::sync::{Arc, Mutex};

    static MICA_TEST_LOCK: Mutex<()> = Mutex::new(());

    struct FixedNativeClock(u64);

    impl NativeClock for FixedNativeClock {
        fn unix_millis(&self) -> u64 {
            self.0
        }
    }

    fn test_editor() -> Editor {
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
        Editor {
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
        }
    }

    fn test_session_with_grants(grants: CapabilityGrants) -> HostSession {
        HostSession::open(test_editor(), grants)
    }

    fn test_session() -> HostSession {
        test_session_with_grants(CapabilityGrants::editor_default())
    }

    fn snapshot(output: &SessionOutput) -> &PresentationSnapshot {
        match output.presentation.as_ref().unwrap() {
            PresentationUpdate::Full(snapshot) => snapshot,
            PresentationUpdate::Delta(delta) => &delta.snapshot,
        }
    }

    #[test]
    fn mica_keymap_inserts_injected_native_time_and_redraws() {
        let _guard = MICA_TEST_LOCK.lock().unwrap();
        compio::runtime::Runtime::new().unwrap().block_on(async {
            let mut session = HostSession::open_with_mica_clock(
                test_editor(),
                CapabilityGrants::editor_default(),
                Arc::new(FixedNativeClock(1_700_000_000_123)),
            )
            .unwrap();

            let output = session
                .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Function(12)])))
                .await
                .unwrap();
            assert_eq!(
                snapshot(&output).views[0].visible_text,
                "hello1700000000123\n"
            );
            assert_eq!(snapshot(&output).views[0].cursor, 19);
            assert!(
                output
                    .lifecycle
                    .iter()
                    .all(|event| !matches!(event, LifecycleEvent::Error(_)))
            );

            let close = session
                .dispatch(session.envelope(InputEvent::Close))
                .await
                .unwrap();
            assert!(close.lifecycle.contains(&LifecycleEvent::EndpointClosed));
        });
    }

    #[test]
    fn mica_context_tracks_rust_cursor_and_new_active_buffer() {
        let _guard = MICA_TEST_LOCK.lock().unwrap();
        compio::runtime::Runtime::new().unwrap().block_on(async {
            let mut session = HostSession::open_with_mica_clock(
                test_editor(),
                CapabilityGrants::editor_default(),
                Arc::new(FixedNativeClock(42)),
            )
            .unwrap();

            let typed = session
                .dispatch(session.envelope(InputEvent::Text("é".to_owned())))
                .await
                .unwrap();
            assert_eq!(snapshot(&typed).views[0].visible_text, "helloé");
            let after_rust_edit = session
                .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Function(12)])))
                .await
                .unwrap();
            assert_eq!(
                snapshot(&after_rust_edit).views[0].visible_text,
                "helloé42\n"
            );

            let original_view = session.editor.active_window;
            let buffer = session
                .editor
                .create_buffer_with_mode(
                    "*dynamic*".to_owned(),
                    "scratch".to_owned(),
                    "new".to_owned(),
                )
                .unwrap();
            let view = session.editor.split_horizontal();
            session.editor.active_window = view;
            session.editor.windows[view].active_buffer = buffer;
            session.editor.windows[view].cursor = 3;
            let _ = session.synchronize_identities();

            let dynamic = session
                .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Function(12)])))
                .await
                .unwrap();
            let active = snapshot(&dynamic)
                .views
                .iter()
                .find(|presented| presented.active)
                .unwrap();
            assert_eq!(active.visible_text, "new42\n");
            assert_eq!(active.cursor, 6);
            assert_eq!(
                session.mica.as_ref().unwrap().identity_counts_for_test(),
                (2, 2)
            );

            session.editor.active_window = original_view;
            session.editor.windows.remove(view);
            session.editor.window_tree = WindowNode::new_leaf(original_view);
            session.editor.buffers.remove(buffer);
            session.editor.buffer_hosts.remove(&buffer);
            let _ = session.synchronize_identities();
            let after_removal = session
                .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Function(12)])))
                .await
                .unwrap();
            assert_eq!(
                session.mica.as_ref().unwrap().identity_counts_for_test(),
                (1, 1)
            );
            assert_eq!(
                snapshot(&after_removal).views[0].visible_text,
                "helloé42\n42\n"
            );

            session
                .dispatch(session.envelope(InputEvent::Close))
                .await
                .unwrap();
        });
    }

    #[test]
    fn mica_native_bridge_enforces_service_authority() {
        let _guard = MICA_TEST_LOCK.lock().unwrap();
        compio::runtime::Runtime::new().unwrap().block_on(async {
            let mut session = HostSession::open_with_mica_clock(
                test_editor(),
                CapabilityGrants::editor_default(),
                Arc::new(FixedNativeClock(42)),
            )
            .unwrap();
            session
                .mica
                .as_ref()
                .unwrap()
                .revoke_service_for_test("clock_read");

            let denied = session
                .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Function(12)])))
                .await
                .unwrap();
            assert!(denied.lifecycle.iter().any(|event| matches!(
                event,
                LifecycleEvent::Error(message) if message.contains("required native service grant")
            )));
            assert_eq!(snapshot(&denied).views[0].visible_text, "hello");

            session
                .dispatch(session.envelope(InputEvent::Close))
                .await
                .unwrap();
        });
    }

    #[test]
    fn mica_owns_global_chords_and_window_policy() {
        let _guard = MICA_TEST_LOCK.lock().unwrap();
        compio::runtime::Runtime::new().unwrap().block_on(async {
            let mut session = HostSession::open_with_mica_clock(
                test_editor(),
                CapabilityGrants::editor_default(),
                Arc::new(FixedNativeClock(42)),
            )
            .unwrap();
            let control =
                LogicalKey::Modifier(crate::keys::KeyModifier::Control(crate::keys::Side::Left));

            session
                .dispatch(session.envelope(InputEvent::Text("Z".to_owned())))
                .await
                .unwrap();
            session
                .dispatch(session.envelope(InputEvent::Keys(vec![
                    control,
                    LogicalKey::AlphaNumeric('x'),
                ])))
                .await
                .unwrap();
            let undone = session
                .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::AlphaNumeric('u')])))
                .await
                .unwrap();
            assert_eq!(snapshot(&undone).views[0].visible_text, "hello");

            session
                .dispatch(session.envelope(InputEvent::Keys(vec![
                    control,
                    LogicalKey::AlphaNumeric('x'),
                ])))
                .await
                .unwrap();
            let unknown = session
                .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::AlphaNumeric('z')])))
                .await
                .unwrap();
            assert_eq!(snapshot(&unknown).views[0].visible_text, "hello");
            assert!(unknown.lifecycle.iter().any(|event| matches!(
                event,
                LifecycleEvent::Error(message) if message.contains("C-x z is undefined")
            )));

            let prefix = session
                .dispatch(session.envelope(InputEvent::Keys(vec![
                    control,
                    LogicalKey::AlphaNumeric('x'),
                ])))
                .await
                .unwrap();
            assert!(snapshot(&prefix).echo_area.contains("C-x"));
            let split = session
                .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::AlphaNumeric('2')])))
                .await
                .unwrap();
            assert_eq!(snapshot(&split).views.len(), 2);

            session
                .dispatch(session.envelope(InputEvent::Keys(vec![
                    control,
                    LogicalKey::AlphaNumeric('x'),
                ])))
                .await
                .unwrap();
            let switched = session
                .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::AlphaNumeric('o')])))
                .await
                .unwrap();
            assert_eq!(snapshot(&switched).views.len(), 2);
            assert_ne!(
                snapshot(&switched).active_view,
                snapshot(&split).active_view,
                "{switched:#?}"
            );

            session
                .dispatch(session.envelope(InputEvent::Keys(vec![
                    control,
                    LogicalKey::AlphaNumeric('x'),
                ])))
                .await
                .unwrap();
            let deleted = session
                .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::AlphaNumeric('0')])))
                .await
                .unwrap();
            assert_eq!(snapshot(&deleted).views.len(), 1);

            session
                .dispatch(session.envelope(InputEvent::Keys(vec![
                    control,
                    LogicalKey::AlphaNumeric('x'),
                ])))
                .await
                .unwrap();
            let quit = session
                .dispatch(session.envelope(InputEvent::Keys(vec![
                    control,
                    LogicalKey::AlphaNumeric('c'),
                ])))
                .await
                .unwrap();
            assert!(quit.lifecycle.contains(&LifecycleEvent::QuitRequested));

            session
                .dispatch(session.envelope(InputEvent::Close))
                .await
                .unwrap();
        });
    }

    #[test]
    fn mica_discovery_drives_command_palette_and_invocation() {
        let _guard = MICA_TEST_LOCK.lock().unwrap();
        compio::runtime::Runtime::new().unwrap().block_on(async {
            let mut session = HostSession::open_with_mica_clock(
                test_editor(),
                CapabilityGrants::editor_default(),
                Arc::new(FixedNativeClock(42)),
            )
            .unwrap();
            let meta =
                LogicalKey::Modifier(crate::keys::KeyModifier::Meta(crate::keys::Side::Left));

            let palette = session
                .dispatch(
                    session.envelope(InputEvent::Keys(vec![meta, LogicalKey::AlphaNumeric('x')])),
                )
                .await
                .unwrap();
            let prompt = snapshot(&palette)
                .views
                .iter()
                .find(|view| view.command_view)
                .unwrap_or_else(|| panic!("Mica command prompt: {palette:#?}"));
            assert!(prompt.visible_text.starts_with("M-x "));

            let filtered = session
                .dispatch(session.envelope(InputEvent::Text("insert-current-time".to_owned())))
                .await
                .unwrap();
            assert!(
                snapshot(&filtered)
                    .views
                    .iter()
                    .find(|view| view.command_view)
                    .unwrap()
                    .visible_text
                    .contains("insert-current-time")
            );
            let inserted = session
                .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Enter])))
                .await
                .unwrap();
            assert_eq!(
                snapshot(&inserted).views[0].visible_text,
                "hello42\n",
                "{inserted:#?}"
            );

            let palette = session
                .dispatch(
                    session.envelope(InputEvent::Keys(vec![meta, LogicalKey::AlphaNumeric('x')])),
                )
                .await
                .unwrap();
            assert!(
                snapshot(&palette)
                    .views
                    .iter()
                    .any(|view| view.command_view)
            );

            session
                .dispatch(
                    session.envelope(InputEvent::Text("split-window-horizontally".to_owned())),
                )
                .await
                .unwrap();
            let selected = session
                .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Enter])))
                .await
                .unwrap();
            assert_eq!(snapshot(&selected).views.len(), 2);

            session
                .dispatch(session.envelope(InputEvent::Close))
                .await
                .unwrap();
        });
    }

    #[test]
    fn mica_owns_incremental_search_state_and_cancellation() {
        let _guard = MICA_TEST_LOCK.lock().unwrap();
        compio::runtime::Runtime::new().unwrap().block_on(async {
            let mut editor = test_editor();
            let buffer = editor.windows[editor.active_window].active_buffer;
            editor.buffers[buffer].load_str("hello hello");
            editor.windows[editor.active_window].cursor = 5;
            let mut session = HostSession::open_with_mica_clock(
                editor,
                CapabilityGrants::editor_default(),
                Arc::new(FixedNativeClock(42)),
            )
            .unwrap();
            let control =
                LogicalKey::Modifier(crate::keys::KeyModifier::Control(crate::keys::Side::Left));

            session
                .dispatch(session.envelope(InputEvent::Keys(vec![
                    control,
                    LogicalKey::AlphaNumeric('s'),
                ])))
                .await
                .unwrap();
            let searched = session
                .dispatch(session.envelope(InputEvent::Text("hello".to_owned())))
                .await
                .unwrap();
            assert_eq!(
                snapshot(&searched)
                    .views
                    .iter()
                    .find(|view| !view.command_view)
                    .unwrap()
                    .cursor,
                0
            );
            let next = session
                .dispatch(session.envelope(InputEvent::Keys(vec![
                    control,
                    LogicalKey::AlphaNumeric('s'),
                ])))
                .await
                .unwrap();
            assert_eq!(
                snapshot(&next)
                    .views
                    .iter()
                    .find(|view| !view.command_view)
                    .unwrap()
                    .cursor,
                6
            );
            let cancelled = session
                .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Esc])))
                .await
                .unwrap();
            assert_eq!(snapshot(&cancelled).views[0].cursor, 5);
            assert!(!snapshot(&cancelled).views[0].command_view);

            session
                .dispatch(session.envelope(InputEvent::Close))
                .await
                .unwrap();
        });
    }

    #[test]
    fn mica_background_completion_is_pumped_by_idle_timer() {
        let _guard = MICA_TEST_LOCK.lock().unwrap();
        compio::runtime::Runtime::new().unwrap().block_on(async {
            let mut session = HostSession::open_with_mica_clock(
                test_editor(),
                CapabilityGrants::editor_default(),
                Arc::new(FixedNativeClock(42)),
            )
            .unwrap();
            let task = session
                .mica
                .as_ref()
                .unwrap()
                .start_background_test_task()
                .await
                .unwrap();
            compio::time::sleep(std::time::Duration::from_millis(40)).await;

            let idle = session
                .dispatch(session.envelope(InputEvent::Timer { token: 0 }))
                .await
                .unwrap();
            assert!(idle.presentation.is_some());
            assert_eq!(snapshot(&idle).views[0].visible_text, "hello");
            assert!(idle.lifecycle.iter().all(|event| !matches!(
                event,
                LifecycleEvent::Error(message) if message.contains(&task.to_string())
            )));

            session
                .dispatch(session.envelope(InputEvent::Close))
                .await
                .unwrap();
        });
    }

    #[test]
    fn mica_replacement_failure_and_recovery_remain_live() {
        let _guard = MICA_TEST_LOCK.lock().unwrap();
        compio::runtime::Runtime::new().unwrap().block_on(async {
            let mut session = HostSession::open_with_mica_clock(
                test_editor(),
                CapabilityGrants::editor_default(),
                Arc::new(FixedNativeClock(42)),
            )
            .unwrap();
            let original = include_str!("../../mica/roe-first-wave.mica");
            let replacement = original.replace(
                "let text = string_concat(to_literal(clock[:value]), \"\\n\")",
                "let text = string_concat(\"v2:\", to_literal(clock[:value]), \"\\n\")",
            );
            assert_ne!(replacement, original);
            session
                .replace_mica_first_wave(replacement.clone())
                .await
                .unwrap();

            let replaced = session
                .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Function(12)])))
                .await
                .unwrap();
            assert_eq!(snapshot(&replaced).views[0].visible_text, "hellov2:42\n");

            assert!(
                session
                    .replace_mica_first_wave("verb this is malformed".to_owned())
                    .await
                    .is_err()
            );
            let retained = session
                .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Function(12)])))
                .await
                .unwrap();
            assert_eq!(
                snapshot(&retained).views[0].visible_text,
                "hellov2:42\nv2:42\n"
            );

            let prefix = original.split("verb roe/insert_current_time").next().unwrap();
            let failing = format!(
                "{prefix}verb roe/insert_current_time(actor, session)\n  raise E_TEST, \"intentional command failure\"\nend\n"
            );
            session.replace_mica_first_wave(failing).await.unwrap();
            let failed = session
                .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Function(12)])))
                .await
                .unwrap();
            assert!(failed.lifecycle.iter().any(|event| matches!(
                event,
                LifecycleEvent::Error(message) if message.contains("intentional command failure")
            )));
            assert!(snapshot(&failed)
                .echo_area
                .contains("intentional command failure"));

            session
                .replace_mica_first_wave(replacement)
                .await
                .unwrap();
            let recovered = session
                .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Function(12)])))
                .await
                .unwrap();
            assert_eq!(
                snapshot(&recovered).views[0].visible_text,
                "hellov2:42\nv2:42\nv2:42\n"
            );
            session
                .dispatch(session.envelope(InputEvent::Close))
                .await
                .unwrap();
        });
    }

    #[test]
    fn mica_close_cancels_pending_request_and_drains_full_event_queue() {
        let _guard = MICA_TEST_LOCK.lock().unwrap();
        compio::runtime::Runtime::new().unwrap().block_on(async {
            let mut session = HostSession::open_with_mica_clock(
                test_editor(),
                CapabilityGrants::editor_default(),
                Arc::new(FixedNativeClock(7)),
            )
            .unwrap();
            let pending = session
                .mica
                .as_ref()
                .unwrap()
                .start_pending_test_request()
                .await
                .unwrap();
            assert!(
                session
                    .mica
                    .as_ref()
                    .unwrap()
                    .verify_event_backpressure_and_refill_for_test()
                    .await
                    .unwrap()
            );

            let close = session
                .dispatch(session.envelope(InputEvent::Close))
                .await
                .unwrap();
            assert!(
                close
                    .lifecycle
                    .contains(&LifecycleEvent::MicaTaskCancelled { task_id: pending })
            );
            assert!(close.lifecycle.contains(&LifecycleEvent::EndpointClosed));
        });
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
            let mut session = test_session_with_grants(CapabilityGrants::new([]));
            let output = session
                .dispatch(session.envelope(InputEvent::NativeRequest {
                    request_id: RequestId(7),
                    operation: NativeOperation::ReadClockMillis,
                }))
                .await
                .unwrap();
            assert_eq!(output.native_completions[0].request_id, RequestId(7));
            assert!(
                output.native_completions[0]
                    .result
                    .as_ref()
                    .unwrap_err()
                    .contains("was not granted")
            );
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
            assert!(
                close
                    .lifecycle
                    .iter()
                    .any(|event| matches!(event, LifecycleEvent::ResourceInvalidated { .. }))
            );
            assert!(matches!(
                session
                    .dispatch(session.envelope(InputEvent::Heartbeat))
                    .await,
                Err(SessionError::Closed)
            ));
        });
    }

    #[test]
    fn overload_and_cancellation_are_explicit_lifecycle_results() {
        compio::runtime::Runtime::new().unwrap().block_on(async {
            let mut session = test_session();
            let oversized = session
                .dispatch(session.envelope(InputEvent::Keys(vec![
                    LogicalKey::AlphaNumeric('x');
                    MAX_KEYS_PER_INPUT + 1
                ])))
                .await
                .unwrap();
            assert!(matches!(
                oversized.lifecycle.as_slice(),
                [LifecycleEvent::Overloaded { .. }]
            ));
            assert_eq!(session.next_sequence(), 1);

            let cancel = session
                .dispatch(session.envelope(InputEvent::Cancel {
                    request_id: RequestId(19),
                }))
                .await
                .unwrap();
            assert!(
                cancel
                    .lifecycle
                    .contains(&LifecycleEvent::RequestCancelled {
                        request_id: RequestId(19),
                        was_pending: false,
                    })
            );
        });
    }

    #[test]
    fn native_completion_payloads_are_bounded() {
        compio::runtime::Runtime::new().unwrap().block_on(async {
            let unique = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "roe-session-output-{}-{unique}",
                std::process::id()
            ));
            std::fs::write(&path, vec![b'x'; MAX_NATIVE_RESULT_BYTES + 1]).unwrap();
            let mut session =
                test_session_with_grants(CapabilityGrants::new([Capability::FileRead]));
            let output = session
                .dispatch(session.envelope(InputEvent::NativeRequest {
                    request_id: RequestId(23),
                    operation: NativeOperation::ReadFile { path: path.clone() },
                }))
                .await
                .unwrap();
            assert!(output.native_completions[0].result.is_err());
            assert!(
                output
                    .lifecycle
                    .iter()
                    .any(|event| matches!(event, LifecycleEvent::Overloaded { .. }))
            );
            std::fs::remove_file(path).unwrap();
        });
    }

    #[test]
    fn native_watch_changes_surface_through_session_lifecycle() {
        compio::runtime::Runtime::new().unwrap().block_on(async {
            let unique = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let directory = std::env::temp_dir()
                .join(format!("roe-session-watch-{}-{unique}", std::process::id()));
            std::fs::create_dir(&directory).unwrap();
            let path = directory.join("watched.txt");
            std::fs::write(&path, "before").unwrap();

            let mut session = test_session();
            let initial = session.initial_output();
            let resource = snapshot(&initial).views[0].resource;
            let registered = session
                .dispatch(session.envelope(InputEvent::NativeRequest {
                    request_id: RequestId(31),
                    operation: NativeOperation::RegisterWatch {
                        resource,
                        path: path.clone(),
                    },
                }))
                .await
                .unwrap();
            assert!(matches!(
                registered.native_completions[0].result,
                Ok(NativeResult::WatchRegistered)
            ));

            std::fs::write(&path, "after").unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            loop {
                let output = session
                    .dispatch(session.envelope(InputEvent::Timer { token: 0 }))
                    .await
                    .unwrap();
                if output.lifecycle.iter().any(|event| {
                    matches!(
                        event,
                        LifecycleEvent::ResourceChanged {
                            resource: changed,
                            ..
                        } if *changed == resource
                    )
                }) {
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "session did not surface native watch notification"
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
            }

            session
                .dispatch(session.envelope(InputEvent::Close))
                .await
                .unwrap();
            std::fs::remove_file(path).unwrap();
            std::fs::remove_dir(directory).unwrap();
        });
    }

    #[test]
    fn host_reports_cleanup_warning_only_after_resource_is_revoked() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "roe-session-revoke-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("watched.txt");
        std::fs::write(&path, "content").unwrap();

        let mut session = test_session();
        let (buffer, resource) = session
            .buffer_resources
            .iter()
            .next()
            .map(|(buffer, resource)| (*buffer, *resource))
            .unwrap();
        session
            .kernel
            .lock()
            .unwrap()
            .execute(NativeOperation::RegisterWatch {
                resource,
                path: path.clone(),
            })
            .unwrap();
        session
            .kernel
            .lock()
            .unwrap()
            .force_backend_unwatch_for_test(&path)
            .unwrap();
        session.editor.buffers.remove(buffer);

        let (invalidated, warnings) = session.synchronize_identities();
        assert_eq!(invalidated, vec![resource]);
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("cleanup failed"))
        );
        assert!(matches!(
            session.kernel.lock().unwrap().snapshot(resource),
            Err(KernelError::StaleResource(id)) if id == resource
        ));

        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn idle_heartbeat_does_not_advance_presentation_revision() {
        compio::runtime::Runtime::new().unwrap().block_on(async {
            let mut session = test_session();
            let initial = session.initial_output();
            let revision = snapshot(&initial).revision;
            let heartbeat = session
                .dispatch(session.envelope(InputEvent::Heartbeat))
                .await
                .unwrap();
            assert!(heartbeat.presentation.is_none());
            let resync = session
                .dispatch(session.envelope(InputEvent::RequestSnapshot {
                    after: Some(revision),
                }))
                .await
                .unwrap();
            assert_eq!(snapshot(&resync).revision.0, revision.0 + 1);
        });
    }

    #[test]
    fn headless_transcript_replays_the_same_ordered_presentation_path() {
        compio::runtime::Runtime::new().unwrap().block_on(async {
            let mut session = test_session();
            let transcript = SessionTranscript {
                events: vec![
                    InputEvent::Text("!".to_string()),
                    InputEvent::Text("?".to_string()),
                    InputEvent::RequestSnapshot { after: None },
                ],
            };
            let outputs = session.replay(&transcript).await.unwrap();
            assert_eq!(snapshot(&outputs[0]).views[0].visible_text, "hello!");
            assert_eq!(snapshot(&outputs[1]).views[0].visible_text, "hello!?");
            assert_eq!(snapshot(&outputs[2]).views[0].visible_text, "hello!?");
            assert!(
                outputs
                    .windows(2)
                    .all(|pair| snapshot(&pair[0]).revision.0 < snapshot(&pair[1]).revision.0)
            );
        });
    }

    #[test]
    fn nested_layout_drag_changes_only_the_identified_split() {
        let mut ids: SlotMap<WindowId, ()> = SlotMap::with_key();
        let left = ids.insert(());
        let middle = ids.insert(());
        let bottom = ids.insert(());
        let mut tree = WindowNode::new_split(
            SplitDirection::Horizontal,
            0.5,
            WindowNode::new_split(
                SplitDirection::Vertical,
                0.4,
                WindowNode::new_leaf(left),
                WindowNode::new_leaf(middle),
            ),
            WindowNode::new_leaf(bottom),
        );
        assert_eq!(
            find_split_path(&tree, middle, SplitDirection::Vertical, 1),
            Some((vec![0], 0.4))
        );
        adjust_ratio_at_path(&mut tree, &[0], 0.1);
        let WindowNode::Split {
            ratio: root_ratio,
            first,
            ..
        } = tree
        else {
            unreachable!();
        };
        let WindowNode::Split {
            ratio: nested_ratio,
            ..
        } = *first
        else {
            unreachable!();
        };
        assert_eq!(root_ratio, 0.5);
        assert!((nested_ratio - 0.5).abs() < f32::EPSILON);
    }
}
