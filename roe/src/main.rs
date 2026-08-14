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
use roe_core::buffer::BufferKind;
use roe_core::native_kernel::CapabilityGrants;
use roe_core::session::{
    AttachmentConfiguration, DirectSessionClient, LifecycleEvent, SessionClient,
    StartupRecoveryOperation, WorkspaceHost,
};
use roe_core::{Buffer, BufferId, Editor, Frame, Window, WindowId, editor, kill_ring};
use roe_terminal::{ECHO_AREA_HEIGHT, TerminalRenderer};
use slotmap::SlotMap;
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

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
                // Regular file argument
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

/// Print help message
fn print_help() {
    println!("Roe - Ryan's Own Emacs");
    println!();
    println!("USAGE:");
    println!("    roe [OPTIONS] [FILES...]");
    println!();
    println!("OPTIONS:");
    println!("    -h, --help           Print this help message");
    println!("    --mica-check FILE    Validate Mica source before entering the editor");
    println!("    --mica-replace UNIT FILE  Replace a named Mica unit");
    println!("    --mica-export UNIT FILE   Export a named Mica unit");
    println!("    --mica-restore-first-wave Restore built-in editor policy");
    println!("    --mica-enable-package PACKAGE / --mica-disable-package PACKAGE");
    println!("    --mica-inspect       Show recovery host diagnostics");
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
    recovery: Vec<StartupRecoveryOperation>,
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

    let mut buffers: SlotMap<BufferId, Buffer> = SlotMap::default();
    let EditorConfig {
        file_paths,
        recovery,
    } = config;

    buffers.insert(Buffer::named("*scratch*", BufferKind::Scratch));
    let mut startup_buffer_ids = Vec::new();

    if file_paths.is_empty() {
        // No files specified, create welcome screen buffer
        let buffer = Buffer::named("*Welcome*", BufferKind::Internal);
        buffer.load_str(&create_welcome_screen_content());
        buffer.set_read_only(true);

        let buffer_id = buffers.insert(buffer);
        startup_buffer_ids.push(buffer_id);
    } else {
        // Create buffers for all specified files
        for file_path in file_paths {
            // Load an existing file, or create a new buffer only when it is absent.
            let buffer = match Buffer::from_file(&file_path).await {
                Ok(buffer) => buffer,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    Buffer::visiting(file_path.clone())
                }
                Err(error) => {
                    return Err(std::io::Error::new(
                        error.kind(),
                        format!("failed to open {file_path}: {error}"),
                    ));
                }
            };

            let buffer_id = buffers.insert(buffer);

            startup_buffer_ids.push(buffer_id);
        }
    }

    // Create windows - split horizontally if we have 2+ files, single window otherwise
    let mut windows: SlotMap<WindowId, Window> = SlotMap::default();
    let window_tree;
    let active_window_id;

    let buffer_ids = startup_buffer_ids;

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
        let active_buffer = *buffer_ids
            .first()
            .expect("startup always displays a buffer");
        let window = Window {
            x: 0,
            y: 0,
            width_chars: tsize.0,
            height_chars: tsize.1 - ECHO_AREA_HEIGHT,
            active_buffer,
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
        windows,
        active_window: active_window_id,
        previous_active_window: None,
        window_tree,
        kill_ring: kill_ring::KillRing::with_capacity(60),
        buffer_history: Vec::new(),
        echo_message: String::new(),
        echo_message_time: None,
        clock: std::sync::Arc::new(roe_core::native_services::SystemClock),
        mouse_drag_state: None,
        messages_buffer_id: None,
        file_watcher,
    };

    // Initialize buffer history with the current buffer
    let initial_buffer_id = editor.windows[active_window_id].active_buffer;
    editor.record_buffer_access(initial_buffer_id);

    // Register file-backed buffers with the file watcher
    for (buffer_id, buffer) in &editor.buffers {
        if let Some(file_path) = buffer.visited_file()
            && file_path.exists()
        {
            let content = buffer.content();
            if let Err(e) = editor
                .file_watcher
                .watch_file(buffer_id, &file_path, content)
            {
                eprintln!("Warning: Failed to watch file {}: {e}", file_path.display());
            }
        }
    }

    // Phase 5 keeps theme realization native; Mica face/configuration
    // relations can describe future theme changes without owning ANSI state.
    let theme = roe_terminal::terminal_renderer::CachedTheme::default();

    let mut renderer = TerminalRenderer::new_with_theme(stdout, theme);
    let attachment = AttachmentConfiguration::local_frontend(
        editor.frame.available_columns,
        editor.frame.available_lines,
    );
    let mut workspace = WorkspaceHost::open_with_mica(editor, CapabilityGrants::editor_default())
        .map_err(|error| {
        std::io::Error::other(format!("failed to start Mica host: {error}"))
    })?;

    let recovery_reports = workspace
        .execute_startup_recovery(&recovery)
        .await
        .map_err(std::io::Error::other)?;
    if let Some(report) = recovery_reports.last() {
        workspace.set_recovery_message(report.clone());
    }

    let mut session = DirectSessionClient::new(workspace, attachment);

    let initial = session.initial_output().await;
    if let Some(update) = initial.presentation.as_ref() {
        renderer.apply_session_presentation(update)?;
    }
    renderer.render_session()?;

    let event_loop_result = roe_terminal::terminal_renderer::session_event_loop_with_renderer(
        &mut renderer,
        &mut session,
        shutdown_requested,
    )
    .await;

    match session.terminate_workspace().await {
        Ok(output) => {
            for event in output.lifecycle {
                if let LifecycleEvent::Warning(error) = event {
                    tracing::warn!(%error, "editor shutdown warning");
                }
            }
        }
        Err(error) => tracing::warn!(%error, "session shutdown warning"),
    }
    event_loop_result?;

    Ok(())
}

