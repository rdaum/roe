use roe_core::renderer::{DirtyRegion, ModelineComponent, Renderer};
use roe_core::{BufferId, WindowId};
use slotmap::SlotMap;

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
