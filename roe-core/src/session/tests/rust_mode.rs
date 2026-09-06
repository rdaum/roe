// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-only

use super::*;

fn rust_editor(source: &str, cursor: usize) -> Editor {
    let buffer = Buffer::named("example.rs", crate::buffer::BufferKind::File);
    buffer.set_visited_file(Some(std::path::PathBuf::from("example.rs")));
    buffer.load_str(source);
    let mut editor = Editor::new(buffer, Frame::new(80, 23));
    editor.move_cursor_to(cursor, false);
    editor
}

fn marked(source: &str) -> (String, usize) {
    let (before, after) = source.split_once('│').unwrap();
    (format!("{before}{after}"), before.chars().count())
}

async fn rust_mode_command(session: &mut DirectSessionClient) -> SessionOutput {
    session
        .dispatch(session.envelope(InputEvent::Keys(vec![
            LogicalKey::Modifier(crate::keys::KeyModifier::Meta(crate::keys::Side::Left)),
            LogicalKey::AlphaNumeric('x'),
        ])))
        .await
        .unwrap();
    session
        .dispatch(session.envelope(InputEvent::Text("rust-mode".into())))
        .await
        .unwrap();
    session
        .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Enter])))
        .await
        .unwrap()
}

#[test]
fn mica_rust_mode_is_buffer_local_replaceable_and_package_controlled() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let buffer = Buffer::named("*scratch*", crate::buffer::BufferKind::Scratch);
        buffer.load_str("fn f() {\nlet λ = 1;\n}");
        let mut editor = Editor::new(buffer, Frame::new(80, 23));
        editor.move_cursor_to(9, false);
        let mut session = test_mica_client(editor, CapabilityGrants::editor_default()).unwrap();
        let output = rust_mode_command(&mut session).await;
        assert!(
            !output
                .lifecycle
                .iter()
                .any(|event| matches!(event, LifecycleEvent::Error(_))),
            "{output:#?}"
        );
        let window = session.workspace.editor.active_window;
        let buffer = session.workspace.editor.windows[window].active_buffer;
        assert_eq!(session.workspace.policy.modes[&buffer], "rust");
        assert!(
            session.workspace.editor.buffers[buffer]
                .visited_file()
                .is_none()
        );
        let original = include_str!("../../../../mica/roe-rust.mica");
        let changed = original
            .replace(":indent_width, 4", ":indent_width, 2")
            .replace("\"#c586c0\"", "\"#ff0000\"");
        session
            .workspace
            .replace_mica_unit("roe/rust", changed)
            .await
            .unwrap();
        let output = session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Tab])))
            .await
            .unwrap();
        assert!(
            !output
                .lifecycle
                .iter()
                .any(|event| matches!(event, LifecycleEvent::Error(_))),
            "{output:#?}"
        );
        assert_eq!(
            session.workspace.editor.buffers[buffer].content(),
            "fn f() {\n  let λ = 1;\n}"
        );
        assert!(
            snapshot(&output)
                .styles
                .iter()
                .any(|style| style.name == "syntax-keyword"
                    && style.foreground == Some(PresentationColor::Rgb { r: 255, g: 0, b: 0 })),
            "{output:#?}"
        );
        let changed_rules = original
            .replace(":indent_width, 4", ":indent_width, 2")
            .replace(":line, 1, 100, true", ":line, 2, 100, true")
            .replace("\"Tab\", #roe/indent_line", "\"F8\", #roe/indent_line")
            .replace("(identifier) @variable", "(identifier) @keyword");
        let with_minor_mode = format!(
            "{changed_rules}\n{}",
            r#"
make_identity(:roe/rust_test_minor)
assert roe/MinorMode(#roe/rust_test_minor)
assert roe/PackageMode(#roe/rust_package, #roe/rust_test_minor)
assert roe/SyntaxHighlightRule(#roe/rust_test_minor, :keyword, #roe/syntax_string_face, 500)
roe/BufferMinorMode(buffer, #roe/rust_test_minor) :-
  roe/BufferModeOverride(buffer, #roe/rust_mode)
"#
        );
        session
            .workspace
            .replace_mica_unit("roe/rust", with_minor_mode)
            .await
            .unwrap();
        let output = session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Function(8)])))
            .await
            .unwrap();
        assert!(
            !output
                .lifecycle
                .iter()
                .any(|event| matches!(event, LifecycleEvent::Error(_))),
            "{output:#?}"
        );
        assert_eq!(
            session.workspace.editor.buffers[buffer].content(),
            "fn f() {\n    let λ = 1;\n}",
            "the replacement rule and F8 binding must take effect without a Rust rebuild"
        );
        let presented = snapshot(&output);
        let lambda = presented.views[0]
            .visible_text
            .chars()
            .position(|ch| ch == 'λ')
            .unwrap();
        let range = presented.views[0]
            .styled_ranges
            .iter()
            .find(|span| span.start <= lambda && span.end > lambda)
            .unwrap();
        assert_eq!(
            presented
                .styles
                .iter()
                .find(|style| style.id == range.style)
                .unwrap()
                .name,
            "syntax-string",
            "the changed query capture must use the effective minor-mode face"
        );
        let good = session
            .workspace
            .export_mica_unit("roe/rust")
            .await
            .unwrap();
        let bad = good.replace("(identifier) @keyword", "(nonexistent_node) @keyword");
        assert_ne!(good, bad);
        assert!(
            session
                .workspace
                .replace_mica_unit("roe/rust", bad)
                .await
                .is_err()
        );
        assert_eq!(
            session
                .workspace
                .export_mica_unit("roe/rust")
                .await
                .unwrap(),
            good,
            "failed native query validation must restore the working unit"
        );
        session
            .workspace
            .set_mica_package_enabled("roe/rust_package", false)
            .unwrap();
        session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Function(1)])))
            .await
            .unwrap();
        assert_eq!(session.workspace.policy.modes[&buffer], "fundamental");
        session
            .workspace
            .set_mica_package_enabled("roe/rust_package", true)
            .unwrap();
        session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Function(1)])))
            .await
            .unwrap();
        assert_eq!(session.workspace.policy.modes[&buffer], "rust");
        session.terminate_workspace().await.unwrap();
    });
}

