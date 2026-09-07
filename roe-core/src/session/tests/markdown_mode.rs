// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-only

use super::*;

fn markdown_editor(source: &str) -> Editor {
    let buffer = Buffer::named("notes.md", crate::buffer::BufferKind::Ordinary);
    buffer.set_visited_file(Some("notes.md".into()));
    buffer.load_str(source);
    Editor::new(buffer, Frame::new(80, 23))
}

async fn key(session: &mut DirectSessionClient, key: LogicalKey) -> SessionOutput {
    let output = session
        .dispatch(session.envelope(InputEvent::Keys(vec![key])))
        .await
        .unwrap();
    assert!(
        !output
            .lifecycle
            .iter()
            .any(|event| matches!(event, LifecycleEvent::Error(_))),
        "{output:#?}"
    );
    output
}

async fn command(session: &mut DirectSessionClient, name: &str) -> SessionOutput {
    session
        .dispatch(session.envelope(InputEvent::Keys(vec![
            LogicalKey::Modifier(crate::keys::KeyModifier::Meta(crate::keys::Side::Left)),
            LogicalKey::AlphaNumeric('x'),
        ])))
        .await
        .unwrap();
    session
        .dispatch(session.envelope(InputEvent::Text(name.into())))
        .await
        .unwrap();
    key(session, LogicalKey::Enter).await
}

fn face_at<'a>(snapshot: &'a PresentationSnapshot, text: &str) -> &'a StyleDefinition {
    let view = &snapshot.views[0];
    let at = view.visible_text[..view.visible_text.find(text).unwrap()]
        .chars()
        .count();
    let span = view
        .styled_ranges
        .iter()
        .find(|span| span.start <= at && span.end > at)
        .unwrap_or_else(|| panic!("no style for {text:?}: {snapshot:#?}"));
    snapshot
        .styles
        .iter()
        .find(|style| style.id == span.style)
        .unwrap()
}

#[test]
fn mica_markdown_highlights_block_and_inline_syntax() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let source = "# Heading λ **strong**\n\nSetext\n------\n\n*italic* and **bold** [link](https://example.com) `code`\n\n- [x] task\n> quote\n\n| H | I |\n|---|---|\n| **cell** | v |\n\n```rust\n*literal*\n```\n\n    *indented*\n";
        let mut session = test_mica_client(markdown_editor(source), CapabilityGrants::editor_default()).unwrap();
        let output = key(&mut session, LogicalKey::Function(1)).await;
        let presented = snapshot(&output);
        assert!(presented.views[0].modeline.contains("(markdown)"));
        for (text, name) in [("Heading", "heading"), ("Setext", "heading"), ("strong", "strong"), ("italic", "emphasis"), ("bold", "strong"), ("link", "link"), ("https", "link"), ("code", "code"), ("[x]", "list"), (">", "quote"), ("cell", "strong"), ("*literal*", "code"), ("*indented*", "code")] {
            assert_eq!(face_at(presented, text).name, format!("markdown-{name}"), "{text}");
        }
        assert!(face_at(presented, "Heading").bold);
        assert!(face_at(presented, "italic").italic);
        assert!(face_at(presented, "link").underline);
        session.terminate_workspace().await.unwrap();
    });
}

#[test]
fn mica_markdown_preserves_structural_whitespace_and_groups_newline_undo() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let mut session =
            test_mica_client(markdown_editor(""), CapabilityGrants::editor_default()).unwrap();
        session.initial_output().await;
        let view = session.workspace.editor.active_window;
        let buffer = session.workspace.editor.windows[view].active_buffer;
        for (source, expected) in [
            ("  - item│", "  - item\n  "),
            ("  10. item│", "  10. item\n  "),
            ("  > quote│", "  > quote\n  "),
            ("text  │", "text  \n"),
            ("```\n    λ│code\n```", "```\n    λ\n    code\n```"),
            ("    code│", "    code\n    "),
            ("\tcode│", "\tcode\n\t"),
        ] {
            let (before, after) = source.split_once('│').unwrap();
            let cursor = before.chars().count();
            let source = format!("{before}{after}");
            session.workspace.editor.buffers[buffer].load_str(&source);
            session.workspace.editor.windows[view].cursor = cursor;
            key(&mut session, LogicalKey::Enter).await;
            assert_eq!(
                session.workspace.editor.buffers[buffer].content(),
                expected,
                "{source:?}"
            );
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
                "one undo restores newline and indentation"
            );
        }
        session.workspace.editor.buffers[buffer].load_str("  - item  \n");
        session.workspace.editor.windows[view].cursor = 2;
        command(&mut session, "indent-line").await;
        assert_eq!(
            session.workspace.editor.buffers[buffer].content(),
            "  - item  \n"
        );
        key(&mut session, LogicalKey::Tab).await;
        assert_eq!(
            session.workspace.editor.buffers[buffer].content(),
            "    - item  \n"
        );
        session.workspace.editor.buffers[buffer].set_read_only(true);
        let before = session.workspace.editor.buffers[buffer].content();
        session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Enter])))
            .await
            .unwrap();
        session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Tab])))
            .await
            .unwrap();
        assert_eq!(session.workspace.editor.buffers[buffer].content(), before);
        session.terminate_workspace().await.unwrap();
    });
}

