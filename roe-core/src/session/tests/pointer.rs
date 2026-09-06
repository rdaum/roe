// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-only

use super::*;

#[test]
fn mica_owns_pointer_selection_and_view_scroll_policy() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let mut session =
            test_mica_client(test_editor(), CapabilityGrants::editor_default()).unwrap();
        let initial = session.initial_output().await;
        let view = snapshot(&initial).active_view;

        for (index, event) in [
            PointerEvent {
                text_hit: None,
                column: 2,
                row: 1,
                kind: PointerKind::Down,
                button: PointerButton::Primary,
            },
            PointerEvent {
                text_hit: None,
                column: 4,
                row: 1,
                kind: PointerKind::Move,
                button: PointerButton::None,
            },
            PointerEvent {
                text_hit: None,
                column: 4,
                row: 1,
                kind: PointerKind::Up,
                button: PointerButton::Primary,
            },
        ]
        .into_iter()
        .enumerate()
        {
            let output = session
                .dispatch(session.envelope(InputEvent::Pointer(event)))
                .await
                .unwrap();
            assert!(
                output
                    .lifecycle
                    .iter()
                    .all(|event| !matches!(event, LifecycleEvent::Error(_)))
            );
            if index == 0 {
                assert_eq!(
                    session
                        .attachment
                        .pointer_selection
                        .map(|(_, anchor)| anchor),
                    Some(1)
                );
            }
            if index == 1 {
                let window = session.workspace.editor.active_window;
                let buffer = session.workspace.editor.windows[window].active_buffer;
                assert_eq!(session.workspace.editor.buffers[buffer].get_mark(), Some(1));
            }
        }

        let window = session.workspace.editor.active_window;
        let buffer = session.workspace.editor.windows[window].active_buffer;
        assert_eq!(session.workspace.editor.buffers[buffer].get_mark(), Some(1));
        assert_eq!(session.workspace.editor.windows[window].cursor, 3);

        let moved = session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Home])))
            .await
            .unwrap();
        assert!(
            moved
                .lifecycle
                .iter()
                .all(|event| !matches!(event, LifecycleEvent::Error(_)))
        );
        assert_eq!(session.workspace.editor.windows[window].cursor, 0);
        for event in [
            PointerEvent {
                text_hit: None,
                column: 2,
                row: 1,
                kind: PointerKind::Down,
                button: PointerButton::Primary,
            },
            PointerEvent {
                text_hit: None,
                column: 2,
                row: 1,
                kind: PointerKind::Up,
                button: PointerButton::Primary,
            },
        ] {
            let repeated = session
                .dispatch(session.envelope(InputEvent::Pointer(event)))
                .await
                .unwrap();
            assert!(
                repeated
                    .lifecycle
                    .iter()
                    .all(|event| !matches!(event, LifecycleEvent::Error(_))),
                "{repeated:#?}"
            );
        }
        assert_eq!(session.workspace.editor.windows[window].cursor, 1);

        let scrolled = session
            .dispatch(session.envelope(InputEvent::SetViewScroll {
                view,
                start_line: Some(0),
                start_column: Some(2),
            }))
            .await
            .unwrap();
        assert_eq!(snapshot(&scrolled).views[0].scroll.start_column, 2);
        assert!(
            scrolled
                .lifecycle
                .iter()
                .all(|event| !matches!(event, LifecycleEvent::Error(_)))
        );

        let control =
            LogicalKey::Modifier(crate::keys::KeyModifier::Control(crate::keys::Side::Left));
        session
            .dispatch(session.envelope(InputEvent::Keys(vec![
                control,
                LogicalKey::AlphaNumeric('x'),
                LogicalKey::AlphaNumeric('2'),
            ])))
            .await
            .unwrap();
        let top = session
            .workspace
            .editor
            .windows
            .iter()
            .min_by_key(|(_, window)| window.y)
            .map(|(_, window)| window.clone())
            .unwrap();
        let border_row = top.y + top.height_chars - 1;
        let before = ratio_at_path(&session.workspace.editor.window_tree, &[]).unwrap();
        for event in [
            PointerEvent {
                text_hit: None,
                column: 40,
                row: border_row,
                kind: PointerKind::Down,
                button: PointerButton::Primary,
            },
            PointerEvent {
                text_hit: None,
                column: 40,
                row: border_row + 8,
                kind: PointerKind::Move,
                button: PointerButton::Primary,
            },
            PointerEvent {
                text_hit: None,
                column: 40,
                row: border_row + 8,
                kind: PointerKind::Up,
                button: PointerButton::Primary,
            },
        ] {
            let output = session
                .dispatch(session.envelope(InputEvent::Pointer(event)))
                .await
                .unwrap();
            assert!(
                output
                    .lifecycle
                    .iter()
                    .all(|event| !matches!(event, LifecycleEvent::Error(_))),
                "layout pointer lifecycle: {:?}",
                output.lifecycle
            );
        }
        assert!(ratio_at_path(&session.workspace.editor.window_tree, &[]).unwrap() > before);

        session.terminate_workspace().await.unwrap();
    });
}
