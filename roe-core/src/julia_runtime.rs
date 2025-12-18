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

// jlrs AsyncTask trait requires this specific fn signature, not async fn
#![allow(clippy::manual_async_fn)]

use crate::buffer::Buffer;
use crate::syntax::{Color, Face, FaceRegistry, HighlightSpan};
use jlrs::memory::target::frame::GcFrame;
use jlrs::prelude::*;
use jlrs::runtime::handle::async_handle::AsyncHandle;
use std::collections::HashMap;
use std::ffi::{c_char, c_longlong, c_uchar, CStr, CString};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::JoinHandle;
use tokio::sync::{mpsc, Mutex};

/// Type alias for a shared Julia runtime
pub type SharedJuliaRuntime = Arc<Mutex<RoeJuliaRuntime>>;

// ============================================
// Buffer context for Julia command execution
// ============================================

/// Storage for the current buffer during Julia command execution
/// This is set before calling a Julia command and cleared after
static CURRENT_BUFFER: std::sync::Mutex<Option<Buffer>> = std::sync::Mutex::new(None);

/// Set the current buffer for Julia command execution
pub fn set_current_buffer(buffer: Buffer) {
    let mut guard = CURRENT_BUFFER.lock().expect("Buffer lock poisoned");
    *guard = Some(buffer);
}

/// Clear the current buffer after Julia command execution
pub fn clear_current_buffer() {
    let mut guard = CURRENT_BUFFER.lock().expect("Buffer lock poisoned");
    *guard = None;
}

/// Get a clone of the current buffer (for use in extern functions)
fn get_current_buffer() -> Option<Buffer> {
    let guard = CURRENT_BUFFER.lock().expect("Buffer lock poisoned");
    guard.clone()
}

// ============================================
// Face registry for syntax highlighting
// ============================================

/// Global face registry, initialized lazily
static FACE_REGISTRY: std::sync::OnceLock<std::sync::Mutex<FaceRegistry>> =
    std::sync::OnceLock::new();

/// Get or initialize the global face registry
fn get_face_registry() -> &'static std::sync::Mutex<FaceRegistry> {
    FACE_REGISTRY.get_or_init(|| std::sync::Mutex::new(FaceRegistry::new()))
}

/// Get the global face registry (public for use by renderers)
pub fn face_registry() -> &'static std::sync::Mutex<FaceRegistry> {
    get_face_registry()
}

// ============================================
// Extern "C" functions callable from Julia
// ============================================

/// Get the entire buffer content as a string
/// Returns a C string that Julia must free
#[no_mangle]
pub extern "C" fn roe_buffer_content() -> *mut c_char {
    let Some(buffer) = get_current_buffer() else {
        return std::ptr::null_mut();
    };
    let content = buffer.content();
    match CString::new(content) {
        Ok(cstr) => cstr.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Free a string returned by roe_buffer_content
/// # Safety
/// The pointer must have been returned by a previous call to a roe_buffer_* function.
#[no_mangle]
pub unsafe extern "C" fn roe_free_string(s: *mut c_char) {
    if !s.is_null() {
        drop(CString::from_raw(s));
    }
}

/// Get a single line from the buffer (0-indexed)
/// Returns a C string that Julia must free
#[no_mangle]
pub extern "C" fn roe_buffer_line(line_idx: c_longlong) -> *mut c_char {
    let Some(buffer) = get_current_buffer() else {
        return std::ptr::null_mut();
    };
    if line_idx < 0 {
        return std::ptr::null_mut();
    }
    let line = buffer.buffer_line(line_idx as usize);
    match CString::new(line) {
        Ok(cstr) => cstr.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Get the number of lines in the buffer
#[no_mangle]
pub extern "C" fn roe_buffer_line_count() -> c_longlong {
    let Some(buffer) = get_current_buffer() else {
        return 0;
    };
    buffer.buffer_len_lines() as c_longlong
}

/// Get the number of characters in the buffer
#[no_mangle]
pub extern "C" fn roe_buffer_char_count() -> c_longlong {
    let Some(buffer) = get_current_buffer() else {
        return 0;
    };
    buffer.buffer_len_chars() as c_longlong
}

/// Get a substring from the buffer (start inclusive, end exclusive)
/// Returns a C string that Julia must free
#[no_mangle]
pub extern "C" fn roe_buffer_substring(start: c_longlong, end: c_longlong) -> *mut c_char {
    let Some(buffer) = get_current_buffer() else {
        return std::ptr::null_mut();
    };
    if start < 0 || end < start {
        return std::ptr::null_mut();
    }

    // Get the substring via the rope
    let content = buffer.content();
    let start_idx = (start as usize).min(content.len());
    let end_idx = (end as usize).min(content.len());
    let substring = &content[start_idx..end_idx];

    match CString::new(substring) {
        Ok(cstr) => cstr.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Insert text at a position in the buffer
/// # Safety
/// The text pointer must be a valid null-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn roe_buffer_insert(pos: c_longlong, text: *const c_char) {
    let Some(buffer) = get_current_buffer() else {
        return;
    };
    if pos < 0 || text.is_null() {
        return;
    }
    let text_str = match CStr::from_ptr(text).to_str() {
        Ok(s) => s.to_string(),
        Err(_) => return,
    };
    buffer.insert_pos(text_str, pos as usize);
}

/// Delete text from the buffer (start inclusive, end exclusive)
#[no_mangle]
pub extern "C" fn roe_buffer_delete(start: c_longlong, end: c_longlong) {
    let Some(buffer) = get_current_buffer() else {
        return;
    };
    if start < 0 || end <= start {
        return;
    }
    let count = end - start;
    buffer.delete_pos(start as usize, count as isize);
}

// ============================================
// Face and syntax highlighting FFI
// ============================================

/// Define a new face with the given name and attributes.
/// Returns 1 on success, 0 on failure.
///
/// # Safety
/// All string pointers must be valid null-terminated C strings or null.
#[no_mangle]
pub unsafe extern "C" fn roe_define_face(
    name: *const c_char,
    fg_hex: *const c_char,
    bg_hex: *const c_char,
    bold: c_uchar,
    italic: c_uchar,
    underline: c_uchar,
) -> c_longlong {
    if name.is_null() {
        return 0;
    }

    let name_str = match CStr::from_ptr(name).to_str() {
        Ok(s) => s.to_string(),
        Err(_) => return 0,
    };

    let mut face = Face::new(&name_str);

    // Parse foreground color
    if !fg_hex.is_null() {
        if let Ok(hex) = CStr::from_ptr(fg_hex).to_str() {
            if let Some(color) = Color::from_hex(hex) {
                face.foreground = Some(color);
            }
        }
    }

    // Parse background color
    if !bg_hex.is_null() {
        if let Ok(hex) = CStr::from_ptr(bg_hex).to_str() {
            if let Some(color) = Color::from_hex(hex) {
                face.background = Some(color);
            }
        }
    }

    face.bold = bold != 0;
    face.italic = italic != 0;
    face.underline = underline != 0;

    let registry = get_face_registry();
    let mut guard = registry.lock().expect("Face registry lock poisoned");

    // Check if face already exists
    if let Some(existing_id) = guard.get_id(&name_str) {
        // Update existing face
        guard.update_face(existing_id, face);
    } else {
        // Define new face
        guard.define_face(face);
    }

    1 // Success
}

/// Check if a face with the given name exists.
/// Returns 1 if found, 0 if not found.
///
/// # Safety
/// The name pointer must be a valid null-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn roe_face_exists(name: *const c_char) -> c_longlong {
    if name.is_null() {
        return 0;
    }

    let name_str = match CStr::from_ptr(name).to_str() {
        Ok(s) => s,
        Err(_) => return 0,
    };

    let registry = get_face_registry();
    let guard = registry.lock().expect("Face registry lock poisoned");

    if guard.get_id(name_str).is_some() {
        1
    } else {
        0
    }
}

/// Add a highlight span to the current buffer.
/// Returns 1 on success, 0 on failure.
///
/// # Safety
/// The face_name pointer must be a valid null-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn roe_add_span(
    start: c_longlong,
    end: c_longlong,
    face_name: *const c_char,
) -> c_longlong {
    if face_name.is_null() || start < 0 || end <= start {
        return 0;
    }

    let Some(buffer) = get_current_buffer() else {
        return 0;
    };

    let face_name_str = match CStr::from_ptr(face_name).to_str() {
        Ok(s) => s,
        Err(_) => return 0,
    };

    // Look up the face ID
    let registry = get_face_registry();
    let guard = registry.lock().expect("Face registry lock poisoned");

    let Some(face_id) = guard.get_id(face_name_str) else {
        return 0; // Face not found
    };

    drop(guard); // Release lock before accessing buffer

    let span = HighlightSpan::new(start as usize, end as usize, face_id);
    buffer.add_span(span);

    1 // Success
}

/// Add multiple highlight spans to the current buffer at once.
/// Takes arrays of starts, ends, and face names.
/// Returns number of successfully added spans.
///
/// # Safety
/// All array pointers must be valid and have at least `count` elements.
/// Face name pointers must be valid null-terminated C strings.
#[no_mangle]
pub unsafe extern "C" fn roe_add_spans(
    starts: *const c_longlong,
    ends: *const c_longlong,
    face_names: *const *const c_char,
    count: c_longlong,
) -> c_longlong {
    if starts.is_null() || ends.is_null() || face_names.is_null() || count <= 0 {
        return 0;
    }

    let Some(buffer) = get_current_buffer() else {
        return 0;
    };

    let registry = get_face_registry();
    let guard = registry.lock().expect("Face registry lock poisoned");

    let mut spans = Vec::with_capacity(count as usize);
    let mut added = 0;

    for i in 0..count as isize {
        let start = *starts.offset(i);
        let end = *ends.offset(i);
        let face_name_ptr = *face_names.offset(i);

        if start < 0 || end <= start || face_name_ptr.is_null() {
            continue;
        }

        let face_name_str = match CStr::from_ptr(face_name_ptr).to_str() {
            Ok(s) => s,
            Err(_) => continue,
        };

        if let Some(face_id) = guard.get_id(face_name_str) {
            spans.push(HighlightSpan::new(start as usize, end as usize, face_id));
            added += 1;
        }
    }

    drop(guard); // Release lock before accessing buffer

    buffer.add_spans(spans);

    added
}

/// Clear all highlight spans from the current buffer.
#[no_mangle]
pub extern "C" fn roe_clear_spans() {
    if let Some(buffer) = get_current_buffer() {
        buffer.clear_spans();
    }
}

/// Clear highlight spans in a specific range from the current buffer.
#[no_mangle]
pub extern "C" fn roe_clear_spans_in_range(start: c_longlong, end: c_longlong) {
    if start < 0 || end <= start {
        return;
    }

    if let Some(buffer) = get_current_buffer() {
        buffer.clear_spans_in_range(start as usize..end as usize);
    }
}

/// Check if the current buffer has any highlight spans.
/// Returns 1 if spans exist, 0 otherwise.
#[no_mangle]
pub extern "C" fn roe_buffer_has_spans() -> c_longlong {
    match get_current_buffer() {
        Some(buffer) => {
            if buffer.has_spans() {
                1
            } else {
                0
            }
        }
        None => 0,
    }
}

/// Error types for Julia runtime operations
#[derive(Debug)]
pub enum JuliaRuntimeError {
    InitializationFailed(String),
    TaskExecutionFailed(String),
    ConfigLoadFailed(String),
    ScriptLoadFailed(String),
}

impl std::fmt::Display for JuliaRuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JuliaRuntimeError::InitializationFailed(msg) => {
                write!(f, "Julia initialization failed: {msg}")
            }
            JuliaRuntimeError::TaskExecutionFailed(msg) => {
                write!(f, "Julia task execution failed: {msg}")
            }
            JuliaRuntimeError::ConfigLoadFailed(msg) => {
                write!(f, "Julia config load failed: {msg}")
            }
            JuliaRuntimeError::ScriptLoadFailed(msg) => {
                write!(f, "Julia script load failed: {msg}")
            }
        }
    }
}

