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

use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::disable_raw_mode;
use roe_core::{
    buffer_host, command_registry, editor,
    julia_runtime::{clear_current_buffer, set_current_buffer},
    kill_ring, mode, Buffer, BufferId, ConfigurableBindings, Editor, Frame, KeyState, Mode, ModeId,
    Renderer, Window, WindowId,
};
use roe_terminal::{TerminalRenderer, ECHO_AREA_HEIGHT};
use slotmap::SlotMap;
use std::collections::HashMap;
use std::io::Write;

/// Parse command line arguments
fn parse_args() -> EditorConfig {
    let args: Vec<String> = std::env::args().collect();
    let mut file_paths = Vec::new();
    let mut init_file = None;
    let mut i = 1; // Skip program name

    while i < args.len() {
        match args[i].as_str() {
            "--init" | "-i" => {
                // Next argument should be the init file path
                if i + 1 < args.len() {
                    init_file = Some(args[i + 1].clone());
                    i += 2;
                } else {
                    eprintln!("Error: --init requires a file path");
                    std::process::exit(1);
                }
            }
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
                // Regular file argument
                file_paths.push(args[i].clone());
                i += 1;
            }
        }
    }

    EditorConfig {
        file_paths,
        init_file,
    }
}

/// Print help message
fn print_help() {
    println!("Roe - Ryan's Own Emacs");
    println!();
    println!("USAGE:");
    println!("    roe [OPTIONS] [FILES...]");
    println!();
    println!("OPTIONS:");
    println!("    -i, --init <FILE>    Specify Julia init file (default: init.jl)");
    println!("    -h, --help           Print this help message");
    println!();
    println!("EXAMPLES:");
    println!("    roe                          # Start with welcome screen");
    println!("    roe file.txt                 # Open file.txt");
    println!("    roe file1.txt file2.txt      # Open multiple files");
    println!("    roe --init myconfig.jl       # Use custom init file");
    println!("    roe -i ~/.config/init.jl main.rs   # Custom init + file");
}

