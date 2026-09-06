// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-only

//! Apply validated bridge messages in emission order.

use super::*;
use crate::mica_host::{MicaEvent, MicaHostAction, MicaNativeAction};

const MAX_EFFECT_CASCADE: usize = 256;
const MAX_EFFECT_DEPTH: usize = 32;

impl WorkspaceHost {
    pub(super) async fn apply_mica_events(
        &mut self,
        attachment: &mut Attachment,
        events: MicaEventBatch,
        lifecycle: &mut Vec<LifecycleEvent>,
        invalidations: &mut Vec<Invalidation>,
    ) {
        if self.mica_effect_depth == 0 {
            self.mica_effect_remaining = MAX_EFFECT_CASCADE;
        }
        if self.mica_effect_depth >= MAX_EFFECT_DEPTH {
            lifecycle.push(LifecycleEvent::Overloaded {
                detail: "Mica effect cascade exceeds the nesting limit".into(),
            });
            return;
        }
        self.mica_effect_depth += 1;
        let mut settle_typeout_closed = false;
        for event in events.into_events() {
            if self.mica_effect_remaining == 0 {
                lifecycle.push(LifecycleEvent::Overloaded {
                    detail: "Mica effect cascade exceeds the event limit".into(),
                });
                break;
            }
            self.mica_effect_remaining -= 1;
            match event {
                MicaEvent::Policy(facts) => match self.policy.replace(facts) {
                    Ok(true) => invalidations
                        .extend(self.view_ids.values().copied().map(Invalidation::View)),
                    Ok(false) => {}
                    Err(message) => lifecycle.push(LifecycleEvent::Error(message)),
                },
                MicaEvent::PromptClosed => {
                    if let Some(window) = self.editor.find_command_window() {
                        self.mica_styled_lines.remove(&window);
                        self.editor.close_command_window(window);
                        invalidations.push(Invalidation::Full);
                    }
                }
                MicaEvent::Presentation(effect) => {
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
                                "Mica effect referred to a stale view/buffer association"
                                    .to_owned(),
                            ));
                        }
                    }
                }
                MicaEvent::Prompt(update) => {
                    let prompt = mica_prompt_content(&update);
                    if let Some(window) = self
                        .editor
                        .update_mica_prompt_window(&prompt.content, prompt.cursor)
                    {
                        attachment.view_scroll.insert(
                            window,
                            ViewScroll {
                                start_line: 0,
                                start_column: 0,
                            },
                        );
                        self.set_prompt_selected_line(window, prompt.selected_line);
                        if let Some(view) = self.view_ids.get(&window).copied() {
                            invalidations.push(Invalidation::View(view));
                        } else {
                            invalidations.push(Invalidation::Full);
                        }
                    } else {
                        let window = self.editor.create_mica_prompt_window(
                            MICA_PROMPT_HEIGHT,
                            prompt.content,
                            prompt.cursor,
                        );
                        attachment.view_scroll.insert(
                            window,
                            ViewScroll {
                                start_line: 0,
                                start_column: 0,
                            },
                        );
                        self.set_prompt_selected_line(window, prompt.selected_line);
                        invalidations.push(Invalidation::Full);
                    }
                }
                MicaEvent::Search(update) => {
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
                MicaEvent::SearchFinished(finish) => {
                    self.mica_search_ranges.remove(&finish.view);
                    if !finish.accepted
                        && let Some(window) = self.editor.windows.get_mut(finish.view)
                    {
                        window.cursor = finish.original_cursor;
                    }
                    if let Some(view) = self.view_ids.get(&finish.view).copied() {
                        invalidations.push(Invalidation::View(view));
                    } else {
                        invalidations.push(Invalidation::Full);
                    }
                }
                MicaEvent::Error(message) => {
                    self.editor.set_echo_message(message.clone());
                    invalidations.push(Invalidation::EchoArea);
                    lifecycle.push(LifecycleEvent::Error(message));
                }
                MicaEvent::Native(action) => {
                    self.apply_native_action(attachment, action, lifecycle, invalidations)
                        .await
                }
                MicaEvent::Host(action) => {
                    settle_typeout_closed |= self
                        .apply_host_action(attachment, action, lifecycle, invalidations)
                        .await;
                }
                MicaEvent::TaskCancelled(task_id) => {
                    lifecycle.push(LifecycleEvent::MicaTaskCancelled { task_id })
                }
                MicaEvent::SubscriptionReady(mailbox) => {
                    lifecycle.push(LifecycleEvent::MicaSubscriptionReady { mailbox })
                }
                MicaEvent::Overloaded => lifecycle.push(LifecycleEvent::Overloaded {
                    detail: "Mica event batch exceeds its count or byte limit".into(),
                }),
            }
        }
        if settle_typeout_closed && self.mica_effect_remaining > 0 {
            let settlement = if let Some(mut mica) = self.mica.take() {
                let result = mica
                    .settle_typeout_closed(&self.editor, &self.buffer_resources)
                    .await;
                self.mica = Some(mica);
                result
            } else {
                Err(MicaHostError::Closed)
            };
            match settlement {
                Ok(events) => {
                    Box::pin(self.apply_mica_events(attachment, events, lifecycle, invalidations))
                        .await;
                }
                Err(error) => lifecycle.push(LifecycleEvent::Error(format!(
                    "failed to settle closed typeout state: {error}"
                ))),
            }
        }
        self.mica_effect_depth -= 1;
    }

    async fn apply_native_action(
        &mut self,
        attachment: &mut Attachment,
        action: MicaNativeAction,
        lifecycle: &mut Vec<LifecycleEvent>,
        invalidations: &mut Vec<Invalidation>,
    ) {
        let label = action.label();
        for capability in action.capabilities() {
            if let Err(error) = self.kernel.lock().unwrap().authorize(*capability) {
                lifecycle.push(LifecycleEvent::Error(format!(
                    "Mica native action {label} was denied: {error}"
                )));
                return;
            }
        }
        if action == MicaNativeAction::Yank
            && attachment
                .frontend_capabilities
                .contains(&FrontendCapability::ClipboardRead)
        {
            if let Err(detail) = attachment.enqueue_frontend_request(
                PendingFrontendRequest::ReadClipboardForYank,
                |request_id| FrontendServiceRequest::ReadClipboard { request_id },
            ) {
                lifecycle.push(LifecycleEvent::Overloaded { detail });
            }
            return;
        }
        let writes_clipboard = action.writes_clipboard();
        let actions = native_actions::NativeActions {
            editor: &mut self.editor,
            policy: &self.policy,
        }
        .apply(action)
        .await;
        match actions {
            Ok(actions) => {
                self.resolve_actions(actions, invalidations);
                if writes_clipboard {
                    self.write_kill_ring_to_frontend(attachment, label, lifecycle);
                }
            }
            Err(native_actions::ActionError::Mechanism(error)) => {
                self.fail_workspace(error, lifecycle)
            }
            Err(native_actions::ActionError::Policy(message)) => {
                self.editor.set_echo_message(message.clone());
                invalidations.push(Invalidation::EchoArea);
                lifecycle.push(LifecycleEvent::Error(message));
            }
        }
    }

    async fn apply_host_action(
        &mut self,
        attachment: &mut Attachment,
        action: MicaHostAction,
        lifecycle: &mut Vec<LifecycleEvent>,
        invalidations: &mut Vec<Invalidation>,
    ) -> bool {
        use MicaHostAction::*;
        match action {
            AgentOpen { view, name, text } => {
                if text.chars().count() > MAX_AGENT_BUFFER_CHARS {
                    lifecycle.push(LifecycleEvent::Overloaded { detail: format!("agent transcript display exceeds the {MAX_AGENT_BUFFER_CHARS}-character limit") });
                    return false;
                }
                if name.is_empty()
                    || name.chars().count() > MAX_BUFFER_NAME_CHARS
                    || !self.editor.windows.contains_key(view)
                {
                    lifecycle.push(LifecycleEvent::Error(
                        "Mica agent display has an invalid name or stale view".into(),
                    ));
                    return false;
                }
                self.editor.active_window = view;
                self.editor
                    .show_special_buffer(&name, crate::buffer::BufferKind::Results, &text);
                let (resources, warnings) = self.synchronize_identities();
                lifecycle.extend(warnings.into_iter().map(LifecycleEvent::Warning));
                lifecycle.extend(
                    resources
                        .into_iter()
                        .map(|resource| LifecycleEvent::ResourceInvalidated { resource }),
                );
                invalidations.push(Invalidation::Full);
            }
            AgentUpdate {
                buffer: buffer_id,
                text,
            } => {
                if text.chars().count() > MAX_AGENT_BUFFER_CHARS {
                    lifecycle.push(LifecycleEvent::Overloaded { detail: format!("agent transcript display exceeds the {MAX_AGENT_BUFFER_CHARS}-character limit") });
                    return false;
                }
                let Some(buffer) = self.editor.buffers.get(buffer_id) else {
                    lifecycle.push(LifecycleEvent::Error(
                        "Mica agent update targeted a stale buffer".into(),
                    ));
                    return false;
                };
                if buffer.kind() != crate::buffer::BufferKind::Results || !buffer.is_read_only() {
                    lifecycle.push(LifecycleEvent::Error(
                        "Mica agent update requires a read-only results buffer".into(),
                    ));
                    return false;
                }
                let revision = buffer.text_revision();
                buffer.load_str(&text);
                if buffer.text_revision() != revision {
                    for (window_id, window) in &mut self.editor.windows {
                        if window.active_buffer == buffer_id {
                            window.cursor = window.cursor.min(buffer.buffer_len_chars());
                            invalidations.push(Invalidation::View(self.view_ids[&window_id]));
                        }
                    }
                }
            }
            StartAgent => {
                let started = if let Some(mut mica) = self.mica.take() {
                    let result = mica.start_agent(&self.editor, &self.buffer_resources).await;
                    self.mica = Some(mica);
                    result
                } else {
                    Err(MicaHostError::Closed)
                };
                match started {
                    Ok(events) => {
                        Box::pin(self.apply_mica_events(
                            attachment,
                            events,
                            lifecycle,
                            invalidations,
                        ))
                        .await;
                    }
                    Err(error) => lifecycle.push(LifecycleEvent::Error(format!(
                        "failed to start the Mica agent: {error}"
                    ))),
                }
            }
            ShowTypeout {
                view,
                buffer,
                kind,
                title,
                text,
            } => match self.show_typeout(attachment, view, buffer, kind, title, text) {
                Ok(view) => invalidations.push(Invalidation::Typeout(view)),
                Err(detail) if detail.contains("limit") => {
                    lifecycle.push(LifecycleEvent::Overloaded { detail })
                }
                Err(message) => lifecycle.push(LifecycleEvent::Error(message)),
            },
            DismissTypeout => {
                if let Some(view) = self.dismiss_typeout(attachment) {
                    invalidations.push(Invalidation::Typeout(view));
                }
            }
            PageTypeout { forward } => {
                if let Some((view, closed)) = self.page_typeout(attachment, forward) {
                    invalidations.push(Invalidation::Typeout(view));
                    return closed;
                }
                return true;
            }
            Echo(text) => {
                self.editor.set_echo_message(text);
                invalidations.push(Invalidation::EchoArea);
            }
            Quit => lifecycle.push(LifecycleEvent::QuitRequested),
            Redraw => invalidations.push(Invalidation::Full),
            Split { view, direction } => {
                if let Err(error) = self.kernel.lock().unwrap().authorize(Capability::Layout) {
                    lifecycle.push(LifecycleEvent::Error(format!(
                        "Mica split was denied: {error}"
                    )));
                    return false;
                }
                if !self.editor.windows.contains_key(view) {
                    lifecycle.push(LifecycleEvent::Error(
                        "Mica split targeted a stale view".into(),
                    ));
                    return false;
                }
                if self.editor.windows.len() >= MAX_SESSION_VIEWS {
                    lifecycle.push(LifecycleEvent::Overloaded {
                        detail: format!("logical view limit of {MAX_SESSION_VIEWS} reached"),
                    });
                    return false;
                }
                if self.realize_layout_change(
                    layout::LayoutChange::Split { view, direction },
                    lifecycle,
                ) {
                    invalidations.push(Invalidation::Full);
                }
            }
            SelectView { view } => {
                if !self.editor.windows.contains_key(view) {
                    lifecycle.push(LifecycleEvent::Error(
                        "Mica selection targeted a stale view".into(),
                    ));
                    return false;
                }
                let previous = self.editor.active_window;
                self.editor.active_window = view;
                for window in [previous, view] {
                    if let Some(view) = self.view_ids.get(&window).copied()
                        && !invalidations.contains(&Invalidation::View(view))
                    {
                        invalidations.push(Invalidation::View(view));
                    }
                }
            }
            DeleteView { view } | CollapseToView { view } => {
                let collapse = matches!(action, CollapseToView { .. });
                if let Err(error) = self.kernel.lock().unwrap().authorize(Capability::Layout) {
                    lifecycle.push(LifecycleEvent::Error(format!(
                        "Mica layout change was denied: {error}"
                    )));
                    return false;
                }
                if !self.editor.windows.contains_key(view) {
                    lifecycle.push(LifecycleEvent::Error(
                        "Mica layout change targeted a stale view".into(),
                    ));
                    return false;
                }
                if self.realize_layout_change(
                    layout::LayoutChange::Delete {
                        view,
                        others: collapse,
                    },
                    lifecycle,
                ) {
                    invalidations.push(Invalidation::Full);
                }
            }
            BeginLayoutDrag { view } => match attachment.pending_pointer_drag.take() {
                Some((border, target, position)) if view == target => {
                    self.editor.mouse_drag_state = Some(MouseDragState {
                        drag_type: DragType::WindowBorder,
                        start_pos: position,
                        last_pos: position,
                        current_pos: position,
                        target_window: Some(target),
                        border_info: Some(border),
                    });
                    attachment.pointer_selection = None;
                }
                _ => lifecycle.push(LifecycleEvent::Error(
                    "Mica layout-drag decision lost its native border target".into(),
                )),
            },
            PointerDown {
                view,
                position,
                anchor,
            }
            | PointerMove {
                view,
                position,
                anchor,
            } => {
                let down = matches!(action, PointerDown { .. });
                let Some(window) = self.editor.windows.get(view) else {
                    lifecycle.push(LifecycleEvent::Error(
                        "Mica pointer decision targeted a stale view".into(),
                    ));
                    return false;
                };
                let buffer = window.active_buffer;
                let len = self.editor.buffers[buffer].buffer_len_chars();
                if position > len || anchor > len {
                    lifecycle.push(LifecycleEvent::Error(
                        "Mica pointer decision exceeds the buffer range".into(),
                    ));
                    return false;
                }
                if down {
                    let previous = self.editor.active_window;
                    if previous != view {
                        self.editor.previous_active_window = Some(previous);
                        self.editor.active_window = view;
                    }
                    self.editor.buffers[buffer].clear_mark();
                    attachment.pointer_selection = Some((view, anchor));
                    if let Some(id) = self.view_ids.get(&previous).copied() {
                        invalidations.push(Invalidation::View(id));
                    }
                } else {
                    self.editor.buffers[buffer].set_mark(anchor);
                }
                self.editor.windows[view].cursor = position;
                if let Some(id) = self.view_ids.get(&view).copied() {
                    invalidations.push(Invalidation::View(id));
                }
            }
            PointerUp => {
                self.editor.mouse_drag_state = None;
                attachment.pointer_selection = None;
                attachment.pending_pointer_drag = None;
            }
            Scroll { view, line, column } => {
                if !self.editor.windows.contains_key(view) {
                    lifecycle.push(LifecycleEvent::Error(
                        "Mica scroll targeted a stale view".into(),
                    ));
                    return false;
                }
                attachment.view_scroll.insert(
                    view,
                    ViewScroll {
                        start_line: line,
                        start_column: column,
                    },
                );
                if let Some(id) = self.view_ids.get(&view).copied() {
                    invalidations.push(Invalidation::View(id));
                }
            }
            SplitRatio { path, ratio } => {
                let mut proposed = self.editor.window_tree.clone();
                if !set_ratio_at_path(&mut proposed, &path, ratio) {
                    lifecycle.push(LifecycleEvent::Error(
                        "Mica split-ratio target is stale".into(),
                    ));
                    return false;
                }
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
            InvalidateSyntax { view } => invalidations.push(
                self.view_ids
                    .get(&view)
                    .copied()
                    .map(Invalidation::View)
                    .unwrap_or(Invalidation::Full),
            ),
            Save { buffer } => {
                let actions = self.save_buffer_via_kernel(buffer, lifecycle).await;
                self.resolve_actions(actions, invalidations);
            }
            SaveAs { buffer, path } => {
                if path.is_empty() {
                    lifecycle.push(LifecycleEvent::Error(
                        "save destination must not be empty".to_owned(),
                    ));
                    return false;
                }
                let was_scratch = self
                    .editor
                    .buffers
                    .get(buffer)
                    .is_some_and(|value| value.kind() == crate::buffer::BufferKind::Scratch);
                if let Err(error) = self
                    .editor
                    .visit_file_for_buffer(buffer, std::path::PathBuf::from(path))
                {
                    lifecycle.push(LifecycleEvent::Error(error));
                    return false;
                }
                if was_scratch {
                    self.editor.ensure_scratch_buffer();
                }
                let actions = self.save_buffer_via_kernel(buffer, lifecycle).await;
                self.resolve_actions(actions, invalidations);
            }
            CreateBuffer { view, name } => {
                let name_len = name.chars().count();
                if name.is_empty() || name_len > MAX_BUFFER_NAME_CHARS {
                    lifecycle.push(LifecycleEvent::Overloaded {
                        detail: format!(
                            "buffer name must contain 1..={MAX_BUFFER_NAME_CHARS} characters"
                        ),
                    });
                    return false;
                }
                let buffer = self.editor.create_buffer(name, String::new());
                if let Some(window) = self.editor.windows.get_mut(view) {
                    window.active_buffer = buffer;
                    window.cursor = 0;
                    self.editor.active_window = view;
                    self.editor.record_buffer_access(buffer);
                    invalidations.push(Invalidation::Full);
                } else {
                    self.editor.buffers.remove(buffer);
                    lifecycle.push(LifecycleEvent::Error(
                        "Mica buffer creation targeted a stale view".to_owned(),
                    ));
                }
            }
            EvalRegion {
                buffer: buffer_id,
                view,
            } => {
                if let Err(error) = self
                    .kernel
                    .lock()
                    .unwrap()
                    .authorize(Capability::MicaEvaluate)
                {
                    lifecycle.push(LifecycleEvent::Error(format!(
                        "Mica region evaluation was denied: {error}"
                    )));
                    return false;
                }
                let Some(window) = self.editor.windows.get(view) else {
                    lifecycle.push(LifecycleEvent::Error(
                        "Mica region evaluation targeted a stale view".to_owned(),
                    ));
                    return false;
                };
                if window.active_buffer != buffer_id {
                    lifecycle.push(LifecycleEvent::Error(
                        "Mica region evaluation targeted a stale buffer".to_owned(),
                    ));
                    return false;
                }
                let Some(source) = self.editor.buffers[buffer_id].get_region_text(window.cursor)
                else {
                    lifecycle.push(LifecycleEvent::Error(
                        "Mica region evaluation requires an active region".to_owned(),
                    ));
                    return false;
                };
                if source.chars().count() > MAX_MICA_SOURCE_CHARS {
                    lifecycle.push(LifecycleEvent::Overloaded {
                        detail: format!(
                            "Mica source exceeds the {MAX_MICA_SOURCE_CHARS}-character limit"
                        ),
                    });
                    return false;
                }
                let evaluation = if let Some(mut mica) = self.mica.take() {
                    let result = mica
                        .evaluate_source(&self.editor, &self.buffer_resources, source)
                        .await;
                    self.mica = Some(mica);
                    result
                } else {
                    Err(MicaHostError::Closed)
                };
                match evaluation {
                    Ok(result) => {
                        Box::pin(self.apply_mica_events(
                            attachment,
                            result.events,
                            lifecycle,
                            invalidations,
                        ))
                        .await;
                        let text = format!("Mica => {}\n", result.value);
                        if text.chars().count() > MAX_TYPEOUT_TEXT_CHARS {
                            lifecycle.push(LifecycleEvent::Overloaded {
                                    detail: format!(
                                        "Mica evaluation output exceeds the {MAX_TYPEOUT_TEXT_CHARS}-character typeout limit"
                                    ),
                                });
                            return false;
                        }
                        let routed = if let Some(mut mica) = self.mica.take() {
                            let result = mica
                                .present_evaluation_result(
                                    &self.editor,
                                    &self.buffer_resources,
                                    view,
                                    buffer_id,
                                    text,
                                    false,
                                )
                                .await;
                            self.mica = Some(mica);
                            result
                        } else {
                            Err(MicaHostError::Closed)
                        };
                        match routed {
                            Ok(events) => {
                                Box::pin(self.apply_mica_events(
                                    attachment,
                                    events,
                                    lifecycle,
                                    invalidations,
                                ))
                                .await;
                            }
                            Err(error) => lifecycle.push(LifecycleEvent::Error(format!(
                                "Mica could not route evaluation output: {error}"
                            ))),
                        }
                    }
                    Err(error) => {
                        let message = error.to_string();
                        lifecycle.push(LifecycleEvent::Error(message.clone()));
                        if message.chars().count() > MAX_TYPEOUT_TEXT_CHARS {
                            lifecycle.push(LifecycleEvent::Overloaded {
                                    detail: format!(
                                        "Mica evaluation diagnostic exceeds the {MAX_TYPEOUT_TEXT_CHARS}-character typeout limit"
                                    ),
                                });
                            return false;
                        }
                        let routed = if let Some(mut mica) = self.mica.take() {
                            let result = mica
                                .present_evaluation_result(
                                    &self.editor,
                                    &self.buffer_resources,
                                    view,
                                    buffer_id,
                                    format!("{message}\n"),
                                    true,
                                )
                                .await;
                            self.mica = Some(mica);
                            result
                        } else {
                            Err(MicaHostError::Closed)
                        };
                        match routed {
                            Ok(events) => {
                                Box::pin(self.apply_mica_events(
                                    attachment,
                                    events,
                                    lifecycle,
                                    invalidations,
                                ))
                                .await;
                            }
                            Err(error) => lifecycle.push(LifecycleEvent::Error(format!(
                                "Mica could not route evaluation diagnostic: {error}"
                            ))),
                        }
                    }
                }
            }
            EvalBuffer {
                buffer: buffer_id,
                unit,
            } => {
                if let Err(error) = self
                    .kernel
                    .lock()
                    .unwrap()
                    .authorize(Capability::MicaFilein)
                {
                    lifecycle.push(LifecycleEvent::Error(format!(
                        "Mica buffer file-in was denied: {error}"
                    )));
                    return false;
                }
                let Some(buffer) = self.editor.buffers.get(buffer_id) else {
                    lifecycle.push(LifecycleEvent::Error(
                        "Mica buffer file-in targeted a stale buffer".to_owned(),
                    ));
                    return false;
                };
                let source = buffer.content();
                let revision = buffer.text_revision();
                if source.chars().count() > MAX_MICA_SOURCE_CHARS {
                    lifecycle.push(LifecycleEvent::Overloaded {
                        detail: format!(
                            "Mica source exceeds the {MAX_MICA_SOURCE_CHARS}-character limit"
                        ),
                    });
                    return false;
                }
                let filein = if let Some(mut mica) = self.mica.take() {
                    let result = mica.replace_unit(&unit, source).await;
                    let policy = if result.is_ok() {
                        mica.publish_policy(&self.editor, &self.buffer_resources)
                            .await
                    } else {
                        Ok(MicaEventBatch::default())
                    };
                    self.mica = Some(mica);
                    result.and(policy)
                } else {
                    Err(MicaHostError::Closed)
                };
                match filein {
                    Ok(policy) => {
                        let report = format!(
                            "Filed in {unit} from {} at native revision {revision}",
                            self.editor.buffers[buffer_id].display_name()
                        );
                        self.editor.set_echo_message(report);
                        invalidations.push(Invalidation::EchoArea);
                        Box::pin(self.apply_mica_events(
                            attachment,
                            policy,
                            lifecycle,
                            invalidations,
                        ))
                        .await;
                    }
                    Err(error) => {
                        let message = error.to_string();
                        self.editor.show_special_buffer(
                            "*Mica Diagnostics*",
                            crate::buffer::BufferKind::Diagnostics,
                            &format!("{message}\n"),
                        );
                        self.editor.set_echo_message(message.clone());
                        lifecycle.push(LifecycleEvent::Error(message));
                        invalidations.push(Invalidation::Full);
                    }
                }
            }
            SelectBuffer { buffer } => {
                let actions = self.editor.select_mica_buffer(buffer);
                self.resolve_actions(actions, invalidations);
            }
            KillBuffer {
                buffer,
                replacement,
            } => {
                let actions = self.editor.kill_mica_buffer(buffer, replacement);
                self.editor.ensure_scratch_buffer();
                self.resolve_actions(actions, invalidations);
            }
            OpenFile { path, kind } => {
                let path = std::path::PathBuf::from(path);
                let content = match crate::native_io::execute(
                    &self.kernel,
                    NativeOperation::ReadFile { path: path.clone() },
                    std::future::pending(),
                )
                .await
                {
                    Ok(NativeResult::FileContents(content)) => Some(content),
                    Err(KernelError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                        None
                    }
                    Ok(other) => {
                        lifecycle.push(LifecycleEvent::Error(format!(
                            "file read returned an unexpected native result: {other:?}"
                        )));
                        return false;
                    }
                    Err(error) => {
                        let message = format!(
                            "failed to open {} through the native kernel: {error}",
                            path.display()
                        );
                        lifecycle.push(LifecycleEvent::Error(message.clone()));
                        self.editor.set_echo_message(message);
                        invalidations.push(Invalidation::EchoArea);
                        return false;
                    }
                };
                let actions = self.editor.open_mica_file(path, kind, content);
                self.resolve_actions(actions, invalidations);
            }
        }
        false
    }
}
