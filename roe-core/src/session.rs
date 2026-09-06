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

use crate::editor::{ChromeAction, DragType, MouseDragState};
use crate::keys::{KeyAction, LogicalKey};
mod attachment;
mod effects;
mod indentation;
mod layout;
mod native_actions;
pub use attachment::Attachment;
use attachment::PendingFrontendRequest;
use layout::{detect_border, logical_layout, ratio_at_path, set_ratio_at_path, update_layout_drag};
mod protocol;
mod recovery;
use protocol::validate_event_size;
pub use protocol::*;
mod policy;
mod presentation;
mod presentation_cache;
use policy::PolicyProjection;
use presentation::PresentationProjector;
pub use presentation_cache::PresentationText;

use crate::mica_host::{
    MicaEventBatch, MicaHost, MicaHostError, MicaKeyResult, MicaPromptUpdate,
    normalized_key_sequence,
};
use crate::native_kernel::{
    Capability, CapabilityGrants, KernelError, NativeClock, NativeKernel, NativeOperation,
    NativeResult, ResourceId, ViewId,
};
use crate::renderer::DirtyRegion;
use crate::{BufferId, Editor, WindowId};
use std::collections::{HashMap, HashSet};
#[cfg(test)]
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

const MICA_PROMPT_HEIGHT: u16 = 10;
const MICA_PROMPT_CANDIDATE_ROWS: usize = MICA_PROMPT_HEIGHT as usize - 3;
const MICA_PROMPT_CONTEXT_BELOW: usize = 2;
const MICA_PROMPT_SELECTION_FACE: &str = "completion-selection";

