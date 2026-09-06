// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-only

use super::*;

#[test]
fn mica_native_bridge_enforces_service_authority() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let mut session = test_mica_client_with_clock(
            test_editor(),
            CapabilityGrants::editor_default(),
            Arc::new(FixedNativeClock(42)),
        )
        .unwrap();
        session
            .workspace
            .mica
            .as_ref()
            .unwrap()
            .revoke_service_for_test("clock_read");

        let denied = session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Function(12)])))
            .await
            .unwrap();
        assert!(denied.lifecycle.iter().any(|event| matches!(
            event,
            LifecycleEvent::Error(message) if message.contains("required native service grant")
        )));
        assert_eq!(snapshot(&denied).views[0].visible_text, "hello");

        session.terminate_workspace().await.unwrap();
    });
}

#[test]
fn mica_host_effects_cannot_bypass_native_capabilities() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let editor = test_editor();
        let active = editor.windows[editor.active_window].active_buffer;
        editor.buffers[active]
            .set_visited_file(Some(std::path::PathBuf::from("denied-save.txt")));
        editor.buffers[active].set_kind(crate::buffer::BufferKind::File);
        let mut session = test_mica_client(
            editor,
            CapabilityGrants::new([]),
        )
        .unwrap();

        let insertion = session
            .dispatch(session.envelope(InputEvent::Text("x".to_owned())))
            .await
            .unwrap();
        let active = session.workspace.editor.windows
            [session.workspace.editor.active_window]
            .active_buffer;
        assert_eq!(session.workspace.editor.buffers[active].content(), "hello");
        assert!(insertion.lifecycle.iter().any(|event| matches!(
            event,
            LifecycleEvent::Error(message) if message.contains("text_write") || message.contains("TextWrite")
        )));

        let control =
            LogicalKey::Modifier(crate::keys::KeyModifier::Control(crate::keys::Side::Left));
        let split = session
            .dispatch(session.envelope(InputEvent::Keys(vec![
                control,
                LogicalKey::AlphaNumeric('x'),
                LogicalKey::AlphaNumeric('2'),
            ])))
            .await
            .unwrap();
        assert_eq!(session.workspace.editor.windows.len(), 1);
        assert!(split.lifecycle.iter().any(|event| matches!(
            event,
            LifecycleEvent::Error(message) if message.contains("Layout")
        )));

        let save = session
            .dispatch(session.envelope(InputEvent::Keys(vec![
                control,
                LogicalKey::AlphaNumeric('x'),
                LogicalKey::Modifier(crate::keys::KeyModifier::Control(
                    crate::keys::Side::Left,
                )),
                LogicalKey::AlphaNumeric('s'),
            ])))
            .await
            .unwrap();
        assert!(save.lifecycle.iter().any(|event| matches!(
            event,
            LifecycleEvent::Error(message) if message.contains("FileWrite")
        )));

        let resource = session.workspace.buffer_resources[&active];
        let direct = session
            .dispatch(session.envelope(InputEvent::NativeRequest {
                request_id: RequestId(77),
                operation: NativeOperation::Snapshot { resource },
            }))
            .await
            .unwrap();
        assert!(direct.native_completions.iter().any(|completion| {
            completion.request_id == RequestId(77)
                && completion.result.as_ref().is_err_and(|error| {
                    error.contains("direct native requests are disabled")
                })
        }));

        session.workspace.editor.windows[session.workspace.editor.active_window].cursor = 5;
        session.workspace.editor.buffers[active].set_mark(0);
        let copy = session
            .dispatch(session.envelope(InputEvent::Keys(vec![
                LogicalKey::Modifier(crate::keys::KeyModifier::Meta(
                    crate::keys::Side::Left,
                )),
                LogicalKey::AlphaNumeric('w'),
            ])))
            .await
            .unwrap();
        assert!(copy.lifecycle.iter().any(|event| matches!(
            event,
            LifecycleEvent::Error(message) if message.contains("text_read") || message.contains("TextRead")
        )));
        assert!(session.workspace.editor.kill_ring.current().is_none());

        session
            .terminate_workspace()
            .await
            .unwrap();
    });
}
