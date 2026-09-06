// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-only

use super::*;

#[test]
fn mica_keymap_inserts_injected_native_time_and_redraws() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let mut session = test_mica_client_with_clock(
            test_editor(),
            CapabilityGrants::editor_default(),
            Arc::new(FixedNativeClock(1_700_000_000_123)),
        )
        .unwrap();

        let output = session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Function(12)])))
            .await
            .unwrap();
        assert_eq!(
            snapshot(&output).views[0].visible_text,
            "hello1700000000123\n",
            "{output:#?}"
        );
        assert_eq!(snapshot(&output).views[0].cursor, 19);
        let PresentationUpdate::Delta(delta) = output.presentation.as_ref().unwrap() else {
            panic!("Mica edit should produce a presentation delta");
        };
        assert!(
            delta
                .invalidations
                .iter()
                .any(|invalidation| matches!(invalidation, Invalidation::View(_)))
        );
        assert!(!delta.invalidations.contains(&Invalidation::Full));
        assert!(
            session
                .workspace
                .policy
                .modes
                .values()
                .any(|mode| mode == "fundamental")
        );
        assert_eq!(
            session
                .workspace
                .policy
                .configuration
                .get("tab_width")
                .map(String::as_str),
            Some("4")
        );
        let indented = session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Tab])))
            .await
            .unwrap();
        assert_eq!(
            snapshot(&indented).views[0].visible_text,
            "hello1700000000123\n    "
        );
        let PresentationUpdate::Delta(delta) = indented.presentation.as_ref().unwrap() else {
            panic!("Mica indent should produce a presentation delta");
        };
        assert!(!delta.invalidations.contains(&Invalidation::Full));
        assert!(
            output
                .lifecycle
                .iter()
                .all(|event| !matches!(event, LifecycleEvent::Error(_)))
        );

        let close = session.terminate_workspace().await.unwrap();
        assert!(
            close
                .lifecycle
                .contains(&LifecycleEvent::WorkspaceTerminated)
        );
    });
}

#[test]
fn unmodified_space_inserts_text_while_control_space_remains_a_key_chord() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let mut session =
            test_mica_client(test_editor(), CapabilityGrants::editor_default()).unwrap();
        let inserted = session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::AlphaNumeric(' ')])))
            .await
            .unwrap();
        assert_eq!(snapshot(&inserted).views[0].visible_text, "hello ");
        assert!(!snapshot(&inserted).echo_area.contains("undefined"));

        let marked = session
            .dispatch(session.envelope(InputEvent::Keys(vec![
                control(),
                LogicalKey::AlphaNumeric(' '),
            ])))
            .await
            .unwrap();
        let active = session.workspace.editor.active_window;
        let buffer = session.workspace.editor.windows[active].active_buffer;
        assert_eq!(session.workspace.editor.buffers[buffer].get_mark(), Some(6));
        assert!(
            marked
                .lifecycle
                .iter()
                .all(|event| { !matches!(event, LifecycleEvent::Error(_)) }),
            "{marked:#?}"
        );

        session.terminate_workspace().await.unwrap();
    });
}

#[test]
fn mica_mark_whole_buffer_selects_the_active_buffer_with_c_x_h() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let mut session =
            test_mica_client(test_editor(), CapabilityGrants::editor_default()).unwrap();
        let selected = session
            .dispatch(session.envelope(InputEvent::Keys(vec![
                control(),
                LogicalKey::AlphaNumeric('x'),
                LogicalKey::AlphaNumeric('h'),
            ])))
            .await
            .unwrap();

        let window = session.workspace.editor.active_window;
        let buffer = session.workspace.editor.windows[window].active_buffer;
        assert_eq!(session.workspace.editor.windows[window].cursor, 0);
        assert_eq!(session.workspace.editor.buffers[buffer].get_mark(), Some(5));
        assert_eq!(
            snapshot(&selected).views[0].selection,
            Some(TextSelection {
                anchor: 0,
                active: 5,
            })
        );
        assert_eq!(snapshot(&selected).echo_area, "Marked whole buffer");
        assert!(
            selected
                .lifecycle
                .iter()
                .all(|event| !matches!(event, LifecycleEvent::Error(_)))
        );

        session.terminate_workspace().await.unwrap();
    });
}

