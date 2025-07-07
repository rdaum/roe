use crate::buffer::Buffer;
use crate::keys::KeyAction;
use crate::mode::{Mode, ModeResult, ModeAction, ActionPosition};
use crate::renderer::DirtyRegion;
use crate::{BufferId, ModeId};
use tokio::sync::{mpsc, oneshot};

/// Message sent to a mode actor
pub struct ModeMessage {
    pub action: KeyAction,
    pub reply: oneshot::Sender<ModeResult>,
}

/// Persistent mode actor that runs in its own task
pub struct ModeActor {
    mode_impl: Box<dyn Mode>,
    receiver: mpsc::Receiver<ModeMessage>,
    buffer: Buffer, // Shared buffer access
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
                let result = self.mode_impl.perform(&message.action);
                
                // Send reply (ignore if receiver dropped)
                let _ = message.reply.send(result);
            }
        })
    }
}

/// Client handle to communicate with a mode actor
pub struct ModeClient {
    sender: mpsc::Sender<ModeMessage>,
    mode_id: ModeId,
    name: String,
}

impl ModeClient {
    pub fn new(sender: mpsc::Sender<ModeMessage>, mode_id: ModeId, name: String) -> Self {
        Self { sender, mode_id, name }
    }
    
    /// Send a key action to the mode and wait for response
    pub async fn handle_key(&self, action: KeyAction) -> Result<ModeResult, String> {
        let (reply_tx, reply_rx) = oneshot::channel();
        let message = ModeMessage {
            action,
            reply: reply_tx,
        };
        
        self.sender.send(message).await
            .map_err(|_| format!("Mode {} disconnected", self.name))?;
            
        reply_rx.await
            .map_err(|_| format!("Mode {} reply failed", self.name))
    }
    
    pub fn mode_id(&self) -> ModeId {
        self.mode_id
    }
    
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// Request sent to BufferHost
#[derive(Debug)]
pub enum BufferRequest {
    /// Process a keystroke through the mode chain
    HandleKey { action: KeyAction, cursor_pos: usize },
    /// Get current buffer state
    GetState,
    /// Save buffer to file
    Save,
    /// Load buffer from file
    Load(String),
}

/// Response from BufferHost
#[derive(Debug, Clone)]
pub enum BufferResponse {
    /// Actions completed with regions that need redrawing
    ActionsCompleted {
        dirty_regions: Vec<DirtyRegion>,
        new_cursor_pos: Option<usize>, // None means cursor didn't move
        command_to_execute: Option<String>, // Command to execute at Editor level
        buffer_to_switch: Option<crate::BufferId>, // Buffer to switch to at Editor level
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
    buffer_id: BufferId,
}

impl BufferHostClient {
    pub fn new(sender: mpsc::Sender<BufferMessage>, buffer_id: BufferId) -> Self {
        Self { sender, buffer_id }
    }
    
    /// Send a key event to the buffer
    pub async fn handle_key(&self, key: KeyAction, cursor_pos: usize) -> Result<BufferResponse, String> {
        let (reply_tx, reply_rx) = oneshot::channel();
        let message = BufferMessage {
            request: BufferRequest::HandleKey { action: key, cursor_pos },
            reply: reply_tx,
        };
        
        self.sender.send(message).await
            .map_err(|_| "BufferHost disconnected".to_string())?;
            
        reply_rx.await
            .map_err(|_| "BufferHost reply failed".to_string())
    }
    
    /// Get current buffer state
    pub async fn get_state(&self) -> Result<BufferResponse, String> {
        let (reply_tx, reply_rx) = oneshot::channel();
        let message = BufferMessage {
            request: BufferRequest::GetState,
            reply: reply_tx,
        };
        
        self.sender.send(message).await
            .map_err(|_| "BufferHost disconnected".to_string())?;
            
        reply_rx.await
            .map_err(|_| "BufferHost reply failed".to_string())
    }
    