impl std::error::Error for JuliaRuntimeError {}

/// Dynamic configuration value that can hold any Julia type
#[derive(Debug, Clone)]
pub enum ConfigValue {
    String(String),
    Integer(i64),
    Float(f64),
    Boolean(bool),
    Array(Vec<ConfigValue>),
    Dict(HashMap<String, ConfigValue>),
    Symbol(String),
}

impl ConfigValue {
    /// Get as string, converting if possible
    pub fn as_string(&self) -> Option<String> {
        match self {
            ConfigValue::String(s) => Some(s.clone()),
            ConfigValue::Symbol(s) => Some(s.clone()),
            _ => None,
        }
    }

    /// Get as integer, converting if possible
    pub fn as_integer(&self) -> Option<i64> {
        match self {
            ConfigValue::Integer(i) => Some(*i),
            ConfigValue::Float(f) => Some(*f as i64),
            _ => None,
        }
    }

    /// Get as boolean
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            ConfigValue::Boolean(b) => Some(*b),
            _ => None,
        }
    }

    /// Get as dictionary
    pub fn as_dict(&self) -> Option<&HashMap<String, ConfigValue>> {
        match self {
            ConfigValue::Dict(d) => Some(d),
            _ => None,
        }
    }
}

/// Simple addition task for testing Julia integration
pub struct AdditionTask {
    a: u64,
    b: u64,
}

impl AdditionTask {
    pub fn new(a: u64, b: u64) -> Self {
        Self { a, b }
    }
}

impl AsyncTask for AdditionTask {
    type Output = JlrsResult<u64>;

    fn run(self, mut frame: AsyncGcFrame<'_>) -> impl std::future::Future<Output = Self::Output> {
        async move {
            let a = Value::new(&mut frame, self.a);
            let b = Value::new(&mut frame, self.b);
            let func = Module::base(&frame).global(&mut frame, "+")?;

            let result = unsafe { func.call_async(&mut frame, [a, b]) }.await;
            match result {
                Ok(val) => val.unbox::<u64>(),
                Err(e) => Err(e.as_value().into()),
            }
        }
    }
}

/// Task for loading .roe.jl configuration files and extracting config values
pub struct ConfigLoadTask {
    config_path: PathBuf,
}

impl ConfigLoadTask {
    pub fn new(config_path: PathBuf) -> Self {
        Self { config_path }
    }
}

impl AsyncTask for ConfigLoadTask {
    type Output = JlrsResult<HashMap<String, ConfigValue>>;

    fn run(self, mut frame: AsyncGcFrame<'_>) -> impl std::future::Future<Output = Self::Output> {
        async move {
            frame.scope(|mut frame| {
                // Read the Julia file content
                let content = std::fs::read_to_string(&self.config_path).unwrap_or_default();
                if content.is_empty() {
                    return Ok(HashMap::new());
                }

                // Execute the Julia code to define roe_config
                let Ok(_) = (unsafe { Value::eval_string(&mut frame, &content) }) else {
                    return Ok(HashMap::new());
                };

                // Try to get roe_config from Main
                let Ok(_) = Module::main(&frame).global(&mut frame, "roe_config") else {
                    return Ok(HashMap::new());
                };

                // Return a test value to show it worked
                let mut config_map = HashMap::new();
                config_map.insert("_loaded".to_string(), ConfigValue::Boolean(true));
                Ok(config_map)
            })
        }
    }
}

/// Task for executing Julia expressions in REPL mode
pub struct JuliaReplTask {
    expression: String,
}

impl JuliaReplTask {
    pub fn new(expression: String) -> Self {
        Self { expression }
    }
}

impl AsyncTask for JuliaReplTask {
    type Output = JlrsResult<String>;

    fn run(self, mut frame: AsyncGcFrame<'_>) -> impl std::future::Future<Output = Self::Output> {
        async move {
            frame.scope(|mut frame| {
                // Execute the Julia expression
                let result = unsafe { Value::eval_string(&mut frame, &self.expression) };

                match result {
                    Ok(value) => {
                        // Convert result to string representation
                        let string_func = Module::base(&frame).global(&mut frame, "string")?;
                        let string_result = match unsafe { string_func.call(&mut frame, [value]) } {
                            Ok(result) => result,
                            Err(_) => {
                                return Ok("(result could not be converted to string)".to_string())
                            }
                        };

                        if let Ok(julia_string) = string_result.cast::<JuliaString>() {
                            if let Ok(rust_string) = julia_string.as_str() {
                                return Ok(rust_string.to_string());
                            }
                        }

                        Ok("(result could not be converted to string)".to_string())
                    }
                    Err(e) => {
                        // Return error message as string
                        Ok(format!("Error: {e:?}"))
                    }
                }
            })
        }
    }
}

/// Task for querying configuration values from persistent Julia runtime
pub struct ConfigQueryTask {
    key: String,
}

impl ConfigQueryTask {
    pub fn new(key: String) -> Self {
        Self { key }
    }