#[test]
fn mica_syntax_policy_controls_native_word_editing() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let mut editor = test_editor();
        let window = editor.active_window;
        let buffer = editor.windows[window].active_buffer;
        editor.buffers[buffer].load_str("foo-bar baz");
        editor.windows[window].cursor = 0;
        let mut session = test_mica_client_with_clock(
            editor,
            CapabilityGrants::editor_default(),
            Arc::new(FixedNativeClock(42)),
        )
        .unwrap();
        let meta = LogicalKey::Modifier(crate::keys::KeyModifier::Meta(crate::keys::Side::Left));

        let output = session
            .dispatch(session.envelope(InputEvent::Keys(vec![meta, LogicalKey::AlphaNumeric('d')])))
            .await
            .unwrap();

        // Mica's [[:alnum:]_] word rule makes '-' punctuation. The native
        // mechanism kills through the punctuation to the next word rather
        // than using the legacy Rust mode's non-whitespace definition.
        assert_eq!(snapshot(&output).views[0].visible_text, "bar baz");
        assert!(
            session.workspace.policy.syntax[&buffer]
                .iter()
                .any(|rule| rule.kind == "word"
                    && rule.pattern == "[[:alnum:]_]"
                    && rule.precedence == 100)
        );

        let original = include_str!("../../../../mica/roe-first-wave.mica");
        let hyphen_is_word = original.replace(
            "assert roe/SyntaxRule(#roe/fundamental_mode, :word, \"[[:alnum:]_]\", 100)",
            "assert roe/SyntaxRule(#roe/fundamental_mode, :word, \"[[:alpha:]-]\", 100)",
        );
        session
            .workspace
            .replace_mica_first_wave(hyphen_is_word)
            .await
            .unwrap();
        session.workspace.editor.buffers[buffer].load_str("foo-bar baz");
        session.workspace.editor.windows[window].cursor = 0;
        let moved = session
            .dispatch(session.envelope(InputEvent::Keys(vec![meta, LogicalKey::AlphaNumeric('f')])))
            .await
            .unwrap();
        assert_eq!(snapshot(&moved).views[0].cursor, 8);
        session.workspace.editor.windows[window].cursor = 0;
        let replaced = session
            .dispatch(session.envelope(InputEvent::Keys(vec![meta, LogicalKey::AlphaNumeric('d')])))
            .await
            .unwrap();
        assert_eq!(snapshot(&replaced).views[0].visible_text, "baz");

        let unsupported = original.replace(
            "assert roe/SyntaxRule(#roe/fundamental_mode, :word, \"[[:alnum:]_]\", 100)",
            "assert roe/SyntaxRule(#roe/fundamental_mode, :word, \"word\", 100)",
        );
        session
            .workspace
            .replace_mica_first_wave(unsupported)
            .await
            .unwrap();
        session.workspace.editor.buffers[buffer].load_str("unchanged");
        session.workspace.editor.windows[window].cursor = 0;
        let rejected = session
            .dispatch(session.envelope(InputEvent::Keys(vec![meta, LogicalKey::AlphaNumeric('d')])))
            .await
            .unwrap();
        assert_eq!(snapshot(&rejected).views[0].visible_text, "unchanged");
        assert!(rejected.lifecycle.iter().any(|event| matches!(
            event,
            LifecycleEvent::Error(message)
                if message.contains("unsupported Mica word syntax pattern")
        )));

        session.terminate_workspace().await.unwrap();
    });
}

