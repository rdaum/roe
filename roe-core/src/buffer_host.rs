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

use crate::buffer::Buffer;
use crate::keys::KeyAction;
use crate::mode::{ActionPosition, Mode, ModeAction, ModeResult};
use crate::renderer::DirtyRegion;
use crate::{BufferId, ModeId};
use tokio::sync::{mpsc, oneshot};

/// Message sent to a mode actor
pub enum ModeMessage {
    /// Key action to process
    KeyAction {
        action: KeyAction,
        reply: oneshot::Sender<ModeResult>,
    },
    /// Mouse event to process
    MouseEvent {
        event: crate::mode::MouseEvent,
        reply: oneshot::Sender<ModeResult>,
    },
}

/// Persistent mode actor that runs in its own task
pub struct ModeActor {
    mode_impl: Box<dyn Mode>,
    receiver: mpsc::Receiver<ModeMessage>,
    #[allow(dead_code)] // Used for potential future mode operations
    buffer: Buffer, // Shared buffer access
    #[allow(dead_code)] // Used for identification in potential future operations
    mode_id: ModeId,
}

impl ModeActor {
    pub fn new(
        mode_impl: Box<dyn Mode>,
        receiver: mpsc::Receiver<ModeMessage>,
        buffer: Buffer,
        mode_id: ModeId,
    ) -> Self {
        Self {
            mode_impl,
            receiver,
            buffer,
            mode_id,
        }
    }

    /// Spawn the mode actor as a persistent task
    pub fn spawn(mut self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            while let Some(message) = self.receiver.recv().await {
                match message {
                    ModeMessage::KeyAction { action, reply } => {
                        let result = self.mode_impl.perform(&action);
                        let _ = reply.send(result);
                        continue;
                    }
                    ModeMessage::MouseEvent { event, reply } => {
                        let result = self.mode_impl.handle_mouse(&event);
                        let _ = reply.send(result);
                        continue;
                    }
                };
            }
        })
    }
}

/// Client handle to communicate with a mode actor
pub struct ModeClient {
    sender: mpsc::Sender<ModeMessage>,
    #[allow(dead_code)] // Used for identification in potential future operations
    mode_id: ModeId,
    name: String,
}

impl ModeClient {
    pub fn new(sender: mpsc::Sender<ModeMessage>, mode_id: ModeId, name: String) -> Self {
        Self {
            sender,
            mode_id,
            name,
        }
    }

    /// Send a key action to the mode and wait for response
    pub async fn handle_key(&self, action: KeyAction) -> Result<ModeResult, String> {
        let (reply_tx, reply_rx) = oneshot::channel();
        let message = ModeMessage::KeyAction {
            action,
            reply: reply_tx,
        };

        self.sender
            .send(message)
            .await
            .map_err(|_| format!("Mode {} disconnected", self.name))?;

        reply_rx
            .await
            .map_err(|_| format!("Mode {} reply failed", self.name))
    }

    /// Send a mouse event to the mode and wait for response
    pub async fn handle_mouse(&self, event: crate::mode::MouseEvent) -> Result<ModeResult, String> {
        let (reply_tx, reply_rx) = oneshot::channel();
        let message = ModeMessage::MouseEvent {
            event,
            reply: reply_tx,
        };

        self.sender
            .send(message)
            .await
            .map_err(|_| format!("Mode {} disconnected", self.name))?;

        reply_rx
            .await
            .map_err(|_| format!("Mode {} reply failed", self.name))
    }
}

/// Request sent to BufferHost
#[derive(Debug)]
pub enum BufferRequest {
    /// Process a keystroke through the mode chain
    HandleKey {
        action: KeyAction,
        cursor_pos: usize,
    },
    /// Process a mouse event through the mode chain
    HandleMouse {
        event: crate::mode::MouseEvent,
        cursor_pos: usize,
    },
    /// Get current buffer state
    GetState,
    /// Save buffer to file
    Save,
    /// Load buffer from file
    Load(String),
}