struct MicaPointerInput<'a> {
    view: WindowId,
    position: usize,
    phase: &'a str,
    button: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MicaSyntaxRule {
    kind: String,
    pattern: String,
    precedence: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MicaHighlightRule {
    capture: String,
    face: String,
    precedence: i64,
}

/// Long-lived editor state. A workspace owns buffers, Mica, native resources,
/// watchers, and processes; it does not own a frontend connection.
pub struct WorkspaceHost {
    editor: Editor,
    kernel: Arc<Mutex<NativeKernel>>,
    buffer_resources: HashMap<BufferId, ResourceId>,
    view_ids: HashMap<WindowId, ViewId>,
    next_view_id: u64,
    mica: Option<MicaHost>,
    policy: PolicyProjection,
    presentation: PresentationProjector,
    syntax: crate::syntax::SyntaxService,
    mica_effect_depth: usize,
    mica_effect_remaining: usize,
    mica_search_ranges: HashMap<WindowId, Vec<(usize, usize, String)>>,
    mica_styled_lines: HashMap<WindowId, Vec<(usize, String)>>,
    typeout: Option<TypeoutState>,
    next_typeout_id: u64,
    terminated: bool,
}

#[derive(Debug, Clone)]
struct TypeoutState {
    id: TypeoutId,
    view: WindowId,
    origin_buffer: BufferId,
    kind: String,
    title: String,
    text: String,
}

/// Embedded frontend connection to a [`WorkspaceHost`]. This is the direct
/// implementation of the same attachment semantics a process transport uses.
pub struct DirectSessionClient {
    workspace: WorkspaceHost,
    attachment: Attachment,
}

impl WorkspaceHost {
    fn activate_attachment(&mut self, attachment: &Attachment) {
        self.editor
            .handle_resize(attachment.viewport.columns, attachment.viewport.rows);
    }

    pub fn set_mica_wake_handler(
        &mut self,
        handler: Arc<dyn crate::native_services::FrontendWake>,
    ) {
        if let Some(mica) = self.mica.as_mut() {
            mica.set_wake_handler(handler);
        }
    }

    pub fn open(editor: Editor, grants: CapabilityGrants) -> Result<Self, KernelError> {
        Self::open_with_kernel(editor, Arc::new(Mutex::new(NativeKernel::new(grants))))
    }

    fn open_with_kernel(
        editor: Editor,
        kernel: Arc<Mutex<NativeKernel>>,
    ) -> Result<Self, KernelError> {
        let mut workspace = Self {
            editor,
            kernel,
            buffer_resources: HashMap::new(),
            view_ids: HashMap::new(),
            next_view_id: 1,
            mica: None,
            policy: PolicyProjection::default(),
            presentation: PresentationProjector::default(),
            syntax: crate::syntax::SyntaxService::default(),
            mica_effect_depth: 0,
            mica_effect_remaining: 0,
            mica_search_ranges: HashMap::new(),
            mica_styled_lines: HashMap::new(),
            typeout: None,
            next_typeout_id: 1,
            terminated: false,
        };
        for (buffer, value) in &workspace.editor.buffers {
            let resource = workspace
                .kernel
                .lock()
                .unwrap()
                .register_buffer(value.clone())?;
            workspace.buffer_resources.insert(buffer, resource);
        }
        for window in workspace.editor.windows.keys() {
            workspace
                .view_ids
                .insert(window, ViewId(workspace.next_view_id));
            workspace.next_view_id += 1;
        }
        Ok(workspace)
    }

    /// Open the public-driver Mica endpoint used by the first integration
    /// wave. The ordinary constructor remains available for headless and
    /// protocol and native-mechanism tests that do not need policy dispatch.
    pub fn open_with_mica(
        mut editor: Editor,
        grants: CapabilityGrants,
    ) -> Result<Self, MicaHostError> {
        editor.ensure_scratch_buffer();
        let mut workspace = Self::open(editor, grants)?;
        workspace.mica = Some(MicaHost::open(
            &workspace.editor,
            Arc::clone(&workspace.kernel),
            &workspace.buffer_resources,
        )?);
        Ok(workspace)
    }

    pub fn open_with_mica_clock(
        mut editor: Editor,
        grants: CapabilityGrants,
        clock: Arc<dyn NativeClock>,
    ) -> Result<Self, MicaHostError> {
        editor.ensure_scratch_buffer();
        let mut workspace = Self::open_with_kernel(
            editor,
            Arc::new(Mutex::new(NativeKernel::with_clock(grants, clock))),
        )?;
        workspace.mica = Some(MicaHost::open(
            &workspace.editor,
            Arc::clone(&workspace.kernel),
            &workspace.buffer_resources,
        )?);
        Ok(workspace)
    }

    #[cfg(test)]
    fn open_with_mica_stream_handler(
        mut editor: Editor,
        grants: CapabilityGrants,
        stream_handler: mica_driver::ExternalStreamRequestHandler,
    ) -> Result<Self, MicaHostError> {
        editor.ensure_scratch_buffer();
        let mut workspace = Self::open(editor, grants)?;
        workspace.mica = Some(MicaHost::open_with_stream_handler_for_test(
            &workspace.editor,
            Arc::clone(&workspace.kernel),
            &workspace.buffer_resources,
            stream_handler,
        )?);
        Ok(workspace)
    }

    pub fn attach(&mut self, configuration: AttachmentConfiguration) -> Attachment {
        self.editor
            .handle_resize(configuration.viewport.columns, configuration.viewport.rows);
        Attachment::new(configuration)
    }

    pub async fn initial_output(&mut self, attachment: &mut Attachment) -> SessionOutput {
        self.activate_attachment(attachment);
        let mut lifecycle = vec![
            LifecycleEvent::AttachmentAttached {
                attachment: attachment.id,
            },
            LifecycleEvent::Ready {
                protocol_version: SESSION_PROTOCOL_VERSION,
                capabilities: capability_list(self.kernel.lock().unwrap().grants()),
            },
        ];
        let startup = std::mem::take(&mut self.editor.startup_buffers);
        if !startup.is_empty()
            && let Some(mut mica) = self.mica.take()
        {
            let events = mica
                .initialize_startup(&self.editor, &self.buffer_resources, &startup)
                .await;
            self.mica = Some(mica);
            match events {
                Ok(events) => {
                    self.apply_mica_events(attachment, events, &mut lifecycle, &mut Vec::new())
                        .await
                }
                Err(error) => lifecycle.push(LifecycleEvent::Error(format!(
                    "Mica startup failed: {error}"
                ))),
            }
            let (resources, warnings) = self.synchronize_identities();
            lifecycle.extend(warnings.into_iter().map(LifecycleEvent::Warning));
            lifecycle.extend(
                resources
                    .into_iter()
                    .map(|resource| LifecycleEvent::ResourceInvalidated { resource }),
            );
        }
        if let Some(mut mica) = self.mica.take() {
            let policy = mica
                .publish_policy(&self.editor, &self.buffer_resources)
                .await;
            self.mica = Some(mica);
            match policy {
                Ok(events) => {
                    let mut initial_invalidations = Vec::new();
                    self.apply_mica_events(
                        attachment,
                        events,
                        &mut lifecycle,
                        &mut initial_invalidations,
                    )
                    .await;
                }
                Err(error) => lifecycle.push(LifecycleEvent::Error(format!(
                    "failed to publish initial Mica policy: {error}"
                ))),
            }
        }
        attachment.revision.0 += 1;
        SessionOutput {
            protocol_version: SESSION_PROTOCOL_VERSION,
            epoch: attachment.epoch,
            acknowledged_input: None,
            presentation: Some(PresentationUpdate::Full(self.capture_snapshot(attachment))),
            native_completions: Vec::new(),
            frontend_requests: attachment.frontend_requests.drain(..).collect(),
            lifecycle,
        }
    }

    pub async fn dispatch(
        &mut self,
        attachment: &mut Attachment,
        envelope: InputEnvelope,
    ) -> Result<SessionOutput, SessionError> {
        if self.terminated {
            return Err(SessionError::WorkspaceTerminated);
        }
        attachment.validate_envelope(&envelope)?;
        self.activate_attachment(attachment);
        if let Err(SessionError::InputTooLarge(detail)) = validate_event_size(&envelope.event) {
            attachment.next_sequence += 1;
            return Ok(SessionOutput {
                protocol_version: SESSION_PROTOCOL_VERSION,
                epoch: attachment.epoch,
                acknowledged_input: Some(envelope.sequence),
                presentation: None,
                native_completions: Vec::new(),
                frontend_requests: Vec::new(),
                lifecycle: vec![LifecycleEvent::Overloaded { detail }],
            });
        }
        attachment.next_sequence += 1;

        let mut lifecycle = Vec::new();
        let mut completions = Vec::new();
        let mut invalidations = Vec::new();
        let force_full = matches!(envelope.event, InputEvent::RequestSnapshot { .. });
        if let Some(mut mica) = self.mica.take() {
            let events = mica.drain_background_events();
            self.mica = Some(mica);
            self.apply_mica_events(attachment, events, &mut lifecycle, &mut invalidations)
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
                                attachment,
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
                                    self.resolve_actions(actions, &mut invalidations);
                                }
                                Err(error) => self.fail_workspace(error, &mut lifecycle),
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
                                    attachment,
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
                            self.resolve_actions(actions, &mut invalidations);
                        }
                        Err(error) => {
                            self.fail_workspace(error, &mut lifecycle);
                            break;
                        }
                    }
                }
            }
            InputEvent::Pointer(pointer) => {
                self.apply_pointer(attachment, pointer, &mut lifecycle, &mut invalidations)
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
                    let (max_line, max_column) = self.presentation.scroll_limits(buffer_id, buffer);
                    let scroll = attachment
                        .view_scroll
                        .entry(window_id)
                        .or_insert(ViewScroll {
                            start_line: 0,
                            start_column: 0,
                        });
                    let line = start_line.unwrap_or(scroll.start_line).min(max_line);
                    let column = start_column.unwrap_or(scroll.start_column).min(max_column);
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
                                self.apply_mica_events(
                                    attachment,
                                    events,
                                    &mut lifecycle,
                                    &mut invalidations,
                                )
                                .await;
                            }
                            Err(error) => lifecycle.push(LifecycleEvent::Error(error.to_string())),
                        }
                    } else {
                        *attachment.view_scroll.get_mut(&window_id).unwrap() = ViewScroll {
                            start_line: line,
                            start_column: column,
                        };
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
                attachment.viewport = AttachmentViewport { columns, rows };
                self.editor.handle_resize(columns, rows);
                invalidations.push(Invalidation::Full);
            }
            InputEvent::PlatformWarning(warning) => {
                lifecycle.push(LifecycleEvent::Warning(warning));
            }
            InputEvent::NativeRequest {
                request_id,
                operation,
            } => {
                let mut result = if self.mica.is_some() {
                    Err("direct native requests are disabled in a Mica-owned session".to_owned())
                } else if matches!(
                    operation,
                    NativeOperation::CloseResource { resource }
                        if self.buffer_resources.values().any(|current| *current == resource)
                ) {
                    Err("cannot close a text resource while a logical buffer owns it".to_string())
                } else {
                    let result =
                        crate::native_io::execute(&self.kernel, operation, std::future::pending())
                            .await;
                    if let Err(KernelError::IoLimit(detail)) = &result {
                        lifecycle.push(LifecycleEvent::Overloaded {
                            detail: detail.clone(),
                        });
                    }
                    result.map_err(|error| error.to_string())
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
                lifecycle.push(recovery::dispatch(self.mica.as_mut(), operation).await);
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
            InputEvent::Focus(focused) => {
                attachment.focused = focused;
            }
        }

        if !self.terminated {
            let (resources, cleanup_warnings) = self.synchronize_identities();
            for warning in cleanup_warnings {
                lifecycle.push(LifecycleEvent::Warning(warning));
            }
            for resource in resources {
                lifecycle.push(LifecycleEvent::ResourceInvalidated { resource });
            }
        }
        let presentation = if self.terminated
            || attachment.status != AttachmentStatus::Attached
            || (!force_full && invalidations.is_empty())
        {
            None
        } else {
            attachment.revision.0 += 1;
            let snapshot = self.capture_snapshot(attachment);
            if force_full {
                Some(PresentationUpdate::Full(snapshot))
            } else {
                Some(PresentationUpdate::Delta(PresentationDelta {
                    epoch: attachment.epoch,
                    base_revision: Revision(attachment.revision.0 - 1),
                    revision: attachment.revision,
                    invalidations,
                    snapshot,
                }))
            }
        };

        Ok(SessionOutput {
            protocol_version: SESSION_PROTOCOL_VERSION,
            epoch: attachment.epoch,
            acknowledged_input: Some(envelope.sequence),
            presentation,
            native_completions: completions,
            frontend_requests: attachment.frontend_requests.drain(..).collect(),
            lifecycle,
        })
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

    pub async fn export_mica_unit(&mut self, unit: &str) -> Result<String, MicaHostError> {
        self.mica
            .as_mut()
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

    pub async fn execute_startup_recovery(
        &mut self,
        operations: &[StartupRecoveryOperation],
    ) -> Result<Vec<String>, String> {
        recovery::startup(self.mica.as_mut(), operations).await
    }

    pub fn set_recovery_message(&mut self, message: String) {
        self.editor.echo_message = message;
        self.editor.echo_message_time = Some(self.editor.clock.now());
    }

    fn set_prompt_selected_line(&mut self, window: WindowId, selected_line: Option<usize>) {
        let lines = selected_line
            .map(|line| vec![(line, MICA_PROMPT_SELECTION_FACE.to_owned())])
            .unwrap_or_default();
        self.mica_styled_lines.insert(window, lines);
    }

    fn realize_layout_change(
        &mut self,
        change: layout::LayoutChange,
        lifecycle: &mut Vec<LifecycleEvent>,
    ) -> bool {
        let authorization = self.kernel.lock().unwrap().authorize(Capability::Layout);
        let result = authorization
            .map_err(|error| error.to_string())
            .and_then(|()| {
                layout::realize(
                    &mut self.editor,
                    &mut self.view_ids,
                    &mut self.next_view_id,
                    change,
                )
            });
        match result {
            Ok(changed) => changed,
            Err(error) => {
                lifecycle.push(LifecycleEvent::Error(format!(
                    "Mica layout decision failed native validation: {error}"
                )));
                false
            }
        }
    }

    fn write_kill_ring_to_frontend(
        &mut self,
        attachment: &mut Attachment,
        action_name: &str,
        lifecycle: &mut Vec<LifecycleEvent>,
    ) {
        let Some(text) = self.editor.kill_ring.current().map(str::to_owned) else {
            return;
        };
        if text.chars().count() > MAX_FRONTEND_TEXT_CHARS {
            lifecycle.push(LifecycleEvent::Overloaded {
                detail: format!(
                    "clipboard write after {action_name} exceeds {MAX_FRONTEND_TEXT_CHARS} characters"
                ),
            });
            return;
        }
        if !attachment
            .frontend_capabilities
            .contains(&FrontendCapability::ClipboardWrite)
        {
            return;
        }
        if let Err(detail) = attachment.enqueue_frontend_request(
            PendingFrontendRequest::WriteClipboard,
            |request_id| FrontendServiceRequest::WriteClipboard {
                request_id,
                contents: text,
            },
        ) {
            lifecycle.push(LifecycleEvent::Overloaded {
                detail: format!("clipboard write after {action_name}: {detail}"),
            });
        }
    }

    fn resolve_actions(
        &mut self,
        actions: Vec<ChromeAction>,
        invalidations: &mut Vec<Invalidation>,
    ) {
        for action in actions {
            match action {
                ChromeAction::Echo(message) => {
                    self.editor.set_echo_message(message);
                    invalidations.push(Invalidation::EchoArea);
                }
                ChromeAction::MarkDirty(region) => {
                    self.push_dirty_invalidation(region, invalidations);
                }
                ChromeAction::BufferChanged { .. } => {}
            }
        }
    }

    fn push_dirty_invalidation(&self, region: DirtyRegion, invalidations: &mut Vec<Invalidation>) {
        let mut push = |invalidation| {
            if !invalidations.contains(&invalidation) {
                invalidations.push(invalidation);
            }
        };
        match region {
            DirtyRegion::FullScreen => push(Invalidation::Full),
            DirtyRegion::WindowChrome { window_id } | DirtyRegion::Modeline { window_id, .. } => {
                if let Some(view) = self.view_ids.get(&window_id).copied() {
                    push(Invalidation::View(view));
                } else {
                    push(Invalidation::Full);
                }
            }
            DirtyRegion::Line { buffer_id, .. }
            | DirtyRegion::LineRange { buffer_id, .. }
            | DirtyRegion::CharRange { buffer_id, .. }
            | DirtyRegion::Buffer { buffer_id } => {
                let mut found = false;
                for (window_id, window) in &self.editor.windows {
                    if window.active_buffer == buffer_id
                        && let Some(view) = self.view_ids.get(&window_id).copied()
                    {
                        push(Invalidation::View(view));
                        found = true;
                    }
                }
                if !found {
                    push(Invalidation::Full);
                }
            }
        }
    }

    async fn save_buffer_via_kernel(
        &mut self,
        buffer_id: BufferId,
        lifecycle: &mut Vec<LifecycleEvent>,
    ) -> Vec<ChromeAction> {
        let Some(buffer) = self.editor.buffers.get(buffer_id).cloned() else {
            return vec![ChromeAction::Echo("No active buffer".to_owned())];
        };
        let captured = buffer.with_read(|inner| {
            if inner.buffer.len_bytes() > crate::native_io::MAX_IO_BYTES {
                return Err("file write exceeds the byte limit".to_owned());
            }
            Ok((
                inner.visited_file.clone(),
                inner.content(),
                inner.text_revision,
            ))
        });
        let (path, content, revision) = match captured {
            Ok(captured) => captured,
            Err(message) => {
                lifecycle.push(LifecycleEvent::Error(message.clone()));
                return vec![ChromeAction::Echo(message)];
            }
        };
        let Some(path) = path else {
            let message = format!(
                "buffer {} has no visited file; choose a destination",
                buffer.display_name()
            );
            lifecycle.push(LifecycleEvent::Error(message.clone()));
            return vec![ChromeAction::Echo(message)];
        };
        match crate::native_io::execute(
            &self.kernel,
            NativeOperation::WriteFile {
                path: path.clone(),
                contents: content.clone(),
            },
            std::future::pending(),
        )
        .await
        {
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
        if buffer.visited_file().as_ref() != Some(&path) {
            return vec![ChromeAction::Echo(format!(
                "Saved {}, but the buffer now visits another file",
                path.display()
            ))];
        }
        let watch_error = self
            .editor
            .file_watcher
            .watch_file(buffer_id, &path, content)
            .err();
        let mut actions = vec![ChromeAction::Echo(format!("Saved: {}", path.display()))];
        buffer.with_write(|inner| {
            if inner.visited_file.as_ref() == Some(&path) {
                inner.last_saved_revision = revision;
            }
        });
        if let Some(error) = watch_error {
            actions.push(ChromeAction::Echo(format!(
                "Saved {}, but failed to watch it: {error}",
                path.display()
            )));
        }
        actions
    }

    fn fail_workspace(&mut self, error: std::io::Error, lifecycle: &mut Vec<LifecycleEvent>) {
        self.terminated = true;
        self.kernel.lock().unwrap().io_owner().cancel_all();
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
        lifecycle.push(LifecycleEvent::WorkspaceTerminated);
    }

    async fn apply_pointer(
        &mut self,
        attachment: &mut Attachment,
        pointer: PointerEvent,
        lifecycle: &mut Vec<LifecycleEvent>,
        invalidations: &mut Vec<Invalidation>,
    ) {
        if let Some(hit) = pointer.text_hit {
            let valid = self
                .view_ids
                .iter()
                .find(|(_, view)| **view == hit.view)
                .and_then(|(window, _)| self.editor.windows.get(*window))
                .and_then(|window| {
                    self.editor
                        .buffers
                        .get(window.active_buffer)
                        .map(|buffer| (window, buffer))
                })
                .is_some_and(|(window, buffer)| {
                    self.buffer_resources.get(&window.active_buffer) == Some(&hit.resource)
                        && buffer.text_revision() == hit.text_revision
                        && hit.position <= buffer.buffer_len_chars()
                });
            if !valid {
                lifecycle.push(LifecycleEvent::Warning(
                    "pointer text hit refers to stale presentation data".into(),
                ));
                return;
            }
        }
        if self.mica.is_none() {
            self.apply_pointer_without_policy(attachment, pointer);
            invalidations.push(Invalidation::Full);
            return;
        }

        let button = pointer_button_name(pointer.button);
        if pointer.kind == PointerKind::Up {
            let window = attachment
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
            self.dispatch_mica_pointer(
                attachment,
                MicaPointerInput {
                    view: window,
                    position,
                    phase: "up",
                    button,
                },
                lifecycle,
                invalidations,
            )
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
                                self.apply_mica_events(
                                    attachment,
                                    events,
                                    lifecycle,
                                    invalidations,
                                )
                                .await;
                            }
                            Err(error) => lifecycle.push(LifecycleEvent::Error(error.to_string())),
                        }
                    }
                }
                return;
            }
            if let Some((window, _)) = attachment.pointer_selection {
                let position = self.pointer_position(attachment, window, &pointer);
                self.dispatch_mica_pointer(
                    attachment,
                    MicaPointerInput {
                        view: window,
                        position,
                        phase: "move",
                        button,
                    },
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
            attachment.pending_pointer_drag = Some((border, target, (pointer.column, pointer.row)));
            let position = self.editor.windows[target].cursor;
            self.dispatch_mica_pointer(
                attachment,
                MicaPointerInput {
                    view: target,
                    position,
                    phase: "layout_down",
                    button,
                },
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
            let position = self.pointer_position(attachment, window, &pointer);
            self.dispatch_mica_pointer(
                attachment,
                MicaPointerInput {
                    view: window,
                    position,
                    phase: "down",
                    button,
                },
                lifecycle,
                invalidations,
            )
            .await;
        }
    }

    async fn dispatch_mica_pointer(
        &mut self,
        attachment: &mut Attachment,
        input: MicaPointerInput<'_>,
        lifecycle: &mut Vec<LifecycleEvent>,
        invalidations: &mut Vec<Invalidation>,
    ) {
        let mut mica = self.mica.take().expect("Mica presence checked by caller");
        let result = mica
            .dispatch_pointer(
                &self.editor,
                &self.buffer_resources,
                input.view,
                input.position,
                input.phase,
                input.button,
            )
            .await;
        self.mica = Some(mica);
        match result {
            Ok(events) => {
                self.apply_mica_events(attachment, events, lifecycle, invalidations)
                    .await;
            }
            Err(error) => lifecycle.push(LifecycleEvent::Error(error.to_string())),
        }
    }

    fn pointer_position(
        &self,
        attachment: &Attachment,
        window: WindowId,
        pointer: &PointerEvent,
    ) -> usize {
        if let Some(hit) = pointer.text_hit
            && self.view_ids.get(&window) == Some(&hit.view)
        {
            return hit.position;
        }
        cursor_at(
            &self.editor,
            attachment,
            window,
            pointer.column,
            pointer.row,
        )
    }

    fn apply_pointer_without_policy(&mut self, attachment: &mut Attachment, pointer: PointerEvent) {
        if pointer.button != PointerButton::Primary && pointer.kind != PointerKind::Move {
            return;
        }
        if pointer.kind == PointerKind::Up {
            self.editor.mouse_drag_state = None;
            attachment.pointer_selection = None;
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
            if let Some((window_id, anchor)) = attachment.pointer_selection {
                let cursor = self.pointer_position(attachment, window_id, &pointer);
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
            attachment.pointer_selection = None;
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
        let cursor = self.pointer_position(attachment, window_id, &pointer);
        let buffer_id = self.editor.windows[window_id].active_buffer;
        self.editor.buffers[buffer_id].clear_mark();
        self.editor.windows[window_id].cursor = cursor;
        attachment.pointer_selection = Some((window_id, cursor));
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
                        buffer.display_name()
                    )),
                }
            }
        }

        let live_windows: HashSet<_> = self.editor.windows.keys().collect();
        if self.typeout.as_ref().is_some_and(|typeout| {
            self.editor
                .windows
                .get(typeout.view)
                .is_none_or(|window| window.active_buffer != typeout.origin_buffer)
        }) {
            self.typeout = None;
        }
        self.view_ids
            .retain(|window, _| live_windows.contains(window));
        self.mica_search_ranges
            .retain(|window, _| live_windows.contains(window));
        self.mica_styled_lines
            .retain(|window, _| live_windows.contains(window));
        self.presentation.retain_live(&live_buffers);
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
        cleanup_warnings.extend(self.kernel.lock().unwrap().shutdown_watches());
        (invalidated, cleanup_warnings)
    }

    fn show_typeout(
        &mut self,
        attachment: &mut Attachment,
        view: WindowId,
        origin_buffer: BufferId,
        kind: String,
        title: String,
        text: String,
    ) -> Result<ViewId, String> {
        let Some(window) = self.editor.windows.get(view) else {
            return Err("typeout targeted a stale view".to_owned());
        };
        if window.active_buffer != origin_buffer {
            return Err("typeout targeted a stale buffer".to_owned());
        }
        if text.chars().count() > MAX_TYPEOUT_TEXT_CHARS {
            return Err(format!(
                "typeout text exceeds the {MAX_TYPEOUT_TEXT_CHARS}-character limit"
            ));
        }
        if title.chars().count() > MAX_TYPEOUT_TITLE_CHARS {
            return Err(format!(
                "typeout title exceeds the {MAX_TYPEOUT_TITLE_CHARS}-character limit"
            ));
        }
        let id = TypeoutId(self.next_typeout_id);
        self.next_typeout_id = self.next_typeout_id.saturating_add(1);
        self.typeout = Some(TypeoutState {
            id,
            view,
            origin_buffer,
            kind,
            title,
            text,
        });
        attachment.typeout_page = Some((id, 0));
        self.view_ids
            .get(&view)
            .copied()
            .ok_or_else(|| "typeout targeted a view without a presentation identity".to_owned())
    }

    fn dismiss_typeout(&mut self, attachment: &mut Attachment) -> Option<ViewId> {
        attachment.typeout_page = None;
        self.typeout
            .take()
            .and_then(|typeout| self.view_ids.get(&typeout.view).copied())
    }

    /// Move an attachment-local page. Returns the owning view and whether the
    /// final page was dismissed.
    fn page_typeout(
        &mut self,
        attachment: &mut Attachment,
        forward: bool,
    ) -> Option<(ViewId, bool)> {
        let typeout = self.typeout.as_ref()?;
        let view = self.view_ids.get(&typeout.view).copied()?;
        let window = self.editor.windows.get(typeout.view)?;
        if window.active_buffer != typeout.origin_buffer {
            self.typeout = None;
            attachment.typeout_page = None;
            return Some((view, true));
        }
        let page_rows = typeout_body_rows(window.height_chars);
        let total_lines = typeout_text_lines(&typeout.text).len();
        let (_, first_line) = attachment.typeout_page.get_or_insert((typeout.id, 0));
        if forward {
            let next = first_line.saturating_add(page_rows);
            if next >= total_lines {
                self.typeout = None;
                attachment.typeout_page = None;
                return Some((view, true));
            }
            *first_line = next;
        } else {
            *first_line = first_line.saturating_sub(page_rows);
        }
        Some((view, false))
    }

    fn capture_snapshot(&mut self, attachment: &mut Attachment) -> PresentationSnapshot {
        self.presentation.capture(
            presentation::ProjectionInput {
                editor: &self.editor,
                policy: &self.policy,
                buffer_resources: &self.buffer_resources,
                view_ids: &self.view_ids,
                typeout: self.typeout.as_ref(),
                search_ranges: &self.mica_search_ranges,
                styled_lines: &self.mica_styled_lines,
            },
            presentation::ProjectionAttachment {
                epoch: attachment.epoch,
                revision: attachment.revision,
                viewport: attachment.viewport,
                view_scroll: &mut attachment.view_scroll,
                presented_cursors: &mut attachment.presented_cursors,
                typeout_page: &mut attachment.typeout_page,
            },
            &mut self.syntax,
        )
    }

    fn finish_server_output(
        &mut self,
        attachment: &mut Attachment,
        invalidations: Vec<Invalidation>,
        lifecycle: Vec<LifecycleEvent>,
    ) -> SessionOutput {
        let presentation =
            if attachment.status != AttachmentStatus::Attached || invalidations.is_empty() {
                None
            } else {
                attachment.revision.0 += 1;
                let snapshot = self.capture_snapshot(attachment);
                Some(PresentationUpdate::Delta(PresentationDelta {
                    epoch: attachment.epoch,
                    base_revision: Revision(attachment.revision.0 - 1),
                    revision: attachment.revision,
                    invalidations,
                    snapshot,
                }))
            };
        SessionOutput {
            protocol_version: SESSION_PROTOCOL_VERSION,
            epoch: attachment.epoch,
            acknowledged_input: None,
            presentation,
            native_completions: Vec::new(),
            frontend_requests: attachment.frontend_requests.drain(..).collect(),
            lifecycle,
        }
    }

    pub async fn poll_output(
        &mut self,
        attachment: &mut Attachment,
    ) -> Result<Option<SessionOutput>, SessionError> {
        if self.terminated {
            return Err(SessionError::WorkspaceTerminated);
        }
        if attachment.status != AttachmentStatus::Attached {
            return Err(SessionError::AttachmentUnavailable);
        }
        self.activate_attachment(attachment);

        let mut lifecycle = Vec::new();
        let mut invalidations = Vec::new();
        if let Some(mut mica) = self.mica.take() {
            let events = mica.drain_background_events();
            self.mica = Some(mica);
            self.apply_mica_events(attachment, events, &mut lifecycle, &mut invalidations)
                .await;
        }
        if self.editor.check_and_clear_expired_echo() {
            invalidations.push(Invalidation::EchoArea);
        }
        let actions = self.editor.poll_file_changes().await;
        self.resolve_actions(actions, &mut invalidations);
        {
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
        let (resources, cleanup_warnings) = self.synchronize_identities();
        lifecycle.extend(cleanup_warnings.into_iter().map(LifecycleEvent::Warning));
        lifecycle.extend(
            resources
                .into_iter()
                .map(|resource| LifecycleEvent::ResourceInvalidated { resource }),
        );

        if invalidations.is_empty()
            && lifecycle.is_empty()
            && attachment.frontend_requests.is_empty()
        {
            return Ok(None);
        }
        Ok(Some(self.finish_server_output(
            attachment,
            invalidations,
            lifecycle,
        )))
    }

    pub async fn complete_frontend_request(
        &mut self,
        attachment: &mut Attachment,
        completion: FrontendServiceResult,
    ) -> Result<SessionOutput, SessionError> {
        if self.terminated {
            return Err(SessionError::WorkspaceTerminated);
        }
        if attachment.status != AttachmentStatus::Attached {
            return Err(SessionError::AttachmentUnavailable);
        }
        self.activate_attachment(attachment);
        let Some(pending) = attachment
            .pending_frontend_requests
            .remove(&completion.request_id)
        else {
            return Ok(self.finish_server_output(
                attachment,
                Vec::new(),
                vec![LifecycleEvent::Warning(format!(
                    "unknown or completed frontend request {:?}",
                    completion.request_id
                ))],
            ));
        };

        let mut lifecycle = Vec::new();
        let mut invalidations = Vec::new();
        let completion_result = match completion.result {
            Err(error) if error.chars().count() > MAX_FRONTEND_TEXT_CHARS => Err(format!(
                "frontend service error exceeds {MAX_FRONTEND_TEXT_CHARS} characters"
            )),
            result => result,
        };
        match pending {
            PendingFrontendRequest::ReadClipboardForYank => {
                match completion_result {
                    Ok(FrontendServiceResponse::ClipboardContents(Some(text)))
                        if text.chars().count() <= MAX_FRONTEND_TEXT_CHARS =>
                    {
                        self.editor.kill_ring.import_external_text(text);
                    }
                    Ok(FrontendServiceResponse::ClipboardContents(Some(_))) => lifecycle.push(
                        LifecycleEvent::Overloaded {
                            detail: format!(
                                "frontend clipboard result exceeds {MAX_FRONTEND_TEXT_CHARS} characters"
                            ),
                        },
                    ),
                    Ok(FrontendServiceResponse::ClipboardContents(None)) => lifecycle.push(LifecycleEvent::Warning(
                        "frontend clipboard read returned no text; using the internal kill ring"
                            .to_owned(),
                    )),
                    Ok(FrontendServiceResponse::Completed) => lifecycle.push(LifecycleEvent::Warning(
                        "frontend clipboard read returned the wrong response type; using the internal kill ring"
                            .to_owned(),
                    )),
                    Err(error) => lifecycle.push(LifecycleEvent::Warning(format!(
                        "frontend clipboard read failed; using the internal kill ring: {error}"
                    ))),
                }
                match self
                    .editor
                    .perform_native_action(KeyAction::Yank(None))
                    .await
                {
                    Ok(actions) => {
                        self.resolve_actions(actions, &mut invalidations);
                    }
                    Err(error) => self.fail_workspace(error, &mut lifecycle),
                }
            }
            PendingFrontendRequest::WriteClipboard => match completion_result {
                Ok(FrontendServiceResponse::Completed) => {}
                Ok(FrontendServiceResponse::ClipboardContents(_)) => {
                    lifecycle.push(LifecycleEvent::Warning(
                        "frontend clipboard write returned the wrong response type".to_owned(),
                    ))
                }
                Err(error) => lifecycle.push(LifecycleEvent::Warning(format!(
                    "frontend clipboard write failed: {error}"
                ))),
            },
        }
        Ok(self.finish_server_output(attachment, invalidations, lifecycle))
    }

    pub fn detach(&mut self, attachment: &mut Attachment) -> Result<SessionOutput, SessionError> {
        if self.terminated {
            return Err(SessionError::WorkspaceTerminated);
        }
        attachment.detach()?;
        let attachment_id = attachment.id;
        Ok(self.finish_server_output(
            attachment,
            Vec::new(),
            vec![LifecycleEvent::AttachmentDetached {
                attachment: attachment_id,
            }],
        ))
    }

    pub fn resume(
        &mut self,
        attachment: &mut Attachment,
        configuration: AttachmentConfiguration,
    ) -> Result<SessionOutput, SessionError> {
        if self.terminated {
            return Err(SessionError::WorkspaceTerminated);
        }
        let viewport = configuration.viewport;
        attachment.resume(configuration)?;
        self.editor.handle_resize(viewport.columns, viewport.rows);
        Ok(SessionOutput {
            protocol_version: SESSION_PROTOCOL_VERSION,
            epoch: attachment.epoch,
            acknowledged_input: None,
            presentation: Some(PresentationUpdate::Full(self.capture_snapshot(attachment))),
            native_completions: Vec::new(),
            frontend_requests: Vec::new(),
            lifecycle: vec![LifecycleEvent::AttachmentAttached {
                attachment: attachment.id,
            }],
        })
    }

    pub fn close_attachment(
        &mut self,
        attachment: &mut Attachment,
    ) -> Result<SessionOutput, SessionError> {
        if attachment.status == AttachmentStatus::Closed {
            return Err(SessionError::AttachmentUnavailable);
        }
        let attachment_id = attachment.id;
        attachment.close();
        Ok(self.finish_server_output(
            attachment,
            Vec::new(),
            vec![LifecycleEvent::AttachmentClosed {
                attachment: attachment_id,
            }],
        ))
    }

    pub async fn terminate_workspace(
        &mut self,
        attachment: &mut Attachment,
    ) -> Result<SessionOutput, SessionError> {
        let mut lifecycle = Vec::new();
        let mut invalidations = Vec::new();
        let io = self.kernel.lock().unwrap().io_owner();
        io.cancel_all();
        if let Some(mut mica) = self.mica.take() {
            match mica.close().await {
                Ok(events) => {
                    self.apply_mica_events(attachment, events, &mut lifecycle, &mut invalidations)
                        .await;
                }
                Err(error) => lifecycle.push(LifecycleEvent::Warning(format!(
                    "Mica workspace shutdown: {error}"
                ))),
            }
        }
        io.close().await;
        {
            self.terminated = true;
            lifecycle.extend(
                self.editor
                    .shutdown_native_work()
                    .into_iter()
                    .map(LifecycleEvent::Warning),
            );
            let (resources, cleanup_warnings) = self.invalidate_all_resources();
            lifecycle.extend(cleanup_warnings.into_iter().map(LifecycleEvent::Warning));
            lifecycle.extend(
                resources
                    .into_iter()
                    .map(|resource| LifecycleEvent::ResourceInvalidated { resource }),
            );
        }
        attachment.close();
        lifecycle.push(LifecycleEvent::WorkspaceTerminated);
        Ok(self.finish_server_output(attachment, Vec::new(), lifecycle))
    }
}

