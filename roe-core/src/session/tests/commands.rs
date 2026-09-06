// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-only

use super::*;

#[test]
fn mica_owns_global_chords_and_window_policy() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let mut session = test_mica_client_with_clock(
            test_editor(),
            CapabilityGrants::editor_default(),
            Arc::new(FixedNativeClock(42)),
        )
        .unwrap();
        let control =
            LogicalKey::Modifier(crate::keys::KeyModifier::Control(crate::keys::Side::Left));

        session
            .dispatch(session.envelope(InputEvent::Text("Z".to_owned())))
            .await
            .unwrap();
        session
            .dispatch(session.envelope(InputEvent::Keys(vec![
                control,
                LogicalKey::AlphaNumeric('x'),
            ])))
            .await
            .unwrap();
        let undone = session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::AlphaNumeric('u')])))
            .await
            .unwrap();
        assert_eq!(snapshot(&undone).views[0].visible_text, "hello");

        session
            .dispatch(session.envelope(InputEvent::Keys(vec![
                control,
                LogicalKey::AlphaNumeric('x'),
            ])))
            .await
            .unwrap();
        let unknown = session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::AlphaNumeric('z')])))
            .await
            .unwrap();
        assert_eq!(snapshot(&unknown).views[0].visible_text, "hello");
        assert!(unknown.lifecycle.iter().any(|event| matches!(
            event,
            LifecycleEvent::Error(message) if message.contains("C-x z is undefined")
        )));

        let prefix = session
            .dispatch(session.envelope(InputEvent::Keys(vec![
                control,
                LogicalKey::AlphaNumeric('x'),
            ])))
            .await
            .unwrap();
        assert!(snapshot(&prefix).echo_area.contains("C-x"));
        let split = session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::AlphaNumeric('2')])))
            .await
            .unwrap();
        assert_eq!(snapshot(&split).views.len(), 2);

        session
            .dispatch(session.envelope(InputEvent::Keys(vec![
                control,
                LogicalKey::AlphaNumeric('x'),
            ])))
            .await
            .unwrap();
        let switched = session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::AlphaNumeric('o')])))
            .await
            .unwrap();
        assert_eq!(snapshot(&switched).views.len(), 2);
        assert_ne!(
            snapshot(&switched).active_view,
            snapshot(&split).active_view,
            "{switched:#?}"
        );

        session
            .dispatch(session.envelope(InputEvent::Keys(vec![
                control,
                LogicalKey::AlphaNumeric('x'),
            ])))
            .await
            .unwrap();
        let deleted = session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::AlphaNumeric('0')])))
            .await
            .unwrap();
        assert_eq!(snapshot(&deleted).views.len(), 1);

        session
            .dispatch(session.envelope(InputEvent::Keys(vec![
                control,
                LogicalKey::AlphaNumeric('x'),
            ])))
            .await
            .unwrap();
        let quit = session
            .dispatch(session.envelope(InputEvent::Keys(vec![
                control,
                LogicalKey::AlphaNumeric('c'),
            ])))
            .await
            .unwrap();
        assert!(
            quit.lifecycle.contains(&LifecycleEvent::QuitRequested),
            "{quit:#?}"
        );

        session.terminate_workspace().await.unwrap();
    });
}

