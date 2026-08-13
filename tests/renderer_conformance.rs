use roe_core::keys::DefaultBindings;
use roe_core::renderer::{DirtyRegion, ModelineComponent, Renderer};
use roe_core::{
    command_registry, Buffer, BufferId, Editor, Frame, KeyState, Mode, ModeId, Window, WindowId,
};
use slotmap::SlotMap;
use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::Arc;

fn fixture_editor() -> Editor {
    let mut modes: SlotMap<ModeId, Box<dyn Mode>> = SlotMap::with_key();
    let mode_id = modes.insert(Box::new(roe_core::mode::ScratchMode {}));
    let mut buffers: SlotMap<BufferId, Buffer> = SlotMap::with_key();
    let buffer = Buffer::new(&[mode_id]);
    buffer.set_object("*conformance*".to_string());
    buffer.load_str("λ baseline");
    let buffer_id = buffers.insert(buffer);
    let mut windows: SlotMap<WindowId, Window> = SlotMap::with_key();
    let window_id = windows.insert(Window {
        x: 0,
        y: 0,
        width_chars: 80,
        height_chars: 22,
        active_buffer: buffer_id,
        start_line: 0,
        start_column: 0,
        cursor: 0,
        window_type: roe_core::editor::WindowType::Normal,
    });

    Editor {
        frame: Frame::new(80, 24),
        buffers,
        buffer_hosts: HashMap::new(),
        windows,
        modes,
        active_window: window_id,
        previous_active_window: None,
        key_state: KeyState::new(),
        bindings: Box::new(DefaultBindings {}),
        window_tree: roe_core::editor::WindowNode::new_leaf(window_id),
        kill_ring: roe_core::kill_ring::KillRing::without_clipboard(60),
        command_registry: command_registry::create_default_registry(),
        buffer_history: vec![buffer_id],
        echo_message: String::new(),
        echo_message_time: None,
        clock: Arc::new(roe_core::native_services::SystemClock),
        current_key_chord: vec![],
        mouse_drag_state: None,
        messages_buffer_id: None,
        file_watcher: roe_core::file_watcher::FileWatcher::new(),
        last_search_term: String::new(),
    }
}

pub fn assert_dirty_lifecycle<R: Renderer>(renderer: &mut R) {
    let mut buffers: SlotMap<BufferId, ()> = SlotMap::with_key();
    let buffer_id = buffers.insert(());
    let mut windows: SlotMap<WindowId, ()> = SlotMap::with_key();
    let window_id = windows.insert(());

    renderer.clear_dirty();
    assert!(!renderer.needs_redraw());

    for region in [
        DirtyRegion::Line { buffer_id, line: 3 },
        DirtyRegion::LineRange {
            buffer_id,
            start_line: 2,
            end_line: 5,
        },
        DirtyRegion::CharRange {
            buffer_id,
            start_char: 7,
            end_char: 11,
        },
        DirtyRegion::Buffer { buffer_id },
        DirtyRegion::WindowChrome { window_id },
        DirtyRegion::Modeline {
            window_id,
            component: ModelineComponent::CursorPosition,
        },
        DirtyRegion::FullScreen,
    ] {
        renderer.mark_dirty(region);
        assert!(renderer.needs_redraw());
        renderer.clear_dirty();
        assert!(!renderer.needs_redraw());
    }

    renderer.mark_dirty(DirtyRegion::Line { buffer_id, line: 0 });
    renderer.mark_dirty(DirtyRegion::FullScreen);
    assert!(
        renderer.needs_redraw(),
        "invalidation must remain monotonic"
    );
}

pub fn assert_logical_presentation<R>(renderer: &mut R)
where
    R: Renderer,
    R::Error: Debug,
{
    let mut editor = fixture_editor();
    renderer.render_full(&editor).unwrap();

    let initial = renderer
        .presentation_snapshot()
        .expect("full render must capture logical presentation");
    assert_eq!((initial.columns, initial.rows), (80, 24));
    assert_eq!(initial.windows.len(), 1);
    assert_eq!(initial.windows[0].buffer_name, "*conformance*");
    assert_eq!(initial.windows[0].text, "λ baseline");
    assert_eq!(initial.windows[0].cursor, 0);
    assert!(initial.windows[0].is_active);

    let window_id = editor.active_window;
    let buffer_id = editor.windows[window_id].active_buffer;
    editor.buffers[buffer_id].insert_pos("é".to_string(), 0);
    editor.windows[window_id].cursor = 1;
    renderer.mark_dirty(DirtyRegion::Buffer { buffer_id });
    renderer.render_incremental(&editor).unwrap();

    let updated = renderer
        .presentation_snapshot()
        .expect("incremental render must refresh logical presentation");
    assert_eq!(updated.windows[0].text, "éλ baseline");
    assert_eq!(updated.windows[0].cursor, 1);
}

#[allow(dead_code)] // This shared source is also compiled by the terminal-only test target.
pub fn assert_cross_frontend_presentation<A, B>(first: &mut A, second: &mut B)
where
    A: Renderer,
    B: Renderer,
    A::Error: Debug,
    B::Error: Debug,
{
    let mut editor = fixture_editor();
    editor.split_horizontal();
    editor.handle_resize(80, 23);
    first.render_full(&editor).unwrap();
    second.render_full(&editor).unwrap();
    assert_eq!(
        first.presentation_snapshot(),
        second.presentation_snapshot()
    );
    let initial = first.presentation_snapshot().unwrap();
    assert_eq!(initial.windows.len(), 2);
    assert_eq!(initial.windows[0].width_chars, 80);
    assert_eq!(initial.windows[0].height_chars, 11);
    assert_eq!(initial.windows[1].y, 11);

    let window_id = editor.active_window;
    let buffer_id = editor.windows[window_id].active_buffer;
    editor.buffers[buffer_id].insert_pos("é".to_string(), 0);
    editor.windows[window_id].cursor = 1;
    editor.handle_resize(96, 31);

    first.mark_dirty(DirtyRegion::Buffer { buffer_id });
    second.mark_dirty(DirtyRegion::Buffer { buffer_id });
    first.render_incremental(&editor).unwrap();
    second.render_incremental(&editor).unwrap();
    assert_eq!(
        first.presentation_snapshot(),
        second.presentation_snapshot()
    );
    let updated = first.presentation_snapshot().unwrap();
    assert_eq!((updated.columns, updated.rows), (96, 31));
    assert_eq!(updated.windows.len(), 2);
    assert_eq!(updated.windows[0].height_chars, 15);
    assert_eq!(updated.windows[1].y, 15);
}
