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
    BorderInfo, ChromeAction, CommandType, DragType, MouseDragState, SplitDirection, WindowNode,
    WindowType,
};
use crate::keys::{CursorDirection, KeyAction, LogicalKey};
use crate::mica_host::{
    MicaEventBatch, MicaHost, MicaHostError, MicaKeyResult, MicaPromptTarget, MicaPromptUpdate,
    normalized_key_sequence,
};
use crate::native_kernel::{
    Capability, CapabilityGrants, KernelError, LayoutNode, LogicalLayout, NativeClock,
    NativeKernel, NativeOperation, NativeResult, ResourceId, SplitAxis, TextSelection, ViewId,
};
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
    Recovery(RecoveryOperation),
    Cancel {
        request_id: RequestId,
    },
    Heartbeat,
    RequestSnapshot {
        after: Option<Revision>,
    },
    Close,
}

/// Small non-programmable surface retained when Mica user policy is broken.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecoveryOperation {
    CheckSource { source: String },
    ReplaceUnit { unit: String, source: String },
    ExportUnit { unit: String },
    RestoreFirstWave,
    SetPackageEnabled { package: String, enabled: bool },
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
    RecoveryResult {
        operation: String,
        result: Result<Option<String>, String>,
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct MicaSyntaxRule {
    kind: String,
    pattern: String,
    precedence: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SyntaxClassAtom {
    Alnum,
    Alpha,
    Digit,
    Lower,
    Upper,
    Space,
    Blank,
    HexDigit,
    Literal(char),
    Range(char, char),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SyntaxClass {
    negated: bool,
    atoms: Vec<SyntaxClassAtom>,
}

impl SyntaxClass {
    fn parse(pattern: &str) -> Result<Self, String> {
        let Some(inner) = pattern
            .strip_prefix('[')
            .and_then(|value| value.strip_suffix(']'))
        else {
            return Err(format!(
                "unsupported Mica word syntax pattern {pattern:?}: expected a character class"
            ));
        };
        let mut characters: Vec<char> = inner.chars().collect();
        let negated = characters.first() == Some(&'^');
        if negated {
            characters.remove(0);
        }
        let mut atoms = Vec::new();
        let mut index = 0;
        while index < characters.len() {
            if characters.get(index) == Some(&'[') && characters.get(index + 1) == Some(&':') {
                let Some(end) = (index + 2..characters.len().saturating_sub(1)).find(|candidate| {
                    characters.get(*candidate) == Some(&':')
                        && characters.get(candidate + 1) == Some(&']')
                }) else {
                    return Err(format!(
                        "invalid POSIX class in Mica syntax pattern {pattern:?}"
                    ));
                };
                let name: String = characters[index + 2..end].iter().collect();
                atoms.push(match name.as_str() {
                    "alnum" => SyntaxClassAtom::Alnum,
                    "alpha" => SyntaxClassAtom::Alpha,
                    "digit" => SyntaxClassAtom::Digit,
                    "lower" => SyntaxClassAtom::Lower,
                    "upper" => SyntaxClassAtom::Upper,
                    "space" => SyntaxClassAtom::Space,
                    "blank" => SyntaxClassAtom::Blank,
                    "xdigit" => SyntaxClassAtom::HexDigit,
                    _ => {
                        return Err(format!(
                            "unsupported POSIX class {name:?} in Mica syntax pattern"
                        ));
                    }
                });
                index = end + 2;
                continue;
            }
            let start = if characters[index] == '\\' {
                index += 1;
                *characters
                    .get(index)
                    .ok_or_else(|| format!("trailing escape in Mica syntax pattern {pattern:?}"))?
            } else {
                characters[index]
            };
            if characters.get(index + 1) == Some(&'-')
                && let Some(end) = characters.get(index + 2).copied()
            {
                atoms.push(SyntaxClassAtom::Range(start, end));
                index += 3;
            } else {
                atoms.push(SyntaxClassAtom::Literal(start));
                index += 1;
            }
        }
        if atoms.is_empty() {
            return Err("Mica word syntax character class is empty".to_owned());
        }
        Ok(Self { negated, atoms })
    }

    fn contains(&self, character: char) -> bool {
        let matched = self.atoms.iter().any(|atom| match atom {
            SyntaxClassAtom::Alnum => character.is_alphanumeric(),
            SyntaxClassAtom::Alpha => character.is_alphabetic(),
            SyntaxClassAtom::Digit => character.is_numeric(),
            SyntaxClassAtom::Lower => character.is_lowercase(),
            SyntaxClassAtom::Upper => character.is_uppercase(),
            SyntaxClassAtom::Space => character.is_whitespace(),
            SyntaxClassAtom::Blank => matches!(character, ' ' | '\t'),
            SyntaxClassAtom::HexDigit => character.is_ascii_hexdigit(),
            SyntaxClassAtom::Literal(expected) => character == *expected,
            SyntaxClassAtom::Range(start, end) => *start <= character && character <= *end,
        });
        matched != self.negated
    }
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
    pending_pointer_drag: Option<(BorderInfo, WindowId, (u16, u16))>,
    mica: Option<MicaHost>,
    mica_modes: HashMap<BufferId, String>,
    mica_faces: HashMap<String, HashMap<String, String>>,
    mica_configuration: HashMap<String, String>,
    mica_syntax: HashMap<BufferId, Vec<MicaSyntaxRule>>,
    mica_search_ranges: HashMap<WindowId, Vec<(usize, usize, String)>>,
    closed: bool,
}

impl HostSession {
    pub fn open(editor: Editor, grants: CapabilityGrants) -> Result<Self, KernelError> {
        Self::open_with_kernel(editor, Arc::new(Mutex::new(NativeKernel::new(grants))))
    }

    fn open_with_kernel(
        editor: Editor,
        kernel: Arc<Mutex<NativeKernel>>,
    ) -> Result<Self, KernelError> {
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
            pending_pointer_drag: None,
            mica: None,
            mica_modes: HashMap::new(),
            mica_faces: HashMap::new(),
            mica_configuration: HashMap::new(),
            mica_syntax: HashMap::new(),
            mica_search_ranges: HashMap::new(),
            closed: false,
        };
        for (buffer, value) in &session.editor.buffers {
            let resource = session
                .kernel
                .lock()
                .unwrap()
                .register_buffer(value.clone())?;
            session.buffer_resources.insert(buffer, resource);
        }
        for window in session.editor.windows.keys() {
            session
                .view_ids
                .insert(window, ViewId(session.next_view_id));
            session.next_view_id += 1;
        }
        Ok(session)
    }

    /// Open the public-driver Mica endpoint used by the first integration
    /// wave. The ordinary constructor remains available for headless and
    /// protocol and native-mechanism tests that do not need policy dispatch.
    pub fn open_with_mica(editor: Editor, grants: CapabilityGrants) -> Result<Self, MicaHostError> {
        let mut session = Self::open(editor, grants)?;
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
        let mut session = Self::open_with_kernel(
            editor,
            Arc::new(Mutex::new(NativeKernel::with_clock(grants, clock))),
        )?;
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

    pub async fn initial_output(&mut self) -> SessionOutput {
        let mut lifecycle = vec![LifecycleEvent::Ready {
            protocol_version: SESSION_PROTOCOL_VERSION,
            capabilities: capability_list(self.kernel.lock().unwrap().grants()),
        }];
        if let Some(mut mica) = self.mica.take() {
            let policy = mica
                .publish_policy(&self.editor, &self.buffer_resources)
                .await;
            self.mica = Some(mica);
            match policy {
                Ok(events) => {
                    let mut initial_invalidations = Vec::new();
                    self.apply_mica_events(events, &mut lifecycle, &mut initial_invalidations)
                        .await;
                }
                Err(error) => lifecycle.push(LifecycleEvent::Error(format!(
                    "failed to publish initial Mica policy: {error}"
                ))),
            }
        }
        self.revision.0 += 1;
        SessionOutput {
            protocol_version: SESSION_PROTOCOL_VERSION,
            epoch: self.epoch,
            input_sequence: self.next_sequence,
            presentation: Some(PresentationUpdate::Full(self.capture_snapshot())),
            native_completions: Vec::new(),
            lifecycle,
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
                    Some(Ok(MicaKeyResult::Unbound)) => {
                        self.editor.set_echo_message(format!(
                            "{} is undefined",
                            normalized_key_sequence(&keys)
                        ));
                        invalidations.push(Invalidation::EchoArea);
                    }
                    None => {
                        if let Some(character) = text_character_from_keys(&keys) {
                            match self
                                .editor
                                .perform_native_action(KeyAction::AlphaNumeric(character))
                                .await
                            {
                                Ok(actions) => {
                                    self.resolve_actions(
                                        actions,
                                        &mut lifecycle,
                                        &mut invalidations,
                                    )
                                    .await;
                                }
                                Err(error) => self.fail_endpoint(error, &mut lifecycle),
                            }
                        } else {
                            self.editor.set_echo_message(format!(
                                "{} is undefined",
                                normalized_key_sequence(&keys)
                            ));
                            invalidations.push(Invalidation::EchoArea);
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
                                Some(Ok(dispatch.key))
                            }
                            Err(error) => Some(Err(error)),
                        }
                    } else {
                        None
                    };
                    match mica_result {
                        Some(Ok(MicaKeyResult::Handled)) => continue,
                        Some(Ok(result)) => {
                            let message =
                                format!("Mica rejected text input {character:?}: {result:?}");
                            self.editor.set_echo_message(message.clone());
                            invalidations.push(Invalidation::EchoArea);
                            lifecycle.push(LifecycleEvent::Error(message));
                            continue;
                        }
                        Some(Err(error)) => {
                            let message = error.to_string();
                            self.editor.set_echo_message(message.clone());
                            invalidations.push(Invalidation::EchoArea);
                            lifecycle.push(LifecycleEvent::Error(message));
                            continue;
                        }
                        None => {}
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
                self.apply_pointer(pointer, &mut lifecycle, &mut invalidations)
                    .await;
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
                    let window = &self.editor.windows[window_id];
                    let line = start_line.unwrap_or(window.start_line).min(max_line);
                    let column = start_column.unwrap_or(window.start_column).min(max_column);
                    if let Some(mut mica) = self.mica.take() {
                        let result = mica
                            .set_view_scroll(
                                &self.editor,
                                &self.buffer_resources,
                                window_id,
                                line,
                                column,
                            )
                            .await;
                        self.mica = Some(mica);
                        match result {
                            Ok(events) => {
                                self.apply_mica_events(events, &mut lifecycle, &mut invalidations)
                                    .await;
                            }
                            Err(error) => lifecycle.push(LifecycleEvent::Error(error.to_string())),
                        }
                    } else {
                        let window = &mut self.editor.windows[window_id];
                        window.start_line = line;
                        window.start_column = column;
                        invalidations.push(Invalidation::View(view));
                    }
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
            InputEvent::Recovery(operation) => {
                let (name, result) = match operation {
                    RecoveryOperation::CheckSource { source } => (
                        "check-source",
                        self.check_mica_source(source)
                            .await
                            .map(|()| None)
                            .map_err(|error| error.to_string()),
                    ),
                    RecoveryOperation::ReplaceUnit { unit, source } => (
                        "replace-unit",
                        self.replace_mica_unit(&unit, source)
                            .await
                            .map(|()| None)
                            .map_err(|error| error.to_string()),
                    ),
                    RecoveryOperation::ExportUnit { unit } => (
                        "export-unit",
                        self.export_mica_unit(&unit)
                            .await
                            .map(Some)
                            .map_err(|error| error.to_string()),
                    ),
                    RecoveryOperation::RestoreFirstWave => (
                        "restore-first-wave",
                        self.restore_mica_first_wave()
                            .await
                            .map(|()| None)
                            .map_err(|error| error.to_string()),
                    ),
                    RecoveryOperation::SetPackageEnabled { package, enabled } => (
                        "set-package-enabled",
                        self.set_mica_package_enabled(&package, enabled)
                            .map(|()| None)
                            .map_err(|error| error.to_string()),
                    ),
                };
                lifecycle.push(LifecycleEvent::RecoveryResult {
                    operation: name.to_owned(),
                    result,
                });
                invalidations.push(Invalidation::Full);
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

    pub async fn check_mica_source(&self, source: String) -> Result<(), MicaHostError> {
        self.mica
            .as_ref()
            .ok_or(MicaHostError::Closed)?
            .check_source(source)
            .await
    }

    pub async fn replace_mica_unit(
        &mut self,
        unit: &str,
        source: String,
    ) -> Result<(), MicaHostError> {
        self.mica
            .as_mut()
            .ok_or(MicaHostError::Closed)?
            .replace_unit(unit, source)
            .await
    }

    pub async fn export_mica_unit(&self, unit: &str) -> Result<String, MicaHostError> {
        self.mica
            .as_ref()
            .ok_or(MicaHostError::Closed)?
            .export_unit(unit)
            .await
    }

    pub async fn restore_mica_first_wave(&mut self) -> Result<(), MicaHostError> {
        self.mica
            .as_mut()
            .ok_or(MicaHostError::Closed)?
            .restore_first_wave()
            .await
    }

    pub fn set_mica_package_enabled(
        &mut self,
        package: &str,
        enabled: bool,
    ) -> Result<(), MicaHostError> {
        self.mica
            .as_mut()
            .ok_or(MicaHostError::Closed)?
            .set_package_enabled(package, enabled)
    }

    pub async fn replace_mica_first_wave(&mut self, source: String) -> Result<(), MicaHostError> {
        self.replace_mica_unit("roe/first-wave", source).await
    }

    async fn apply_mica_events(
        &mut self,
        events: MicaEventBatch,
        lifecycle: &mut Vec<LifecycleEvent>,
        invalidations: &mut Vec<Invalidation>,
    ) {
        if events.policy_reset {
            self.mica_modes.clear();
            self.mica_faces.clear();
            self.mica_configuration.clear();
            self.mica_syntax.clear();
        }
        for policy in events.policy_facts {
            match policy.kind.as_str() {
                "mode_policy" => {
                    if let Some(buffer) = policy.subject {
                        self.mica_modes.insert(buffer, policy.name);
                    }
                }
                "face_policy" => {
                    if let Some(attribute) = policy.attribute {
                        self.mica_faces
                            .entry(policy.name)
                            .or_default()
                            .insert(attribute, policy.value);
                    }
                }
                "configuration_policy" => {
                    self.mica_configuration.insert(policy.name, policy.value);
                }
                "syntax_policy" => {
                    if let Some(buffer) = policy.subject {
                        let rules = self.mica_syntax.entry(buffer).or_default();
                        let rule = MicaSyntaxRule {
                            kind: policy.name,
                            pattern: policy.value,
                            precedence: policy.precedence.unwrap_or_default(),
                        };
                        if !rules.contains(&rule) {
                            rules.push(rule);
                        }
                    }
                }
                _ => {}
            }
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
                    "command_argument" => CommandType::Argument,
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
            let Some(window) = self.editor.windows.get_mut(update.view) else {
                lifecycle.push(LifecycleEvent::Warning(
                    "Mica search update referred to a stale view".to_owned(),
                ));
                continue;
            };
            self.mica_search_ranges.insert(
                update.view,
                update
                    .matches
                    .iter()
                    .enumerate()
                    .map(|(index, (start, end))| {
                        (
                            *start,
                            *end,
                            if update.selected == Some(index) {
                                "isearch-current"
                            } else {
                                "isearch-match"
                            }
                            .to_owned(),
                        )
                    })
                    .collect(),
            );
            if let Some(index) = update.selected
                && let Some((start, _)) = update.matches.get(index)
            {
                window.cursor = *start;
            }
            if let Some(view) = self.view_ids.get(&update.view).copied() {
                invalidations.push(Invalidation::View(view));
            } else {
                invalidations.push(Invalidation::Full);
            }
        }
        for finish in events.search_finishes {
            self.mica_search_ranges.remove(&finish.view);
            if !finish.accepted
                && let Some(window) = self.editor.windows.get_mut(finish.view)
            {
                window.cursor = finish.original_cursor;
            }
            invalidations.push(Invalidation::Full);
        }
        for message in events.errors {
            self.editor.set_echo_message(message.clone());
            invalidations.push(Invalidation::EchoArea);
            lifecycle.push(LifecycleEvent::Error(message));
        }
        for action in events.native_actions {
            let mut denied = false;
            for capability in mica_native_capabilities(&action.name) {
                if let Err(error) = self.kernel.lock().unwrap().authorize(*capability) {
                    lifecycle.push(LifecycleEvent::Error(format!(
                        "Mica native action {} was denied: {error}",
                        action.name
                    )));
                    denied = true;
                    break;
                }
            }
            if denied {
                continue;
            }
            if action.name == "yank" {
                match self
                    .kernel
                    .lock()
                    .unwrap()
                    .execute(NativeOperation::ReadClipboard)
                {
                    Ok(NativeResult::ClipboardContents(text)) => {
                        self.editor.kill_ring.import_external_text(text);
                    }
                    Ok(other) => lifecycle.push(LifecycleEvent::Error(format!(
                        "clipboard read returned an unexpected native result: {other:?}"
                    ))),
                    Err(error) => {
                        lifecycle.push(LifecycleEvent::Warning(format!(
                            "clipboard read failed before yank; using the internal kill ring: {error}"
                        )));
                    }
                }
            }
            if action.name == "insert_text" {
                let actions = self.editor.insert_text(
                    action.text.unwrap_or_default(),
                    &crate::editor::ActionPosition::Cursor,
                );
                self.resolve_actions(actions, lifecycle, invalidations)
                    .await;
                continue;
            }
            if matches!(action.name.as_str(), "kill_word" | "backward_kill_word") {
                let action_name = action.name.clone();
                match self.mica_kill_word(action.name == "kill_word") {
                    Ok(actions) => {
                        self.resolve_actions(actions, lifecycle, invalidations)
                            .await;
                        self.write_kill_ring_to_native(&action_name, lifecycle);
                    }
                    Err(message) => {
                        self.editor.set_echo_message(message.clone());
                        lifecycle.push(LifecycleEvent::Error(message));
                    }
                }
                continue;
            }
            let action_name = action.name.clone();
            let Some(action) = mica_native_action(&action_name) else {
                lifecycle.push(LifecycleEvent::Error(format!(
                    "unknown Mica native action: {action_name}"
                )));
                continue;
            };
            match self.editor.perform_native_action(action).await {
                Ok(actions) => {
                    self.resolve_actions(actions, lifecycle, invalidations)
                        .await;
                    self.write_kill_ring_to_native(&action_name, lifecycle);
                }
                Err(error) => self.fail_endpoint(error, lifecycle),
            }
        }
        for action in events.host_actions {
            match action.name.as_str() {
                "quit" => lifecycle.push(LifecycleEvent::QuitRequested),
                "redraw" => invalidations.push(Invalidation::Full),
                "split_horizontal" => {
                    if let Err(error) = self.kernel.lock().unwrap().authorize(Capability::Layout) {
                        lifecycle.push(LifecycleEvent::Error(format!(
                            "Mica horizontal split was denied: {error}"
                        )));
                        continue;
                    }
                    if let Some(view) = action.view {
                        self.editor.active_window = view;
                    }
                    self.editor.split_horizontal();
                    invalidations.push(Invalidation::Full);
                }
                "split_vertical" => {
                    if let Err(error) = self.kernel.lock().unwrap().authorize(Capability::Layout) {
                        lifecycle.push(LifecycleEvent::Error(format!(
                            "Mica vertical split was denied: {error}"
                        )));
                        continue;
                    }
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
                    if let Err(error) = self.kernel.lock().unwrap().authorize(Capability::Layout) {
                        lifecycle.push(LifecycleEvent::Error(format!(
                            "Mica window deletion was denied: {error}"
                        )));
                        continue;
                    }
                    if let Some(view) = action.view {
                        self.editor.active_window = view;
                    }
                    if self.editor.delete_window() {
                        invalidations.push(Invalidation::Full);
                    }
                }
                "delete_other_windows" => {
                    if let Err(error) = self.kernel.lock().unwrap().authorize(Capability::Layout) {
                        lifecycle.push(LifecycleEvent::Error(format!(
                            "Mica window collapse was denied: {error}"
                        )));
                        continue;
                    }
                    if let Some(view) = action.view {
                        self.editor.active_window = view;
                    }
                    if self.editor.delete_other_windows() {
                        invalidations.push(Invalidation::Full);
                    }
                }
                "begin_layout_drag" => match (action.view, self.pending_pointer_drag.take()) {
                    (Some(view), Some((border, target, position))) if view == target => {
                        self.editor.mouse_drag_state = Some(MouseDragState {
                            drag_type: DragType::WindowBorder,
                            start_pos: position,
                            last_pos: position,
                            current_pos: position,
                            target_window: Some(target),
                            border_info: Some(border),
                        });
                        self.pointer_selection = None;
                    }
                    _ => lifecycle.push(LifecycleEvent::Error(
                        "Mica layout-drag decision lost its native border target".to_owned(),
                    )),
                },
                "pointer_selection" => match action.phase.as_deref() {
                    Some("down") => {
                        if let (Some(view), Some(position), Some(anchor)) =
                            (action.view, action.position, action.anchor)
                        {
                            if self.editor.active_window != view {
                                self.editor.previous_active_window =
                                    Some(self.editor.active_window);
                                self.editor.active_window = view;
                            }
                            let buffer = self.editor.windows[view].active_buffer;
                            self.editor.buffers[buffer].clear_mark();
                            self.editor.windows[view].cursor = position;
                            self.pointer_selection = Some((view, anchor));
                            invalidations.push(Invalidation::Full);
                        }
                    }
                    Some("move") => {
                        if let (Some(view), Some(position), Some(anchor)) =
                            (action.view, action.position, action.anchor)
                        {
                            let buffer = self.editor.windows[view].active_buffer;
                            self.editor.buffers[buffer].set_mark(anchor);
                            self.editor.windows[view].cursor = position;
                            invalidations.push(Invalidation::Full);
                        }
                    }
                    Some("up") => {
                        self.editor.mouse_drag_state = None;
                        self.pointer_selection = None;
                        self.pending_pointer_drag = None;
                    }
                    _ => lifecycle.push(LifecycleEvent::Error(
                        "Mica pointer decision had an unknown phase".to_owned(),
                    )),
                },
                "set_view_scroll" => {
                    if let (Some(view), Some(line), Some(column)) =
                        (action.view, action.line, action.column)
                    {
                        let window = &mut self.editor.windows[view];
                        window.start_line = line;
                        window.start_column = column;
                        if let Some(id) = self.view_ids.get(&view).copied() {
                            invalidations.push(Invalidation::View(id));
                        }
                    }
                }
                "set_split_ratio" => {
                    if let (Some(path), Some(ratio)) = (action.split_path, action.ratio) {
                        let mut proposed = self.editor.window_tree.clone();
                        if set_ratio_at_path(&mut proposed, &path, ratio) {
                            let layout = logical_layout(&proposed, &self.editor, &self.view_ids);
                            match layout.and_then(|layout| {
                                self.kernel
                                    .lock()
                                    .unwrap()
                                    .execute(NativeOperation::ValidateLayout { layout })
                                    .map_err(|error| error.to_string())
                            }) {
                                Ok(_) => {
                                    self.editor.window_tree = proposed;
                                    self.editor.calculate_window_layout();
                                    invalidations.push(Invalidation::Full);
                                }
                                Err(error) => lifecycle.push(LifecycleEvent::Error(format!(
                                    "Mica split-ratio decision failed native validation: {error}"
                                ))),
                            }
                        }
                    }
                }
                "invalidate_syntax" => {
                    if let Some(window) = action.view {
                        if let Some(view) = self.view_ids.get(&window).copied() {
                            invalidations.push(Invalidation::View(view));
                        } else {
                            invalidations.push(Invalidation::Full);
                        }
                    }
                }
                "save_buffer" => {
                    self.resolve_actions(vec![ChromeAction::Save], lifecycle, invalidations)
                        .await;
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
                        let path = std::path::PathBuf::from(path);
                        let open_type = if action.name == "find_file_selected" {
                            crate::editor::OpenType::New
                        } else {
                            crate::editor::OpenType::Visit
                        };
                        let content = match self
                            .kernel
                            .lock()
                            .unwrap()
                            .execute(NativeOperation::ReadFile { path: path.clone() })
                        {
                            Ok(NativeResult::FileContents(content)) => Some(content),
                            Err(KernelError::Io(error))
                                if error.kind() == std::io::ErrorKind::NotFound =>
                            {
                                None
                            }
                            Ok(other) => {
                                lifecycle.push(LifecycleEvent::Error(format!(
                                    "file read returned an unexpected native result: {other:?}"
                                )));
                                continue;
                            }
                            Err(error) => {
                                let message = format!(
                                    "failed to open {} through the native kernel: {error}",
                                    path.display()
                                );
                                lifecycle.push(LifecycleEvent::Error(message.clone()));
                                self.editor.set_echo_message(message);
                                invalidations.push(Invalidation::EchoArea);
                                continue;
                            }
                        };
                        let actions = self.editor.open_mica_file(path, open_type, content);
                        self.resolve_actions(actions, lifecycle, invalidations)
                            .await;
                    } else {
                        lifecycle.push(LifecycleEvent::Error(
                            "Mica file prompt result lost its path".to_owned(),
                        ));
                    }
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

    fn mica_kill_word(&mut self, forward: bool) -> Result<Vec<ChromeAction>, String> {
        let window = &self.editor.windows[self.editor.active_window];
        let buffer_id = window.active_buffer;
        let cursor = window.cursor;
        let text: Vec<char> = self.editor.buffers[buffer_id].content().chars().collect();
        let word_rules: Vec<_> = self
            .mica_syntax
            .get(&buffer_id)
            .into_iter()
            .flatten()
            .filter(|rule| rule.kind == "word")
            .collect();
        let precedence = word_rules
            .iter()
            .map(|rule| rule.precedence)
            .max()
            .ok_or_else(|| "Mica has no effective word syntax rule".to_owned())?;
        let mut patterns: Vec<_> = word_rules
            .into_iter()
            .filter(|rule| rule.precedence == precedence)
            .map(|rule| rule.pattern.as_str())
            .collect();
        patterns.sort_unstable();
        patterns.dedup();
        if patterns.len() != 1 {
            return Err(format!(
                "Mica word syntax is ambiguous at precedence {precedence}"
            ));
        }
        let syntax = SyntaxClass::parse(patterns[0])?;
        let is_word = |character: char| syntax.contains(character);
        let boundary = if forward {
            let mut position = cursor.min(text.len());
            while position < text.len() && !is_word(text[position]) {
                position += 1;
            }
            while position < text.len() && is_word(text[position]) {
                position += 1;
            }
            while position < text.len() && !is_word(text[position]) {
                position += 1;
            }
            position
        } else {
            let mut position = cursor.min(text.len());
            while position > 0 && !is_word(text[position - 1]) {
                position -= 1;
            }
            while position > 0 && is_word(text[position - 1]) {
                position -= 1;
            }
            position
        };
        let count = if forward {
            isize::try_from(boundary.saturating_sub(cursor)).unwrap_or(isize::MAX)
        } else {
            -isize::try_from(cursor.saturating_sub(boundary)).unwrap_or(isize::MAX)
        };
        Ok(self
            .editor
            .kill_text(&crate::editor::ActionPosition::Cursor, count))
    }

    fn write_kill_ring_to_native(
        &mut self,
        action_name: &str,
        lifecycle: &mut Vec<LifecycleEvent>,
    ) {
        if !mica_action_writes_clipboard(action_name) {
            return;
        }
        let Some(text) = self.editor.kill_ring.current().map(str::to_owned) else {
            return;
        };
        match self
            .kernel
            .lock()
            .unwrap()
            .execute(NativeOperation::WriteClipboard { contents: text })
        {
            Ok(NativeResult::ClipboardWritten) => {}
            Ok(other) => lifecycle.push(LifecycleEvent::Error(format!(
                "clipboard write returned an unexpected native result: {other:?}"
            ))),
            Err(error) => lifecycle.push(LifecycleEvent::Error(format!(
                "clipboard write failed after {action_name}: {error}"
            ))),
        }
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
            InputEvent::Recovery(RecoveryOperation::CheckSource { source })
            | InputEvent::Recovery(RecoveryOperation::ReplaceUnit { source, .. })
                if source.chars().count() > MAX_TEXT_CHARS_PER_INPUT =>
            {
                Err(SessionError::InputTooLarge(format!(
                    "Mica recovery source exceeds {MAX_TEXT_CHARS_PER_INPUT} characters"
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
        let mut pending: VecDeque<_> = actions.into();
        while let Some(action) = pending.pop_front() {
            match action {
                ChromeAction::Echo(message) => {
                    self.editor.set_echo_message(message);
                    invalidations.push(Invalidation::EchoArea);
                }
                ChromeAction::MarkDirty(_) => invalidations.push(Invalidation::Full),
                ChromeAction::Save => {
                    pending.extend(self.save_active_buffer_via_kernel(lifecycle));
                }
                ChromeAction::CursorMove(_) | ChromeAction::BufferChanged { .. } => {}
            }
        }
    }

    fn save_active_buffer_via_kernel(
        &mut self,
        lifecycle: &mut Vec<LifecycleEvent>,
    ) -> Vec<ChromeAction> {
        let window = self.editor.active_window;
        let buffer_id = self.editor.windows[window].active_buffer;
        let Some(buffer) = self.editor.buffers.get(buffer_id).cloned() else {
            return vec![ChromeAction::Echo("No active buffer".to_owned())];
        };
        let path = std::path::PathBuf::from(buffer.object());
        let content = buffer.content();
        match self
            .kernel
            .lock()
            .unwrap()
            .execute(NativeOperation::WriteFile {
                path: path.clone(),
                contents: content.clone(),
            }) {
            Ok(NativeResult::FileWritten) => {}
            Ok(other) => {
                let message = format!("save returned an unexpected native result: {other:?}");
                lifecycle.push(LifecycleEvent::Error(message.clone()));
                return vec![ChromeAction::Echo(message)];
            }
            Err(error) => {
                let message = format!("failed to save {}: {error}", path.display());
                lifecycle.push(LifecycleEvent::Error(message.clone()));
                return vec![ChromeAction::Echo(message)];
            }
        }
        let watch_error = self
            .editor
            .file_watcher
            .watch_file(buffer_id, &path, content)
            .err();
        let mut actions = vec![ChromeAction::Echo(format!("Saved: {}", path.display()))];
        if let Some(error) = watch_error {
            actions.push(ChromeAction::Echo(format!(
                "Saved {}, but failed to watch it: {error}",
                path.display()
            )));
        }
        actions
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

    async fn apply_pointer(
        &mut self,
        pointer: PointerEvent,
        lifecycle: &mut Vec<LifecycleEvent>,
        invalidations: &mut Vec<Invalidation>,
    ) {
        if self.mica.is_none() {
            self.apply_pointer_without_policy(pointer);
            invalidations.push(Invalidation::Full);
            return;
        }

        let button = pointer_button_name(pointer.button);
        if pointer.kind == PointerKind::Up {
            let window = self
                .pointer_selection
                .map(|(window, _)| window)
                .or_else(|| {
                    self.editor
                        .mouse_drag_state
                        .as_ref()
                        .and_then(|drag| drag.target_window)
                })
                .unwrap_or(self.editor.active_window);
            let position = self.editor.windows[window].cursor;
            self.dispatch_mica_pointer(window, position, "up", button, lifecycle, invalidations)
                .await;
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
                if let Some(border) = drag_state.border_info.as_ref()
                    && let Some(current) =
                        ratio_at_path(&self.editor.window_tree, &border.split_node_path)
                {
                    const SENSITIVITY: f32 = 0.005;
                    let delta = if border.is_vertical {
                        dx as f32 * SENSITIVITY
                    } else {
                        dy as f32 * SENSITIVITY
                    };
                    let proposed = (current + delta).clamp(0.15, 0.85);
                    if proposed != current {
                        let mut mica = self.mica.take().expect("Mica presence checked above");
                        let result = mica
                            .set_split_ratio(
                                &self.editor,
                                &self.buffer_resources,
                                &border.split_node_path,
                                proposed,
                            )
                            .await;
                        self.mica = Some(mica);
                        match result {
                            Ok(events) => {
                                self.apply_mica_events(events, lifecycle, invalidations)
                                    .await;
                            }
                            Err(error) => lifecycle.push(LifecycleEvent::Error(error.to_string())),
                        }
                    }
                }
                return;
            }
            if let Some((window, _)) = self.pointer_selection {
                let position = cursor_at(&self.editor, window, pointer.column, pointer.row);
                self.dispatch_mica_pointer(
                    window,
                    position,
                    "move",
                    button,
                    lifecycle,
                    invalidations,
                )
                .await;
            }
            return;
        }

        if pointer.button == PointerButton::Primary
            && let Some((border, target)) = detect_border(&self.editor, pointer.column, pointer.row)
        {
            self.pending_pointer_drag = Some((border, target, (pointer.column, pointer.row)));
            let position = self.editor.windows[target].cursor;
            self.dispatch_mica_pointer(
                target,
                position,
                "layout_down",
                button,
                lifecycle,
                invalidations,
            )
            .await;
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
        if let Some(window) = selected {
            let position = cursor_at(&self.editor, window, pointer.column, pointer.row);
            self.dispatch_mica_pointer(window, position, "down", button, lifecycle, invalidations)
                .await;
        }
    }

    async fn dispatch_mica_pointer(
        &mut self,
        view: WindowId,
        position: usize,
        phase: &str,
        button: &str,
        lifecycle: &mut Vec<LifecycleEvent>,
        invalidations: &mut Vec<Invalidation>,
    ) {
        let mut mica = self.mica.take().expect("Mica presence checked by caller");
        let result = mica
            .dispatch_pointer(
                &self.editor,
                &self.buffer_resources,
                view,
                position,
                phase,
                button,
            )
            .await;
        self.mica = Some(mica);
        match result {
            Ok(events) => {
                self.apply_mica_events(events, lifecycle, invalidations)
                    .await;
            }
            Err(error) => lifecycle.push(LifecycleEvent::Error(error.to_string())),
        }
    }

    fn apply_pointer_without_policy(&mut self, pointer: PointerEvent) {
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
            if !self.buffer_resources.contains_key(&buffer_id) {
                match self.kernel.lock().unwrap().register_buffer(buffer.clone()) {
                    Ok(resource) => {
                        self.buffer_resources.insert(buffer_id, resource);
                    }
                    Err(error) => cleanup_warnings.push(format!(
                        "buffer {} has no native resource: {error}",
                        buffer.object()
                    )),
                }
            }
        }

        let live_windows: HashSet<_> = self.editor.windows.keys().collect();
        self.view_ids
            .retain(|window, _| live_windows.contains(window));
        self.mica_search_ranges
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

            let styled_ranges = self
                .mica_search_ranges
                .get(&window_id)
                .into_iter()
                .flatten()
                .filter(|(start, end, _)| *end > visible_start_char && *start < visible_end_char)
                .map(|(start, end, name)| {
                    let style = *style_by_name.entry(name.clone()).or_insert_with(|| {
                        let id = StyleRef(styles.len() as u32 + 1);
                        let attributes = self.mica_faces.get(name);
                        styles.push(StyleDefinition {
                            id,
                            name: name.clone(),
                            foreground: attributes
                                .and_then(|values| values.get("foreground"))
                                .and_then(|value| presentation_color_hex(value)),
                            background: attributes
                                .and_then(|values| values.get("background"))
                                .and_then(|value| presentation_color_hex(value)),
                            bold: attributes
                                .and_then(|values| values.get("weight"))
                                .is_some_and(|value| value == "bold"),
                            italic: attributes
                                .and_then(|values| values.get("slant"))
                                .is_some_and(|value| value == "italic"),
                            underline: attributes
                                .and_then(|values| values.get("underline"))
                                .is_some_and(|value| value == "true"),
                            strikethrough: false,
                        });
                        id
                    });
                    StyledRange {
                        start: *start,
                        end: *end,
                        style,
                    }
                })
                .collect();

            let selection = buffer
                .get_region(window.cursor)
                .map(|(anchor, active)| TextSelection { anchor, active });
            let (column, line) = buffer.to_column_line(window.cursor);
            let mode = self
                .mica_modes
                .get(&window.active_buffer)
                .cloned()
                .unwrap_or_else(|| "unpublished".to_string());
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

fn mica_native_capabilities(name: &str) -> &'static [Capability] {
    match name {
        "kill_line" | "kill_region" | "kill_word" | "backward_kill_word" => {
            &[Capability::TextWrite, Capability::ClipboardWrite]
        }
        "copy_region" => &[Capability::ClipboardWrite],
        "yank" => &[Capability::TextWrite, Capability::ClipboardRead],
        "insert_text" | "backspace" | "delete" | "enter" | "indent" | "undo" | "redo" => {
            &[Capability::TextWrite]
        }
        _ => &[],
    }
}

fn mica_action_writes_clipboard(name: &str) -> bool {
    matches!(
        name,
        "kill_line" | "kill_region" | "copy_region" | "kill_word" | "backward_kill_word"
    )
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
            MicaPromptTarget::Selector(_)
                | MicaPromptTarget::Buffer(_)
                | MicaPromptTarget::View(_)
                | MicaPromptTarget::Path(_)
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

fn pointer_button_name(button: PointerButton) -> &'static str {
    match button {
        PointerButton::Primary => "primary",
        PointerButton::Secondary => "secondary",
        PointerButton::Middle => "middle",
        PointerButton::None => "none",
    }
}

fn ratio_at_path(node: &WindowNode, path: &[usize]) -> Option<f32> {
    if path.is_empty() {
        return match node {
            WindowNode::Split { ratio, .. } => Some(*ratio),
            WindowNode::Leaf { .. } => None,
        };
    }
    match node {
        WindowNode::Leaf { .. } => None,
        WindowNode::Split { first, second, .. } => match path[0] {
            0 => ratio_at_path(first, &path[1..]),
            1 => ratio_at_path(second, &path[1..]),
            _ => None,
        },
    }
}

fn set_ratio_at_path(node: &mut WindowNode, path: &[usize], ratio: f32) -> bool {
    if path.is_empty() {
        if let WindowNode::Split { ratio: current, .. } = node {
            *current = ratio;
            return true;
        }
        return false;
    }
    match node {
        WindowNode::Leaf { .. } => false,
        WindowNode::Split { first, second, .. } => match path[0] {
            0 => set_ratio_at_path(first, &path[1..], ratio),
            1 => set_ratio_at_path(second, &path[1..], ratio),
            _ => false,
        },
    }
}

fn logical_layout(
    tree: &WindowNode,
    editor: &Editor,
    view_ids: &HashMap<WindowId, ViewId>,
) -> Result<LogicalLayout, String> {
    fn convert(
        node: &WindowNode,
        view_ids: &HashMap<WindowId, ViewId>,
    ) -> Result<LayoutNode, String> {
        match node {
            WindowNode::Leaf { window_id } => view_ids
                .get(window_id)
                .copied()
                .map(LayoutNode::View)
                .ok_or_else(|| "layout leaf has no transport view identity".to_owned()),
            WindowNode::Split {
                direction,
                ratio,
                first,
                second,
            } => Ok(LayoutNode::Split {
                axis: match direction {
                    SplitDirection::Horizontal => SplitAxis::Horizontal,
                    SplitDirection::Vertical => SplitAxis::Vertical,
                },
                ratio: *ratio,
                first: Box::new(convert(first, view_ids)?),
                second: Box::new(convert(second, view_ids)?),
            }),
        }
    }

    Ok(LogicalLayout {
        columns: editor.frame.available_columns,
        rows: editor.frame.available_lines,
        active: *view_ids
            .get(&editor.active_window)
            .ok_or_else(|| "active window has no transport view identity".to_owned())?,
        root: convert(tree, view_ids)?,
    })
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

fn presentation_color_hex(value: &str) -> Option<PresentationColor> {
    let hex = value.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    Some(PresentationColor::Rgb {
        r: u8::from_str_radix(&hex[0..2], 16).ok()?,
        g: u8::from_str_radix(&hex[2..4], 16).ok()?,
        b: u8::from_str_radix(&hex[4..6], 16).ok()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::{Frame, Window, WindowNode};
    use crate::kill_ring::KillRing;
    use crate::native_services::SystemClock;
    use crate::{Buffer, BufferId};
    use slotmap::SlotMap;
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
        let buffer = Buffer::new();
        buffer.set_object("*test*".to_string());
        buffer.load_str("hello");
        let buffer_id = buffers.insert(buffer);
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
            windows,
            active_window: window_id,
            previous_active_window: None,
            window_tree: WindowNode::new_leaf(window_id),
            kill_ring: KillRing::without_clipboard(60),
            buffer_history: vec![buffer_id],
            echo_message: String::new(),
            echo_message_time: None,
            clock: Arc::new(SystemClock),
            mouse_drag_state: None,
            messages_buffer_id: None,
            file_watcher: crate::file_watcher::FileWatcher::new(),
        }
    }

    fn test_session_with_grants(grants: CapabilityGrants) -> HostSession {
        HostSession::open(test_editor(), grants).unwrap()
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
                "hello1700000000123\n",
                "{output:#?}"
            );
            assert_eq!(snapshot(&output).views[0].cursor, 19);
            assert!(
                session
                    .mica_modes
                    .values()
                    .any(|mode| mode == "fundamental")
            );
            assert_eq!(
                session
                    .mica_configuration
                    .get("tab_width")
                    .map(String::as_str),
                Some("4")
            );
            let indented = session
                .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Tab])))
                .await
                .unwrap();
            assert_eq!(
                snapshot(&indented).views[0].visible_text,
                "hello1700000000123\n    "
            );
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
    fn mica_syntax_policy_controls_native_word_editing() {
        let _guard = MICA_TEST_LOCK.lock().unwrap();
        compio::runtime::Runtime::new().unwrap().block_on(async {
            let mut editor = test_editor();
            let window = editor.active_window;
            let buffer = editor.windows[window].active_buffer;
            editor.buffers[buffer].load_str("foo-bar baz");
            editor.windows[window].cursor = 0;
            let mut session = HostSession::open_with_mica_clock(
                editor,
                CapabilityGrants::editor_default(),
                Arc::new(FixedNativeClock(42)),
            )
            .unwrap();
            let meta =
                LogicalKey::Modifier(crate::keys::KeyModifier::Meta(crate::keys::Side::Left));

            let output = session
                .dispatch(
                    session.envelope(InputEvent::Keys(vec![meta, LogicalKey::AlphaNumeric('d')])),
                )
                .await
                .unwrap();

            // Mica's [[:alnum:]_] word rule makes '-' punctuation. The native
            // mechanism kills through the punctuation to the next word rather
            // than using the legacy Rust mode's non-whitespace definition.
            assert_eq!(snapshot(&output).views[0].visible_text, "bar baz");
            assert!(
                session.mica_syntax[&buffer]
                    .iter()
                    .any(|rule| rule.kind == "word"
                        && rule.pattern == "[[:alnum:]_]"
                        && rule.precedence == 100)
            );

            let original = include_str!("../../mica/roe-first-wave.mica");
            let hyphen_is_word = original.replace(
                "assert roe/SyntaxRule(#roe/fundamental_mode, :word, \"[[:alnum:]_]\", 100)",
                "assert roe/SyntaxRule(#roe/fundamental_mode, :word, \"[[:alpha:]-]\", 100)",
            );
            session
                .replace_mica_first_wave(hyphen_is_word)
                .await
                .unwrap();
            session.editor.buffers[buffer].load_str("foo-bar baz");
            session.editor.windows[window].cursor = 0;
            let replaced = session
                .dispatch(
                    session.envelope(InputEvent::Keys(vec![meta, LogicalKey::AlphaNumeric('d')])),
                )
                .await
                .unwrap();
            assert_eq!(snapshot(&replaced).views[0].visible_text, "baz");

            let unsupported = original.replace(
                "assert roe/SyntaxRule(#roe/fundamental_mode, :word, \"[[:alnum:]_]\", 100)",
                "assert roe/SyntaxRule(#roe/fundamental_mode, :word, \"word\", 100)",
            );
            session.replace_mica_first_wave(unsupported).await.unwrap();
            session.editor.buffers[buffer].load_str("unchanged");
            session.editor.windows[window].cursor = 0;
            let rejected = session
                .dispatch(
                    session.envelope(InputEvent::Keys(vec![meta, LogicalKey::AlphaNumeric('d')])),
                )
                .await
                .unwrap();
            assert_eq!(snapshot(&rejected).views[0].visible_text, "unchanged");
            assert!(rejected.lifecycle.iter().any(|event| matches!(
                event,
                LifecycleEvent::Error(message)
                    if message.contains("unsupported Mica word syntax pattern")
            )));

            session
                .dispatch(session.envelope(InputEvent::Close))
                .await
                .unwrap();
        });
    }

    #[test]
    fn mica_orders_bindings_and_edit_hooks_by_precedence() {
        let _guard = MICA_TEST_LOCK.lock().unwrap();
        compio::runtime::Runtime::new().unwrap().block_on(async {
            let mut session = HostSession::open_with_mica_clock(
                test_editor(),
                CapabilityGrants::editor_default(),
                Arc::new(FixedNativeClock(42)),
            )
            .unwrap();
            let original = include_str!("../../mica/roe-first-wave.mica");
            let ordered = format!(
                "{original}\nassert roe/NativeBinding(\"x\", :cursor_right, 6000)\nassert roe/KeyBinding(#roe/global_map, \"x\", #roe/redraw, 7000)\nassert roe/ModeHook(#roe/fundamental_mode, :low_hook, 10)\nassert roe/ModeHook(#roe/fundamental_mode, :high_hook, 50)\n"
            );
            session.replace_mica_first_wave(ordered).await.unwrap();

            let command_wins = session
                .dispatch(session.envelope(InputEvent::Text("x".to_owned())))
                .await
                .unwrap();
            assert_eq!(snapshot(&command_wins).views[0].visible_text, "hello");

            let buffer = session.editor.windows[session.editor.active_window].active_buffer;
            session.editor.buffers[buffer].load_str("hello");
            session.editor.windows[session.editor.active_window].cursor = 5;
            let with_hooks = session
                .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Backspace])))
                .await
                .unwrap();
            let hook_errors: Vec<_> = with_hooks
                .lifecycle
                .iter()
                .filter_map(|event| match event {
                    LifecycleEvent::Error(message) if message.contains("_hook") => Some(message),
                    _ => None,
                })
                .collect();
            assert_eq!(hook_errors.len(), 2);
            assert!(hook_errors[0].contains("high_hook"));
            assert!(hook_errors[1].contains("low_hook"));

            let ambiguous = format!(
                "{original}\nassert roe/KeyBinding(#roe/global_map, \"x\", #roe/redraw, 7000)\nassert roe/KeyBinding(#roe/global_map, \"x\", #roe/quit, 7000)\n"
            );
            session.replace_mica_first_wave(ambiguous).await.unwrap();
            let rejected = session
                .dispatch(session.envelope(InputEvent::Text("x".to_owned())))
                .await
                .unwrap();
            assert!(rejected.lifecycle.iter().any(|event| matches!(
                event,
                LifecycleEvent::Error(message) if message.contains("ambiguous command key binding")
            )));
            assert!(!rejected.lifecycle.contains(&LifecycleEvent::QuitRequested));

            session
                .dispatch(session.envelope(InputEvent::Close))
                .await
                .unwrap();
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
                .create_buffer("*dynamic*".to_owned(), "new".to_owned());
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
                (3, 2)
            );

            session.editor.active_window = original_view;
            session.editor.windows.remove(view);
            session.editor.window_tree = WindowNode::new_leaf(original_view);
            session.editor.buffers.remove(buffer);
            let _ = session.synchronize_identities();
            let after_removal = session
                .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Function(12)])))
                .await
                .unwrap();
            assert_eq!(
                session.mica.as_ref().unwrap().identity_counts_for_test(),
                (2, 1)
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
    fn mica_host_effects_cannot_bypass_native_capabilities() {
        let _guard = MICA_TEST_LOCK.lock().unwrap();
        compio::runtime::Runtime::new().unwrap().block_on(async {
            let mut session = HostSession::open_with_mica(
                test_editor(),
                CapabilityGrants::new([Capability::TextRead]),
            )
            .unwrap();

            let insertion = session
                .dispatch(session.envelope(InputEvent::Text("x".to_owned())))
                .await
                .unwrap();
            let active = session.editor.windows[session.editor.active_window].active_buffer;
            assert_eq!(session.editor.buffers[active].content(), "hello");
            assert!(insertion.lifecycle.iter().any(|event| matches!(
                event,
                LifecycleEvent::Error(message) if message.contains("text_write") || message.contains("TextWrite")
            )));

            let control =
                LogicalKey::Modifier(crate::keys::KeyModifier::Control(crate::keys::Side::Left));
            let split = session
                .dispatch(session.envelope(InputEvent::Keys(vec![
                    control.clone(),
                    LogicalKey::AlphaNumeric('x'),
                    LogicalKey::AlphaNumeric('2'),
                ])))
                .await
                .unwrap();
            assert_eq!(session.editor.windows.len(), 1);
            assert!(split.lifecycle.iter().any(|event| matches!(
                event,
                LifecycleEvent::Error(message) if message.contains("Layout")
            )));

            let save = session
                .dispatch(session.envelope(InputEvent::Keys(vec![
                    control,
                    LogicalKey::AlphaNumeric('x'),
                    LogicalKey::Modifier(crate::keys::KeyModifier::Control(
                        crate::keys::Side::Left,
                    )),
                    LogicalKey::AlphaNumeric('s'),
                ])))
                .await
                .unwrap();
            assert!(save.lifecycle.iter().any(|event| matches!(
                event,
                LifecycleEvent::Error(message) if message.contains("FileWrite")
            )));

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

            let original = session
                .editor
                .windows
                .keys()
                .find(|window| *window != session.editor.active_window)
                .unwrap();
            let other = session.editor.active_window;
            let other_buffer = session
                .editor
                .create_buffer("*argument-target*".to_owned(), "target".to_owned());
            session.editor.windows[other].active_buffer = other_buffer;
            session.editor.active_window = original;
            let _ = session.synchronize_identities();

            session
                .dispatch(
                    session.envelope(InputEvent::Keys(vec![meta, LogicalKey::AlphaNumeric('x')])),
                )
                .await
                .unwrap();
            session
                .dispatch(session.envelope(InputEvent::Text("select-window".to_owned())))
                .await
                .unwrap();
            let argument_prompt = session
                .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Enter])))
                .await
                .unwrap();
            let argument_view = snapshot(&argument_prompt)
                .views
                .iter()
                .find(|view| view.command_view)
                .unwrap_or_else(|| panic!("argument prompt missing: {argument_prompt:#?}"));
            assert!(
                argument_view.visible_text.contains("*argument-target*"),
                "unexpected argument prompt: {argument_view:#?}"
            );
            session
                .dispatch(session.envelope(InputEvent::Text("argument-target".to_owned())))
                .await
                .unwrap();
            let selected_argument = session
                .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Enter])))
                .await
                .unwrap();
            assert_eq!(
                snapshot(&selected_argument).active_view,
                session.view_ids[&other]
            );

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
            assert!(
                snapshot(&searched)
                    .views
                    .iter()
                    .any(|view| { !view.command_view && view.styled_ranges.len() == 2 })
            );
            assert!(snapshot(&searched).styles.iter().any(|style| {
                style.name == "isearch-current"
                    && style.background
                        == Some(PresentationColor::Rgb {
                            r: 255,
                            g: 255,
                            b: 0,
                        })
            }));
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
    fn mica_owns_pointer_selection_and_view_scroll_policy() {
        let _guard = MICA_TEST_LOCK.lock().unwrap();
        compio::runtime::Runtime::new().unwrap().block_on(async {
            let mut session =
                HostSession::open_with_mica(test_editor(), CapabilityGrants::editor_default())
                    .unwrap();
            let initial = session.initial_output().await;
            let view = snapshot(&initial).active_view;

            for (index, event) in [
                PointerEvent {
                    column: 2,
                    row: 1,
                    kind: PointerKind::Down,
                    button: PointerButton::Primary,
                },
                PointerEvent {
                    column: 4,
                    row: 1,
                    kind: PointerKind::Move,
                    button: PointerButton::None,
                },
                PointerEvent {
                    column: 4,
                    row: 1,
                    kind: PointerKind::Up,
                    button: PointerButton::Primary,
                },
            ]
            .into_iter()
            .enumerate()
            {
                let output = session
                    .dispatch(session.envelope(InputEvent::Pointer(event)))
                    .await
                    .unwrap();
                assert!(
                    output
                        .lifecycle
                        .iter()
                        .all(|event| !matches!(event, LifecycleEvent::Error(_)))
                );
                if index == 0 {
                    assert_eq!(session.pointer_selection.map(|(_, anchor)| anchor), Some(1));
                }
                if index == 1 {
                    let window = session.editor.active_window;
                    let buffer = session.editor.windows[window].active_buffer;
                    assert_eq!(session.editor.buffers[buffer].get_mark(), Some(1));
                }
            }

            let window = session.editor.active_window;
            let buffer = session.editor.windows[window].active_buffer;
            assert_eq!(session.editor.buffers[buffer].get_mark(), Some(1));
            assert_eq!(session.editor.windows[window].cursor, 3);

            let scrolled = session
                .dispatch(session.envelope(InputEvent::SetViewScroll {
                    view,
                    start_line: Some(0),
                    start_column: Some(2),
                }))
                .await
                .unwrap();
            assert_eq!(snapshot(&scrolled).views[0].scroll.start_column, 2);
            assert!(
                scrolled
                    .lifecycle
                    .iter()
                    .all(|event| !matches!(event, LifecycleEvent::Error(_)))
            );

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
            assert!(session
                .export_mica_unit("roe/first-wave")
                .await
                .unwrap()
                .contains("v2:"));

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

            session
                .set_mica_package_enabled("roe/core_package", false)
                .unwrap();
            let disabled = session
                .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Function(12)])))
                .await
                .unwrap();
            assert_eq!(
                snapshot(&disabled).views[0].visible_text,
                "hellov2:42\nv2:42\n"
            );
            assert_eq!(snapshot(&disabled).echo_area, "F12 is undefined");
            session
                .set_mica_package_enabled("roe/core_package", true)
                .unwrap();

            let without_yellow = replacement.replace(
                "assert roe/FaceAttribute(#roe/isearch_current_face, :background, \"#ffff00\")\n",
                "",
            );
            session
                .replace_mica_first_wave(without_yellow)
                .await
                .unwrap();
            session
                .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Function(1)])))
                .await
                .unwrap();
            assert!(!session.mica_faces["isearch-current"].contains_key("background"));

            let start = original.find("verb roe/insert_current_time").unwrap();
            let end = start + original[start..].find("\nend\n").unwrap() + "\nend\n".len();
            let failing = format!(
                "{}verb roe/insert_current_time(actor, session)\n  raise E_TEST, \"intentional command failure\"\nend\n{}",
                &original[..start],
                &original[end..]
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

            session.restore_mica_first_wave().await.unwrap();
            let recovered = session
                .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Function(12)])))
                .await
                .unwrap();
            assert_eq!(
                snapshot(&recovered).views[0].visible_text,
                "hellov2:42\nv2:42\n42\n"
            );
            session
                .dispatch(session.envelope(InputEvent::Close))
                .await
                .unwrap();
        });
    }

    #[test]
    fn native_recovery_surface_operates_before_user_policy_load() {
        let _guard = MICA_TEST_LOCK.lock().unwrap();
        compio::runtime::Runtime::new().unwrap().block_on(async {
            let mut session = HostSession::open_with_mica_clock(
                test_editor(),
                CapabilityGrants::editor_default(),
                Arc::new(FixedNativeClock(42)),
            )
            .unwrap();

            let rejected = session
                .dispatch(
                    session.envelope(InputEvent::Recovery(RecoveryOperation::CheckSource {
                        source: "verb this is malformed".to_owned(),
                    })),
                )
                .await
                .unwrap();
            assert!(rejected.lifecycle.iter().any(|event| matches!(
                event,
                LifecycleEvent::RecoveryResult { result: Err(_), .. }
            )));
            assert!(!session.closed);

            let installed = session
                .dispatch(
                    session.envelope(InputEvent::Recovery(RecoveryOperation::ReplaceUnit {
                        unit: "roe/first-wave".to_owned(),
                        source: include_str!("../../mica/roe-first-wave.mica").to_owned(),
                    })),
                )
                .await
                .unwrap();
            assert!(installed.lifecycle.iter().any(|event| matches!(
                event,
                LifecycleEvent::RecoveryResult {
                    result: Ok(None),
                    ..
                }
            )));

            let command = session
                .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Function(12)])))
                .await
                .unwrap();
            assert_eq!(snapshot(&command).views[0].visible_text, "hello42\n");

            let exported = session
                .dispatch(
                    session.envelope(InputEvent::Recovery(RecoveryOperation::ExportUnit {
                        unit: "roe/first-wave".to_owned(),
                    })),
                )
                .await
                .unwrap();
            assert!(exported.lifecycle.iter().any(|event| matches!(
                event,
                LifecycleEvent::RecoveryResult { result: Ok(Some(source)), .. }
                    if source.contains("insert_current_time")
            )));

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
            let initial = session.initial_output().await;
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
            let initial = session.initial_output().await;
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
            let initial = session.initial_output().await;
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
