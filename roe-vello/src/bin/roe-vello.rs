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

//! Roe editor with Vello/GPU rendering backend.

use roe_core::session::StartupRecoveryOperation;
use roe_core::{Buffer, BufferId, Editor, Frame, Window, WindowId, editor, kill_ring};
use slotmap::SlotMap;

/// Default window size in character cells (will be adjusted by actual window size)
const DEFAULT_COLS: u16 = 120;
const DEFAULT_LINES: u16 = 40;

fn required_argument(args: &[String], index: usize, option: &str, operands: &str) -> String {
    args.get(index)
        .filter(|argument| !argument.starts_with('-'))
        .cloned()
        .unwrap_or_else(|| {
            eprintln!("Error: {option} requires {operands}");
            print_help();
            std::process::exit(2);
        })
}

/// Parse command line arguments
fn parse_args() -> EditorConfig {
    let args: Vec<String> = std::env::args().collect();
    let mut file_paths = Vec::new();
    let mut recovery = Vec::new();
    let mut i = 1; // Skip program name

    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            "--mica-check" => {
                i += 1;
                recovery.push(StartupRecoveryOperation::CheckFile(
                    required_argument(&args, i, "--mica-check", "FILE").into(),
                ));
                i += 1;
            }
            "--mica-replace" => {
                let unit = required_argument(&args, i + 1, "--mica-replace", "UNIT FILE");
                let path = required_argument(&args, i + 2, "--mica-replace", "UNIT FILE").into();
                recovery.push(StartupRecoveryOperation::ReplaceUnit { unit, path });
                i += 3;
            }
            "--mica-export" => {
                let unit = required_argument(&args, i + 1, "--mica-export", "UNIT FILE");
                let path = required_argument(&args, i + 2, "--mica-export", "UNIT FILE").into();
                recovery.push(StartupRecoveryOperation::ExportUnit { unit, path });
                i += 3;
            }
            "--mica-restore-first-wave" => {
                recovery.push(StartupRecoveryOperation::RestoreFirstWave);
                i += 1;
            }
            "--mica-enable-package" | "--mica-disable-package" => {
                let enabled = args[i] == "--mica-enable-package";
                let package = required_argument(&args, i + 1, args[i].as_str(), "PACKAGE");
                recovery.push(StartupRecoveryOperation::SetPackageEnabled { package, enabled });
                i += 2;
            }
            "--mica-inspect" => {
                recovery.push(StartupRecoveryOperation::Inspect);
                i += 1;
            }
            arg if arg.starts_with('-') => {
                eprintln!("Error: Unknown option '{arg}'");
                print_help();
                std::process::exit(1);
            }
            _ => {
                file_paths.push(args[i].clone());
                i += 1;
            }
        }
    }

    EditorConfig {
        file_paths,
        recovery,
    }
}

fn print_help() {
    println!("Roe (Vello) - Ryan's Own Emacs with GPU rendering");
    println!();
    println!("USAGE:");
    println!("    roe-vello [OPTIONS] [FILES...]");
    println!();
    println!("OPTIONS:");
    println!("    -h, --help           Print this help message");
    println!("    --mica-check FILE / --mica-replace UNIT FILE / --mica-export UNIT FILE");
    println!("    --mica-restore-first-wave / --mica-enable-package PACKAGE");
    println!("    --mica-disable-package PACKAGE / --mica-inspect");
    println!();
    println!("EXAMPLES:");
    println!("    roe-vello                      # Start with welcome screen");
    println!("    roe-vello file.txt             # Open file.txt");
}

fn create_welcome_screen_content() -> String {
    const RUNE_ART: &str = include_str!("../../../rune.txt");

    let mut content = String::new();
    content.push_str(RUNE_ART);
    content.push_str("\n\n");

    let title = "ROE - Ryan's Own Emacs (Vello)";
    let title_padding = " ".repeat(18);
    content.push_str(&format!("{title_padding}{title}\n\n"));

    content.push_str("                        Getting Started:\n\n");
    content.push_str("                     C-x C-f  -  Find and open a file\n");
    content.push_str("                     C-x C-s  -  Save current buffer\n");
    content.push_str("                     C-x C-c  -  Exit Roe\n");
    content.push_str("                     M-x      -  Execute command\n");
    content.push_str("                     C-x b    -  Switch buffer\n");
    content.push_str("                     C-x 2    -  Split window horizontally\n");
    content.push_str("                     C-x 3    -  Split window vertically\n");
    content.push_str("                     C-x o    -  Switch to other window\n\n");
    content.push_str("                     Press C-x C-f to open your first file!\n");

    content
}

