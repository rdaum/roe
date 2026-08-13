#[path = "../../tests/renderer_conformance.rs"]
mod shared;

use roe_vello::VelloRenderer;

#[test]
fn vello_renderer_obeys_shared_dirty_lifecycle() {
    let mut renderer = VelloRenderer::new();
    shared::assert_dirty_lifecycle(&mut renderer);
    shared::assert_logical_presentation(&mut renderer);
}
