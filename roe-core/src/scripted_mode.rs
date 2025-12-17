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

//! Scripted mode implementation
//!
//! This module provides a Mode implementation that delegates all key handling
//! to Julia code. Any mode can be implemented this way - file-selector, etc.

use crate::editor::OpenType;
use crate::julia_runtime::{JuliaModeAction, JuliaModeResult, SharedJuliaRuntime};
use crate::keys::{CursorDirection, KeyAction};
use crate::mode::{ActionPosition, Mode, ModeAction, ModeResult};
use crate::BufferId;
use std::collections::HashMap;
use std::path::PathBuf;

/// A Mode that delegates key handling to a Julia script
///
/// This allows modes to be implemented in Julia rather than Rust.
/// The mode name corresponds to a registered Julia mode handler.
pub struct ScriptedMode {
    /// Name of this mode (matches Julia mode registration)
    mode_name: String,
    /// Julia runtime handle
    runtime: SharedJuliaRuntime,
    /// Parameters to pass on first/init call
    init_params: HashMap<String, String>,
    /// Whether we've done first call yet
    initialized: bool,
    /// Mapping from index to BufferId for buffer switching
    buffer_id_map: Vec<BufferId>,
}

impl ScriptedMode {
    pub fn new(mode_name: String, runtime: SharedJuliaRuntime) -> Self {
        Self {
            mode_name,
            runtime,
            init_params: HashMap::new(),
            initialized: false,
            buffer_id_map: Vec::new(),
        }
    }

    /// Set a parameter to pass to Julia on initialization
    pub fn set_init_param(&mut self, key: &str, value: &str) {
        self.init_params.insert(key.to_string(), value.to_string());
    }

    /// Set the buffer ID mapping for buffer switch modes
    /// The mapping allows Julia to return an index which gets converted to a BufferId
    pub fn set_buffer_id_map(&mut self, map: Vec<BufferId>) {
        self.buffer_id_map = map;
    }

    /// Convert KeyAction to a dictionary representation for Julia
    fn key_action_to_dict(action: &KeyAction) -> HashMap<String, String> {
        let mut dict = HashMap::new();

        match action {
            KeyAction::AlphaNumeric(c) => {
                dict.insert("type".to_string(), "alphanumeric".to_string());
                dict.insert("char".to_string(), c.to_string());
            }
            KeyAction::Cursor(dir) => {
                dict.insert("type".to_string(), "cursor".to_string());
                dict.insert("direction".to_string(), cursor_direction_str(dir));
            }
            KeyAction::Delete => {
                dict.insert("type".to_string(), "delete".to_string());
            }
            KeyAction::Backspace => {
                dict.insert("type".to_string(), "backspace".to_string());
            }
            KeyAction::Enter => {
                dict.insert("type".to_string(), "enter".to_string());
            }
            KeyAction::Tab => {
                dict.insert("type".to_string(), "tab".to_string());
            }
            KeyAction::Escape => {
                dict.insert("type".to_string(), "escape".to_string());
            }
            KeyAction::Cancel => {
                dict.insert("type".to_string(), "cancel".to_string());
            }
            KeyAction::Command(cmd) => {
                dict.insert("type".to_string(), "command".to_string());
                dict.insert("name".to_string(), cmd.clone());
            }
            _ => {
                dict.insert("type".to_string(), "other".to_string());
                dict.insert("debug".to_string(), format!("{:?}", action));
            }
        }

        dict
    }

    /// Convert Julia mode result to Rust ModeResult
    fn convert_result(&self, result: JuliaModeResult) -> ModeResult {
        let actions: Vec<ModeAction> = result
            .actions
            .into_iter()
            .filter_map(|a| self.convert_action(a))
            .collect();

        match result.result_type.as_str() {
            "consumed" => ModeResult::Consumed(actions),
            "annotated" => ModeResult::Annotated(actions),
            _ => ModeResult::Ignored,
        }
    }