impl SessionClient for DirectSessionClient {
    fn attachment_id(&self) -> AttachmentId {
        self.attachment.id
    }

    fn epoch(&self) -> SessionEpoch {
        self.attachment.epoch
    }

    fn next_sequence(&self) -> u64 {
        self.attachment.next_sequence
    }

    fn envelope(&self, event: InputEvent) -> InputEnvelope {
        InputEnvelope {
            protocol_version: SESSION_PROTOCOL_VERSION,
            epoch: self.attachment.epoch,
            sequence: self.attachment.next_sequence,
            event,
        }
    }

    async fn initial_output(&mut self) -> SessionOutput {
        self.workspace.initial_output(&mut self.attachment).await
    }

    async fn dispatch(&mut self, envelope: InputEnvelope) -> Result<SessionOutput, SessionError> {
        self.workspace
            .dispatch(&mut self.attachment, envelope)
            .await
    }

    async fn poll_output(&mut self) -> Result<Option<SessionOutput>, SessionError> {
        self.workspace.poll_output(&mut self.attachment).await
    }

    async fn complete_frontend_request(
        &mut self,
        completion: FrontendServiceResult,
    ) -> Result<SessionOutput, SessionError> {
        self.workspace
            .complete_frontend_request(&mut self.attachment, completion)
            .await
    }