#[test]
fn mica_discovery_drives_command_palette_and_invocation() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let mut session = test_mica_client_with_clock(
            test_editor(),
            CapabilityGrants::editor_default(),
            Arc::new(FixedNativeClock(42)),
        )
        .unwrap();
        let meta = LogicalKey::Modifier(crate::keys::KeyModifier::Meta(crate::keys::Side::Left));

        let palette = session
            .dispatch(session.envelope(InputEvent::Keys(vec![meta, LogicalKey::AlphaNumeric('x')])))
            .await
            .unwrap();
        let prompt = snapshot(&palette)
            .views
            .iter()
            .find(|view| view.command_view)
            .unwrap_or_else(|| panic!("Mica command prompt: {palette:#?}"));
        assert!(prompt.visible_text.starts_with("M-x "));
        assert_eq!(prompt.styled_lines.len(), 1);
        assert!(snapshot(&palette).styles.iter().any(|style| {
            style.id == prompt.styled_lines[0].style
                && style.name == MICA_PROMPT_SELECTION_FACE
                && style.background.is_some()
        }));

        let filtered = session
            .dispatch(session.envelope(InputEvent::Text("insert-current-time".to_owned())))
            .await
            .unwrap();
        assert!(
            snapshot(&filtered)
                .views
                .iter()
                .find(|view| view.command_view)
                .unwrap()
                .visible_text
                .contains("insert-current-time")
        );
        let PresentationUpdate::Delta(delta) = filtered.presentation.as_ref().unwrap() else {
            panic!("Mica prompt update should produce a presentation delta");
        };
        assert!(
            delta
                .invalidations
                .iter()
                .any(|invalidation| matches!(invalidation, Invalidation::View(_)))
        );
        assert!(!delta.invalidations.contains(&Invalidation::Full));
        let inserted = session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Enter])))
            .await
            .unwrap();
        assert_eq!(
            snapshot(&inserted).views[0].visible_text,
            "hello42\n",
            "{inserted:#?}"
        );

        let palette = session
            .dispatch(session.envelope(InputEvent::Keys(vec![meta, LogicalKey::AlphaNumeric('x')])))
            .await
            .unwrap();
        assert!(
            snapshot(&palette)
                .views
                .iter()
                .any(|view| view.command_view)
        );

        session
            .dispatch(session.envelope(InputEvent::Text("split-window-horizontally".to_owned())))
            .await
            .unwrap();
        let selected = session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Enter])))
            .await
            .unwrap();
        assert_eq!(snapshot(&selected).views.len(), 2);

        let original = session
            .workspace
            .editor
            .windows
            .keys()
            .find(|window| *window != session.workspace.editor.active_window)
            .unwrap();
        let other = session.workspace.editor.active_window;
        let other_buffer = session
            .workspace
            .editor
            .create_buffer("*argument-target*".to_owned(), "target".to_owned());
        session.workspace.editor.windows[other].active_buffer = other_buffer;
        session.workspace.editor.active_window = original;
        let _ = session.workspace.synchronize_identities();

        session
            .dispatch(session.envelope(InputEvent::Keys(vec![meta, LogicalKey::AlphaNumeric('x')])))
            .await
            .unwrap();
        session
            .dispatch(session.envelope(InputEvent::Text("select-window".to_owned())))
            .await
            .unwrap();
        let argument_prompt = session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Enter])))
            .await
            .unwrap();
        let argument_view = snapshot(&argument_prompt)
            .views
            .iter()
            .find(|view| view.command_view)
            .unwrap_or_else(|| panic!("argument prompt missing: {argument_prompt:#?}"));
        assert!(
            argument_view.visible_text.contains("*argument-target*"),
            "unexpected argument prompt: {argument_view:#?}; lifecycle={:?}",
            argument_prompt.lifecycle
        );
        session
            .dispatch(session.envelope(InputEvent::Text("argument-target".to_owned())))
            .await
            .unwrap();
        let selected_argument = session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Enter])))
            .await
            .unwrap();
        assert_eq!(
            snapshot(&selected_argument).active_view,
            session.workspace.view_ids[&other]
        );

        let mismatched_provider = include_str!("../../../../mica/roe-first-wave.mica").replace(
            "assert roe/ArgumentCandidateKind(:visible_views, :logical_view)",
            "assert roe/ArgumentCandidateKind(:visible_views, :logical_buffer)",
        );
        session
            .workspace
            .replace_mica_first_wave(mismatched_provider)
            .await
            .unwrap();
        session
            .dispatch(session.envelope(InputEvent::Keys(vec![meta, LogicalKey::AlphaNumeric('x')])))
            .await
            .unwrap();
        session
            .dispatch(session.envelope(InputEvent::Text("select-window".to_owned())))
            .await
            .unwrap();
        let rejected_argument_prompt = session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Enter])))
            .await
            .unwrap();
        assert!(
            snapshot(&rejected_argument_prompt)
                .views
                .iter()
                .all(|view| !view.command_view),
            "mismatched candidate provider kind opened a prompt: {rejected_argument_prompt:#?}"
        );

        session.terminate_workspace().await.unwrap();
    });
}

