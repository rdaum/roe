use buffer::Buffer;
use crossterm::event::{KeyboardEnhancementFlags, PushKeyboardEnhancementFlags};
use crossterm::execute;
use editor::{Editor, Frame, Window};
use keys::KeyState;
use mode::{Mode, ScratchMode};
use renderer::Renderer;
use slotmap::{new_key_type, SlotMap};
use std::io::Write;
use terminal_renderer::TerminalRenderer;
use terminal_renderer::ECHO_AREA_HEIGHT;

mod buffer;
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
fn terminal_main<W: Write>(stdout: W) -> Result<(), std::io::Error> {
    assert!(crossterm::terminal::is_raw_mode_enabled()?);
    let _ws = crossterm::terminal::window_size()?;

    // Set the size of the screen
    assert!(crossterm::terminal::size()? != (0, 0));

    let tsize = crossterm::terminal::size()?;

    let scratch_mode = Box::new(ScratchMode {});

    let mut modes: SlotMap<ModeId, Box<dyn Mode>> = SlotMap::default();
    let scratch_mode_id = modes.insert(scratch_mode);

    let mut buffers: SlotMap<BufferId, Buffer> = SlotMap::default();
    let scratch_buffer = Buffer {
        object: "** scratch **".to_string(),
        modes: vec![scratch_mode_id],
        buffer: ropey::Rope::from_str("scratch content"),
        mark: None,
    };
    let scratch_buffer_id = buffers.insert(scratch_buffer);
    let initial_window = Window {
        x: 0,
        y: 0,
        width_chars: tsize.0,
        height_chars: tsize.1 - ECHO_AREA_HEIGHT,
        active_buffer: scratch_buffer_id,
        start_line: 0,
        cursor: buffers[scratch_buffer_id].buffer.len_chars(),
    };
    let mut windows: SlotMap<WindowId, Window> = SlotMap::default();
    let initial_window_id = windows.insert(initial_window);

    let mut editor = Editor {
        frame: Frame::new(tsize.0, tsize.1),
        buffers,
        windows,
        modes,
        active_window: initial_window_id,
        key_state: KeyState::new(),
        bindings: Box::new(keys::DefaultBindings {}),
        window_tree: editor::WindowNode::new_leaf(initial_window_id),
        kill_ring: kill_ring::KillRing::new(),
    };

    // Create terminal renderer
    let mut renderer = TerminalRenderer::new(stdout);

    // Initial full render
    renderer.render_full(&editor)?;

    // Event loop with renderer
    terminal_renderer::event_loop_with_renderer(&mut renderer, &mut editor)?;

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

fn main() -> Result<(), std::io::Error> {
    let mut stdout = std::io::stdout();

    crossterm::terminal::enable_raw_mode()?;
    // Disambiguate keyboard modifier codes
    execute!(
        stdout,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    )?;
    execute!(stdout, crossterm::cursor::EnableBlinking)?;
    if let Err(e) = terminal_main(&mut stdout) {
        exit_state(&mut stdout)?;
        eprintln!("Error: {e}");
        return Err(e);
    }

    exit_state(&mut stdout)?;

    Ok(())
}
