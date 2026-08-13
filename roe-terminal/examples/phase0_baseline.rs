use roe_core::command_registry;
use roe_core::editor::{WindowNode, WindowType};
use roe_core::file_watcher::FileWatcher;
use roe_core::keys::{DefaultBindings, KeyState};
use roe_core::kill_ring::KillRing;
use roe_core::mode::{Mode, ScratchMode};
use roe_core::{Buffer, BufferId, Editor, Frame, ModeId, Renderer, Window, WindowId};
use roe_terminal::TerminalRenderer;
use slotmap::SlotMap;
use std::collections::HashMap;
use std::hint::black_box;
use std::io::{self, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

const EDIT_ITERATIONS: usize = 10_000;
const REDRAW_ITERATIONS: usize = 100;
const FIXTURE_LINES: usize = 2_000;

#[derive(Clone)]
struct CountingWriter {
    bytes: Arc<AtomicUsize>,
}

impl Write for CountingWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.bytes.fetch_add(buffer.len(), Ordering::Relaxed);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn fixture() -> Editor {
    let scratch_mode = Box::new(ScratchMode {});
    let mut modes: SlotMap<ModeId, Box<dyn Mode>> = SlotMap::default();
    let scratch_mode_id = modes.insert(scratch_mode);

    let mut buffers: SlotMap<BufferId, Buffer> = SlotMap::default();
    let buffer = Buffer::new(&[scratch_mode_id]);
    buffer.set_object("*baseline*".to_string());
    let mut content = String::with_capacity(FIXTURE_LINES * 64);
    for line in 0..FIXTURE_LINES {
        content.push_str(&format!(
            "line {line:04}: Roe baseline text with unicode lambda λ\n"
        ));
    }
    buffer.load_str(&content);
    let buffer_id = buffers.insert(buffer);

    let window = Window {
        x: 0,
        y: 0,
        width_chars: 120,
        height_chars: 39,
        active_buffer: buffer_id,
        start_line: 0,
        start_column: 0,
        cursor: 0,
        window_type: WindowType::Normal,
    };
    let mut windows: SlotMap<WindowId, Window> = SlotMap::default();
    let window_id = windows.insert(window);

    Editor {
        frame: Frame::new(120, 40),
        buffers,
        buffer_hosts: HashMap::new(),
        windows,
        modes,
        active_window: window_id,
        key_state: KeyState::new(),
        bindings: Box::new(DefaultBindings {}),
        window_tree: WindowNode::new_leaf(window_id),
        kill_ring: KillRing::without_clipboard(60),
        command_registry: command_registry::create_default_registry(),
        previous_active_window: None,
        buffer_history: Vec::new(),
        echo_message: String::new(),
        echo_message_time: None,
        clock: Arc::new(roe_core::native_services::SystemClock),
        current_key_chord: Vec::new(),
        mouse_drag_state: None,
        messages_buffer_id: None,
        file_watcher: FileWatcher::new(),
        last_search_term: String::new(),
    }
}

#[cfg(target_os = "linux")]
fn resident_memory_kib() -> Option<usize> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|line| line.starts_with("VmRSS:"))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

#[cfg(not(target_os = "linux"))]
fn resident_memory_kib() -> Option<usize> {
    None
}

fn main() -> io::Result<()> {
    let process_started = Instant::now();
    let editor = fixture();
    let fixture_construction = process_started.elapsed();
    let post_fixture_rss_kib = resident_memory_kib();

    let buffer_id = editor.windows[editor.active_window].active_buffer;
    let buffer = &editor.buffers[buffer_id];
    let edit_position = buffer.buffer_len_chars() / 2;
    let editing_started = Instant::now();
    for _ in 0..EDIT_ITERATIONS {
        buffer.insert_pos("x".to_string(), edit_position);
        black_box(buffer.delete_pos(edit_position, 1));
    }
    let editing_elapsed = editing_started.elapsed();

    let rendered_bytes = Arc::new(AtomicUsize::new(0));
    let writer = CountingWriter {
        bytes: rendered_bytes.clone(),
    };
    let mut renderer = TerminalRenderer::new(writer);
    let redraw_started = Instant::now();
    for _ in 0..REDRAW_ITERATIONS {
        renderer.render_full(&editor)?;
    }
    let redraw_elapsed = redraw_started.elapsed();

    println!("fixture_lines={FIXTURE_LINES}");
    println!(
        "fixture_construction_us={}",
        fixture_construction.as_micros()
    );
    if let Some(rss_kib) = post_fixture_rss_kib {
        println!("post_fixture_rss_kib={rss_kib}");
    } else {
        println!("post_fixture_rss_kib=unavailable");
    }
    println!("edit_iterations={EDIT_ITERATIONS}");
    println!(
        "edit_round_trip_ns_per_iteration={}",
        editing_elapsed.as_nanos() / EDIT_ITERATIONS as u128
    );
    println!("redraw_iterations={REDRAW_ITERATIONS}");
    println!(
        "terminal_full_redraw_us_per_iteration={}",
        redraw_elapsed.as_micros() / REDRAW_ITERATIONS as u128
    );
    println!(
        "terminal_bytes_per_full_redraw={}",
        rendered_bytes.load(Ordering::Relaxed) / REDRAW_ITERATIONS
    );

    Ok(())
}