/// Actions that need to be performed at the Editor level
#[derive(Debug, Clone)]
pub enum EditorAction {
    /// Execute a command by name
    ExecuteCommand(String),
    /// Switch to a specific buffer
    SwitchToBuffer(crate::BufferId),
    /// Kill a specific buffer
    KillBuffer(crate::BufferId),
    /// Open a file at a path with specified open type
    OpenFile {
        path: std::path::PathBuf,
        open_type: crate::editor::OpenType,
    },
    /// Kill line (to kill-ring)
    KillLine,
    /// Kill region (to kill-ring)
    KillRegion,
    /// Copy region to kill-ring without deleting
    CopyRegion,
    /// Yank from kill-ring
    Yank {
        position: crate::mode::ActionPosition,
    },
    /// Yank from specific kill-ring index
    YankIndex {
        position: crate::mode::ActionPosition,
        index: usize,
    },
    /// Update isearch highlights and cursor in target buffer/window
    UpdateIsearch {
        target_buffer_id: crate::BufferId,
        target_window_id: crate::WindowId,
        matches: Vec<(usize, usize)>,
        current_match: Option<usize>,
    },
    /// Accept isearch result - close command window, keep cursor
    AcceptIsearch {
        target_buffer_id: crate::BufferId,
        search_term: String,
    },
    /// Cancel isearch - close command window, restore cursor
    CancelIsearch {
        target_buffer_id: crate::BufferId,
        target_window_id: crate::WindowId,
        original_cursor: usize,
    },
}

/// Represents a buffer content change for after-change hooks
#[derive(Debug, Clone)]
pub struct BufferChange {
    pub start: usize,
    pub old_end: usize,
    pub new_end: usize,
}

/// Response from BufferHost
#[derive(Debug, Clone)]
pub enum BufferResponse {
    /// Actions completed with regions that need redrawing
    ActionsCompleted {
        dirty_regions: Vec<DirtyRegion>,
        new_cursor_pos: Option<usize>, // None means cursor didn't move
        editor_action: Option<EditorAction>, // Action to perform at Editor level
        buffer_change: Option<BufferChange>, // Change info for after-change hooks
    },
    /// File operation completed
    Saved(String),
    /// File loaded
    Loaded(String),
    /// Operation failed
    Error(String),
    /// Request processed but no significant change
    NoChange,
}

/// Message combining request with reply channel
pub struct BufferMessage {
    pub request: BufferRequest,
    pub reply: oneshot::Sender<BufferResponse>,
}

/// Client-side interface for communicating with BufferHost
#[derive(Clone)]
pub struct BufferHostClient {
    sender: mpsc::Sender<BufferMessage>,
    #[allow(dead_code)] // Used for identification in potential future operations
    buffer_id: BufferId,
}

impl BufferHostClient {
    pub fn new(sender: mpsc::Sender<BufferMessage>, buffer_id: BufferId) -> Self {
        Self { sender, buffer_id }
    }

    /// Send a key event to the buffer
    pub async fn handle_key(
        &self,
        key: KeyAction,
        cursor_pos: usize,
    ) -> Result<BufferResponse, String> {
        let (reply_tx, reply_rx) = oneshot::channel();
        let message = BufferMessage {
            request: BufferRequest::HandleKey {
                action: key,
                cursor_pos,
            },
            reply: reply_tx,
        };

        self.sender
            .send(message)
            .await
            .map_err(|_| "BufferHost disconnected".to_string())?;

        reply_rx
            .await
            .map_err(|_| "BufferHost reply failed".to_string())
    }

    /// Send a mouse event to the buffer and wait for response
    pub async fn handle_mouse(
        &self,
        event: crate::mode::MouseEvent,
        cursor_pos: usize,
    ) -> Result<BufferResponse, String> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();

        let message = BufferMessage {
            request: BufferRequest::HandleMouse { event, cursor_pos },
            reply: reply_tx,
        };

        self.sender
            .send(message)
            .await
            .map_err(|_| "BufferHost disconnected".to_string())?;

        reply_rx
            .await
            .map_err(|_| "BufferHost reply failed".to_string())
    }
}

/// The BufferHost that processes requests and coordinates with mode actors
pub struct BufferHost {
    buffer: Buffer,
    mode_clients: Vec<ModeClient>,
    receiver: mpsc::Receiver<BufferMessage>,
    buffer_id: BufferId,
    mode_handles: Vec<tokio::task::JoinHandle<()>>, // Keep track of spawned mode tasks
    julia_runtime:
        Option<std::sync::Arc<tokio::sync::Mutex<crate::julia_runtime::RoeJuliaRuntime>>>,
}