/// Generate welcome screen content with ASCII art logo and getting started text
fn create_welcome_screen_content() -> String {
    // Include the ASCII art from rune.txt at compile time
    const RUNE_ART: &str = include_str!("../../rune.txt");

    let mut content = String::new();

    // Add the ASCII art
    content.push_str(RUNE_ART);

    // Add some spacing
    content.push_str("\n\n");

    // Add centered title - we'll center it manually for now
    let title = "ROE - Ryan's Own Emacs";
    let title_padding = " ".repeat(20); // Rough centering
    content.push_str(&format!("{title_padding}{title}\n\n"));

    // Add getting started information
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

// Configuration for the editor
struct EditorConfig {
    file_paths: Vec<String>,
    init_file: Option<String>,
}

// Everything to run in raw_mode
async fn terminal_main<W: Write>(stdout: W, config: EditorConfig) -> Result<(), std::io::Error> {
    assert!(crossterm::terminal::is_raw_mode_enabled()?);
    let _ws = crossterm::terminal::window_size()?;

    // Set the size of the screen
    assert_ne!(crossterm::terminal::size()?, (0, 0));

    let tsize = crossterm::terminal::size()?;

    // Initialize Julia runtime with Arc for sharing
    let julia_runtime = roe_core::julia_runtime::create_shared_runtime().ok();

    // Load Julia configuration and keybindings early, before creating Editor
    let mut bindings = ConfigurableBindings::new();
    if let Some(ref julia_runtime) = julia_runtime {
        let config_path = if let Some(init_file) = &config.init_file {
            std::path::PathBuf::from(init_file)
        } else {
            roe_core::julia_runtime::RoeJuliaRuntime::default_config_path()
        };

        let runtime = julia_runtime.lock().await;

        // Load the Roe module first (provides command infrastructure and default keybindings)
        if let Some(roe_module_path) =
            roe_core::julia_runtime::RoeJuliaRuntime::bundled_roe_module_path()
        {
            if let Err(e) = runtime.load_roe_module(roe_module_path.clone()).await {
                eprintln!("Fatal: Failed to load Roe Julia module: {}", e);
                eprintln!("The editor cannot start without the Roe module.");
                std::process::exit(1);
            }
        } else {
            eprintln!("Fatal: Could not find Roe Julia module (jl/roe.jl)");
            eprintln!("Make sure to run from the roe directory or install properly.");
            std::process::exit(1);
        }
        drop(runtime);

        // Load user config (may override default keybindings)
        let mut runtime = julia_runtime.lock().await;
        let _ = runtime.load_config(Some(config_path)).await;
        drop(runtime);

        // Query keybindings from Julia and populate ConfigurableBindings
        let runtime = julia_runtime.lock().await;
        if let Ok(julia_bindings) = runtime.list_keybindings().await {
            for (key_seq, action) in julia_bindings {
                bindings.add_binding(&key_seq, &action);
            }
        }
        drop(runtime);
    }

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

        // Create BufferHost with MessagesMode for the welcome buffer
        let welcome_mode = modes
            .remove(welcome_mode_id)
            .expect("MessagesMode should exist in modes SlotMap");
        let mode_list = vec![(welcome_mode_id, "welcome".to_string(), welcome_mode)];

        let (buffer_client, _buffer_handle) =
            buffer_host::create_buffer_host(buffer, mode_list, buffer_id, julia_runtime.clone());
        buffer_hosts.insert(buffer_id, buffer_client);
    } else {
        // Create buffers for all specified files
        for file_path in config.file_paths {
            // Create FileMode for this file
            let file_mode = Box::new(mode::FileMode {
                file_path: file_path.clone(),
            });
            let file_mode_id = modes.insert(file_mode);

            // Try to load the file, create empty buffer if it doesn't exist
            let buffer = match Buffer::from_file(&file_path, &[file_mode_id]).await {
                Ok(buffer) => buffer,
                Err(_) => {
                    // File doesn't exist, create empty buffer with FileMode
                    let buffer = Buffer::new(&[file_mode_id]);
                    buffer.set_object(file_path.clone());
                    buffer
                }
            };

            // Get and apply major mode for this file
            if let Some(ref jr) = julia_runtime {
                let runtime = jr.lock().await;
                if let Ok(major_mode) = runtime.get_major_mode_for_file(&file_path).await {
                    buffer.set_major_mode(major_mode.clone());

                    // Call the major mode's init hook
                    set_current_buffer(buffer.clone());
                    let _ = runtime.call_major_mode_init(&major_mode).await;
                    clear_current_buffer();
                }
                drop(runtime);
            }

            let buffer_id = buffers.insert(buffer.clone());

            // Remember the first buffer for the initial window
            if first_buffer_id.is_none() {
                first_buffer_id = Some(buffer_id);
            }

            // Create BufferHost with mode for this buffer
            let file_mode = modes
                .remove(file_mode_id)
                .expect("FileMode should exist in modes SlotMap");
            let mode_list = vec![(file_mode_id, "file".to_string(), file_mode)];

            // Create BufferHost and client
            let (buffer_client, _buffer_handle) = buffer_host::create_buffer_host(
                buffer,
                mode_list,
                buffer_id,
                julia_runtime.clone(),
            );
            buffer_hosts.insert(buffer_id, buffer_client);
        }
    }

    // Create windows - split horizontally if we have 2+ files, single window otherwise
    let mut windows: SlotMap<WindowId, Window> = SlotMap::default();
    let window_tree;
    let active_window_id;

    let buffer_ids: Vec<BufferId> = buffers.keys().collect();

    if buffer_ids.len() >= 2 {
        // Two-window horizontal split - frame already accounts for echo area
        let available_height = tsize.1 - ECHO_AREA_HEIGHT;
        let window_height = available_height / 2;

        // Top window (first file)
        let top_window = Window {
            x: 0,
            y: 0,
            width_chars: tsize.0,
            height_chars: window_height,
            active_buffer: buffer_ids[0],
            start_line: 0,
            start_column: 0,
            cursor: 0,
            window_type: editor::WindowType::Normal,
        };
        let top_window_id = windows.insert(top_window);

        // Bottom window (second file)
        let bottom_window = Window {
            x: 0,
            y: window_height,
            width_chars: tsize.0,
            height_chars: available_height - window_height,
            active_buffer: buffer_ids[1],
            start_line: 0,
            start_column: 0,
            cursor: 0,
            window_type: editor::WindowType::Normal,
        };
        let bottom_window_id = windows.insert(bottom_window);

        // Create horizontal split tree
        window_tree = editor::WindowNode::new_split(
            editor::SplitDirection::Horizontal,
            0.5, // 50/50 split
            editor::WindowNode::new_leaf(top_window_id),
            editor::WindowNode::new_leaf(bottom_window_id),
        );

        active_window_id = top_window_id; // Start with top window active
    } else {
        // Single window (full screen)
        let active_buffer = first_buffer_id.expect("Should have at least one buffer");
        let window = Window {
            x: 0,
            y: 0,
            width_chars: tsize.0,
            height_chars: tsize.1 - ECHO_AREA_HEIGHT,
            active_buffer,
            start_line: 0,
            start_column: 0,
            cursor: 0,
            window_type: editor::WindowType::Normal,
        };
        active_window_id = windows.insert(window);
        window_tree = editor::WindowNode::new_leaf(active_window_id);
    }

    // Initialize file watcher
    let mut file_watcher = roe_core::file_watcher::FileWatcher::new();
    if let Err(e) = file_watcher.init() {
        eprintln!("Warning: Failed to initialize file watcher: {e}");
    }

    let mut editor = Editor {
        frame: Frame::new(tsize.0, tsize.1 - ECHO_AREA_HEIGHT),
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
        julia_runtime,
        file_watcher,
        last_search_term: String::new(),
    };

    // Initialize buffer history with the current buffer
    let initial_buffer_id = editor.windows[active_window_id].active_buffer;
    editor.record_buffer_access(initial_buffer_id);

    // Register file-backed buffers with the file watcher
    for (buffer_id, buffer) in &editor.buffers {
        let file_path = buffer.object();
        if !file_path.is_empty() && std::path::Path::new(&file_path).exists() {
            let content = buffer.content();
            if let Err(e) =
                editor
                    .file_watcher
                    .watch_file(buffer_id, std::path::Path::new(&file_path), content)
            {
                eprintln!("Warning: Failed to watch file {file_path}: {e}");
            }
        }
    }

    // Register Julia commands into the command registry
    // (Julia runtime and config were already loaded earlier for keybindings)
    if let Some(ref julia_runtime) = editor.julia_runtime {
        command_registry::register_julia_commands(&mut editor.command_registry, julia_runtime)
            .await;
    }

    // Load Julia theme and create terminal renderer with it
    let julia_theme = if editor.julia_runtime.is_some() {
        roe_terminal::terminal_renderer::load_julia_theme(&editor).await
    } else {
        roe_terminal::terminal_renderer::CachedTheme::default()
    };

    let mut renderer = TerminalRenderer::new_with_theme(stdout, julia_theme);

    // Initial full render
    renderer.render_full(&editor)?;

    // Event loop with renderer
    roe_terminal::terminal_renderer::event_loop_with_renderer(&mut renderer, &mut editor).await?;

    Ok(())
}