    /// Convert a single Julia mode action to Rust ModeAction
    fn convert_action(&self, action: JuliaModeAction) -> Option<ModeAction> {
        match action.action_type.as_str() {
            "clear_text" => Some(ModeAction::ClearText),
            "insert_text" => {
                let text = action.text.unwrap_or_default();
                let position = match action.position.as_deref() {
                    Some("start") => ActionPosition::Absolute(0, 0),
                    Some("end") => ActionPosition::End,
                    _ => ActionPosition::Cursor,
                };
                Some(ModeAction::InsertText(position, text))
            }
            "open_file" => {
                let path = PathBuf::from(action.path.unwrap_or_default());
                let open_type = match action.open_type.as_deref() {
                    Some("visit") => OpenType::Visit,
                    _ => OpenType::New,
                };
                Some(ModeAction::OpenFile { path, open_type })
            }
            "execute_command" => {
                let name = action.command.unwrap_or_default();
                Some(ModeAction::ExecuteCommand(name))
            }
            "switch_buffer" => {
                // Julia returns a buffer index, convert to BufferId using our mapping
                let index = action.buffer_index.unwrap_or(0) as usize;
                if index < self.buffer_id_map.len() {
                    Some(ModeAction::SwitchToBuffer(self.buffer_id_map[index]))
                } else {
                    None
                }
            }
            "kill_buffer" => {
                // Julia returns a buffer index, convert to BufferId using our mapping
                let index = action.buffer_index.unwrap_or(0) as usize;
                if index < self.buffer_id_map.len() {
                    Some(ModeAction::KillBuffer(self.buffer_id_map[index]))
                } else {
                    None
                }
            }
            "cursor_up" => Some(ModeAction::CursorUp),
            "cursor_down" => Some(ModeAction::CursorDown),
            "cursor_left" => Some(ModeAction::CursorLeft),
            "cursor_right" => Some(ModeAction::CursorRight),
            _ => None,
        }
    }
}

fn cursor_direction_str(dir: &CursorDirection) -> String {
    match dir {
        CursorDirection::Left => "left",
        CursorDirection::Right => "right",
        CursorDirection::Up => "up",
        CursorDirection::Down => "down",
        CursorDirection::LineEnd => "line_end",
        CursorDirection::LineStart => "line_start",
        CursorDirection::BufferStart => "buffer_start",
        CursorDirection::BufferEnd => "buffer_end",
        CursorDirection::PageUp => "page_up",
        CursorDirection::PageDown => "page_down",
        CursorDirection::WordForward => "word_forward",
        CursorDirection::WordBackward => "word_backward",
        CursorDirection::ParagraphForward => "paragraph_forward",
        CursorDirection::ParagraphBackward => "paragraph_backward",
    }
    .to_string()
}

impl Mode for ScriptedMode {
    fn name(&self) -> &str {
        &self.mode_name
    }

    fn perform(&mut self, action: &KeyAction) -> ModeResult {
        let mut action_dict = Self::key_action_to_dict(action);

        // On first call, add init params and mark as init action
        if !self.initialized {
            self.initialized = true;
            // Merge init_params into action_dict
            for (k, v) in &self.init_params {
                action_dict.insert(k.clone(), v.clone());
            }
            // Always make the first call an "init" - store original action for Julia to handle
            let original_type = action_dict.get("type").cloned().unwrap_or_default();
            action_dict.insert("original_type".to_string(), original_type);
            action_dict.insert("type".to_string(), "init".to_string());
        }

        let mode_name = self.mode_name.clone();
        let runtime = self.runtime.clone();

        // Use block_in_place to safely run blocking code from async context
        let result = tokio::task::block_in_place(|| {
            let runtime_guard = runtime.blocking_lock();
            runtime_guard.call_mode_perform(&mode_name, action_dict)
        });

        match result {
            Ok(result) => self.convert_result(result),
            Err(e) => {
                eprintln!("Scripted mode error: {}", e);
                ModeResult::Ignored
            }
        }
    }
}