    fn handle_nested_key<'frame>(
        &self,
        frame: &mut GcFrame<'frame>,
        config_dict: Value<'_, 'frame>,
    ) -> JlrsResult<Option<ConfigValue>> {
        let parts: Vec<&str> = self.key.split('.').collect();
        if parts.len() != 2 {
            return Ok(None);
        }

        let getindex = Module::base(frame).global(&mut *frame, "getindex")?;

        // Get first level: getindex(config_dict, "colors")
        let first_key = JuliaString::new(&mut *frame, parts[0]);
        let Ok(first_value) =
            (unsafe { getindex.call(&mut *frame, [config_dict, first_key.as_value()]) })
        else {
            return Ok(None);
        };

        // Get second level: getindex(colors_dict, "background")
        let second_key = JuliaString::new(&mut *frame, parts[1]);
        let Ok(final_value) =
            (unsafe { getindex.call(&mut *frame, [first_value, second_key.as_value()]) })
        else {
            return Ok(None);
        };

        // Try to convert based on Julia type - string first
        if let Ok(julia_string) = final_value.cast::<JuliaString>() {
            if let Ok(rust_string) = julia_string.as_str() {
                return Ok(Some(ConfigValue::String(rust_string.to_string())));
            }
        }

        // Try integer
        if let Ok(int_val) = final_value.unbox::<i64>() {
            return Ok(Some(ConfigValue::Integer(int_val)));
        }

        // Try boolean - Julia Bool to Rust bool
        if let Ok(bool_val) = final_value.unbox::<Bool>() {
            return Ok(Some(ConfigValue::Boolean(bool_val.as_bool())));
        }

        Ok(None)
    }

    fn handle_top_level_key<'frame>(
        &self,
        frame: &mut GcFrame<'frame>,
        config_dict: Value<'_, 'frame>,
    ) -> JlrsResult<Option<ConfigValue>> {
        let getindex = Module::base(frame).global(&mut *frame, "getindex")?;
        let key_str = JuliaString::new(&mut *frame, &self.key);

        let Ok(value) = (unsafe { getindex.call(&mut *frame, [config_dict, key_str.as_value()]) })
        else {
            return Ok(None);
        };

        // Try to convert based on Julia type - string first
        if let Ok(julia_string) = value.cast::<JuliaString>() {
            if let Ok(rust_string) = julia_string.as_str() {
                return Ok(Some(ConfigValue::String(rust_string.to_string())));
            }
        }

        // Try integer
        if let Ok(int_val) = value.unbox::<i64>() {
            return Ok(Some(ConfigValue::Integer(int_val)));
        }

        // Try boolean - Julia Bool to Rust bool
        if let Ok(bool_val) = value.unbox::<Bool>() {
            return Ok(Some(ConfigValue::Boolean(bool_val.as_bool())));
        }

        Ok(None)
    }
}

impl AsyncTask for ConfigQueryTask {
    type Output = JlrsResult<Option<ConfigValue>>;

    fn run(self, mut frame: AsyncGcFrame<'_>) -> impl std::future::Future<Output = Self::Output> {
        async move {
            frame.scope(|mut frame| {
                let main_module = Module::main(&frame);

                // Get roe_config global or return None
                let Ok(config_dict) = main_module.global(&mut frame, "roe_config") else {
                    return Ok(None);
                };

                if self.key.contains('.') {
                    self.handle_nested_key(&mut frame, config_dict)
                } else {
                    self.handle_top_level_key(&mut frame, config_dict)
                }
            })
        }
    }
}

/// Task for loading the Roe Julia module
pub struct LoadRoeModuleTask {
    module_path: PathBuf,
}

impl LoadRoeModuleTask {
    pub fn new(module_path: PathBuf) -> Self {
        Self { module_path }
    }
}

impl AsyncTask for LoadRoeModuleTask {
    type Output = JlrsResult<Result<(), String>>;

    fn run(self, mut frame: AsyncGcFrame<'_>) -> impl std::future::Future<Output = Self::Output> {
        async move {
            frame.scope(|mut frame| {
                // Make path absolute if it isn't already
                let abs_path = if self.module_path.is_absolute() {
                    self.module_path.clone()
                } else {
                    std::env::current_dir()
                        .map(|cwd| cwd.join(&self.module_path))
                        .unwrap_or(self.module_path.clone())
                };

                // Include the Roe module
                let include_code = format!("include({:?})", abs_path.to_string_lossy());
                let result = unsafe { Value::eval_string(&mut frame, &include_code) };

                match result {
                    Ok(_) => {
                        // Bring Roe module into scope
                        let using_result = unsafe { Value::eval_string(&mut frame, "using .Roe") };
                        match using_result {
                            Ok(_) => Ok(Ok(())),
                            Err(e) => Ok(Err(format!("Failed 'using .Roe': {:?}", e))),
                        }
                    }
                    Err(e) => Ok(Err(format!("Failed to include {:?}: {:?}", abs_path, e))),
                }
            })
        }
    }
}

/// Task for calling a Julia command
pub struct CallCommandTask {
    command_name: String,
    context: JuliaCommandContext,
}

impl CallCommandTask {
    pub fn new(command_name: String, context: JuliaCommandContext) -> Self {
        Self {
            command_name,
            context,
        }
    }

    /// Helper to extract a string field from a Julia Dict
    fn get_string_field<'target>(
        frame: &mut GcFrame<'target>,
        getindex: &Value,
        dict: Value,
        key: &str,
    ) -> Option<String> {
        let key_val = JuliaString::new(&mut *frame, key);
        let result = unsafe { getindex.call(&mut *frame, [dict, key_val.as_value()]) };
        if let Ok(val) = result {
            if let Ok(js) = val.cast::<JuliaString>() {
                return Some(js.as_str().unwrap_or("").to_string());
            }
        }
        None
    }

    /// Helper to extract an integer field from a Julia Dict
    fn get_int_field<'target>(
        frame: &mut GcFrame<'target>,
        getindex: &Value,
        dict: Value,
        key: &str,
    ) -> Option<i64> {
        let key_val = JuliaString::new(&mut *frame, key);
        let result = unsafe { getindex.call(&mut *frame, [dict, key_val.as_value()]) };
        if let Ok(val) = result {
            if let Ok(n) = val.unbox::<i64>() {
                return Some(n);
            }
        }
        None
    }
}

impl AsyncTask for CallCommandTask {
    type Output = JlrsResult<JuliaCommandResult>;

    fn run(self, mut frame: AsyncGcFrame<'_>) -> impl std::future::Future<Output = Self::Output> {
        async move {
            frame.scope(|mut frame| {
                // Get the Roe module and call_command function
                let main_module = Module::main(&frame);

                // Get the Roe module
                let Ok(roe_module) = main_module.global(&mut frame, "Roe") else {
                    return Ok(JuliaCommandResult::Error(
                        "Roe module not loaded".to_string(),
                    ));
                };

                // Get call_command function
                let Ok(call_command_fn) = roe_module
                    .cast::<Module>()
                    .unwrap()
                    .global(&mut frame, "call_command")
                else {
                    return Ok(JuliaCommandResult::Error(
                        "call_command function not found".to_string(),
                    ));
                };

                // Build Dict using comprehension-style eval
                let dict_code = format!(
                    r#"Dict(
                        "buffer_name" => {:?},
                        "buffer_modified" => {},
                        "cursor_pos" => {},
                        "current_line" => {},
                        "current_column" => {},
                        "line_count" => {},
                        "char_count" => {},
                        "mark_pos" => {}
                    )"#,
                    self.context.buffer_name,
                    self.context.buffer_modified,
                    self.context.cursor_pos,
                    self.context.current_line,
                    self.context.current_column,
                    self.context.line_count,
                    self.context.char_count,
                    self.context.mark_pos
                );

                let Ok(context_dict) = (unsafe { Value::eval_string(&mut frame, &dict_code) })
                else {
                    return Ok(JuliaCommandResult::Error(
                        "Failed to create context Dict".to_string(),
                    ));
                };

                // Call Roe.call_command(name, context)
                let command_name_val = JuliaString::new(&mut frame, &self.command_name);

                let result = unsafe {
                    call_command_fn.call(&mut frame, [command_name_val.as_value(), context_dict])
                };

                match result {
                    Ok(result_dict) => {
                        // Extract the result type and message
                        let getindex = Module::base(&frame).global(&mut frame, "getindex")?;
                        let type_key = JuliaString::new(&mut frame, "type");

                        let Ok(type_val) = (unsafe {
                            getindex.call(&mut frame, [result_dict, type_key.as_value()])
                        }) else {
                            return Ok(JuliaCommandResult::Error(
                                "Missing 'type' in result".to_string(),
                            ));
                        };

                        let type_str = if let Ok(js) = type_val.cast::<JuliaString>() {
                            js.as_str().unwrap_or("unknown").to_string()
                        } else {
                            "unknown".to_string()
                        };

                        match type_str.as_str() {
                            "echo" => {
                                let message_key = JuliaString::new(&mut frame, "message");
                                if let Ok(msg_val) = unsafe {
                                    getindex.call(&mut frame, [result_dict, message_key.as_value()])
                                } {
                                    if let Ok(js) = msg_val.cast::<JuliaString>() {
                                        let msg = js.as_str().unwrap_or("").to_string();
                                        return Ok(JuliaCommandResult::Echo(msg));
                                    }
                                }
                                Ok(JuliaCommandResult::Echo("".to_string()))
                            }
                            "error" => {
                                let message_key = JuliaString::new(&mut frame, "message");
                                if let Ok(msg_val) = unsafe {
                                    getindex.call(&mut frame, [result_dict, message_key.as_value()])
                                } {
                                    if let Ok(js) = msg_val.cast::<JuliaString>() {
                                        let msg =
                                            js.as_str().unwrap_or("Unknown error").to_string();
                                        return Ok(JuliaCommandResult::Error(msg));
                                    }
                                }
                                Ok(JuliaCommandResult::Error("Unknown error".to_string()))
                            }
                            "none" => Ok(JuliaCommandResult::None),
                            // Buffer operations
                            "insert" => {
                                let pos =
                                    Self::get_int_field(&mut frame, &getindex, result_dict, "pos")
                                        .unwrap_or(0) as usize;
                                let text = Self::get_string_field(
                                    &mut frame,
                                    &getindex,
                                    result_dict,
                                    "text",
                                )
                                .unwrap_or_default();
                                Ok(JuliaCommandResult::BufferOps(vec![JuliaBufferOp::Insert {
                                    pos,
                                    text,
                                }]))
                            }
                            "delete" => {
                                let start = Self::get_int_field(
                                    &mut frame,
                                    &getindex,
                                    result_dict,
                                    "start",
                                )
                                .unwrap_or(0) as usize;
                                let end =
                                    Self::get_int_field(&mut frame, &getindex, result_dict, "end")
                                        .unwrap_or(0) as usize;
                                Ok(JuliaCommandResult::BufferOps(vec![JuliaBufferOp::Delete {
                                    start,
                                    end,
                                }]))
                            }
                            "replace" => {
                                let start = Self::get_int_field(
                                    &mut frame,
                                    &getindex,
                                    result_dict,
                                    "start",
                                )
                                .unwrap_or(0) as usize;
                                let end =
                                    Self::get_int_field(&mut frame, &getindex, result_dict, "end")
                                        .unwrap_or(0) as usize;
                                let text = Self::get_string_field(
                                    &mut frame,
                                    &getindex,
                                    result_dict,
                                    "text",
                                )
                                .unwrap_or_default();
                                Ok(JuliaCommandResult::BufferOps(vec![
                                    JuliaBufferOp::Replace { start, end, text },
                                ]))
                            }
                            "set_cursor" => {
                                let pos =
                                    Self::get_int_field(&mut frame, &getindex, result_dict, "pos")
                                        .unwrap_or(0) as usize;
                                Ok(JuliaCommandResult::BufferOps(vec![
                                    JuliaBufferOp::SetCursor(pos),
                                ]))
                            }
                            "set_mark" => {
                                let pos =
                                    Self::get_int_field(&mut frame, &getindex, result_dict, "pos")
                                        .unwrap_or(0) as usize;
                                Ok(JuliaCommandResult::BufferOps(vec![JuliaBufferOp::SetMark(
                                    pos,
                                )]))
                            }
                            "clear_mark" => Ok(JuliaCommandResult::BufferOps(vec![
                                JuliaBufferOp::ClearMark,
                            ])),
                            "set_content" => {
                                let content = Self::get_string_field(
                                    &mut frame,
                                    &getindex,
                                    result_dict,
                                    "content",
                                )
                                .unwrap_or_default();
                                Ok(JuliaCommandResult::BufferOps(vec![
                                    JuliaBufferOp::SetContent(content),
                                ]))
                            }
                            "multi" => {
                                // Parse array of actions - for now just return None
                                // TODO: implement multi-action parsing
                                Ok(JuliaCommandResult::None)
                            }
                            _ => Ok(JuliaCommandResult::None),
                        }
                    }
                    Err(_) => Ok(JuliaCommandResult::Error(
                        "Failed to call Julia command".to_string(),
                    )),
                }
            })
        }
    }
}