fn exit_state(device: &mut impl Write) -> Result<(), std::io::Error> {
    // Restore terminal to original state
    execute!(device, DisableMouseCapture)?;
    execute!(device, crossterm::cursor::Show)?;
    execute!(device, crossterm::cursor::SetCursorStyle::DefaultUserShape)?;
    execute!(device, PopKeyboardEnhancementFlags)?;
    device.flush()?;

    disable_raw_mode()?;

    execute!(
        device,
        crossterm::terminal::Clear(crossterm::terminal::ClearType::All)
    )?;
    let (_, height) = crossterm::terminal::size().unwrap_or((80, 24));
    execute!(device, crossterm::cursor::MoveTo(0, height))?;
    device.flush()?;

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), std::io::Error> {
    // Set panic handler to clean up terminal state while preserving panic info
    std::panic::set_hook(Box::new(|panic_info| {
        let _ = exit_state(&mut std::io::stdout());
        eprintln!("💥 Roe has crashed! This shouldn't happen - please file a bug report at:");
        eprintln!("   https://github.com/rdaum/roe/issues");
        eprintln!();
        eprintln!("Include the following crash details in your report:");
        eprintln!("{panic_info}");
    }));

    let mut stdout = std::io::stdout();

    // Parse command line arguments
    let config = parse_args();

    // Set up terminal state
    crossterm::terminal::enable_raw_mode()?;
    execute!(
        stdout,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    )?;
    execute!(stdout, crossterm::cursor::EnableBlinking)?;
    execute!(stdout, EnableMouseCapture)?;

    // Run the application
    let result = terminal_main(&mut stdout, config).await;

    // Always clean up terminal state, regardless of success or failure
    if let Err(cleanup_err) = exit_state(&mut stdout) {
        eprintln!("Warning: Failed to clean up terminal state: {cleanup_err}");
    }

    // Handle the main result
    if let Err(e) = result {
        eprintln!("Error: {e}");
        return Err(e);
    }

    Ok(())
}
