// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-only

use super::*;

#[test]
fn mica_projects_buffer_metadata_and_prompts_before_saving_a_non_file_buffer() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let mut session =
            test_mica_client(test_editor(), CapabilityGrants::editor_default()).unwrap();
        let save = session
            .dispatch(session.envelope(InputEvent::Keys(vec![
                control(),
                LogicalKey::AlphaNumeric('x'),
                control(),
                LogicalKey::AlphaNumeric('s'),
            ])))
            .await
            .unwrap();

        let ordinary = snapshot(&save)
            .views
            .iter()
            .find(|view| !view.command_view)
            .unwrap();
        assert_eq!(ordinary.name, "*test*");
        assert_eq!(ordinary.buffer_kind, "ordinary");
        assert_eq!(ordinary.visited_file, None);
        assert_eq!(ordinary.text_revision, 1);
        assert_eq!(ordinary.last_saved_revision, 1);
        assert!(!ordinary.modified);
        assert!(!ordinary.read_only);
        assert!(
            snapshot(&save).views.iter().any(|view| {
                view.command_view && view.visible_text.starts_with("Save buffer as ")
            })
        );

        session.terminate_workspace().await.unwrap();
    });
}

#[test]
fn switch_to_buffer_creates_a_missing_ordinary_buffer() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let mut session =
            test_mica_client(test_editor(), CapabilityGrants::editor_default()).unwrap();
        session
            .dispatch(session.envelope(InputEvent::Keys(vec![
                control(),
                LogicalKey::AlphaNumeric('x'),
                LogicalKey::AlphaNumeric('b'),
            ])))
            .await
            .unwrap();
        session
            .dispatch(session.envelope(InputEvent::Text("new-notes".to_owned())))
            .await
            .unwrap();
        let created = session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Enter])))
            .await
            .unwrap();

        let active = snapshot(&created)
            .views
            .iter()
            .find(|view| view.active)
            .unwrap();
        assert_eq!(active.name, "new-notes");
        assert_eq!(active.buffer_kind, "ordinary");
        assert_eq!(active.visited_file, None);
        assert!(active.visible_text.is_empty());

        session.terminate_workspace().await.unwrap();
    });
}

#[test]
fn mica_kill_policy_rejects_a_modified_ordinary_buffer() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let mut session =
            test_mica_client(test_editor(), CapabilityGrants::editor_default()).unwrap();
        let active =
            session.workspace.editor.windows[session.workspace.editor.active_window].active_buffer;
        session
            .dispatch(session.envelope(InputEvent::Text("!".to_owned())))
            .await
            .unwrap();
        assert!(session.workspace.editor.buffers[active].is_modified());

        session
            .dispatch(session.envelope(InputEvent::Keys(vec![
                control(),
                LogicalKey::AlphaNumeric('x'),
                LogicalKey::AlphaNumeric('k'),
            ])))
            .await
            .unwrap();
        session
            .dispatch(session.envelope(InputEvent::Text("test".to_owned())))
            .await
            .unwrap();
        let denied = session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Enter])))
            .await
            .unwrap();
        assert!(
            denied.lifecycle.iter().any(|event| {
                matches!(event, LifecycleEvent::Error(message) if message.contains("modified"))
            }),
            "{denied:#?}"
        );
        assert!(session.workspace.editor.buffers.contains_key(active));

        session.terminate_workspace().await.unwrap();
    });
}

#[test]
fn saving_scratch_to_a_destination_preserves_a_fresh_scratch_buffer() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let editor = test_editor();
        let original = editor.windows[editor.active_window].active_buffer;
        editor.buffers[original].set_kind(crate::buffer::BufferKind::Scratch);
        editor.buffers[original].set_display_name("*scratch*");
        let path = std::env::temp_dir().join(format!(
            "roe-scratch-save-{}-{}.mica",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut session = test_mica_client(editor, CapabilityGrants::editor_default()).unwrap();

        session
            .dispatch(session.envelope(InputEvent::Keys(vec![
                control(),
                LogicalKey::AlphaNumeric('x'),
                control(),
                LogicalKey::AlphaNumeric('s'),
            ])))
            .await
            .unwrap();
        session
            .dispatch(session.envelope(InputEvent::Text(path.to_string_lossy().into_owned())))
            .await
            .unwrap();
        let saved = session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Enter])))
            .await
            .unwrap();
        assert!(
            saved
                .lifecycle
                .iter()
                .all(|event| { !matches!(event, LifecycleEvent::Error(_)) }),
            "{saved:#?}"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");
        assert_eq!(
            session.workspace.editor.buffers[original].visited_file(),
            Some(path.clone())
        );
        assert_eq!(
            session.workspace.editor.buffers[original].kind(),
            crate::buffer::BufferKind::File
        );
        assert!(!session.workspace.editor.buffers[original].is_modified());
        assert_eq!(
            session
                .workspace
                .editor
                .buffers
                .iter()
                .filter(|(_, buffer)| buffer.kind() == crate::buffer::BufferKind::Scratch)
                .count(),
            1
        );

        session.terminate_workspace().await.unwrap();
        std::fs::remove_file(path).unwrap();
    });
}