/// Task for listing available Julia commands
pub struct ListCommandsTask;

impl AsyncTask for ListCommandsTask {
    type Output = JlrsResult<Vec<(String, String)>>;

    fn run(self, mut frame: AsyncGcFrame<'_>) -> impl std::future::Future<Output = Self::Output> {
        async move {
            frame.scope(|mut frame| {
                let main_module = Module::main(&frame);

                // Get the Roe module
                let Ok(roe_module) = main_module.global(&mut frame, "Roe") else {
                    return Ok(Vec::new());
                };

                // Get list_commands function
                let Ok(list_commands_fn) = roe_module
                    .cast::<Module>()
                    .unwrap()
                    .global(&mut frame, "list_commands")
                else {
                    return Ok(Vec::new());
                };

                // Call Roe.list_commands()
                let Ok(result) = (unsafe { list_commands_fn.call(&mut frame, []) }) else {
                    return Ok(Vec::new());
                };

                // Parse the result - it's a Vector of Tuples
                let length_fn = Module::base(&frame).global(&mut frame, "length")?;
                let getindex = Module::base(&frame).global(&mut frame, "getindex")?;

                let Ok(length_val) = (unsafe { length_fn.call(&mut frame, [result]) }) else {
                    return Ok(Vec::new());
                };

                let length: i64 = length_val.unbox::<i64>().unwrap_or(0);
                let mut commands = Vec::new();

                for i in 1..=length {
                    let idx = Value::new(&mut frame, i);
                    let Ok(tuple) = (unsafe { getindex.call(&mut frame, [result, idx]) }) else {
                        continue;
                    };

                    // Get first and second elements of tuple
                    let idx1 = Value::new(&mut frame, 1i64);
                    let idx2 = Value::new(&mut frame, 2i64);

                    let Ok(name_val) = (unsafe { getindex.call(&mut frame, [tuple, idx1]) }) else {
                        continue;
                    };
                    let Ok(desc_val) = (unsafe { getindex.call(&mut frame, [tuple, idx2]) }) else {
                        continue;
                    };

                    if let (Ok(name_js), Ok(desc_js)) = (
                        name_val.cast::<JuliaString>(),
                        desc_val.cast::<JuliaString>(),
                    ) {
                        if let (Ok(name), Ok(desc)) = (name_js.as_str(), desc_js.as_str()) {
                            commands.push((name.to_string(), desc.to_string()));
                        }
                    }
                }

                Ok(commands)
            })
        }
    }
}

/// Task for listing user-defined keybindings from Julia
pub struct ListKeybindingsTask;

impl AsyncTask for ListKeybindingsTask {
    type Output = JlrsResult<Vec<(String, String)>>;

    fn run(self, mut frame: AsyncGcFrame<'_>) -> impl std::future::Future<Output = Self::Output> {
        async move {
            frame.scope(|mut frame| {
                let main_module = Module::main(&frame);

                // Get the Roe module
                let Ok(roe_module) = main_module.global(&mut frame, "Roe") else {
                    return Ok(Vec::new());
                };

                // Get list_keybindings function
                let Ok(list_keybindings_fn) = roe_module
                    .cast::<Module>()
                    .unwrap()
                    .global(&mut frame, "list_keybindings")
                else {
                    return Ok(Vec::new());
                };

                // Call Roe.list_keybindings()
                let Ok(result) = (unsafe { list_keybindings_fn.call(&mut frame, []) }) else {
                    return Ok(Vec::new());
                };

                // Parse the result - it's a Vector of Tuples (key_sequence, action)
                let length_fn = Module::base(&frame).global(&mut frame, "length")?;
                let getindex = Module::base(&frame).global(&mut frame, "getindex")?;

                let Ok(length_val) = (unsafe { length_fn.call(&mut frame, [result]) }) else {
                    return Ok(Vec::new());
                };

                let length: i64 = length_val.unbox::<i64>().unwrap_or(0);
                let mut bindings = Vec::new();

                for i in 1..=length {
                    let idx = Value::new(&mut frame, i);
                    let Ok(tuple) = (unsafe { getindex.call(&mut frame, [result, idx]) }) else {
                        continue;
                    };

                    // Get first and second elements of tuple
                    let idx1 = Value::new(&mut frame, 1i64);
                    let idx2 = Value::new(&mut frame, 2i64);

                    let Ok(key_seq_val) = (unsafe { getindex.call(&mut frame, [tuple, idx1]) })
                    else {
                        continue;
                    };
                    let Ok(action_val) = (unsafe { getindex.call(&mut frame, [tuple, idx2]) })
                    else {
                        continue;
                    };

                    if let (Ok(key_seq_js), Ok(action_js)) = (
                        key_seq_val.cast::<JuliaString>(),
                        action_val.cast::<JuliaString>(),
                    ) {
                        if let (Ok(key_seq), Ok(action)) = (key_seq_js.as_str(), action_js.as_str())
                        {
                            bindings.push((key_seq.to_string(), action.to_string()));
                        }
                    }
                }

                Ok(bindings)
            })
        }
    }
}

/// Task to call a Julia mode's perform handler
pub struct ModePerformTask {
    pub mode_name: String,
    pub action_dict: std::collections::HashMap<String, String>,
}

impl AsyncTask for ModePerformTask {
    type Output = JlrsResult<Option<JuliaModeResult>>;

