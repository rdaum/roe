use roe_core::keys::LogicalKey;
use roe_core::native_kernel::CapabilityGrants;
use roe_core::session::{
    AttachmentConfiguration, DirectSessionClient, InputEvent, SessionClient, WorkspaceHost,
};
use roe_core::{Buffer, Editor, Frame};
use roe_terminal::TerminalRenderer;
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

fn fixture(rust: bool, markdown: bool) -> Editor {
    let buffer = Buffer::named("*baseline*", roe_core::buffer::BufferKind::Ordinary);
    let mut content = String::with_capacity(FIXTURE_LINES * 64);
    if rust {
        buffer.set_visited_file(Some("baseline.rs".into()));
        content.push_str("fn baseline() {\n");
    } else if markdown {
        buffer.set_visited_file(Some("baseline.md".into()));
    }
    for line in 0..FIXTURE_LINES {
        if rust {
            content.push_str(&format!("    let value_{line} = \"Rust source with λ\";\n"));
        } else if markdown {
            match line % 4 {
                0 => content.push_str(&format!("## Section {line}\n")),
                1 => content.push_str("Text with **strong words** and `code` λ.\n"),
                2 => content.push_str("Continuation with a [local link](notes.md).\n"),
                _ => content.push('\n'),
            }
        } else {
            content.push_str(&format!(
                "line {line:04}: Roe baseline text with unicode lambda λ\n"
            ));
        }
    }
    if rust {
        content.push_str("}\n");
    }
    buffer.load_str(&content);
    Editor::new(buffer, Frame::new(120, 40))
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
    let rust = std::env::args().any(|argument| argument == "--rust");
    let markdown = std::env::args().any(|argument| argument == "--markdown");
    if rust && markdown {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "select one syntax mode",
        ));
    }
    let editor = fixture(rust, markdown);
    let fixture_construction = process_started.elapsed();
    let workspace = WorkspaceHost::open_with_mica(editor, CapabilityGrants::editor_default())
        .map_err(io::Error::other)?;
    let mut session =
        DirectSessionClient::new(workspace, AttachmentConfiguration::headless(80, 23));
    let initial = session.initial_output().await;
    check_output(&initial)?;
    if rust || markdown {
        let Some(roe_core::session::PresentationUpdate::Full(snapshot)) = &initial.presentation
        else {
            return Err(io::Error::other("missing initial syntax presentation"));
        };
        if snapshot
            .views
            .iter()
            .all(|view| view.styled_ranges.is_empty())
        {
            return Err(io::Error::other("syntax benchmark has no highlight spans"));
        }
    }
    let ready = process_started.elapsed();
    let post_fixture_rss_kib = resident_memory_kib();

    let editing_started = Instant::now();
    for _ in 0..EDIT_ITERATIONS {
        let inserted = session
            .dispatch(session.envelope(InputEvent::Text("x".to_owned())))
            .await
            .map_err(io::Error::other)?;
        check_output(&inserted)?;
        black_box(inserted);
        let deleted = session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Backspace])))
            .await
            .map_err(io::Error::other)?;
        check_output(&deleted)?;
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

    println!(
        "syntax_mode={}",
        if rust {
            "rust"
        } else if markdown {
            "markdown"
        } else {
            "fundamental"
        }
    );
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

fn check_output(output: &roe_core::session::SessionOutput) -> io::Result<()> {
    for event in &output.lifecycle {
        if let roe_core::session::LifecycleEvent::Error(error) = event {
            return Err(io::Error::other(error.clone()));
        }
    }
    Ok(())
}
