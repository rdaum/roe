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
    Buffer, BufferId, ConfigurableBindings, Editor, Frame, KeyState, Mode, ModeId, Renderer,
    Window, WindowId, buffer_host, command_registry, editor, kill_ring, mode,
};
use roe_terminal::{ECHO_AREA_HEIGHT, TerminalRenderer};
use slotmap::SlotMap;
use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

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
                // Regular file argument
                file_paths.push(args[i].clone());
                i += 1;
            }
        }
    }

    EditorConfig { file_paths }
}

/// Print help message
fn print_help() {
    println!("Roe - Ryan's Own Emacs");
    println!();
    println!("USAGE:");
    println!("    roe [OPTIONS] [FILES...]");
    println!();
    println!("OPTIONS:");
    println!("    -h, --help           Print this help message");
    println!();
    println!("EXAMPLES:");
    println!("    roe                          # Start with welcome screen");
    println!("    roe file.txt                 # Open file.txt");
    println!("    roe file1.txt file2.txt      # Open multiple files");
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
}

struct TerminalSession<W: Write> {
    device: W,
    active: bool,
}

impl<W: Write> TerminalSession<W> {
    fn enter(device: W) -> Result<Self, std::io::Error> {
        crossterm::terminal::enable_raw_mode()?;
        let mut session = Self {
            device,
            active: true,
        };
        execute!(
            session.device,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
        execute!(session.device, crossterm::cursor::EnableBlinking)?;
        execute!(session.device, EnableMouseCapture)?;
        Ok(session)
    }

    fn device_mut(&mut self) -> &mut W {
        &mut self.device
    }

    fn cleanup(&mut self) -> Result<(), std::io::Error> {
        if !self.active {
            return Ok(());
        }
        exit_state(&mut self.device)?;
        self.active = false;
        Ok(())
    }
}

impl<W: Write> Drop for TerminalSession<W> {
    fn drop(&mut self) {
        if let Err(error) = self.cleanup() {
            eprintln!("Warning: Failed to clean up terminal state: {error}");
        }
    }
}

fn install_signal_handlers(shutdown_requested: Arc<AtomicBool>) -> Result<(), std::io::Error> {
    signal_hook::flag::register(signal_hook::consts::SIGINT, shutdown_requested.clone())?;
    signal_hook::flag::register(signal_hook::consts::SIGTERM, shutdown_requested)?;
    Ok(())
}

// Everything to run in raw_mode
async fn terminal_main<W: Write>(
    stdout: W,
    config: EditorConfig,
    shutdown_requested: &AtomicBool,
) -> Result<(), std::io::Error> {
    assert!(crossterm::terminal::is_raw_mode_enabled()?);
    let _ws = crossterm::terminal::window_size()?;

    // Set the size of the screen
    assert_ne!(crossterm::terminal::size()?, (0, 0));

    let tsize = crossterm::terminal::size()?;

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

        // Create BufferHost with MessagesMode for the welcome buffer
        let welcome_mode = modes
            .remove(welcome_mode_id)
            .expect("MessagesMode should exist in modes SlotMap");
        let mode_list = vec![(welcome_mode_id, "welcome".to_string(), welcome_mode)];

        let buffer_client = buffer_host::create_buffer_host(buffer, mode_list, buffer_id);
        buffer_hosts.insert(buffer_id, buffer_client);
    } else {
        // Create buffers for all specified files
        for file_path in config.file_paths {
            // Create FileMode for this file
            let file_mode = Box::new(mode::FileMode {
                file_path: file_path.clone(),
            });
            let file_mode_id = modes.insert(file_mode);

            // Load an existing file, or create a new buffer only when it is absent.
            let buffer = match Buffer::from_file(&file_path, &[file_mode_id]).await {
                Ok(buffer) => buffer,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    let buffer = Buffer::new(&[file_mode_id]);
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
            let buffer_client = buffer_host::create_buffer_host(buffer, mode_list, buffer_id);
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
        clock: std::sync::Arc::new(roe_core::native_services::SystemClock),
        current_key_chord: Vec::new(),
        mouse_drag_state: None,
        messages_buffer_id: None,
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

    // Theme configuration will come from the scripting runtime (mica) once
    // integrated; use defaults for now.
    let theme = roe_terminal::terminal_renderer::CachedTheme::default();

    let mut renderer = TerminalRenderer::new_with_theme(stdout, theme);

    // Initial full render
    renderer.render_full(&editor)?;

    // Event loop with renderer
    roe_terminal::terminal_renderer::event_loop_with_renderer(
        &mut renderer,
        &mut editor,
        shutdown_requested,
    )
    .await?;

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

fn main() -> Result<(), std::io::Error> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    // Preserve panic diagnostics. TerminalSession performs cleanup while the
    // stack unwinds, including failures after partial terminal setup.
    std::panic::set_hook(Box::new(|panic_info| {
        eprintln!("💥 Roe has crashed! This shouldn't happen - please file a bug report at:");
        eprintln!("   https://github.com/rdaum/roe/issues");
        eprintln!();
        eprintln!("Include the following crash details in your report:");
        eprintln!("{panic_info}");
    }));

    // Parse command line arguments
    let config = parse_args();
    let shutdown_requested = Arc::new(AtomicBool::new(false));
    install_signal_handlers(shutdown_requested.clone())?;

    tracing::info!("starting terminal frontend");
    let mut terminal = TerminalSession::enter(std::io::stdout())?;

    let result = compio::runtime::Runtime::new()?.block_on(terminal_main(
        terminal.device_mut(),
        config,
        shutdown_requested.as_ref(),
    ));

    terminal.cleanup()?;
    tracing::info!("terminal frontend stopped");

    // Handle the main result
    if let Err(e) = result {
        eprintln!("Error: {e}");
        return Err(e);
    }

    Ok(())
}