    fn run(self, mut frame: AsyncGcFrame<'_>) -> impl std::future::Future<Output = Self::Output> {
        async move {
            frame.scope(|mut frame| {
                let main_module = Module::main(&frame);

                // Get the Roe module
                let Ok(roe_module) = main_module.global(&mut frame, "Roe") else {
                    return Ok(None);
                };
                let roe_module = roe_module.cast::<Module>().unwrap();

                // Get mode_perform function
                let Ok(mode_perform_fn) = roe_module.global(&mut frame, "mode_perform") else {
                    return Ok(None);
                };

                // Create Julia Dict for the action
                let dict_type = Module::base(&frame).global(&mut frame, "Dict")?;
                let action_jl = unsafe { dict_type.call(&mut frame, [])? };

                // Populate the action dict
                let setindex_fn = Module::base(&frame).global(&mut frame, "setindex!")?;
                for (key, value) in &self.action_dict {
                    let key_jl = JuliaString::new(&mut frame, key);
                    let value_jl = JuliaString::new(&mut frame, value);
                    unsafe {
                        setindex_fn.call(
                            &mut frame,
                            [action_jl, value_jl.as_value(), key_jl.as_value()],
                        )?;
                    };
                }

                // Call Roe.mode_perform(mode_name, action_dict)
                let mode_name_jl = JuliaString::new(&mut frame, &self.mode_name);
                let Ok(result) = (unsafe {
                    mode_perform_fn.call(&mut frame, [mode_name_jl.as_value(), action_jl])
                }) else {
                    return Ok(None);
                };

                // Parse the result Dict
                let getindex = Module::base(&frame).global(&mut frame, "getindex")?;
                let haskey = Module::base(&frame).global(&mut frame, "haskey")?;
                let length_fn = Module::base(&frame).global(&mut frame, "length")?;

                // Get result type
                let result_key = JuliaString::new(&mut frame, "result");
                let Ok(result_type_val) =
                    (unsafe { getindex.call(&mut frame, [result, result_key.as_value()]) })
                else {
                    return Ok(None);
                };
                let result_type = result_type_val
                    .cast::<JuliaString>()
                    .ok()
                    .and_then(|s| s.as_str().ok())
                    .unwrap_or("ignored")
                    .to_string();

                // Get actions array
                let actions_key = JuliaString::new(&mut frame, "actions");
                let has_actions = unsafe {
                    haskey
                        .call(&mut frame, [result, actions_key.as_value()])
                        .ok()
                        .and_then(|v| v.unbox::<Bool>().ok())
                        .map(|b| b.as_bool())
                        .unwrap_or(false)
                };

                let mut actions = Vec::new();
                if has_actions {
                    let Ok(actions_arr) =
                        (unsafe { getindex.call(&mut frame, [result, actions_key.as_value()]) })
                    else {
                        return Ok(Some(JuliaModeResult {
                            result_type,
                            actions,
                        }));
                    };

                    let actions_len: i64 = unsafe {
                        length_fn
                            .call(&mut frame, [actions_arr])
                            .ok()
                            .and_then(|v| v.unbox::<i64>().ok())
                            .unwrap_or(0)
                    };

                    for i in 1..=actions_len {
                        let idx = Value::new(&mut frame, i);
                        let Ok(action_dict) =
                            (unsafe { getindex.call(&mut frame, [actions_arr, idx]) })
                        else {
                            continue;
                        };

                        // Parse individual action
                        let type_key = JuliaString::new(&mut frame, "type");
                        let action_type = unsafe {
                            getindex
                                .call(&mut frame, [action_dict, type_key.as_value()])
                                .ok()
                                .and_then(|v| v.cast::<JuliaString>().ok())
                                .and_then(|s| s.as_str().ok().map(|s| s.to_string()))
                                .unwrap_or_default()
                        };

                        let mut mode_action = JuliaModeAction {
                            action_type,
                            ..Default::default()
                        };

                        // Get optional fields
                        let get_str_field =
                            |field: &str, frame: &mut GcFrame, dict: Value| -> Option<String> {
                                let key = JuliaString::new(&mut *frame, field);
                                let has = unsafe {
                                    haskey
                                        .call(&mut *frame, [dict, key.as_value()])
                                        .ok()
                                        .and_then(|v| v.unbox::<Bool>().ok())
                                        .map(|b| b.as_bool())
                                        .unwrap_or(false)
                                };
                                if has {
                                    unsafe {
                                        getindex
                                            .call(&mut *frame, [dict, key.as_value()])
                                            .ok()
                                            .and_then(|v| v.cast::<JuliaString>().ok())
                                            .and_then(|s| s.as_str().ok().map(|s| s.to_string()))
                                    }
                                } else {
                                    None
                                }
                            };

                        mode_action.text = get_str_field("text", &mut frame, action_dict);
                        mode_action.position = get_str_field("position", &mut frame, action_dict);
                        mode_action.path = get_str_field("path", &mut frame, action_dict);
                        mode_action.open_type = get_str_field("open_type", &mut frame, action_dict);
                        mode_action.command = get_str_field("command", &mut frame, action_dict);

                        // Get buffer_index as integer
                        mode_action.buffer_index = {
                            let key = JuliaString::new(&mut frame, "buffer_index");
                            let has = unsafe {
                                haskey
                                    .call(&mut frame, [action_dict, key.as_value()])
                                    .ok()
                                    .and_then(|v| v.unbox::<Bool>().ok())
                                    .map(|b| b.as_bool())
                                    .unwrap_or(false)
                            };
                            if has {
                                unsafe {
                                    getindex
                                        .call(&mut frame, [action_dict, key.as_value()])
                                        .ok()
                                        .and_then(|v| v.unbox::<i64>().ok())
                                }
                            } else {
                                None
                            }
                        };

                        actions.push(mode_action);
                    }
                }

                Ok(Some(JuliaModeResult {
                    result_type,
                    actions,
                }))
            })
        }
    }
}

/// Task to get major mode for a file path
pub struct GetMajorModeForFileTask {
    pub file_path: String,
}

impl AsyncTask for GetMajorModeForFileTask {
    type Output = JlrsResult<String>;

    fn run(self, mut frame: AsyncGcFrame<'_>) -> impl std::future::Future<Output = Self::Output> {
        async move {
            frame.scope(|mut frame| {
                let main_module = Module::main(&frame);

                // Get the Roe module
                let Ok(roe_module) = main_module.global(&mut frame, "Roe") else {
                    return Ok("fundamental-mode".to_string());
                };
                let roe_module = roe_module.cast::<Module>().unwrap();

                // Get get_major_mode_for_file function
                let Ok(get_mode_fn) = roe_module.global(&mut frame, "get_major_mode_for_file")
                else {
                    return Ok("fundamental-mode".to_string());
                };

                // Call Roe.get_major_mode_for_file(file_path)
                let file_path_jl = JuliaString::new(&mut frame, &self.file_path);
                let result = unsafe { get_mode_fn.call(&mut frame, [file_path_jl.as_value()]) };

                match result {
                    Ok(mode_name) => {
                        let mode_str = mode_name
                            .cast::<JuliaString>()
                            .ok()
                            .and_then(|s| s.as_str().ok())
                            .unwrap_or("fundamental-mode")
                            .to_string();
                        Ok(mode_str)
                    }
                    Err(_) => Ok("fundamental-mode".to_string()),
                }
            })
        }
    }
}

/// Task to call a major mode's init hook
pub struct CallMajorModeInitTask {
    pub mode_name: String,
}

impl AsyncTask for CallMajorModeInitTask {
    type Output = JlrsResult<bool>;

    fn run(self, mut frame: AsyncGcFrame<'_>) -> impl std::future::Future<Output = Self::Output> {
        async move {
            frame.scope(|mut frame| {
                let main_module = Module::main(&frame);

                // Get the Roe module
                let Ok(roe_module) = main_module.global(&mut frame, "Roe") else {
                    return Ok(false);
                };
                let roe_module = roe_module.cast::<Module>().unwrap();

                // Get call_major_mode_init function
                let Ok(init_fn) = roe_module.global(&mut frame, "call_major_mode_init") else {
                    return Ok(false);
                };

                // Call Roe.call_major_mode_init(mode_name)
                let mode_name_jl = JuliaString::new(&mut frame, &self.mode_name);
                let result = unsafe { init_fn.call(&mut frame, [mode_name_jl.as_value()]) };

                match result {
                    Ok(success) => {
                        let success_bool = success
                            .unbox::<Bool>()
                            .ok()
                            .map(|b| b.as_bool())
                            .unwrap_or(false);
                        Ok(success_bool)
                    }
                    Err(_) => Ok(false),
                }
            })
        }
    }
}

/// Task to call a major mode's after_change hook
pub struct CallMajorModeAfterChangeTask {
    pub mode_name: String,
    pub start: i64,
    pub old_end: i64,
    pub new_end: i64,
}

impl AsyncTask for CallMajorModeAfterChangeTask {
    type Output = JlrsResult<bool>;

    fn run(self, mut frame: AsyncGcFrame<'_>) -> impl std::future::Future<Output = Self::Output> {
        async move {
            frame.scope(|mut frame| {
                let main_module = Module::main(&frame);

                // Get the Roe module
                let Ok(roe_module) = main_module.global(&mut frame, "Roe") else {
                    return Ok(false);
                };
                let roe_module = roe_module.cast::<Module>().unwrap();

                // Get call_major_mode_after_change function
                let Ok(after_change_fn) =
                    roe_module.global(&mut frame, "call_major_mode_after_change")
                else {
                    return Ok(false);
                };

                // Call Roe.call_major_mode_after_change(mode_name, start, old_end, new_end)
                let mode_name_jl = JuliaString::new(&mut frame, &self.mode_name);
                let start_jl = Value::new(&mut frame, self.start);
                let old_end_jl = Value::new(&mut frame, self.old_end);
                let new_end_jl = Value::new(&mut frame, self.new_end);

                let result = unsafe {
                    after_change_fn.call(
                        &mut frame,
                        [mode_name_jl.as_value(), start_jl, old_end_jl, new_end_jl],
                    )
                };

                match result {
                    Ok(success) => {
                        let success_bool = success
                            .unbox::<Bool>()
                            .ok()
                            .map(|b| b.as_bool())
                            .unwrap_or(false);
                        Ok(success_bool)
                    }
                    Err(_) => Ok(false),
                }
            })
        }
    }
}