#[test]
fn mica_markdown_selection_replacement_and_packages_are_independent() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let mut session = test_mica_client(
            markdown_editor("**bold**"),
            CapabilityGrants::editor_default(),
        )
        .unwrap();
        let view = session.workspace.editor.active_window;
        let buffer = session.workspace.editor.windows[view].active_buffer;
        for path in ["notes.md", "notes.markdown", "notes.mdown"] {
            session.workspace.editor.buffers[buffer].set_visited_file(Some(path.into()));
            key(&mut session, LogicalKey::Function(1)).await;
            assert_eq!(session.workspace.policy.modes[&buffer], "markdown");
        }
        session.workspace.editor.buffers[buffer].set_visited_file(None);
        key(&mut session, LogicalKey::Function(1)).await;
        assert_eq!(
            session.workspace.policy.modes[&buffer], "fundamental",
            "a display name is not a file association"
        );
        command(&mut session, "markdown-mode").await;
        assert_eq!(session.workspace.policy.modes[&buffer], "markdown");
        session
            .workspace
            .set_mica_package_enabled("roe/rust_package", false)
            .unwrap();
        let changed = include_str!("../../../../mica/roe-markdown.mica")
            .replace(":indent_width, 2", ":indent_width, 4")
            .replace("(strong_emphasis) @strong", "(strong_emphasis) @emphasis");
        session
            .workspace
            .replace_mica_unit("roe/markdown", changed)
            .await
            .unwrap();
        let output = key(&mut session, LogicalKey::Function(1)).await;
        assert_eq!(face_at(snapshot(&output), "bold").name, "markdown-emphasis");
        session.workspace.editor.windows[view].cursor = 0;
        key(&mut session, LogicalKey::Tab).await;
        assert_eq!(
            session.workspace.editor.buffers[buffer].content(),
            "    **bold**"
        );
        session.workspace.editor.windows[view].cursor = 12;
        key(&mut session, LogicalKey::Enter).await;
        assert_eq!(
            session.workspace.editor.buffers[buffer].content(),
            "    **bold**\n    ",
            "shared commands must work with Rust disabled"
        );
        let good = session
            .workspace
            .export_mica_unit("roe/markdown")
            .await
            .unwrap();
        for bad in [
            good.replace("(emphasis) @emphasis", "(not_a_node) @emphasis"),
            good.replace("] @content", "] @wrong"),
        ] {
            assert_ne!(bad, good);
            assert!(
                session
                    .workspace
                    .replace_mica_unit("roe/markdown", bad)
                    .await
                    .is_err()
            );
            assert_eq!(
                session
                    .workspace
                    .export_mica_unit("roe/markdown")
                    .await
                    .unwrap(),
                good
            );
        }
        session
            .workspace
            .set_mica_package_enabled("roe/markdown_package", false)
            .unwrap();
        key(&mut session, LogicalKey::Function(1)).await;
        assert_eq!(session.workspace.policy.modes[&buffer], "fundamental");
        session
            .workspace
            .set_mica_package_enabled("roe/markdown_package", true)
            .unwrap();
        key(&mut session, LogicalKey::Function(1)).await;
        assert_eq!(session.workspace.policy.modes[&buffer], "markdown");
        session.workspace.restore_mica_first_wave().await.unwrap();
        session.workspace.editor.buffers[buffer].load_str("**bold**");
        session.workspace.editor.windows[view].cursor = 0;
        let restored = key(&mut session, LogicalKey::Tab).await;
        assert_eq!(
            session.workspace.editor.buffers[buffer].content(),
            "  **bold**"
        );
        assert_eq!(face_at(snapshot(&restored), "bold").name, "markdown-strong");
        session.terminate_workspace().await.unwrap();
    });
}
