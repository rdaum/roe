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

use roe_core::{
    buffer_host, command_registry, editor, kill_ring, mode, Buffer, BufferId,
    ConfigurableBindings, Editor, Frame, KeyState, Mode, ModeId, Window, WindowId,
};
use slotmap::SlotMap;
use std::collections::HashMap;

/// Default window size in character cells (will be adjusted by actual window size)
const DEFAULT_COLS: u16 = 120;
const DEFAULT_LINES: u16 = 40;

/// Parse command line arguments
fn parse_args() -> EditorConfig {
    let args: Vec<String> = std::env::args().collect();
    let mut file_paths = Vec::new();
    let mut i = 1; // Skip program name

    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
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

    EditorConfig { file_paths }
}

fn print_help() {
    println!("Roe (Vello) - Ryan's Own Emacs with GPU rendering");
    println!();
    println!("USAGE:");
    println!("    roe-vello [OPTIONS] [FILES...]");
    println!();
    println!("OPTIONS:");
    println!("    -h, --help           Print this help message");
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
}

async fn create_editor(config: EditorConfig) -> Editor {
    // Default keybindings ship in Rust; the scripting runtime (mica) will be
    // able to extend them once integrated.
    let bindings = ConfigurableBindings::new();

    let mut buffers: SlotMap<BufferId, Buffer> = SlotMap::default();
    let mut buffer_hosts: HashMap<BufferId, buffer_host::BufferHostClient> = HashMap::new();
    let mut modes: SlotMap<ModeId, Box<dyn Mode>> = SlotMap::default();

    let mut first_buffer_id = None;

    if config.file_paths.is_empty() {
        // No files specified, create welcome screen buffer
        let welcome_mode = Box::new(mode::MessagesMode {});
        let welcome_mode_id = modes.insert(welcome_mode);

        let buffer = Buffer::new(&[welcome_mode_id]);
        buffer.set_object("*Welcome*".to_string());
        buffer.load_str(&create_welcome_screen_content());

        let buffer_id = buffers.insert(buffer.clone());
        first_buffer_id = Some(buffer_id);

        let welcome_mode = modes
            .remove(welcome_mode_id)
            .expect("MessagesMode should exist");
        let mode_list = vec![(welcome_mode_id, "welcome".to_string(), welcome_mode)];

        let buffer_client =
            buffer_host::create_buffer_host(buffer, mode_list, buffer_id);
        buffer_hosts.insert(buffer_id, buffer_client);
    } else {
        // Create buffers for all specified files
        for file_path in config.file_paths {
            let file_mode = Box::new(mode::FileMode {
                file_path: file_path.clone(),
            });
            let file_mode_id = modes.insert(file_mode);

            let buffer = match Buffer::from_file(&file_path, &[file_mode_id]).await {
                Ok(buffer) => buffer,
                Err(_) => {
                    let buffer = Buffer::new(&[file_mode_id]);
                    buffer.set_object(file_path.clone());
                    buffer
                }
            };

            let buffer_id = buffers.insert(buffer.clone());

            if first_buffer_id.is_none() {
                first_buffer_id = Some(buffer_id);
            }

            let file_mode = modes.remove(file_mode_id).expect("FileMode should exist");
            let mode_list = vec![(file_mode_id, "file".to_string(), file_mode)];

            let buffer_client =
                buffer_host::create_buffer_host(buffer, mode_list, buffer_id);
            buffer_hosts.insert(buffer_id, buffer_client);
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
    let _ = file_watcher.init(); // Ignore errors for now

    let mut editor = Editor {
        frame: Frame::new(DEFAULT_COLS, DEFAULT_LINES),
        buffers,
        buffer_hosts,
        windows,
        modes,
        active_window: active_window_id,
        previous_active_window: None,
        key_state: KeyState::new(),
        bindings: Box::new(bindings),
        window_tree,
        kill_ring: kill_ring::KillRing::new(),
        command_registry: command_registry::create_default_registry(),
        buffer_history: Vec::new(),
        echo_message: String::new(),
        echo_message_time: None,
        current_key_chord: Vec::new(),
        mouse_drag_state: None,
        messages_buffer_id: None,
        file_watcher,
        last_search_term: String::new(),
    };

    // Initialize buffer history
    editor.record_buffer_access(active_buffer);

    // Register file-backed buffers with the file watcher
    for (buffer_id, buffer) in &editor.buffers {
        let file_path = buffer.object();
        if !file_path.is_empty() && std::path::Path::new(&file_path).exists() {
            let content = buffer.content();
            let _ = editor.file_watcher.watch_file(
                buffer_id,
                std::path::Path::new(&file_path),
                content,
            );
        }
    }

    editor
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = parse_args();

    let runtime =
        compio::runtime::Runtime::new().expect("Failed to create compio runtime");

    let mut editor = runtime.block_on(create_editor(config));

    // Run with Vello renderer
    roe_vello::run_vello(&mut editor, runtime)?;

    Ok(())
}