/// Context passed to Julia commands (mirrors CommandContext)
#[derive(Debug, Clone)]
pub struct JuliaCommandContext {
    pub buffer_name: String,
    pub buffer_modified: bool,
    pub cursor_pos: usize,
    pub current_line: u16,
    pub current_column: u16,
    pub line_count: usize,
    pub char_count: usize,
    /// Mark position (-1 if not set)
    pub mark_pos: i64,
}

/// Buffer operation from Julia (mirrors editor::BufferOperation)
#[derive(Debug, Clone)]
pub enum JuliaBufferOp {
    Insert {
        pos: usize,
        text: String,
    },
    Delete {
        start: usize,
        end: usize,
    },
    Replace {
        start: usize,
        end: usize,
        text: String,
    },
    SetCursor(usize),
    SetMark(usize),
    ClearMark,
    SetContent(String),
}

/// Result from a Julia command
#[derive(Debug, Clone)]
pub enum JuliaCommandResult {
    Echo(String),
    Error(String),
    None,
    /// Buffer manipulation operations
    BufferOps(Vec<JuliaBufferOp>),
    /// Multiple actions combined (echo + buffer ops, etc.)
    Multi(Vec<JuliaCommandResult>),
}

/// A single action returned from a Julia mode handler
#[derive(Debug, Clone, Default)]
pub struct JuliaModeAction {
    pub action_type: String,
    pub text: Option<String>,
    pub position: Option<String>,
    pub path: Option<String>,
    pub open_type: Option<String>,
    pub command: Option<String>,
    pub buffer_index: Option<i64>,
}

/// Result from a Julia mode perform call
#[derive(Debug, Clone)]
pub struct JuliaModeResult {
    pub result_type: String, // "consumed", "annotated", "ignored"
    pub actions: Vec<JuliaModeAction>,
}

/// Command to send to the persistent Julia runtime
#[derive(Debug)]
pub enum JuliaCommand {
    LoadConfig(PathBuf),
    LoadRoeModule(PathBuf, tokio::sync::oneshot::Sender<Result<(), String>>),
    QueryConfig(String, tokio::sync::oneshot::Sender<Option<ConfigValue>>),
    TestAddition(u64, u64, tokio::sync::oneshot::Sender<u64>),
    EvalExpression(String, tokio::sync::oneshot::Sender<String>),
    CallCommand(
        String,
        JuliaCommandContext,
        Buffer, // The buffer for this command's context
        tokio::sync::oneshot::Sender<JuliaCommandResult>,
    ),
    ListCommands(tokio::sync::oneshot::Sender<Vec<(String, String)>>),
    ListKeybindings(tokio::sync::oneshot::Sender<Vec<(String, String)>>),
    /// Call a Julia mode's perform handler
    ModePerform(
        String,                                    // mode name
        std::collections::HashMap<String, String>, // key action as dict
        tokio::sync::oneshot::Sender<JuliaModeResult>,
    ),
    /// Get major mode name for a file path
    GetMajorModeForFile(
        String,                               // file path
        tokio::sync::oneshot::Sender<String>, // mode name
    ),
    /// Call a major mode's init hook
    CallMajorModeInit(
        String,                             // mode name
        tokio::sync::oneshot::Sender<bool>, // success
    ),
    /// Call a major mode's after_change hook
    CallMajorModeAfterChange(
        String, // mode name
        i64,
        i64,
        i64,                                // start, old_end, new_end
        tokio::sync::oneshot::Sender<bool>, // success
    ),
    Shutdown,
}

/// The main Julia runtime wrapper for Roe editor
/// Maintains a persistent Julia runtime for live configuration and scripting
pub struct RoeJuliaRuntime {
    /// Channel to send commands to the persistent Julia runtime
    command_tx: Option<mpsc::UnboundedSender<JuliaCommand>>,
    /// Julia thread handle
    thread_handle: Option<JoinHandle<()>>,
    /// Whether configuration has been loaded
    config_loaded: bool,
    /// Path to the current configuration file
    config_path: Option<PathBuf>,
}

impl RoeJuliaRuntime {
    /// Get the default config file path
    pub fn default_config_path() -> PathBuf {
        std::env::current_dir().unwrap_or_default().join(".roe.jl")
    }

    /// Create a new Julia runtime instance and keep it alive
    /// Initializes a persistent jlrs async runtime for live queries
    pub fn new() -> Result<Self, JuliaRuntimeError> {
        let (command_tx, command_rx) = mpsc::unbounded_channel();

        // Spawn the Julia runtime in a separate thread
        let thread_handle = std::thread::spawn(move || {
            let rt =
                tokio::runtime::Runtime::new().expect("Failed to create Tokio runtime for Julia");

            rt.block_on(async {
                let (julia, julia_thread) = Builder::new()
                    .async_runtime(Tokio::<3>::new(false))
                    .spawn()
                    .expect("Failed to spawn Julia runtime");

                // Run the command handler
                Self::julia_command_handler(julia, command_rx).await;

                // Clean up Julia thread
                julia_thread.join().expect("Julia thread failed");
            });
        });

        Ok(Self {
            command_tx: Some(command_tx),
            thread_handle: Some(thread_handle),
            config_loaded: false,
            config_path: None,
        })
    }

    /// Handle commands sent to the persistent Julia runtime
    async fn julia_command_handler(
        julia: AsyncHandle,
        mut command_rx: mpsc::UnboundedReceiver<JuliaCommand>,
    ) {
        while let Some(command) = command_rx.recv().await {
            match command {
                JuliaCommand::LoadConfig(path) => {
                    let task = ConfigLoadTask::new(path);
                    let _result = julia.task(task).try_dispatch();
                }
                JuliaCommand::QueryConfig(key, response_tx) => {
                    let task = ConfigQueryTask::new(key);
                    let Ok(async_task) = julia.task(task).try_dispatch() else {
                        let _ = response_tx.send(None);
                        continue;
                    };

                    let Ok(result) = async_task.await else {
                        let _ = response_tx.send(None);
                        continue;
                    };

                    let config_value = result.unwrap_or(None);
                    let _ = response_tx.send(config_value);
                }
                JuliaCommand::TestAddition(a, b, response_tx) => {
                    let task = AdditionTask::new(a, b);
                    let Ok(async_task) = julia.task(task).try_dispatch() else {
                        let _ = response_tx.send(0);
                        continue;
                    };

                    let Ok(result) = async_task.await else {
                        let _ = response_tx.send(0);
                        continue;
                    };

                    let sum = result.unwrap_or(0);
                    let _ = response_tx.send(sum);
                }
                JuliaCommand::EvalExpression(expression, response_tx) => {
                    let task = JuliaReplTask::new(expression);
                    let Ok(async_task) = julia.task(task).try_dispatch() else {
                        let _ = response_tx.send("Error: Failed to dispatch task".to_string());
                        continue;
                    };

                    let Ok(result) = async_task.await else {
                        let _ = response_tx.send("Error: Task execution failed".to_string());
                        continue;
                    };

                    let output =
                        result.unwrap_or_else(|_| "Error: Result processing failed".to_string());
                    let _ = response_tx.send(output);
                }
                JuliaCommand::LoadRoeModule(path, response_tx) => {
                    let task = LoadRoeModuleTask::new(path);
                    let Ok(async_task) = julia.task(task).try_dispatch() else {
                        let _ = response_tx.send(Err("Failed to dispatch task".to_string()));
                        continue;
                    };

                    let Ok(result) = async_task.await else {
                        let _ = response_tx.send(Err("Task execution failed".to_string()));
                        continue;
                    };

                    let output = result.unwrap_or_else(|e| Err(format!("Julia error: {:?}", e)));
                    let _ = response_tx.send(output);
                }
                JuliaCommand::CallCommand(name, context, buffer, response_tx) => {
                    // Set the buffer for this command's execution
                    set_current_buffer(buffer);

                    let task = CallCommandTask::new(name, context);
                    let Ok(async_task) = julia.task(task).try_dispatch() else {
                        clear_current_buffer();
                        let _ = response_tx.send(JuliaCommandResult::Error(
                            "Failed to dispatch task".to_string(),
                        ));
                        continue;
                    };

                    let Ok(result) = async_task.await else {
                        clear_current_buffer();
                        let _ = response_tx.send(JuliaCommandResult::Error(
                            "Task execution failed".to_string(),
                        ));
                        continue;
                    };

                    // Clear the buffer after command execution
                    clear_current_buffer();

                    let output = result.unwrap_or(JuliaCommandResult::Error(
                        "Result processing failed".to_string(),
                    ));
                    let _ = response_tx.send(output);
                }
                JuliaCommand::ListCommands(response_tx) => {
                    let task = ListCommandsTask;
                    let Ok(async_task) = julia.task(task).try_dispatch() else {
                        let _ = response_tx.send(Vec::new());
                        continue;
                    };

                    let Ok(result) = async_task.await else {
                        let _ = response_tx.send(Vec::new());
                        continue;
                    };

                    let commands = result.unwrap_or_default();
                    let _ = response_tx.send(commands);
                }
                JuliaCommand::ListKeybindings(response_tx) => {
                    let task = ListKeybindingsTask;
                    let Ok(async_task) = julia.task(task).try_dispatch() else {
                        let _ = response_tx.send(Vec::new());
                        continue;
                    };

                    let Ok(result) = async_task.await else {
                        let _ = response_tx.send(Vec::new());
                        continue;
                    };

                    let bindings = result.unwrap_or_default();
                    let _ = response_tx.send(bindings);
                }
                JuliaCommand::ModePerform(mode_name, action_dict, response_tx) => {
                    let task = ModePerformTask {
                        mode_name,
                        action_dict,
                    };
                    let Ok(async_task) = julia.task(task).try_dispatch() else {
                        let _ = response_tx.send(JuliaModeResult {
                            result_type: "ignored".to_string(),
                            actions: vec![],
                        });
                        continue;
                    };

                    let Ok(result) = async_task.await else {
                        let _ = response_tx.send(JuliaModeResult {
                            result_type: "ignored".to_string(),
                            actions: vec![],
                        });
                        continue;
                    };

                    let mode_result = result.ok().flatten().unwrap_or_else(|| JuliaModeResult {
                        result_type: "ignored".to_string(),
                        actions: vec![],
                    });
                    let _ = response_tx.send(mode_result);
                }
                JuliaCommand::GetMajorModeForFile(file_path, response_tx) => {
                    let task = GetMajorModeForFileTask { file_path };
                    let Ok(async_task) = julia.task(task).try_dispatch() else {
                        let _ = response_tx.send("fundamental-mode".to_string());
                        continue;
                    };

                    let Ok(result) = async_task.await else {
                        let _ = response_tx.send("fundamental-mode".to_string());
                        continue;
                    };

                    let mode_name = result.unwrap_or_else(|_| "fundamental-mode".to_string());
                    let _ = response_tx.send(mode_name);
                }
                JuliaCommand::CallMajorModeInit(mode_name, response_tx) => {
                    let task = CallMajorModeInitTask { mode_name };
                    let Ok(async_task) = julia.task(task).try_dispatch() else {
                        let _ = response_tx.send(false);
                        continue;
                    };

                    let Ok(result) = async_task.await else {
                        let _ = response_tx.send(false);
                        continue;
                    };

                    let success = result.unwrap_or(false);
                    let _ = response_tx.send(success);
                }
                JuliaCommand::CallMajorModeAfterChange(
                    mode_name,
                    start,
                    old_end,
                    new_end,
                    response_tx,
                ) => {
                    let task = CallMajorModeAfterChangeTask {
                        mode_name,
                        start,
                        old_end,
                        new_end,
                    };
                    let Ok(async_task) = julia.task(task).try_dispatch() else {
                        let _ = response_tx.send(false);
                        continue;
                    };

                    let Ok(result) = async_task.await else {
                        let _ = response_tx.send(false);
                        continue;
                    };

                    let success = result.unwrap_or(false);
                    let _ = response_tx.send(success);
                }
                JuliaCommand::Shutdown => {
                    break;
                }
            }
        }
    }