#[test]
fn mica_policy_highlights_scratch_and_mica_file_buffers() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let source = "// λ\nverb roe/demo(?value)\n  return :ok\nend";
        let mut scratch_editor = test_editor();
        let scratch = scratch_editor.windows[scratch_editor.active_window].active_buffer;
        scratch_editor.buffers[scratch].set_kind(crate::buffer::BufferKind::Scratch);
        scratch_editor.buffers[scratch].load_str(source);
        scratch_editor.windows[scratch_editor.active_window].cursor = 0;
        let mut scratch_session =
            test_mica_client(scratch_editor, CapabilityGrants::editor_default()).unwrap();
        let scratch_output = scratch_session.initial_output().await;
        let scratch_snapshot = snapshot(&scratch_output);
        let scratch_view = scratch_snapshot
            .views
            .iter()
            .find(|view| view.active)
            .unwrap();
        let style_names: HashMap<_, _> = scratch_snapshot
            .styles
            .iter()
            .map(|style| (style.id, style.name.as_str()))
            .collect();
        let range_styles: Vec<_> = scratch_view
            .styled_ranges
            .iter()
            .map(|range| style_names[&range.style])
            .collect();
        assert!(range_styles.contains(&"mica-comment"));
        assert!(range_styles.contains(&"mica-keyword"));
        assert!(range_styles.contains(&"mica-identifier"));
        assert!(range_styles.contains(&"mica-variable"));
        assert!(range_styles.contains(&"mica-symbol"));
        assert_eq!(scratch_session.workspace.policy.modes[&scratch], "mica");
        assert_eq!(
            scratch_session
                .workspace
                .presentation
                .highlight_revision(scratch)
                .unwrap(),
            scratch_session.workspace.editor.buffers[scratch].text_revision()
        );
        let highlighted_revision = scratch_session
            .workspace
            .presentation
            .highlight_revision(scratch)
            .unwrap();
        let edited = scratch_session
            .dispatch(scratch_session.envelope(InputEvent::Text("1".to_owned())))
            .await
            .unwrap();
        assert!(
            scratch_session
                .workspace
                .presentation
                .highlight_revision(scratch)
                .unwrap()
                > highlighted_revision
        );
        assert!(
            snapshot(&edited)
                .styles
                .iter()
                .any(|style| style.name == "mica-number")
        );
        scratch_session.terminate_workspace().await.unwrap();

        let mut file_editor = test_editor();
        let file_buffer = file_editor.windows[file_editor.active_window].active_buffer;
        file_editor.buffers[file_buffer].set_visited_file(Some(PathBuf::from("/tmp/policy.MICA")));
        file_editor.buffers[file_buffer].load_str("assert roe/Face(#roe/demo)");
        file_editor.windows[file_editor.active_window].cursor = 0;
        let mut file_session =
            test_mica_client(file_editor, CapabilityGrants::editor_default()).unwrap();
        let file_output = file_session.initial_output().await;
        let file_snapshot = snapshot(&file_output);
        let file_view = file_snapshot.views.iter().find(|view| view.active).unwrap();
        let file_style_names: HashMap<_, _> = file_snapshot
            .styles
            .iter()
            .map(|style| (style.id, style.name.as_str()))
            .collect();
        assert!(
            file_view
                .styled_ranges
                .iter()
                .any(|range| file_style_names[&range.style] == "mica-keyword")
        );
        assert!(
            file_view
                .styled_ranges
                .iter()
                .any(|range| file_style_names[&range.style] == "mica-identity")
        );
        assert_eq!(file_session.workspace.policy.modes[&file_buffer], "mica");
        file_session.terminate_workspace().await.unwrap();
    });
}