struct EditorConfig {
    file_paths: Vec<String>,
    recovery: Vec<StartupRecoveryOperation>,
}

async fn create_editor(config: EditorConfig) -> std::io::Result<Editor> {
    let mut buffers: SlotMap<BufferId, Buffer> = SlotMap::default();

    let mut first_buffer_id = None;

    if config.file_paths.is_empty() {
        // No files specified, create welcome screen buffer
        let buffer = Buffer::new();
        buffer.set_object("*Welcome*".to_string());
        buffer.load_str(&create_welcome_screen_content());

        let buffer_id = buffers.insert(buffer);
        first_buffer_id = Some(buffer_id);
    } else {
        // Create buffers for all specified files
        for file_path in config.file_paths {
            let buffer = match Buffer::from_file(&file_path).await {
                Ok(buffer) => buffer,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    let buffer = Buffer::new();
                    buffer.set_object(file_path.clone());
                    buffer
                }
                Err(error) => {
                    return Err(std::io::Error::new(
                        error.kind(),
                        format!("failed to open {file_path}: {error}"),
                    ));
                }
            };

            let buffer_id = buffers.insert(buffer);

            if first_buffer_id.is_none() {
                first_buffer_id = Some(buffer_id);
            }
        }
    }

    // Create single window (Vello will resize it properly)
    let mut windows: SlotMap<WindowId, Window> = SlotMap::default();

    let active_buffer = first_buffer_id.expect("Should have at least one buffer");
    let window = Window {
        x: 0,
        y: 0,
        width_chars: DEFAULT_COLS,
        height_chars: DEFAULT_LINES,
        active_buffer,
        start_line: 0,
        start_column: 0,
        cursor: 0,
        window_type: editor::WindowType::Normal,
    };
    let active_window_id = windows.insert(window);
    let window_tree = editor::WindowNode::new_leaf(active_window_id);

    // Initialize file watcher
    let mut file_watcher = roe_core::file_watcher::FileWatcher::new();
    if let Err(error) = file_watcher.init() {
        tracing::warn!(%error, "file watcher is unavailable");
    }

    let mut editor = Editor {
        frame: Frame::new(DEFAULT_COLS, DEFAULT_LINES),
        buffers,
        windows,
        active_window: active_window_id,
        previous_active_window: None,
        window_tree,
        kill_ring: kill_ring::KillRing::without_clipboard(60),
        buffer_history: Vec::new(),
        echo_message: String::new(),
        echo_message_time: None,
        clock: std::sync::Arc::new(roe_core::native_services::SystemClock),
        mouse_drag_state: None,
        messages_buffer_id: None,
        file_watcher,
    };

    // Initialize buffer history
    editor.record_buffer_access(active_buffer);

    // Register file-backed buffers with the file watcher
    for (buffer_id, buffer) in &editor.buffers {
        let file_path = buffer.object();
        if !file_path.is_empty() && std::path::Path::new(&file_path).exists() {
            let content = buffer.content();
            if let Err(error) =
                editor
                    .file_watcher
                    .watch_file(buffer_id, std::path::Path::new(&file_path), content)
            {
                tracing::warn!(%error, %file_path, "failed to watch file");
            }
        }
    }

    Ok(editor)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();
    let config = parse_args();

    tracing::info!("starting Vello frontend");
    let runtime = compio::runtime::Runtime::new()?;

    let recovery = config.recovery.clone();
    let editor = runtime.block_on(create_editor(config))?;

    // Run with Vello renderer
    roe_vello::run_vello_with_recovery(editor, runtime, recovery)?;
    tracing::info!("Vello frontend stopped");

    Ok(())
}