#[test]
fn mica_rust_indentation_denial_and_stale_effects_are_recoverable() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let mut session = test_mica_client(rust_editor("fn f() {\nlet x = 1;\n}", 9), CapabilityGrants::editor_default()).unwrap();
        session.initial_output().await;
        let view = session.workspace.editor.active_window;
        let buffer = session.workspace.editor.windows[view].active_buffer;
        let revision = session.workspace.editor.buffers[buffer].text_revision();
        session.workspace.editor.buffers[buffer].insert_pos(" ".into(), 9);
        let mut events = MicaEventBatch::default();
        events.push(crate::mica_host::MicaEvent::Host(crate::mica_host::MicaHostAction::Indent { view, buffer, revision, newline: false, width: 4, tab_width: 4 }));
        let mut lifecycle = Vec::new();
        session.workspace.apply_mica_events(&mut session.attachment, events, &mut lifecycle, &mut Vec::new()).await;
        assert!(lifecycle.iter().any(|event| matches!(event, LifecycleEvent::Error(message) if message.contains("revision changed"))));
        // Direct effect application bypasses the normal dispatch epilogue.
        session.workspace.synchronize_identities();
        let before = session.workspace.editor.buffers[buffer].content();
        session.workspace.mica.as_ref().unwrap().revoke_service_for_test("text_write");
        let denied = session.dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Tab]))).await.unwrap();
        assert!(denied.lifecycle.iter().any(|event| matches!(event, LifecycleEvent::Error(message) if message.contains("not authorized"))), "{denied:#?}");
        assert_eq!(session.workspace.editor.buffers[buffer].content(), before);
        assert!(!session.workspace.terminated);
        session.terminate_workspace().await.unwrap();
    });
}

