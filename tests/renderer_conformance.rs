use roe_core::renderer::{DirtyRegion, Renderer};
use roe_core::BufferId;
use slotmap::SlotMap;

pub fn assert_dirty_lifecycle<R: Renderer>(renderer: &mut R) {
    let mut buffers: SlotMap<BufferId, ()> = SlotMap::with_key();
    let buffer_id = buffers.insert(());

    renderer.clear_dirty();
    assert!(!renderer.needs_redraw());

    renderer.mark_dirty(DirtyRegion::Line { buffer_id, line: 3 });
    assert!(renderer.needs_redraw());

    renderer.clear_dirty();
    assert!(!renderer.needs_redraw());

    renderer.mark_dirty(DirtyRegion::FullScreen);
    assert!(renderer.needs_redraw());

    renderer.clear_dirty();
    assert!(!renderer.needs_redraw());
}
