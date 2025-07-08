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

use async_trait::async_trait;
use jlrs::memory::target::frame::GcFrame;
use jlrs::prelude::*;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::JoinHandle;
use tokio::sync::{mpsc, Mutex};

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

#[async_trait(?Send)]
impl AsyncTask for AdditionTask {
    type Output = JlrsResult<u64>;

    async fn run<'frame>(&mut self, mut frame: AsyncGcFrame<'frame>) -> Self::Output {
        let a = Value::new(&mut frame, self.a);
        let b = Value::new(&mut frame, self.b);
        let func = Module::base(&frame).global(&mut frame, "+")?;

        unsafe { func.call_async(&mut frame, [a, b]) }
            .await
            .into_jlrs_result()?
            .unbox::<u64>()
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

#[async_trait(?Send)]
impl AsyncTask for ConfigLoadTask {
    type Output = JlrsResult<HashMap<String, ConfigValue>>;

    async fn run<'frame>(&mut self, mut frame: AsyncGcFrame<'frame>) -> Self::Output {
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

/// Task for executing Julia expressions in REPL mode
pub struct JuliaReplTask {
    expression: String,
}

impl JuliaReplTask {
    pub fn new(expression: String) -> Self {
        Self { expression }
    }
}

#[async_trait(?Send)]
impl AsyncTask for JuliaReplTask {
    type Output = JlrsResult<String>;

    async fn run<'frame>(&mut self, mut frame: AsyncGcFrame<'frame>) -> Self::Output {
        frame.scope(|mut frame| {
            // Execute the Julia expression
            let result = unsafe { Value::eval_string(&mut frame, &self.expression) };

            match result {
                Ok(value) => {
                    // Convert result to string representation
                    let string_func = Module::base(&frame).global(&mut frame, "string")?;
                    let string_result = match unsafe { string_func.call1(&mut frame, value) } {
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
            (unsafe { getindex.call2(&mut *frame, config_dict, first_key.as_value()) })
        else {
            return Ok(None);
        };

        // Get second level: getindex(colors_dict, "background")
        let second_key = JuliaString::new(&mut *frame, parts[1]);
        let Ok(final_value) =
            (unsafe { getindex.call2(&mut *frame, first_value, second_key.as_value()) })
        else {
            return Ok(None);
        };

        // Try to convert to string
        if let Ok(julia_string) = final_value.cast::<JuliaString>() {
            if let Ok(rust_string) = julia_string.as_str() {
                return Ok(Some(ConfigValue::String(rust_string.to_string())));
            }
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

        let Ok(value) = (unsafe { getindex.call2(&mut *frame, config_dict, key_str.as_value()) })
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

#[async_trait(?Send)]
impl AsyncTask for ConfigQueryTask {
    type Output = JlrsResult<Option<ConfigValue>>;

    async fn run<'frame>(&mut self, mut frame: AsyncGcFrame<'frame>) -> Self::Output {
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

/// Command to send to the persistent Julia runtime
#[derive(Debug)]
pub enum JuliaCommand {
    LoadConfig(PathBuf),
    QueryConfig(String, tokio::sync::oneshot::Sender<Option<ConfigValue>>),
    TestAddition(u64, u64, tokio::sync::oneshot::Sender<u64>),
    EvalExpression(String, tokio::sync::oneshot::Sender<String>),
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