impl BufferHost {
    pub fn new(
        buffer: Buffer,
        modes: Vec<(ModeId, String, Box<dyn Mode>)>,
        receiver: mpsc::Receiver<BufferMessage>,
        buffer_id: BufferId,
        julia_runtime: Option<
            std::sync::Arc<tokio::sync::Mutex<crate::julia_runtime::RoeJuliaRuntime>>,
        >,
    ) -> Self {
        let mut mode_clients = Vec::new();
        let mut mode_handles = Vec::new();

        // Spawn each mode as a persistent actor
        for (mode_id, name, mode_impl) in modes {
            let (sender, mode_receiver) = mpsc::channel(32);

            let mode_actor = ModeActor::new(
                mode_impl,
                mode_receiver,
                buffer.clone(), // Share the buffer
                mode_id,
            );

            // Spawn the mode actor
            let handle = mode_actor.spawn();
            mode_handles.push(handle);

            mode_clients.push(ModeClient::new(sender, mode_id, name));
        }

        Self {
            buffer,
            mode_clients,
            receiver,
            buffer_id,
            mode_handles,
            julia_runtime,
        }
    }

    /// Spawn the BufferHost as an async task
    pub fn spawn(mut self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            while let Some(message) = self.receiver.recv().await {
                let response = self.handle_request(message.request).await;

                // Send reply (ignore if receiver dropped)
                let _ = message.reply.send(response);
            }

            // Clean up mode tasks when BufferHost shuts down
            for handle in self.mode_handles {
                handle.abort();
            }
        })
    }

    /// Handle a single request
    async fn handle_request(&mut self, request: BufferRequest) -> BufferResponse {
        match request {
            BufferRequest::HandleKey { action, cursor_pos } => {
                self.handle_key_action(action, cursor_pos).await
            }
            BufferRequest::HandleMouse { event, cursor_pos } => {
                self.handle_mouse_action(event, cursor_pos).await
            }
            BufferRequest::GetState => self.get_state(),
            BufferRequest::Save => self.save_buffer().await,
            BufferRequest::Load(file_path) => self.load_buffer(file_path).await,
        }
    }

    /// Process keystroke through mode chain sequentially
    async fn handle_key_action(
        &mut self,
        key_action: KeyAction,
        cursor_pos: usize,
    ) -> BufferResponse {
        let mut actions_to_execute = vec![];

        // Process modes sequentially: major mode first, then minor modes
        for mode_client in &self.mode_clients {
            match mode_client.handle_key(key_action.clone()).await {
                Ok(result) => {
                    match result {
                        ModeResult::Consumed(actions) => {
                            // This mode consumed the event - stop processing
                            actions_to_execute.extend(actions);
                            break;
                        }
                        ModeResult::Annotated(actions) => {
                            // This mode handled the event but allows others to see it too
                            actions_to_execute.extend(actions);
                            // Continue to next mode
                        }
                        ModeResult::Ignored => {
                            // This mode ignored the event - continue to next mode
                        }
                    }
                }
                Err(_) => {
                    // Mode communication failed, continue to next mode
                    continue;
                }
            }
        }

        // Execute collected actions on the shared buffer
        self.execute_actions(actions_to_execute, cursor_pos).await
    }

    /// Process mouse event through mode chain sequentially
    async fn handle_mouse_action(
        &mut self,
        mouse_event: crate::mode::MouseEvent,
        cursor_pos: usize,
    ) -> BufferResponse {
        let mut actions_to_execute = vec![];

        // Process through mode chain in order
        for mode_client in &self.mode_clients {
            match mode_client.handle_mouse(mouse_event.clone()).await {
                Ok(result) => {
                    match result {
                        ModeResult::Consumed(actions) => {
                            // This mode consumed the event - add its actions and stop
                            actions_to_execute.extend(actions);
                            break;
                        }
                        ModeResult::Annotated(actions) => {
                            // This mode annotated the event - add its actions and continue
                            actions_to_execute.extend(actions);
                        }
                        ModeResult::Ignored => {
                            // This mode ignored the event - continue to next mode
                        }
                    }
                }
                Err(_) => {
                    // Mode communication failed, continue to next mode
                    continue;
                }
            }
        }

        // Execute collected actions on the shared buffer
        self.execute_actions(actions_to_execute, cursor_pos).await
    }

    /// Execute mode actions and return dirty regions
    async fn execute_actions(
        &mut self,
        actions: Vec<ModeAction>,
        mut cursor_pos: usize,
    ) -> BufferResponse {
        let mut dirty_regions = vec![];
        let mut new_cursor_pos = None;
        let mut editor_action = None;
        let mut buffer_changed = false; // Track if any text change occurred

        for action in actions {
            match action {
                ModeAction::InsertText(pos, text) => {
                    match pos {
                        ActionPosition::Cursor => {
                            let has_newline = text.contains('\n');
                            self.buffer.insert_pos(text.clone(), cursor_pos);
                            buffer_changed = true;

                            // Advance the cursor by number of characters (not bytes)
                            cursor_pos += text.chars().count();
                            new_cursor_pos = Some(cursor_pos);

                            // Mark appropriate dirty regions
                            if has_newline {
                                dirty_regions.push(DirtyRegion::Buffer {
                                    buffer_id: self.buffer_id,
                                });
                            } else {
                                let line = self.buffer.to_column_line(cursor_pos).1 as usize;
                                dirty_regions.push(DirtyRegion::Line {
                                    buffer_id: self.buffer_id,
                                    line,
                                });
                            }
                        }
                        ActionPosition::Absolute(col, row) => {
                            let has_newline = text.contains('\n');
                            self.buffer.insert_col_line(text.clone(), (col, row));
                            buffer_changed = true;

                            // For command mode, position cursor at end of user input line
                            if let Some(newline_pos) = text.find('\n') {
                                let first_line = &text[..newline_pos];
                                // Position cursor at the end of the first line (user input)
                                new_cursor_pos = Some(first_line.chars().count());
                            } else {
                                // Single line case - position at the end
                                new_cursor_pos = Some(text.chars().count());
                            }

                            // Mark appropriate dirty regions
                            if has_newline {
                                dirty_regions.push(DirtyRegion::Buffer {
                                    buffer_id: self.buffer_id,
                                });
                            } else {
                                dirty_regions.push(DirtyRegion::Line {
                                    buffer_id: self.buffer_id,
                                    line: row as usize,
                                });
                            }
                        }
                        ActionPosition::End => {
                            // Insert at end of buffer
                            let buffer_len = self.buffer.buffer_len_chars();
                            let has_newline = text.contains('\n');
                            self.buffer.insert_pos(text.clone(), buffer_len);
                            buffer_changed = true;

                            if has_newline {
                                dirty_regions.push(DirtyRegion::Buffer {
                                    buffer_id: self.buffer_id,
                                });
                            } else {
                                let line = self.buffer.to_column_line(buffer_len).1 as usize;
                                dirty_regions.push(DirtyRegion::Line {
                                    buffer_id: self.buffer_id,
                                    line,
                                });
                            }
                        }
                    }
                }
                ModeAction::DeleteText(pos, count) => {
                    match pos {
                        ActionPosition::Cursor => {
                            if let Some(deleted) = self.buffer.delete_pos(cursor_pos, count) {
                                buffer_changed = true;
                                // Check if deleted text contains newlines
                                let has_newline = deleted.contains('\n');

                                if has_newline {
                                    // Newlines affect multiple lines, mark entire buffer dirty
                                    dirty_regions.push(DirtyRegion::Buffer {
                                        buffer_id: self.buffer_id,
                                    });
                                } else {
                                    // Simple text deletion, only current line affected
                                    // For backspace (count < 0), deletion happened BEFORE cursor_pos
                                    // So use the position where deletion occurred
                                    let delete_pos = if count < 0 {
                                        cursor_pos.saturating_sub(1)
                                    } else {
                                        cursor_pos
                                    };
                                    let line = self.buffer.to_column_line(delete_pos).1 as usize;
                                    dirty_regions.push(DirtyRegion::Line {
                                        buffer_id: self.buffer_id,
                                        line,
                                    });
                                }

                                // Update cursor if we deleted backwards
                                if count < 0 {
                                    cursor_pos = cursor_pos.saturating_sub(count.unsigned_abs());
                                    new_cursor_pos = Some(cursor_pos);
                                }
                            }
                        }
                        ActionPosition::Absolute(col, row) => {
                            if let Some(deleted) = self.buffer.delete_col_line((col, row), count) {
                                buffer_changed = true;
                                // Check if deleted text contains newlines
                                let has_newline = deleted.contains('\n');

                                if has_newline {
                                    // Newlines affect multiple lines, mark entire buffer dirty
                                    dirty_regions.push(DirtyRegion::Buffer {
                                        buffer_id: self.buffer_id,
                                    });
                                } else {
                                    // Simple text deletion, only current line affected
                                    dirty_regions.push(DirtyRegion::Line {
                                        buffer_id: self.buffer_id,
                                        line: row as usize,
                                    });
                                }
                            }
                        }
                        ActionPosition::End => {
                            // Delete from end of buffer backwards
                            let buffer_len = self.buffer.buffer_len_chars();
                            if let Some(deleted) = self.buffer.delete_pos(buffer_len, count) {
                                buffer_changed = true;
                                let has_newline = deleted.contains('\n');

                                if has_newline {
                                    dirty_regions.push(DirtyRegion::Buffer {
                                        buffer_id: self.buffer_id,
                                    });
                                } else {
                                    let line = self.buffer.to_column_line(buffer_len).1 as usize;
                                    dirty_regions.push(DirtyRegion::Line {
                                        buffer_id: self.buffer_id,
                                        line,
                                    });
                                }
                            }
                        }
                    }
                }
                ModeAction::Save => {
                    return self.save_buffer().await;
                }
                ModeAction::ClearText => {
                    // Clear all text from the buffer
                    let buffer_len = self.buffer.buffer_len_chars();
                    if buffer_len > 0 {
                        self.buffer.delete_pos(0, buffer_len as isize);
                        buffer_changed = true;
                        dirty_regions.push(DirtyRegion::Buffer {
                            buffer_id: self.buffer_id,
                        });
                        // Move cursor to start - update both so subsequent actions use correct pos
                        cursor_pos = 0;
                        new_cursor_pos = Some(0);
                    }
                }
                ModeAction::SetMark => {
                    self.buffer.set_mark(cursor_pos);
                    // Mark highlighting might change
                    dirty_regions.push(DirtyRegion::Buffer {
                        buffer_id: self.buffer_id,
                    });
                }
                ModeAction::ClearMark => {
                    self.buffer.clear_mark();
                    // Mark highlighting might change
                    dirty_regions.push(DirtyRegion::Buffer {
                        buffer_id: self.buffer_id,
                    });
                }
                ModeAction::ExecuteCommand(command_name) => {
                    // Store command for execution at Editor level
                    editor_action = Some(EditorAction::ExecuteCommand(command_name));
                }
                ModeAction::SwitchToBuffer(buffer_id) => {
                    // Store buffer switch for execution at Editor level
                    editor_action = Some(EditorAction::SwitchToBuffer(buffer_id));
                }
                ModeAction::KillBuffer(buffer_id) => {
                    // Store buffer kill for execution at Editor level
                    editor_action = Some(EditorAction::KillBuffer(buffer_id));
                }
                ModeAction::OpenFile { path, open_type } => {
                    // Store file open for execution at Editor level
                    editor_action = Some(EditorAction::OpenFile { path, open_type });
                }
                ModeAction::KillLine => {
                    // Kill from cursor to end of line (store in kill-ring - will be handled at Editor level)
                    editor_action = Some(EditorAction::KillLine);
                }
                ModeAction::KillRegion => {
                    // Kill the region between mark and cursor (store in kill-ring - will be handled at Editor level)
                    editor_action = Some(EditorAction::KillRegion);
                }
                ModeAction::CopyRegion => {
                    // Copy the region to kill-ring without deleting (will be handled at Editor level)
                    editor_action = Some(EditorAction::CopyRegion);
                    dirty_regions.push(DirtyRegion::Buffer {
                        buffer_id: self.buffer_id,
                    });
                }
                ModeAction::Yank(position) => {
                    // Yank from kill-ring (will be handled at Editor level)
                    editor_action = Some(EditorAction::Yank {
                        position: position.clone(),
                    });
                }
                ModeAction::YankIndex(position, index) => {
                    // Yank from specific kill-ring index (will be handled at Editor level)
                    editor_action = Some(EditorAction::YankIndex {
                        position: position.clone(),
                        index,
                    });
                }
                ModeAction::MoveCursor(row, col) => {
                    // Convert window coordinates to buffer position
                    let line = row as usize;
                    let column = col as usize;

                    // Make sure we don't go past the end of the buffer
                    let buffer_lines = self.buffer.buffer_len_lines();
                    let target_line = line.min(buffer_lines.saturating_sub(1));

                    // Get line start and make sure column is within line bounds
                    let line_start = self.buffer.buffer_line_to_char(target_line);
                    let line_len = if target_line < buffer_lines {
                        self.buffer.buffer_line(target_line).len().saturating_sub(1)
                    // -1 for newline
                    } else {
                        0
                    };
                    let target_column = column.min(line_len);

                    let new_pos = line_start + target_column;
                    new_cursor_pos = Some(new_pos);

                    dirty_regions.push(DirtyRegion::Buffer {
                        buffer_id: self.buffer_id,
                    });
                }
                ModeAction::EvalJulia(expression) => {
                    if let Some(ref julia_runtime) = self.julia_runtime {
                        let result = {
                            let runtime = julia_runtime.lock().await;
                            runtime.eval_expression(&expression).await
                        };

                        let formatted_output = match result {
                            Ok(output) => format!("{output}\njulia> "),
                            Err(e) => format!("Error: {e}\njulia> "),
                        };

                        let buffer_len = self.buffer.buffer_len_chars();
                        let output_len = formatted_output.len();
                        self.buffer.insert_pos(formatted_output, buffer_len);
                        new_cursor_pos = Some(buffer_len + output_len);
                        dirty_regions.push(DirtyRegion::Buffer {
                            buffer_id: self.buffer_id,
                        });
                    } else {
                        let error_msg = "Error: Julia runtime not available\njulia> ";
                        let buffer_len = self.buffer.buffer_len_chars();
                        self.buffer.insert_pos(error_msg.to_string(), buffer_len);
                        new_cursor_pos = Some(buffer_len + error_msg.len());
                        dirty_regions.push(DirtyRegion::Buffer {
                            buffer_id: self.buffer_id,
                        });
                    }
                }
                ModeAction::UpdateIsearch {
                    target_buffer_id,
                    target_window_id,
                    matches,
                    current_match,
                } => {
                    editor_action = Some(EditorAction::UpdateIsearch {
                        target_buffer_id,
                        target_window_id,
                        matches,
                        current_match,
                    });
                }
                ModeAction::AcceptIsearch {
                    target_buffer_id,
                    search_term,
                } => {
                    editor_action = Some(EditorAction::AcceptIsearch {
                        target_buffer_id,
                        search_term,
                    });
                }
                ModeAction::CancelIsearch {
                    target_buffer_id,
                    target_window_id,
                    original_cursor,
                } => {
                    editor_action = Some(EditorAction::CancelIsearch {
                        target_buffer_id,
                        target_window_id,
                        original_cursor,
                    });
                }
                _ => {}
            }
        }

        if !dirty_regions.is_empty()
            || new_cursor_pos.is_some()
            || editor_action.is_some()
            || buffer_changed
        {
            // Create buffer change info if content was modified
            let buffer_change = if buffer_changed {
                // Use full buffer range since we're re-highlighting everything anyway
                let buffer_len = self.buffer.buffer_len_chars();
                Some(BufferChange {
                    start: 0,
                    old_end: buffer_len,
                    new_end: buffer_len,
                })
            } else {
                None
            };

            BufferResponse::ActionsCompleted {
                dirty_regions,
                new_cursor_pos,
                editor_action,
                buffer_change,
            }
        } else {
            BufferResponse::NoChange
        }
    }

    /// Get current buffer state
    fn get_state(&self) -> BufferResponse {
        BufferResponse::NoChange // State queries don't change anything
    }

    /// Save buffer to file
    async fn save_buffer(&self) -> BufferResponse {
        let file_path = self.buffer.object();

        let content = self.buffer.with_read(|b| b.buffer.to_string());

        match tokio::fs::write(&file_path, content.as_bytes()).await {
            Ok(()) => BufferResponse::Saved(file_path),
            Err(e) => BufferResponse::Error(format!("Save failed: {e}")),
        }
    }

    /// Load buffer from file
    async fn load_buffer(&mut self, file_path: String) -> BufferResponse {
        match Buffer::from_file(&file_path, &[]).await {
            Ok(new_buffer) => {
                // Replace the shared buffer content
                self.buffer.with_write(|inner| {
                    new_buffer.with_read(|new_inner| {
                        inner.object = new_inner.object.clone();
                        inner.modes = new_inner.modes.clone();
                        inner.buffer = new_inner.buffer.clone();
                        inner.mark = new_inner.mark;
                    });
                });
                BufferResponse::Loaded(file_path)
            }
            Err(e) => BufferResponse::Error(format!("Load failed: {e}")),
        }
    }
}

/// Create a BufferHost and its client
pub fn create_buffer_host(
    buffer: Buffer,
    modes: Vec<(ModeId, String, Box<dyn Mode>)>,
    buffer_id: BufferId,
    julia_runtime: Option<
        std::sync::Arc<tokio::sync::Mutex<crate::julia_runtime::RoeJuliaRuntime>>,
    >,
) -> (BufferHostClient, tokio::task::JoinHandle<()>) {
    let (sender, receiver) = mpsc::channel(100);

    let client = BufferHostClient::new(sender, buffer_id);
    let host = BufferHost::new(buffer, modes, receiver, buffer_id, julia_runtime);
    let handle = host.spawn();

    (client, handle)
}