    /// Save buffer to file
    pub async fn save(&self) -> Result<BufferResponse, String> {
        let (reply_tx, reply_rx) = oneshot::channel();
        let message = BufferMessage {
            request: BufferRequest::Save,
            reply: reply_tx,
        };
        
        self.sender.send(message).await
            .map_err(|_| "BufferHost disconnected".to_string())?;
            
        reply_rx.await
            .map_err(|_| "BufferHost reply failed".to_string())
    }
    
    pub fn buffer_id(&self) -> BufferId {
        self.buffer_id
    }
}

/// The BufferHost that processes requests and coordinates with mode actors
pub struct BufferHost {
    buffer: Buffer,
    mode_clients: Vec<ModeClient>,
    receiver: mpsc::Receiver<BufferMessage>,
    buffer_id: BufferId,
    mode_handles: Vec<tokio::task::JoinHandle<()>>, // Keep track of spawned mode tasks
}

impl BufferHost {
    pub fn new(
        buffer: Buffer,
        modes: Vec<(ModeId, String, Box<dyn Mode>)>,
        receiver: mpsc::Receiver<BufferMessage>,
        buffer_id: BufferId,
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
            BufferRequest::GetState => {
                self.get_state()
            }
            BufferRequest::Save => {
                self.save_buffer().await
            }
            BufferRequest::Load(file_path) => {
                self.load_buffer(file_path).await
            }
        }
    }
    