fn exit_state(device: &mut impl Write) -> Result<(), std::io::Error> {
    exit_state_with(device, disable_raw_mode)
}

fn exit_state_with(
    device: &mut impl Write,
    restore_raw_mode: impl FnOnce() -> Result<(), std::io::Error>,
) -> Result<(), std::io::Error> {
    let mut first_error = None;
    let mut retain_error = |result: Result<(), std::io::Error>| {
        if let Err(error) = result
            && first_error.is_none()
        {
            first_error = Some(error);
        }
    };

    // Every presentation reset is best effort. Raw-mode restoration must run
    // even if the output device has already failed.
    retain_error(execute!(device, DisableMouseCapture));
    retain_error(execute!(device, crossterm::cursor::Show));
    retain_error(execute!(
        device,
        crossterm::cursor::SetCursorStyle::DefaultUserShape
    ));
    retain_error(execute!(device, PopKeyboardEnhancementFlags));
    retain_error(device.flush());
    retain_error(restore_raw_mode());

    retain_error(execute!(
        device,
        crossterm::terminal::Clear(crossterm::terminal::ClearType::All)
    ));
    let (_, height) = crossterm::terminal::size().unwrap_or((80, 24));
    retain_error(execute!(device, crossterm::cursor::MoveTo(0, height)));
    retain_error(device.flush());

    first_error.map_or(Ok(()), Err)
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

#[cfg(test)]
mod tests {
    use super::exit_state_with;
    use std::cell::Cell;
    use std::io::{self, Write};

    struct FailedTerminal;

    impl Write for FailedTerminal {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "terminal gone"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "terminal gone"))
        }
    }

    #[test]
    fn raw_mode_restoration_runs_after_terminal_output_failure() {
        let restored = Cell::new(false);
        let error = exit_state_with(&mut FailedTerminal, || {
            restored.set(true);
            Ok(())
        })
        .expect_err("the original terminal output error must be retained");

        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        assert!(restored.get());
    }
}