    /// Test basic jlrs functionality to validate the integration
    fn test_jlrs_basic() -> Result<(), Box<dyn std::error::Error>> {
        // Use a blocking runtime for this test
        let rt = tokio::runtime::Runtime::new()?;

        rt.block_on(async {
            let (julia, thread_handle) = Builder::new()
                .async_runtime(Tokio::<3>::new(false))
                .spawn()?;

            let async_task = julia
                .task(AdditionTask { a: 1, b: 2 })
                .try_dispatch()
                .map_err(|e| format!("Task dispatch failed: {e:?}"))?;

            let res = async_task
                .await
                .map_err(|e| format!("Task receive failed: {e:?}"))?
                .map_err(|e| format!("Julia task failed: {e:?}"))?;

            if res != 3 {
                return Err("Julia addition test failed: expected 3".into());
            }

            std::mem::drop(julia);
            thread_handle.join().map_err(|_| "Thread join failed")?;
            Ok::<(), Box<dyn std::error::Error>>(())
        })
    }

    /// Execute a simple Julia addition for testing
    /// This demonstrates real Julia task dispatch
    pub async fn test_addition(&self, a: u64, b: u64) -> Result<u64, JuliaRuntimeError> {
        let Some(ref command_tx) = self.command_tx else {
            return Err(JuliaRuntimeError::TaskExecutionFailed(
                "Runtime not initialized".to_string(),
            ));
        };

        let (response_tx, response_rx) = tokio::sync::oneshot::channel();

        command_tx
            .send(JuliaCommand::TestAddition(a, b, response_tx))
            .map_err(|_| {
                JuliaRuntimeError::TaskExecutionFailed("Command channel closed".to_string())
            })?;

        response_rx.await.map_err(|_| {
            JuliaRuntimeError::TaskExecutionFailed("Response channel closed".to_string())
        })
    }

    /// Load configuration from a .roe.jl file into the persistent Julia runtime
    pub async fn load_config(
        &mut self,
        config_path: Option<PathBuf>,
    ) -> Result<bool, JuliaRuntimeError> {
        let config_path = config_path.unwrap_or_else(Self::default_config_path);

        if !config_path.exists() {
            return Ok(false);
        }

        let Some(ref command_tx) = self.command_tx else {
            return Err(JuliaRuntimeError::ConfigLoadFailed(
                "Runtime not initialized".to_string(),
            ));
        };

        command_tx
            .send(JuliaCommand::LoadConfig(config_path.clone()))
            .map_err(|_| {
                JuliaRuntimeError::ConfigLoadFailed("Command channel closed".to_string())
            })?;

        // Mark as loaded and store path
        self.config_loaded = true;
        self.config_path = Some(config_path);
        Ok(true)
    }

    /// Query a configuration value from the live Julia runtime
    pub async fn get_config(&self, key: &str) -> Result<Option<ConfigValue>, JuliaRuntimeError> {
        let Some(ref command_tx) = self.command_tx else {
            return Err(JuliaRuntimeError::TaskExecutionFailed(
                "Runtime not initialized".to_string(),
            ));
        };

        let (response_tx, response_rx) = tokio::sync::oneshot::channel();

        command_tx
            .send(JuliaCommand::QueryConfig(key.to_string(), response_tx))
            .map_err(|_| {
                JuliaRuntimeError::TaskExecutionFailed("Command channel closed".to_string())
            })?;

        response_rx.await.map_err(|_| {
            JuliaRuntimeError::TaskExecutionFailed("Response channel closed".to_string())
        })
    }

    /// Evaluate a Julia expression and return the result as a string
    pub async fn eval_expression(&self, expression: &str) -> Result<String, JuliaRuntimeError> {
        let Some(ref command_tx) = self.command_tx else {
            return Err(JuliaRuntimeError::TaskExecutionFailed(
                "Runtime not initialized".to_string(),
            ));
        };

        let (response_tx, response_rx) = tokio::sync::oneshot::channel();

        command_tx
            .send(JuliaCommand::EvalExpression(
                expression.to_string(),
                response_tx,
            ))
            .map_err(|_| {
                JuliaRuntimeError::TaskExecutionFailed("Command channel closed".to_string())
            })?;

        response_rx.await.map_err(|_| {
            JuliaRuntimeError::TaskExecutionFailed("Response channel closed".to_string())
        })
    }

    /// Get config value as string with fallback
    pub async fn get_config_string(&self, key: &str, default: &str) -> String {
        self.get_config(key)
            .await
            .ok()
            .flatten()
            .and_then(|v| v.as_string())
            .unwrap_or_else(|| default.to_string())
    }

    /// Get config value as integer with fallback
    pub async fn get_config_int(&self, key: &str, default: i64) -> i64 {
        self.get_config(key)
            .await
            .ok()
            .flatten()
            .and_then(|v| v.as_integer())
            .unwrap_or(default)
    }

    /// Get config value as boolean with fallback
    pub async fn get_config_bool(&self, key: &str, default: bool) -> bool {
        self.get_config(key)
            .await
            .ok()
            .flatten()
            .and_then(|v| v.as_bool())
            .unwrap_or(default)
    }

    /// Check if configuration has been loaded
    pub fn is_config_loaded(&self) -> bool {
        self.config_loaded
    }

    /// Get the path to the loaded configuration file
    pub fn config_path(&self) -> Option<&PathBuf> {
        self.config_path.as_ref()
    }

    /// Test that Julia integration is working
    pub fn test_julia_basic(&self) -> Result<(), JuliaRuntimeError> {
        Self::test_jlrs_basic()
            .map_err(|e| JuliaRuntimeError::TaskExecutionFailed(format!("Julia test failed: {e}")))
    }

    /// Load the Roe Julia module for command definitions
    pub async fn load_roe_module(&self, module_path: PathBuf) -> Result<(), JuliaRuntimeError> {
        let Some(ref command_tx) = self.command_tx else {
            return Err(JuliaRuntimeError::ScriptLoadFailed(
                "Runtime not initialized".to_string(),
            ));
        };

        let (response_tx, response_rx) = tokio::sync::oneshot::channel();

        command_tx
            .send(JuliaCommand::LoadRoeModule(module_path, response_tx))
            .map_err(|_| {
                JuliaRuntimeError::ScriptLoadFailed("Command channel closed".to_string())
            })?;

        let result = response_rx.await.map_err(|_| {
            JuliaRuntimeError::ScriptLoadFailed("Response channel closed".to_string())
        })?;

        result.map_err(JuliaRuntimeError::ScriptLoadFailed)
    }

