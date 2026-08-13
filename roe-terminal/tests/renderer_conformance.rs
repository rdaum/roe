#[path = "../../tests/renderer_conformance.rs"]
mod shared;

use roe_terminal::TerminalRenderer;

#[test]
fn terminal_renderer_obeys_shared_dirty_lifecycle() {
    let mut renderer = TerminalRenderer::new(Vec::new());
    shared::assert_dirty_lifecycle(&mut renderer);
}
