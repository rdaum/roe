// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-only

//! Owned wire messages and protocol bounds. No workspace state or native execution.

use super::PresentationText;
use crate::keys::LogicalKey;
use crate::native_kernel::{
    Capability, KernelError, NativeOperation, NativeResult, ResourceId, TextSelection, ViewId,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::PathBuf;

pub const SESSION_PROTOCOL_VERSION: u16 = 1;
pub const MAX_KEYS_PER_INPUT: usize = 64;
pub const MAX_TEXT_CHARS_PER_INPUT: usize = 65_536;
pub const MAX_PRESENTATION_CHARS: usize = 1_000_000;
pub const MAX_NATIVE_RESULT_BYTES: usize = 1_048_576;
pub const MAX_FRONTEND_REQUESTS: usize = 16;
pub const MAX_FRONTEND_TEXT_CHARS: usize = 65_536;
pub const MAX_SESSION_VIEWS: usize = 64;
pub const MAX_BUFFER_NAME_CHARS: usize = 256;
pub const MAX_MICA_SOURCE_CHARS: usize = 1_048_576;
pub const MAX_TYPEOUT_TEXT_CHARS: usize = 65_536;
pub const MAX_TYPEOUT_TITLE_CHARS: usize = 256;
pub const MAX_AGENT_BUFFER_CHARS: usize = 524_288;
pub const MAX_FRAME_COLUMNS: u16 = 1_000;
pub const MAX_FRAME_ROWS: u16 = 1_000;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionEpoch(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RequestId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Revision(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AttachmentId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TypeoutId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentViewport {
    pub columns: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum FrontendCapability {
    ClipboardRead,
    ClipboardWrite,
    Notifications,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentConfiguration {
    pub viewport: AttachmentViewport,
    pub frontend_capabilities: BTreeSet<FrontendCapability>,
}

impl AttachmentConfiguration {
    pub fn headless(columns: u16, rows: u16) -> Self {
        Self {
            viewport: AttachmentViewport { columns, rows },
            frontend_capabilities: BTreeSet::new(),
        }
    }

    pub fn local_frontend(columns: u16, rows: u16) -> Self {
        Self {
            viewport: AttachmentViewport { columns, rows },
            frontend_capabilities: [
                FrontendCapability::ClipboardRead,
                FrontendCapability::ClipboardWrite,
            ]
            .into_iter()
            .collect(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttachmentStatus {
    Attached,
    Detached,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttachmentControl {
    Attach {
        configuration: AttachmentConfiguration,
    },
    Resume {
        attachment: AttachmentId,
        epoch: SessionEpoch,
        after: Option<Revision>,
    },
    Detach,
    CloseAttachment,
    TerminateWorkspace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FrontendRequestId(pub u64);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FrontendServiceRequest {
    ReadClipboard {
        request_id: FrontendRequestId,
    },
    WriteClipboard {
        request_id: FrontendRequestId,
        contents: String,
    },
    Notify {
        request_id: FrontendRequestId,
        title: String,
        body: String,
    },
}

impl FrontendServiceRequest {
    pub fn request_id(&self) -> FrontendRequestId {
        match self {
            Self::ReadClipboard { request_id }
            | Self::WriteClipboard { request_id, .. }
            | Self::Notify { request_id, .. } => *request_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrontendServiceResult {
    pub request_id: FrontendRequestId,
    pub result: Result<FrontendServiceResponse, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FrontendServiceResponse {
    ClipboardContents(Option<String>),
    Completed,
}

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
        start_line: Option<usize>,
        start_column: Option<usize>,
    },
    Resize {
        columns: u16,
        rows: u16,
    },
    Focus(bool),
    PlatformWarning(String),
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
}

/// Small non-programmable surface retained when Mica user policy is broken.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecoveryOperation {
    CheckSource { source: String },
    ReplaceUnit { unit: String, source: String },
    ExportUnit { unit: String },
    RestoreFirstWave,
    SetPackageEnabled { package: String, enabled: bool },
    Inspect,
}

/// File-oriented bootstrap recovery commands shared by both shipped frontends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupRecoveryOperation {
    CheckFile(PathBuf),
    ReplaceUnit { unit: String, path: PathBuf },
    ExportUnit { unit: String, path: PathBuf },
    RestoreFirstWave,
    SetPackageEnabled { package: String, enabled: bool },
    Inspect,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PointerEvent {
    /// Renderer-provided character hit, checked against current native identity and text.
    #[serde(default)]
    pub text_hit: Option<PointerTextHit>,
    pub column: u16,
    pub row: u16,
    pub kind: PointerKind,
    pub button: PointerButton,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PointerTextHit {
    pub view: ViewId,
    pub resource: ResourceId,
    pub text_revision: u64,
    pub position: usize,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionOutput {
    pub protocol_version: u16,
    pub epoch: SessionEpoch,
    /// The accepted client input which caused this output. Server-originated
    /// background output carries `None` and therefore never consumes input
    /// sequence space.
    pub acknowledged_input: Option<u64>,
    pub presentation: Option<PresentationUpdate>,
    pub native_completions: Vec<NativeCompletion>,
    pub frontend_requests: Vec<FrontendServiceRequest>,
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
    AttachmentAttached {
        attachment: AttachmentId,
    },
    AttachmentDetached {
        attachment: AttachmentId,
    },
    AttachmentClosed {
        attachment: AttachmentId,
    },
    WorkspaceTerminated,
    QuitRequested,
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
    Typeout(ViewId),
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
    pub buffer_kind: String,
    pub visited_file: Option<PathBuf>,
    pub text_revision: u64,
    pub last_saved_revision: u64,
    pub modified: bool,
    pub read_only: bool,
    /// Renderer-neutral visible slice, bounded by the logical view height.
    pub visible_text: PresentationText,
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
    #[serde(default)]
    pub styled_lines: Vec<StyledLine>,
    #[serde(default)]
    pub typeout: Option<PresentedTypeout>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresentedTypeout {
    pub id: TypeoutId,
    pub kind: String,
    pub title: String,
    pub visible_text: String,
    pub first_visible_line: usize,
    pub total_lines: usize,
    pub more_before: bool,
    pub more_after: bool,
    pub complete: bool,
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
    /// Document line offset, independent of the viewport row range.
    pub start_line: usize,
    /// Document character-column offset, not a terminal cell coordinate.
    pub start_column: usize,
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
pub struct StyledLine {
    /// Zero-based absolute logical line within the presented buffer.
    pub line: usize,
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
    #[error("the frontend attachment is not active")]
    AttachmentUnavailable,
    #[error("the workspace has terminated")]
    WorkspaceTerminated,
    #[error("editor input failed: {0}")]
    Editor(#[from] std::io::Error),
    #[error("native kernel failed: {0}")]
    Kernel(#[from] KernelError),
}

/// Transport-independent frontend contract. A remote client implements this
/// directly over its transport; the embedded client calls the workspace in
/// process. Lifecycle methods are asynchronous because a process transport
/// must receive an authoritative server response.
#[allow(async_fn_in_trait)]
pub trait SessionClient {
    fn attachment_id(&self) -> AttachmentId;
    fn epoch(&self) -> SessionEpoch;
    fn next_sequence(&self) -> u64;
    fn envelope(&self, event: InputEvent) -> InputEnvelope;
    async fn initial_output(&mut self) -> SessionOutput;
    async fn dispatch(&mut self, envelope: InputEnvelope) -> Result<SessionOutput, SessionError>;
    async fn poll_output(&mut self) -> Result<Option<SessionOutput>, SessionError>;
    async fn complete_frontend_request(
        &mut self,
        completion: FrontendServiceResult,
    ) -> Result<SessionOutput, SessionError>;
    async fn detach(&mut self) -> Result<SessionOutput, SessionError>;
    async fn resume(
        &mut self,
        configuration: AttachmentConfiguration,
    ) -> Result<SessionOutput, SessionError>;
    async fn close_attachment(&mut self) -> Result<SessionOutput, SessionError>;
    async fn terminate_workspace(&mut self) -> Result<SessionOutput, SessionError>;

    async fn replay(
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
}

pub(super) fn native_operation_text_size(operation: &NativeOperation) -> usize {
    match operation {
        NativeOperation::CreateText { name, initial } => {
            name.chars().count().saturating_add(initial.chars().count())
        }
        NativeOperation::Insert { text, .. } | NativeOperation::Replace { text, .. } => {
            text.chars().count()
        }
        NativeOperation::WriteFile { contents, .. } => contents.chars().count(),
        NativeOperation::SpawnProcess { program, args } => {
            args.iter().fold(program.chars().count(), |size, arg| {
                size.saturating_add(arg.chars().count())
            })
        }
        _ => 0,
    }
}

pub(super) fn validate_event_size(event: &InputEvent) -> Result<(), SessionError> {
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
