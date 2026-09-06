// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-only

use super::*;

#[test]
fn mica_eval_buffer_atomically_files_in_the_scratch_unit() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let editor = test_editor();
        let scratch = editor.windows[editor.active_window].active_buffer;
        editor.buffers[scratch].set_kind(crate::buffer::BufferKind::Scratch);
        editor.buffers[scratch].set_display_name("*scratch*");
        editor.buffers[scratch].load_str("make_identity(:roe/scratch_probe)\n");
        let mut session = test_mica_client(editor, CapabilityGrants::editor_default()).unwrap();

        let filed_in = session
            .dispatch(session.envelope(InputEvent::Keys(vec![
                control(),
                LogicalKey::AlphaNumeric('c'),
                control(),
                LogicalKey::AlphaNumeric('b'),
            ])))
            .await
            .unwrap();
        assert!(
            filed_in
                .lifecycle
                .iter()
                .all(|event| { !matches!(event, LifecycleEvent::Error(_)) }),
            "{filed_in:#?}"
        );
        assert!(
            snapshot(&filed_in)
                .echo_area
                .contains("Filed in roe/user_scratch")
        );
        let retained = session
            .workspace
            .export_mica_unit("roe/user_scratch")
            .await
            .unwrap();
        assert!(retained.contains("scratch_probe"));

        session.workspace.editor.buffers[scratch].load_str("verb this is malformed");
        let rejected = session
            .dispatch(session.envelope(InputEvent::Keys(vec![
                control(),
                LogicalKey::AlphaNumeric('c'),
                control(),
                LogicalKey::AlphaNumeric('b'),
            ])))
            .await
            .unwrap();
        assert!(
            rejected
                .lifecycle
                .iter()
                .any(|event| { matches!(event, LifecycleEvent::Error(_)) }),
            "{rejected:#?}"
        );
        let diagnostics = snapshot(&rejected)
            .views
            .iter()
            .find(|view| view.active)
            .unwrap();
        assert_eq!(diagnostics.buffer_kind, "diagnostics");
        assert!(diagnostics.read_only);
        assert_eq!(
            session
                .workspace
                .export_mica_unit("roe/user_scratch")
                .await
                .unwrap(),
            retained
        );

        session.terminate_workspace().await.unwrap();
    });
}

#[test]
fn mica_eval_region_runs_selected_task_code_in_endpoint_context() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let mut editor = test_editor();
        let buffer = editor.windows[editor.active_window].active_buffer;
        editor.buffers[buffer].load_str("1 + 2");
        editor.buffers[buffer].set_mark(0);
        editor.windows[editor.active_window].cursor = 5;
        let mut session = test_mica_client(editor, CapabilityGrants::editor_default()).unwrap();

        let evaluated = session
            .dispatch(session.envelope(InputEvent::Keys(vec![
                control(),
                LogicalKey::AlphaNumeric('c'),
                control(),
                LogicalKey::AlphaNumeric('r'),
            ])))
            .await
            .unwrap();
        assert!(
            evaluated
                .lifecycle
                .iter()
                .all(|event| { !matches!(event, LifecycleEvent::Error(_)) }),
            "{evaluated:#?}"
        );
        let typeout = snapshot(&evaluated).views[0]
            .typeout
            .as_ref()
            .expect("region evaluation should route output to a typeout");
        assert_eq!(typeout.kind, "evaluation");
        assert_eq!(typeout.title, "Mica evaluation");
        assert_eq!(typeout.visible_text, "Mica => 3");
        assert_eq!(session.workspace.editor.buffers[buffer].content(), "1 + 2");

        let dismissed = session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::AlphaNumeric(' ')])))
            .await
            .unwrap();
        assert!(snapshot(&dismissed).views[0].typeout.is_none());
        assert_eq!(session.workspace.editor.buffers[buffer].content(), "1 + 2");

        let reevaluated = session
            .dispatch(session.envelope(InputEvent::Keys(vec![
                control(),
                LogicalKey::AlphaNumeric('c'),
                control(),
                LogicalKey::AlphaNumeric('r'),
            ])))
            .await
            .unwrap();
        assert!(snapshot(&reevaluated).views[0].typeout.is_some());
        let redispatched = session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::AlphaNumeric('z')])))
            .await
            .unwrap();
        assert!(snapshot(&redispatched).views[0].typeout.is_none());
        assert_eq!(session.workspace.editor.buffers[buffer].content(), "1 + 2z");

        session.terminate_workspace().await.unwrap();
    });
}

#[test]
fn mica_source_provider_prefers_live_roe_buffers_and_falls_back_to_disk() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let editor = test_editor();
        let buffer = editor.windows[editor.active_window].active_buffer;
        let cargo_toml = std::env::current_dir()
            .unwrap()
            .canonicalize()
            .unwrap()
            .join("Cargo.toml");
        editor.buffers[buffer].set_visited_file(Some(cargo_toml));
        editor.buffers[buffer].load_str("unsaved Roe source\n");
        let mut session = test_mica_client(editor, CapabilityGrants::editor_default()).unwrap();
        let query = concat!(
            "let exactly {provider, text, hash, source_version} = ",
            "source/FileText(#roe/source_repository, #roe/source_worktree, ",
            "\"Cargo.toml\", ?provider, ?text, ?hash, ?source_version)\n",
            "return [provider, text, source_version]"
        );

        let live = {
            let WorkspaceHost {
                editor,
                buffer_resources,
                mica,
                ..
            } = &mut session.workspace;
            mica.as_mut()
                .unwrap()
                .evaluate_source(editor, buffer_resources, query.to_owned())
                .await
                .unwrap()
        };
        assert!(live.value.contains("roe-buffer"), "{live:#?}");
        assert!(live.value.contains("unsaved Roe source"), "{live:#?}");
        assert!(live.value.contains("roe-buffer:"), "{live:#?}");

        session.workspace.editor.buffers[buffer].set_visited_file(None);
        let disk = {
            let WorkspaceHost {
                editor,
                buffer_resources,
                mica,
                ..
            } = &mut session.workspace;
            mica.as_mut()
                .unwrap()
                .evaluate_source(editor, buffer_resources, query.to_owned())
                .await
                .unwrap()
        };
        assert!(disk.value.contains("local-worktree"), "{disk:#?}");
        assert!(!disk.value.contains("unsaved Roe source"), "{disk:#?}");

        session.terminate_workspace().await.unwrap();
    });
}