#[test]
fn mica_owns_incremental_search_state_and_cancellation() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let mut editor = test_editor();
        let buffer = editor.windows[editor.active_window].active_buffer;
        editor.buffers[buffer].load_str("hello hello");
        editor.windows[editor.active_window].cursor = 5;
        let mut session = test_mica_client_with_clock(
            editor,
            CapabilityGrants::editor_default(),
            Arc::new(FixedNativeClock(42)),
        )
        .unwrap();
        let control =
            LogicalKey::Modifier(crate::keys::KeyModifier::Control(crate::keys::Side::Left));

        session
            .dispatch(session.envelope(InputEvent::Keys(vec![
                control,
                LogicalKey::AlphaNumeric('s'),
            ])))
            .await
            .unwrap();
        let searched = session
            .dispatch(session.envelope(InputEvent::Text("hello".to_owned())))
            .await
            .unwrap();
        assert_eq!(
            snapshot(&searched)
                .views
                .iter()
                .find(|view| !view.command_view)
                .unwrap()
                .cursor,
            0
        );
        assert!(
            snapshot(&searched)
                .views
                .iter()
                .any(|view| { !view.command_view && view.styled_ranges.len() == 2 })
        );
        assert!(snapshot(&searched).styles.iter().any(|style| {
            style.name == "isearch-current"
                && style.background
                    == Some(PresentationColor::Rgb {
                        r: 255,
                        g: 255,
                        b: 0,
                    })
        }));
        let next = session
            .dispatch(session.envelope(InputEvent::Keys(vec![
                control,
                LogicalKey::AlphaNumeric('s'),
            ])))
            .await
            .unwrap();
        assert_eq!(
            snapshot(&next)
                .views
                .iter()
                .find(|view| !view.command_view)
                .unwrap()
                .cursor,
            6
        );
        let cancelled = session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Esc])))
            .await
            .unwrap();
        assert_eq!(snapshot(&cancelled).views[0].cursor, 5);
        assert!(!snapshot(&cancelled).views[0].command_view);

        session.terminate_workspace().await.unwrap();
    });
}

#[test]
fn mica_quit_chord_remains_available_while_a_prompt_is_active() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let control =
            LogicalKey::Modifier(crate::keys::KeyModifier::Control(crate::keys::Side::Left));
        let meta = LogicalKey::Modifier(crate::keys::KeyModifier::Meta(crate::keys::Side::Left));

        for prompt_keys in [
            vec![meta, LogicalKey::AlphaNumeric('x')],
            vec![control, LogicalKey::AlphaNumeric('s')],
        ] {
            let mut session = test_mica_client_with_clock(
                test_editor(),
                CapabilityGrants::editor_default(),
                Arc::new(FixedNativeClock(42)),
            )
            .unwrap();
            let prompt = session
                .dispatch(session.envelope(InputEvent::Keys(prompt_keys)))
                .await
                .unwrap();
            assert!(snapshot(&prompt).views.iter().any(|view| view.command_view));

            let prefix = session
                .dispatch(session.envelope(InputEvent::Keys(vec![
                    control,
                    LogicalKey::AlphaNumeric('x'),
                ])))
                .await
                .unwrap();
            assert!(!prefix.lifecycle.contains(&LifecycleEvent::QuitRequested));

            let quit = session
                .dispatch(session.envelope(InputEvent::Keys(vec![
                    control,
                    LogicalKey::AlphaNumeric('c'),
                ])))
                .await
                .unwrap();
            assert!(quit.lifecycle.contains(&LifecycleEvent::QuitRequested));

            session.terminate_workspace().await.unwrap();
        }
    });
}
