use buffer::Buffer;
use crossterm::event::{KeyboardEnhancementFlags, PushKeyboardEnhancementFlags};
use crossterm::execute;
use editor::{Editor, Frame, Window};
use keys::KeyState;
use mode::{FileMode, Mode};
use renderer::Renderer;
use slotmap::{new_key_type, SlotMap};
use std::collections::HashMap;
use std::io::Write;
use terminal_renderer::TerminalRenderer;
use terminal_renderer::ECHO_AREA_HEIGHT;

mod buffer;
mod buffer_host;
mod buffer_switch_mode;
mod command_mode;
mod command_registry;
mod editor;
mod keys;
mod kill_ring;
mod mode;
mod renderer;
mod terminal_renderer;
mod window;

new_key_type! {
    pub struct WindowId;
}

new_key_type! {
    pub struct BufferId;
}

new_key_type! {
    pub struct ModeId;
}

// Everything to run in raw_mode
async fn terminal_main<W: Write>(stdout: W, file_paths: Vec<String>) -> Result<(), std::io::Error> {
    assert!(crossterm::terminal::is_raw_mode_enabled()?);
    let _ws = crossterm::terminal::window_size()?;

    // Set the size of the screen
    assert_ne!(crossterm::terminal::size()?, (0, 0));

    let tsize = crossterm::terminal::size()?;

    let mut buffers: SlotMap<BufferId, Buffer> = SlotMap::default();
    let mut buffer_hosts: HashMap<BufferId, buffer_host::BufferHostClient> = HashMap::new();
    let mut modes: SlotMap<ModeId, Box<dyn Mode>> = SlotMap::default();

    // Determine which files to open
    let files_to_open = if file_paths.is_empty() {
        // No files specified, try README.md as fallback
        vec!["README.md".to_string()]
    } else {
        file_paths
    };

    let mut first_buffer_id = None;

    // Create buffers for all specified files
    for file_path in files_to_open {
        // Create FileMode for this file
        let file_mode = Box::new(FileMode {
            file_path: file_path.clone(),
        });
        let file_mode_id = modes.insert(file_mode);

        // Try to load the file, create empty buffer if it doesn't exist
        let buffer = match Buffer::from_file(&file_path, &[]).await {
            Ok(buffer) => buffer,
            Err(_) => {
                // File doesn't exist, create empty buffer
                let buffer = Buffer::new(&[]);
                buffer.set_object(file_path.clone());
                if file_path == "README.md" {
                    // Special case for README.md - add default content
                    buffer.load_str("# README\n\nThis is a new file created by the red editor.\nTry typing some text and press Ctrl-X Ctrl-S to save!\n");
                }
                buffer
            }
        };

        let buffer_id = buffers.insert(buffer.clone());

        // Remember the first buffer for the initial window
        if first_buffer_id.is_none() {
            first_buffer_id = Some(buffer_id);
        }

        // Create BufferHost with mode for this buffer
        let file_mode = modes.remove(file_mode_id).unwrap();
        let mode_list = vec![(file_mode_id, "file".to_string(), file_mode)];

        // Create BufferHost and client
        let (buffer_client, _buffer_handle) =
            buffer_host::create_buffer_host(buffer, mode_list, buffer_id);
        buffer_hosts.insert(buffer_id, buffer_client);
    }

    // Create windows - split horizontally if we have 2+ files, single window otherwise
    let mut windows: SlotMap<WindowId, Window> = SlotMap::default();
    let window_tree;
    let active_window_id;

    let buffer_ids: Vec<BufferId> = buffers.keys().collect();

    if buffer_ids.len() >= 2 {
        // Two-window horizontal split
        let window_height = (tsize.1 - ECHO_AREA_HEIGHT) / 2;

        // Top window (first file)
        let top_window = Window {
            x: 0,
            y: 0,
            width_chars: tsize.0,
            height_chars: window_height,
            active_buffer: buffer_ids[0],
            start_line: 0,
            cursor: 0,
            window_type: editor::WindowType::Normal,
        };
        let top_window_id = windows.insert(top_window);

        // Bottom window (second file)
        let bottom_window = Window {
            x: 0,
            y: window_height,
            width_chars: tsize.0,
            height_chars: (tsize.1 - ECHO_AREA_HEIGHT) - window_height,
            active_buffer: buffer_ids[1],
            start_line: 0,
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
            cursor: 0,
            window_type: editor::WindowType::Normal,
        };
        active_window_id = windows.insert(window);
        window_tree = editor::WindowNode::new_leaf(active_window_id);
    }

    let mut editor = Editor {
        frame: Frame::new(tsize.0, tsize.1),
        buffers,
        buffer_hosts,
        windows,
        modes,
        active_window: active_window_id,
        previous_active_window: None,
        key_state: KeyState::new(),
        bindings: Box::new(keys::DefaultBindings {}),
        window_tree,
        kill_ring: kill_ring::KillRing::new(),
        command_registry: command_registry::create_default_registry(),
    };

    // Create terminal renderer
    let mut renderer = TerminalRenderer::new(stdout);

    // Initial full render
    renderer.render_full(&editor)?;

    // Event loop with renderer
    terminal_renderer::event_loop_with_renderer(&mut renderer, &mut editor).await?;

    Ok(())
}

fn exit_state(device: &mut impl Write) -> Result<(), std::io::Error> {
    execute!(
        device,
        crossterm::terminal::Clear(crossterm::terminal::ClearType::All)
    )?;
    execute!(device, crossterm::cursor::Show)?;
    crossterm::terminal::disable_raw_mode()?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), std::io::Error> {
    let mut stdout = std::io::stdout();

    // Collect command line arguments (skip the program name)
    let file_paths: Vec<String> = std::env::args().skip(1).collect();

    crossterm::terminal::enable_raw_mode()?;
    // Disambiguate keyboard modifier codes
    execute!(
        stdout,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    )?;
    execute!(stdout, crossterm::cursor::EnableBlinking)?;
    if let Err(e) = terminal_main(&mut stdout, file_paths).await {
        exit_state(&mut stdout)?;
        eprintln!("Error: {e}");
        return Err(e);
    }

    exit_state(&mut stdout)?;

    Ok(())
}