    /// Call a Julia-defined command
    pub async fn call_command(
        &self,
        name: &str,
        context: JuliaCommandContext,
        buffer: Buffer,
    ) -> Result<JuliaCommandResult, JuliaRuntimeError> {
        let Some(ref command_tx) = self.command_tx else {
            return Err(JuliaRuntimeError::TaskExecutionFailed(
                "Runtime not initialized".to_string(),
            ));
        };

        let (response_tx, response_rx) = tokio::sync::oneshot::channel();

        command_tx
            .send(JuliaCommand::CallCommand(
                name.to_string(),
                context,
                buffer,
                response_tx,
            ))
            .map_err(|_| {
                JuliaRuntimeError::TaskExecutionFailed("Command channel closed".to_string())
            })?;

        response_rx.await.map_err(|_| {
            JuliaRuntimeError::TaskExecutionFailed("Response channel closed".to_string())
        })
    }

    /// List all available Julia commands
    pub async fn list_commands(&self) -> Result<Vec<(String, String)>, JuliaRuntimeError> {
        let Some(ref command_tx) = self.command_tx else {
            return Err(JuliaRuntimeError::TaskExecutionFailed(
                "Runtime not initialized".to_string(),
            ));
        };

        let (response_tx, response_rx) = tokio::sync::oneshot::channel();

        command_tx
            .send(JuliaCommand::ListCommands(response_tx))
            .map_err(|_| {
                JuliaRuntimeError::TaskExecutionFailed("Command channel closed".to_string())
            })?;

        response_rx.await.map_err(|_| {
            JuliaRuntimeError::TaskExecutionFailed("Response channel closed".to_string())
        })
    }

    /// List all user-defined keybindings from Julia
    pub async fn list_keybindings(&self) -> Result<Vec<(String, String)>, JuliaRuntimeError> {
        let Some(ref command_tx) = self.command_tx else {
            return Err(JuliaRuntimeError::TaskExecutionFailed(
                "Runtime not initialized".to_string(),
            ));
        };

        let (response_tx, response_rx) = tokio::sync::oneshot::channel();

        command_tx
            .send(JuliaCommand::ListKeybindings(response_tx))
            .map_err(|_| {
                JuliaRuntimeError::TaskExecutionFailed("Command channel closed".to_string())
            })?;

        response_rx.await.map_err(|_| {
            JuliaRuntimeError::TaskExecutionFailed("Response channel closed".to_string())
        })
    }

    /// Call a Julia mode's perform handler (synchronous version for Mode trait)
    pub fn call_mode_perform(
        &self,
        mode_name: &str,
        action: std::collections::HashMap<String, String>,
    ) -> Result<JuliaModeResult, JuliaRuntimeError> {
        let Some(ref command_tx) = self.command_tx else {
            return Err(JuliaRuntimeError::TaskExecutionFailed(
                "Runtime not initialized".to_string(),
            ));
        };

        let (response_tx, response_rx) = tokio::sync::oneshot::channel();

        command_tx
            .send(JuliaCommand::ModePerform(
                mode_name.to_string(),
                action,
                response_tx,
            ))
            .map_err(|_| {
                JuliaRuntimeError::TaskExecutionFailed("Command channel closed".to_string())
            })?;

        // Block waiting for response - this is called from Mode::perform which is sync
        response_rx.blocking_recv().map_err(|_| {
            JuliaRuntimeError::TaskExecutionFailed("Response channel closed".to_string())
        })
    }

    /// Get the major mode name for a file path based on extension
    pub async fn get_major_mode_for_file(
        &self,
        file_path: &str,
    ) -> Result<String, JuliaRuntimeError> {
        let Some(ref command_tx) = self.command_tx else {
            return Err(JuliaRuntimeError::TaskExecutionFailed(
                "Runtime not initialized".to_string(),
            ));
        };

        let (response_tx, response_rx) = tokio::sync::oneshot::channel();

        command_tx
            .send(JuliaCommand::GetMajorModeForFile(
                file_path.to_string(),
                response_tx,
            ))
            .map_err(|_| {
                JuliaRuntimeError::TaskExecutionFailed("Command channel closed".to_string())
            })?;

        response_rx.await.map_err(|_| {
            JuliaRuntimeError::TaskExecutionFailed("Response channel closed".to_string())
        })
    }

    /// Call a major mode's init hook
    pub async fn call_major_mode_init(&self, mode_name: &str) -> Result<bool, JuliaRuntimeError> {
        let Some(ref command_tx) = self.command_tx else {
            return Err(JuliaRuntimeError::TaskExecutionFailed(
                "Runtime not initialized".to_string(),
            ));
        };

        let (response_tx, response_rx) = tokio::sync::oneshot::channel();

        command_tx
            .send(JuliaCommand::CallMajorModeInit(
                mode_name.to_string(),
                response_tx,
            ))
            .map_err(|_| {
                JuliaRuntimeError::TaskExecutionFailed("Command channel closed".to_string())
            })?;

        response_rx.await.map_err(|_| {
            JuliaRuntimeError::TaskExecutionFailed("Response channel closed".to_string())
        })
    }

    /// Call a major mode's after_change hook
    pub async fn call_major_mode_after_change(
        &self,
        mode_name: &str,
        start: i64,
        old_end: i64,
        new_end: i64,
    ) -> Result<bool, JuliaRuntimeError> {
        let Some(ref command_tx) = self.command_tx else {
            return Err(JuliaRuntimeError::TaskExecutionFailed(
                "Runtime not initialized".to_string(),
            ));
        };

        let (response_tx, response_rx) = tokio::sync::oneshot::channel();

        command_tx
            .send(JuliaCommand::CallMajorModeAfterChange(
                mode_name.to_string(),
                start,
                old_end,
                new_end,
                response_tx,
            ))
            .map_err(|_| {
                JuliaRuntimeError::TaskExecutionFailed("Command channel closed".to_string())
            })?;

        response_rx.await.map_err(|_| {
            JuliaRuntimeError::TaskExecutionFailed("Response channel closed".to_string())
        })
    }

    /// Get path to the bundled roe.jl module
    pub fn bundled_roe_module_path() -> Option<PathBuf> {
        // Look for roe.jl in the jl/ directory
        let exe_path = std::env::current_exe().ok()?;
        let exe_dir = exe_path.parent()?;

        // Try several locations
        let candidates = [
            exe_dir.join("jl/roe.jl"),
            exe_dir.join("../jl/roe.jl"),
            PathBuf::from("jl/roe.jl"),
            std::env::current_dir().ok()?.join("jl/roe.jl"),
        ];

        candidates.into_iter().find(|candidate| candidate.exists())
    }
}

impl Drop for RoeJuliaRuntime {
    fn drop(&mut self) {
        // Send shutdown command to Julia runtime
        if let Some(ref command_tx) = self.command_tx {
            let _ = command_tx.send(JuliaCommand::Shutdown);
        }

        // Wait for Julia thread to finish
        if let Some(thread_handle) = self.thread_handle.take() {
            let _ = thread_handle.join();
        }
    }
}

/// Create a new Julia runtime instance with shared access
/// Returns an Arc<Mutex<RoeJuliaRuntime>> for thread-safe access
pub fn create_shared_runtime() -> Result<Arc<Mutex<RoeJuliaRuntime>>, JuliaRuntimeError> {
    let runtime = RoeJuliaRuntime::new()?;
    Ok(Arc::new(Mutex::new(runtime)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_julia_runtime_creation() {
        let runtime = RoeJuliaRuntime::new();
        assert!(runtime.is_ok());
    }

    #[tokio::test]
    async fn test_simple_addition() {
        let runtime = RoeJuliaRuntime::new().unwrap();
        let result = runtime.test_addition(2, 3).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 5);
    }

    #[test]
    fn test_shared_runtime_creation() {
        let shared_runtime = create_shared_runtime();
        assert!(shared_runtime.is_ok());
    }

    #[tokio::test]
    async fn test_config_loading() {
        let mut runtime = RoeJuliaRuntime::new().unwrap();

        // Try to load config from current directory
        let config_path = RoeJuliaRuntime::default_config_path();

        let load_result = runtime.load_config(Some(config_path)).await;

        // Try to get a config value
        let bg_result = runtime.get_config("colors.background").await;

        let theme_result = runtime.get_config("theme").await;

        // Basic assertions to verify the test worked
        assert!(load_result.is_ok());
        assert!(bg_result.is_ok());
        assert!(theme_result.is_ok());
    }

    #[tokio::test]
    async fn test_main_app_config_path() {
        // Test from the main app's working directory
        let main_config_path = PathBuf::from("/Users/ryan/rymacs/.roe.jl");

        if main_config_path.exists() {
            let mut runtime = RoeJuliaRuntime::new().unwrap();
            let load_result = runtime.load_config(Some(main_config_path)).await;

            let bg_result = runtime.get_config("colors.background").await;

            // Basic assertions
            assert!(load_result.is_ok());
            assert!(bg_result.is_ok());
        }
    }
}