#[test]
fn mica_rust_indentation_covers_structural_context_and_protected_text() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let mut session =
            test_mica_client(rust_editor("", 0), CapabilityGrants::editor_default()).unwrap();
        let window = session.workspace.editor.active_window;
        let buffer = session.workspace.editor.windows[window].active_buffer;
        let cases = [
            ("fn f() {\n│let x = 1;\n}", "fn f() {\n    let x = 1;\n}"),
            ("fn f() {\n    │}", "fn f() {\n}"),
            (
                "fn f() {\n    if true {\n│let x = 1;\n    }\n}",
                "fn f() {\n    if true {\n        let x = 1;\n    }\n}",
            ),
            ("fn f(\n│a: i32,\n) {}", "fn f(\n    a: i32,\n) {}"),
            ("fn f() {\n    let x =\n│foo();\n}", "fn f() {\n    let x =\n        foo();\n}"),
            ("fn f() {\n    match v {\n        Some(x) => {\n            if x {\n│foo();\n            }\n        }\n    }\n}", "fn f() {\n    match v {\n        Some(x) => {\n            if x {\n                foo();\n            }\n        }\n    }\n}"),
            (
                "fn f() {\n    foo(\n│value,\n    );\n}",
                "fn f() {\n    foo(\n        value,\n    );\n}",
            ),
            (
                "fn f() {\n    foo(one,\n│two);\n}",
                "fn f() {\n    foo(one,\n        two);\n}",
            ),
            (
                "fn f() {\n    let x = thing\n│.method();\n}",
                "fn f() {\n    let x = thing\n        .method();\n}",
            ),
            (
                "fn f() {\n    match v {\n│Some(x) => x,\n    }\n}",
                "fn f() {\n    match v {\n        Some(x) => x,\n    }\n}",
            ),
            (
                "fn f() {\n    match v {\n        Some(x) =>\n│handle(x),\n    }\n}",
                "fn f() {\n    match v {\n        Some(x) =>\n            handle(x),\n    }\n}",
            ),
            (
                "fn f() {\n    let s = r###\"hi\n  │keep λ🙂\n\"###;\n}",
                "fn f() {\n    let s = r###\"hi\n  keep λ🙂\n\"###;\n}",
            ),
            (
                "fn f() {\n    /* outer /* nested */\n  │keep\n    */\n}",
                "fn f() {\n    /* outer /* nested */\n  keep\n    */\n}",
            ),
            (
                "fn f() {\n    custom! {\n  │keep tokens\n    }\n}",
                "fn f() {\n    custom! {\n  keep tokens\n    }\n}",
            ),
            ("    │fn f() {}", "fn f() {}"),
        ];
        for (source, expected) in cases {
            let (source, cursor) = marked(source);
            session.workspace.editor.buffers[buffer].load_str(&source);
            session.workspace.editor.windows[window].cursor = cursor;
            let output = session
                .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Tab])))
                .await
                .unwrap();
            assert!(
                !output
                    .lifecycle
                    .iter()
                    .any(|event| matches!(event, LifecycleEvent::Error(_))),
                "source={source:?}: {output:#?}"
            );
            assert_eq!(
                session.workspace.editor.buffers[buffer].content(),
                expected,
                "source={source:?}"
            );
        }
        session.terminate_workspace().await.unwrap();
    });
}

#[test]
fn mica_rust_newline_is_one_undoable_edit_and_preserves_literal_contents() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let mut session = test_mica_client(
            rust_editor("fn f() {", 8),
            CapabilityGrants::editor_default(),
        )
        .unwrap();
        let window = session.workspace.editor.active_window;
        let buffer = session.workspace.editor.windows[window].active_buffer;
        for (source, expected) in [
            ("fn f() {│", "fn f() {\n    "),
            (
                "fn f() {\n    let x = 1;│\n}",
                "fn f() {\n    let x = 1;\n    \n}",
            ),
            (
                "fn f() {\n    let s = r#\"hello│world\"#;\n}",
                "fn f() {\n    let s = r#\"hello\nworld\"#;\n}",
            ),
        ] {
            let (source, cursor) = marked(source);
            session.workspace.editor.buffers[buffer].load_str(&source);
            session.workspace.editor.windows[window].cursor = cursor;
            let output = session
                .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Enter])))
                .await
                .unwrap();
            assert!(
                !output
                    .lifecycle
                    .iter()
                    .any(|event| matches!(event, LifecycleEvent::Error(_))),
                "{output:#?}"
            );
            assert_eq!(session.workspace.editor.buffers[buffer].content(), expected);
            session
                .dispatch(session.envelope(InputEvent::Keys(vec![
                    control(),
                    LogicalKey::AlphaNumeric('/'),
                ])))
                .await
                .unwrap();
            assert_eq!(
                session.workspace.editor.buffers[buffer].content(),
                source,
                "newline and indentation must undo together"
            );
        }
        session.terminate_workspace().await.unwrap();
    });
}

#[test]
fn mica_rust_mode_highlights_and_indents_through_the_session() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let mut session = test_mica_client(
            rust_editor("fn main() {\nlet λ = 42;\n}\n", 12),
            CapabilityGrants::editor_default(),
        )
        .unwrap();
        let output = session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Tab])))
            .await
            .unwrap();
        assert!(
            !output
                .lifecycle
                .iter()
                .any(|event| matches!(event, LifecycleEvent::Error(_))),
            "{output:#?}"
        );
        assert_eq!(
            snapshot(&output).views[0].visible_text,
            "fn main() {\n    let λ = 42;\n}\n",
            "{output:#?}"
        );
        assert!(
            snapshot(&output)
                .styles
                .iter()
                .any(|style| style.name == "syntax-keyword"),
            "{output:#?}"
        );
        let buffer =
            session.workspace.editor.windows[session.workspace.editor.active_window].active_buffer;
        assert_eq!(session.workspace.policy.modes[&buffer], "rust");
        let revision = session.workspace.editor.buffers[buffer].text_revision();
        session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Tab])))
            .await
            .unwrap();
        assert_eq!(
            session.workspace.editor.buffers[buffer].text_revision(),
            revision,
            "Tab must be idempotent"
        );
        session.terminate_workspace().await.unwrap();
    });
}