    /// Process keystroke through mode chain sequentially
    async fn handle_key_action(&mut self, key_action: KeyAction, cursor_pos: usize) -> BufferResponse {
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
    
    /// Execute mode actions and return dirty regions
    async fn execute_actions(&mut self, actions: Vec<ModeAction>, mut cursor_pos: usize) -> BufferResponse {
        let mut dirty_regions = vec![];
        let mut new_cursor_pos = None;
        let mut command_to_execute = None;
        let mut buffer_to_switch = None;
        
        for action in actions {
            match action {
                ModeAction::InsertText(pos, text) => {
                    match pos {
                        ActionPosition::Cursor => {
                            let has_newline = text.contains('\n');
                            self.buffer.insert_pos(text.clone(), cursor_pos);
                            
                            // Advance the cursor
                            cursor_pos += text.len();
                            new_cursor_pos = Some(cursor_pos);
                            
                            // Mark appropriate dirty regions
                            if has_newline {
                                dirty_regions.push(DirtyRegion::Buffer { buffer_id: self.buffer_id });
                            } else {
                                let line = self.buffer.to_column_line(cursor_pos).1 as usize;
                                dirty_regions.push(DirtyRegion::Line { buffer_id: self.buffer_id, line });
                            }
                        }
                        ActionPosition::Absolute(col, row) => {
                            let has_newline = text.contains('\n');
                            self.buffer.insert_col_line(text.clone(), (col, row));
                            
                            // For command mode, position cursor at end of user input line
                            if let Some(newline_pos) = text.find('\n') {
                                let first_line = &text[..newline_pos];
                                // Position cursor at the end of the first line (user input)
                                new_cursor_pos = Some(first_line.len());
                            } else {
                                // Single line case - position at the end
                                new_cursor_pos = Some(text.len());
                            }
                            
                            // Mark appropriate dirty regions
                            if has_newline {
                                dirty_regions.push(DirtyRegion::Buffer { buffer_id: self.buffer_id });
                            } else {
                                dirty_regions.push(DirtyRegion::Line { buffer_id: self.buffer_id, line: row as usize });
                            }
                        }
                        ActionPosition::End => {
                            // Insert at end of buffer
                            let buffer_len = self.buffer.buffer_len_chars();
                            let has_newline = text.contains('\n');
                            self.buffer.insert_pos(text.clone(), buffer_len);
                            
                            if has_newline {
                                dirty_regions.push(DirtyRegion::Buffer { buffer_id: self.buffer_id });
                            } else {
                                let line = self.buffer.to_column_line(buffer_len).1 as usize;
                                dirty_regions.push(DirtyRegion::Line { buffer_id: self.buffer_id, line });
                            }
                        }
                    }
                }
                ModeAction::DeleteText(pos, count) => {
                    match pos {
                        ActionPosition::Cursor => {
                            if let Some(deleted) = self.buffer.delete_pos(cursor_pos, count) {
                                // Check if deleted text contains newlines
                                let has_newline = deleted.contains('\n');
                                
                                if has_newline {
                                    // Newlines affect multiple lines, mark entire buffer dirty
                                    dirty_regions.push(DirtyRegion::Buffer { buffer_id: self.buffer_id });
                                } else {
                                    // Simple text deletion, only current line affected
                                    let line = self.buffer.to_column_line(cursor_pos).1 as usize;
                                    dirty_regions.push(DirtyRegion::Line { buffer_id: self.buffer_id, line });
                                }
                                
                                // Update cursor if we deleted backwards
                                if count < 0 {
                                    cursor_pos = cursor_pos.saturating_sub(count.abs() as usize);
                                    new_cursor_pos = Some(cursor_pos);
                                }
                            }
                        }
                        ActionPosition::Absolute(col, row) => {
                            if let Some(deleted) = self.buffer.delete_col_line((col, row), count) {
                                // Check if deleted text contains newlines
                                let has_newline = deleted.contains('\n');
                                
                                if has_newline {
                                    // Newlines affect multiple lines, mark entire buffer dirty
                                    dirty_regions.push(DirtyRegion::Buffer { buffer_id: self.buffer_id });
                                } else {
                                    // Simple text deletion, only current line affected
                                    dirty_regions.push(DirtyRegion::Line { buffer_id: self.buffer_id, line: row as usize });
                                }
                            }
                        }
                        ActionPosition::End => {
                            // Delete from end of buffer backwards
                            let buffer_len = self.buffer.buffer_len_chars();
                            if let Some(deleted) = self.buffer.delete_pos(buffer_len, count) {
                                let has_newline = deleted.contains('\n');
                                
                                if has_newline {
                                    dirty_regions.push(DirtyRegion::Buffer { buffer_id: self.buffer_id });
                                } else {
                                    let line = self.buffer.to_column_line(buffer_len).1 as usize;
                                    dirty_regions.push(DirtyRegion::Line { buffer_id: self.buffer_id, line });
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
                        dirty_regions.push(DirtyRegion::Buffer { buffer_id: self.buffer_id });
                        new_cursor_pos = Some(0); // Move cursor to start
                    }
                }
                ModeAction::SetMark => {
                    self.buffer.set_mark(cursor_pos);
                    // Mark highlighting might change
                    dirty_regions.push(DirtyRegion::Buffer { buffer_id: self.buffer_id });
                }
                ModeAction::ClearMark => {
                    self.buffer.clear_mark();
                    // Mark highlighting might change
                    dirty_regions.push(DirtyRegion::Buffer { buffer_id: self.buffer_id });
                }
                ModeAction::ExecuteCommand(command_name) => {
                    // Store command for execution at Editor level
                    command_to_execute = Some(command_name);
                }
                ModeAction::SwitchToBuffer(buffer_id) => {
                    // Store buffer switch for execution at Editor level
                    buffer_to_switch = Some(buffer_id);
                }
                // TODO: Implement other actions
                _ => {}
            }
        }
        
        if !dirty_regions.is_empty() || new_cursor_pos.is_some() || command_to_execute.is_some() || buffer_to_switch.is_some() {
            BufferResponse::ActionsCompleted {
                dirty_regions,
                new_cursor_pos,
                command_to_execute,
                buffer_to_switch,
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
            Err(e) => BufferResponse::Error(format!("Save failed: {}", e)),
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
            Err(e) => BufferResponse::Error(format!("Load failed: {}", e)),
        }
    }
}

/// Create a BufferHost and its client
pub fn create_buffer_host(
    buffer: Buffer,
    modes: Vec<(ModeId, String, Box<dyn Mode>)>,
    buffer_id: BufferId,
) -> (BufferHostClient, tokio::task::JoinHandle<()>) {
    let (sender, receiver) = mpsc::channel(100);
    
    let client = BufferHostClient::new(sender, buffer_id);
    let host = BufferHost::new(buffer, modes, receiver, buffer_id);
    let handle = host.spawn();
    
    (client, handle)
}