// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-only

use super::*;

#[test]
fn mica_scroll_retains_document_offsets_beyond_u16() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let mut editor = test_editor();
        let window = editor.active_window;
        let buffer = editor.windows[window].active_buffer;
        let text = "\n".repeat(65_540) + &"λ".repeat(65_540);
        editor.buffers[buffer].load_str(&text);
        editor.windows[window].cursor = 131_080;
        let mut session = test_mica_client(editor, CapabilityGrants::editor_default()).unwrap();
        let view = session.workspace.view_ids[&window];
        let scrolled = session
            .dispatch(session.envelope(InputEvent::SetViewScroll {
                view,
                start_line: Some(65_536),
                start_column: Some(65_536),
            }))
            .await
            .unwrap();
        assert!(
            !scrolled
                .lifecycle
                .iter()
                .any(|event| matches!(event, LifecycleEvent::Error(_))),
            "{scrolled:?}"
        );
        let presented = snapshot(&scrolled)
            .views
            .iter()
            .find(|candidate| candidate.id == view)
            .unwrap();
        assert_eq!(presented.scroll.start_line, 65_536);
        assert_eq!(presented.scroll.start_column, 65_536);
        assert_eq!(presented.cursor, 131_080);
        session.terminate_workspace().await.unwrap();
    });
}

#[test]
fn absolute_character_mutation_uses_named_line_and_column() {
    let mut editor = test_editor();
    let buffer = editor.windows[editor.active_window].active_buffer;
    editor.buffers[buffer].load_str("abc\nλμν");
    editor.insert_text(
        "!".into(),
        &crate::editor::ActionPosition::Absolute { column: 2, line: 1 },
    );
    assert_eq!(editor.buffers[buffer].content(), "abc\nλμ!ν");
}

#[test]
fn mica_pointer_hit_validates_revision_and_resource_before_selection() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let mut session = test_mica_client(test_editor(), CapabilityGrants::editor_default()).unwrap();
        let window = session.workspace.editor.active_window;
        let buffer = session.workspace.editor.windows[window].active_buffer;
        let hit = PointerTextHit {
            view: session.workspace.view_ids[&window],
            resource: session.workspace.buffer_resources[&buffer],
            text_revision: session.workspace.editor.buffers[buffer].text_revision(),
            position: 2,
        };
        let pointer = PointerEvent { text_hit: Some(hit), column: 10, row: 1, kind: PointerKind::Down, button: PointerButton::Primary };
        session.dispatch(session.envelope(InputEvent::Pointer(pointer.clone()))).await.unwrap();
        assert_eq!(session.workspace.editor.windows[window].cursor, 2);
        session.workspace.editor.buffers[buffer].insert_pos("changed".into(), 0);
        let stale = session.dispatch(session.envelope(InputEvent::Pointer(pointer.clone()))).await.unwrap();
        assert!(stale.lifecycle.iter().any(|event| matches!(event, LifecycleEvent::Warning(message) if message.contains("stale presentation"))));
        let mut wrong_resource = pointer;
        wrong_resource.text_hit.as_mut().unwrap().resource.generation += 1;
        let rejected = session.dispatch(session.envelope(InputEvent::Pointer(wrong_resource))).await.unwrap();
        assert!(rejected.lifecycle.iter().any(|event| matches!(event, LifecycleEvent::Warning(message) if message.contains("stale presentation"))));
        session.terminate_workspace().await.unwrap();
    });
}