#[test]
fn mica_orders_bindings_and_edit_hooks_by_precedence() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let mut session = test_mica_client_with_clock(
            test_editor(),
            CapabilityGrants::editor_default(),
            Arc::new(FixedNativeClock(42)),
        )
        .unwrap();
        let original = include_str!("../../../../mica/roe-first-wave.mica");
        let ordered = format!(
            "{original}\nassert roe/NativeBinding(\"x\", :cursor_right, 6000)\nassert roe/KeyBinding(#roe/global_map, \"x\", #roe/redraw, 7000)\nassert roe/ModeHook(#roe/fundamental_mode, :low_hook, 10)\nassert roe/ModeHook(#roe/fundamental_mode, :high_hook, 50)\nassert RoleCanInvoke(#roe/editor_role, :low_hook)\nassert RoleCanInvoke(#roe/editor_role, :high_hook)\nverb low_hook(actor, session, view, buffer)\n  emit(session, {{:kind -> :host_action, :action -> :low_hook, :view -> view}})\n  return :ok\nend\nverb high_hook(actor, session, view, buffer)\n  emit(session, {{:kind -> :host_action, :action -> :high_hook, :view -> view}})\n  return :ok\nend\n"
        );
        session.workspace.replace_mica_first_wave(ordered).await.unwrap();

        let command_wins = session
            .dispatch(session.envelope(InputEvent::Text("x".to_owned())))
            .await
            .unwrap();
        assert_eq!(snapshot(&command_wins).views[0].visible_text, "hello");

        let buffer = session.workspace.editor.windows[session.workspace.editor.active_window].active_buffer;
        session.workspace.editor.buffers[buffer].load_str("hello");
        session.workspace.editor.windows[session.workspace.editor.active_window].cursor = 5;
        let with_hooks = session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Backspace])))
            .await
            .unwrap();
        let hook_errors: Vec<_> = with_hooks
            .lifecycle
            .iter()
            .filter_map(|event| match event {
                LifecycleEvent::Error(message) if message.contains("_hook") => Some(message),
                _ => None,
            })
            .collect();
        assert_eq!(hook_errors.len(), 2);
        assert!(hook_errors[0].contains("high_hook"));
        assert!(hook_errors[1].contains("low_hook"));

        let ambiguous = format!(
            "{original}\nassert roe/KeyBinding(#roe/global_map, \"x\", #roe/redraw, 7000)\nassert roe/KeyBinding(#roe/global_map, \"x\", #roe/quit, 7000)\n"
        );
        session.workspace.replace_mica_first_wave(ambiguous).await.unwrap();
        let rejected = session
            .dispatch(session.envelope(InputEvent::Text("x".to_owned())))
            .await
            .unwrap();
        assert!(rejected.lifecycle.iter().any(|event| matches!(
            event,
            LifecycleEvent::Error(message) if message.contains("ambiguous command key binding")
        )));
        assert!(!rejected.lifecycle.contains(&LifecycleEvent::QuitRequested));

        session
            .terminate_workspace()
            .await
            .unwrap();
    });
}

#[test]
fn mica_context_tracks_rust_cursor_and_new_active_buffer() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let mut session = test_mica_client_with_clock(
            test_editor(),
            CapabilityGrants::editor_default(),
            Arc::new(FixedNativeClock(42)),
        )
        .unwrap();

        let typed = session
            .dispatch(session.envelope(InputEvent::Text("é".to_owned())))
            .await
            .unwrap();
        assert_eq!(snapshot(&typed).views[0].visible_text, "helloé");
        let after_rust_edit = session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Function(12)])))
            .await
            .unwrap();
        assert_eq!(
            snapshot(&after_rust_edit).views[0].visible_text,
            "helloé42\n"
        );

        let original_view = session.workspace.editor.active_window;
        let buffer = session
            .workspace
            .editor
            .create_buffer("*dynamic*".to_owned(), "new".to_owned());
        let view = session.workspace.editor.split_horizontal();
        session.workspace.editor.active_window = view;
        session.workspace.editor.windows[view].active_buffer = buffer;
        session.workspace.editor.windows[view].cursor = 3;
        let _ = session.workspace.synchronize_identities();

        let dynamic = session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Function(12)])))
            .await
            .unwrap();
        let active = snapshot(&dynamic)
            .views
            .iter()
            .find(|presented| presented.active)
            .unwrap();
        assert_eq!(active.visible_text, "new42\n");
        assert_eq!(active.cursor, 6);
        assert_eq!(
            session
                .workspace
                .mica
                .as_ref()
                .unwrap()
                .identity_counts_for_test(),
            (4, 2)
        );

        session.workspace.editor.active_window = original_view;
        session.workspace.editor.windows.remove(view);
        session.workspace.editor.window_tree = WindowNode::new_leaf(original_view);
        session.workspace.editor.buffers.remove(buffer);
        let _ = session.workspace.synchronize_identities();
        let after_removal = session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Function(12)])))
            .await
            .unwrap();
        assert_eq!(
            session
                .workspace
                .mica
                .as_ref()
                .unwrap()
                .identity_counts_for_test(),
            (3, 1)
        );
        assert_eq!(
            snapshot(&after_removal).views[0].visible_text,
            "helloé42\n42\n"
        );

        session.terminate_workspace().await.unwrap();
    });
}
