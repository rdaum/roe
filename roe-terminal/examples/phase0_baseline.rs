use roe_core::editor::{WindowNode, WindowType};
use roe_core::file_watcher::FileWatcher;
use roe_core::keys::LogicalKey;
use roe_core::kill_ring::KillRing;
use roe_core::native_kernel::CapabilityGrants;
use roe_core::session::{HostSession, InputEvent};
use roe_core::{Buffer, BufferId, Editor, Frame, Window, WindowId};
use roe_terminal::TerminalRenderer;
use slotmap::SlotMap;
use std::hint::black_box;
use std::io::{self, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

const EDIT_ITERATIONS: usize = 100;
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
    let mut buffers: SlotMap<BufferId, Buffer> = SlotMap::default();
    let buffer = Buffer::new();
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
        windows,
        active_window: window_id,
        window_tree: WindowNode::new_leaf(window_id),
        kill_ring: KillRing::without_clipboard(60),
        previous_active_window: None,
        buffer_history: Vec::new(),
        echo_message: String::new(),
        echo_message_time: None,
        clock: Arc::new(roe_core::native_services::SystemClock),
        mouse_drag_state: None,
        messages_buffer_id: None,
        file_watcher: FileWatcher::new(),
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
    compio::runtime::Runtime::new()?.block_on(run())
}

async fn run() -> io::Result<()> {
    let process_started = Instant::now();
    let editor = fixture();
    let fixture_construction = process_started.elapsed();
    let mut session = HostSession::open_with_mica(editor, CapabilityGrants::editor_default())
        .map_err(io::Error::other)?;
    let initial = session.initial_output().await;
    let ready = process_started.elapsed();
    let post_fixture_rss_kib = resident_memory_kib();

    let editing_started = Instant::now();
    for _ in 0..EDIT_ITERATIONS {
        let inserted = session
            .dispatch(session.envelope(InputEvent::Text("x".to_owned())))
            .await
            .map_err(io::Error::other)?;
        black_box(inserted);
        let deleted = session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Backspace])))
            .await
            .map_err(io::Error::other)?;
        black_box(deleted);
    }
    let editing_elapsed = editing_started.elapsed();
    let post_edit_rss_kib = resident_memory_kib();

    let rendered_bytes = Arc::new(AtomicUsize::new(0));
    let writer = CountingWriter {
        bytes: rendered_bytes.clone(),
    };
    let mut renderer = TerminalRenderer::new(writer);
    renderer.apply_session_presentation(initial.presentation.as_ref().unwrap())?;
    let redraw_started = Instant::now();
    for _ in 0..REDRAW_ITERATIONS {
        let output = session
            .dispatch(session.envelope(InputEvent::RequestSnapshot { after: None }))
            .await
            .map_err(io::Error::other)?;
        renderer.apply_session_presentation(output.presentation.as_ref().unwrap())?;
        renderer.render_session()?;
    }
    let redraw_elapsed = redraw_started.elapsed();
    let post_workload_rss_kib = resident_memory_kib();

    println!("fixture_lines={FIXTURE_LINES}");
    println!(
        "fixture_construction_us={}",
        fixture_construction.as_micros()
    );
    println!("mica_session_ready_us={}", ready.as_micros());
    if let Some(rss_kib) = post_fixture_rss_kib {
        println!("post_fixture_rss_kib={rss_kib}");
    } else {
        println!("post_fixture_rss_kib=unavailable");
    }
    if let Some(rss_kib) = post_edit_rss_kib {
        println!("post_mica_edit_rss_kib={rss_kib}");
    } else {
        println!("post_mica_edit_rss_kib=unavailable");
    }
    println!("edit_iterations={EDIT_ITERATIONS}");
    println!(
        "mica_edit_insert_delete_ns_per_iteration={}",
        editing_elapsed.as_nanos() / EDIT_ITERATIONS as u128
    );
    println!("redraw_iterations={REDRAW_ITERATIONS}");
    println!(
        "mica_terminal_snapshot_redraw_us_per_iteration={}",
        redraw_elapsed.as_micros() / REDRAW_ITERATIONS as u128
    );
    println!(
        "terminal_bytes_per_full_redraw={}",
        rendered_bytes.load(Ordering::Relaxed) / REDRAW_ITERATIONS
    );
    if let Some(rss_kib) = post_workload_rss_kib {
        println!("post_mica_workload_rss_kib={rss_kib}");
        if let Some(before) = post_fixture_rss_kib {
            println!(
                "mica_workload_rss_growth_kib={}",
                rss_kib.saturating_sub(before)
            );
        }
    } else {
        println!("post_mica_workload_rss_kib=unavailable");
        println!("mica_workload_rss_growth_kib=unavailable");
    }

    Ok(())
}