    async fn detach(&mut self) -> Result<SessionOutput, SessionError> {
        self.workspace.detach(&mut self.attachment)
    }

    async fn resume(
        &mut self,
        configuration: AttachmentConfiguration,
    ) -> Result<SessionOutput, SessionError> {
        self.workspace.resume(&mut self.attachment, configuration)
    }

    async fn close_attachment(&mut self) -> Result<SessionOutput, SessionError> {
        self.workspace.close_attachment(&mut self.attachment)
    }

    async fn terminate_workspace(&mut self) -> Result<SessionOutput, SessionError> {
        self.workspace
            .terminate_workspace(&mut self.attachment)
            .await
    }
}

impl DirectSessionClient {
    pub fn new(mut workspace: WorkspaceHost, configuration: AttachmentConfiguration) -> Self {
        let attachment = workspace.attach(configuration);
        Self {
            workspace,
            attachment,
        }
    }

    /// Recover the embedded workspace after permanently closing its attachment,
    /// allowing a new direct attachment to be created without terminating the
    /// workspace.
    pub fn into_workspace(self) -> Result<WorkspaceHost, SessionError> {
        if self.attachment.status != AttachmentStatus::Closed {
            return Err(SessionError::AttachmentUnavailable);
        }
        Ok(self.workspace)
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

struct MicaPromptContent {
    content: String,
    cursor: usize,
    selected_line: Option<usize>,
}

fn mica_prompt_content(update: &MicaPromptUpdate) -> MicaPromptContent {
    let prefix = &update.prefix;
    let mut content = format!("{prefix}{}", update.query);
    let selected_row = MICA_PROMPT_CANDIDATE_ROWS
        .saturating_sub(MICA_PROMPT_CONTEXT_BELOW)
        .saturating_sub(1);
    let first_candidate = update.selected.saturating_sub(selected_row).min(
        update
            .candidates
            .len()
            .saturating_sub(MICA_PROMPT_CANDIDATE_ROWS),
    );
    for name in update
        .candidates
        .iter()
        .skip(first_candidate)
        .take(MICA_PROMPT_CANDIDATE_ROWS)
    {
        content.push('\n');
        content.push_str(name);
    }
    let selected_line = (update.selected < update.candidates.len())
        .then(|| update.selected.saturating_sub(first_candidate) + 1);
    MicaPromptContent {
        content,
        cursor: prefix.chars().count() + update.query.chars().count(),
        selected_line,
    }
}

fn capability_list(grants: &CapabilityGrants) -> Vec<Capability> {
    [
        Capability::TextRead,
        Capability::TextWrite,
        Capability::Layout,
        Capability::FileRead,
        Capability::FileWrite,
        Capability::ClockRead,
        Capability::ProcessSpawn,
        Capability::Watch,
    ]
    .into_iter()
    .filter(|capability| grants.contains(*capability))
    .collect()
}

fn typeout_body_rows(view_rows: u16) -> usize {
    let content_rows = usize::from(view_rows.saturating_sub(2)).max(1);
    let overlay_rows = (content_rows * 2).div_ceil(3);
    overlay_rows.saturating_sub(2).max(1)
}

fn typeout_text_lines(text: &str) -> Vec<&str> {
    let lines: Vec<_> = text.lines().collect();
    if lines.is_empty() { vec![""] } else { lines }
}

fn cursor_at(
    editor: &Editor,
    attachment: &Attachment,
    window_id: WindowId,
    column: u16,
    row: u16,
) -> usize {
    let window = &editor.windows[window_id];
    let buffer = &editor.buffers[window.active_buffer];
    let scroll = attachment
        .view_scroll
        .get(&window_id)
        .copied()
        .unwrap_or(ViewScroll {
            start_line: 0,
            start_column: 0,
        });
    let line = usize::from(row.saturating_sub(window.y.saturating_add(1)))
        .saturating_add(scroll.start_line);
    let column = usize::from(column.saturating_sub(window.x.saturating_add(1)))
        .saturating_add(scroll.start_column);
    buffer.to_char_index(column, line)
}

fn pointer_button_name(button: PointerButton) -> &'static str {
    match button {
        PointerButton::Primary => "primary",
        PointerButton::Secondary => "secondary",
        PointerButton::Middle => "middle",
        PointerButton::None => "none",
    }
}

fn native_result_size(result: &NativeResult) -> usize {
    match result {
        NativeResult::Snapshot(snapshot) => snapshot.name.len().saturating_add(snapshot.text.len()),
        NativeResult::FileContents(contents) => contents.len(),
        NativeResult::DirectoryEntries { directory, entries } => {
            entries
                .iter()
                .fold(directory.as_os_str().len(), |size, entry| {
                    size.saturating_add(entry.name.len())
                        .saturating_add(entry.path.as_os_str().len())
                })
        }
        NativeResult::ProcessOutput { stdout, stderr, .. } => {
            stdout.len().saturating_add(stderr.len())
        }
        NativeResult::ResourceCreated(_)
        | NativeResult::ResourceClosed
        | NativeResult::TextChanged { .. }
        | NativeResult::LayoutValidated
        | NativeResult::FileWritten
        | NativeResult::ClockMillis(_)
        | NativeResult::WatchRegistered
        | NativeResult::WatchUnregistered => 0,
    }
}

#[cfg(test)]
mod tests;
