use roe_core::keys::{KeyModifier, LogicalKey, Side};
use roe_core::native_kernel::CapabilityGrants;
use roe_core::session::{
    AttachmentConfiguration, DirectSessionClient, InputEvent, PresentationUpdate, SessionClient,
    WorkspaceHost,
};
use roe_core::{Buffer, Editor, Frame};
use roe_terminal::TerminalRenderer;
use roe_vello::VelloRenderer;

fn editor_fixture() -> Editor {
    let buffer = Buffer::named("*conformance*", roe_core::buffer::BufferKind::Scratch);
    buffer.load_str("let one = \"λ\"\n1 + 2");
    buffer.set_mark(14);
    let mut editor = Editor::new(buffer, Frame::new(80, 23));
    editor.move_cursor_to(19, false);
    editor
}

#[test]
fn terminal_and_vello_consume_the_same_production_mica_session_stream() {
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let workspace =
            WorkspaceHost::open_with_mica(editor_fixture(), CapabilityGrants::editor_default())
                .unwrap();
        let mut session =
            DirectSessionClient::new(workspace, AttachmentConfiguration::headless(80, 23));
        let mut outputs = vec![session.initial_output().await];
        let control = || LogicalKey::Modifier(KeyModifier::Control(Side::Left));
        outputs.push(
            session
                .dispatch(session.envelope(InputEvent::Keys(vec![
                    control(),
                    LogicalKey::AlphaNumeric('c'),
                    control(),
                    LogicalKey::AlphaNumeric('r'),
                ])))
                .await
                .unwrap(),
        );
        assert!(
            outputs
                .last()
                .unwrap()
                .presentation
                .as_ref()
                .is_some_and(|update| update_snapshot(update).views[0].typeout.is_some())
        );
        outputs.push(
            session
                .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::AlphaNumeric(' ')])))
                .await
                .unwrap(),
        );
        outputs.push(
            session
                .dispatch(session.envelope(InputEvent::Text("x".to_owned())))
                .await
                .unwrap(),
        );
        outputs.push(
            session
                .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Backspace])))
                .await
                .unwrap(),
        );
        outputs.push(
            session
                .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Function(12)])))
                .await
                .unwrap(),
        );

        assert!(matches!(
            outputs[0].presentation,
            Some(PresentationUpdate::Full(_))
        ));
        assert!(
            outputs[1..]
                .iter()
                .all(|output| matches!(output.presentation, Some(PresentationUpdate::Delta(_))))
        );

        let mut terminal = TerminalRenderer::new(Vec::new());
        let mut vello = VelloRenderer::new();
        for output in outputs {
            let update = output.presentation.as_ref().unwrap();
            terminal.apply_session_presentation(update).unwrap();
            terminal.render_session().unwrap();
            vello.apply_session_presentation(update).unwrap();
            assert_eq!(
                terminal.session_presentation().current(),
                vello.session_presentation().current()
            );
        }

        let current = terminal.session_presentation().current().unwrap();
        assert!(current.views[0].visible_text.starts_with("let one = \"λ\""));
        assert!(current.views[0].visible_text.len() > "let one = \"λ\"".len());
        assert!(!current.views[0].styled_ranges.is_empty());
        assert!(
            current
                .styles
                .iter()
                .any(|style| style.name == "mica-keyword")
        );
        session.terminate_workspace().await.unwrap();
    });
}

fn update_snapshot(update: &PresentationUpdate) -> &roe_core::session::PresentationSnapshot {
    match update {
        PresentationUpdate::Full(snapshot) => snapshot,
        PresentationUpdate::Delta(delta) => &delta.snapshot,
    }
}

#[test]
fn markdown_mode_reaches_both_frontends() {
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let source = "# λ **bold**\n\n  - item";
        let buffer = Buffer::named("notes.md", roe_core::buffer::BufferKind::Ordinary);
        buffer.set_visited_file(Some("notes.md".into()));
        buffer.load_str(source);
        let mut editor = Editor::new(buffer, Frame::new(80, 23));
        editor.move_cursor_to(source.chars().count(), false);
        let workspace =
            WorkspaceHost::open_with_mica(editor, CapabilityGrants::editor_default()).unwrap();
        let mut session =
            DirectSessionClient::new(workspace, AttachmentConfiguration::headless(80, 23));
        let initial = session.initial_output().await;
        let tab = session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Tab])))
            .await
            .unwrap();
        let newline = session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Enter])))
            .await
            .unwrap();
        let undo = session
            .dispatch(session.envelope(InputEvent::Keys(vec![
                LogicalKey::Modifier(KeyModifier::Control(Side::Left)),
                LogicalKey::AlphaNumeric('/'),
            ])))
            .await
            .unwrap();
        let mut terminal = TerminalRenderer::new(Vec::new());
        let mut vello = VelloRenderer::new();
        assert!(matches!(
            initial.presentation,
            Some(PresentationUpdate::Full(_))
        ));
        for output in [&initial, &tab, &newline, &undo] {
            assert!(
                !output
                    .lifecycle
                    .iter()
                    .any(|event| matches!(event, roe_core::session::LifecycleEvent::Error(_))),
                "{output:#?}"
            );
            let update = output.presentation.as_ref().unwrap();
            terminal.apply_session_presentation(update).unwrap();
            terminal.render_session().unwrap();
            vello.apply_session_presentation(update).unwrap();
            assert_eq!(
                terminal.session_presentation().current(),
                vello.session_presentation().current()
            );
        }
        assert!(matches!(
            newline.presentation,
            Some(PresentationUpdate::Delta(_))
        ));
        assert_eq!(
            update_snapshot(newline.presentation.as_ref().unwrap()).views[0].visible_text,
            format!("{source}  \n  ")
        );
        let presented = terminal.session_presentation().current().unwrap();
        assert_eq!(presented.views[0].visible_text, format!("{source}  "));
        assert!(presented.views[0].modeline.contains("(markdown)"));
        assert!(
            presented
                .styles
                .iter()
                .any(|style| style.name == "markdown-strong" && style.bold)
        );
        assert!(!presented.views[0].styled_ranges.is_empty());
        session.terminate_workspace().await.unwrap();
    });
}

#[test]
fn rust_mode_highlights_and_indentation_reach_both_frontends() {
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let buffer = Buffer::named("example.rs", roe_core::buffer::BufferKind::File);
        buffer.set_visited_file(Some("example.rs".into()));
        buffer.load_str("fn main() {\nlet λ = 42;\n}\n");
        let mut editor = Editor::new(buffer, Frame::new(80, 23));
        editor.move_cursor_to(12, false);
        let workspace = WorkspaceHost::open_with_mica(editor, CapabilityGrants::editor_default()).unwrap();
        let mut session = DirectSessionClient::new(workspace, AttachmentConfiguration::headless(80, 23));
        let mut terminal = TerminalRenderer::new(Vec::new());
        let mut vello = VelloRenderer::new();
        let initial = session.initial_output().await;
        let indented = session.dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Tab]))).await.unwrap();
        let newline = session.dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Enter]))).await.unwrap();
        let undo = session.dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Modifier(KeyModifier::Control(Side::Left)), LogicalKey::AlphaNumeric('/')]))).await.unwrap();
        for output in [&initial, &indented, &newline, &undo] {
            assert!(!output.lifecycle.iter().any(|event| matches!(event, roe_core::session::LifecycleEvent::Error(_))), "{output:#?}");
            let update = output.presentation.as_ref().unwrap();
            terminal.apply_session_presentation(update).unwrap();
            terminal.render_session().unwrap();
            vello.apply_session_presentation(update).unwrap();
            assert_eq!(terminal.session_presentation().current(), vello.session_presentation().current());
        }
        let snapshot = terminal.session_presentation().current().unwrap();
        assert_eq!(snapshot.views[0].visible_text, "fn main() {\n    let λ = 42;\n}\n");
        assert!(snapshot.views[0].modeline.contains("(rust)"));
        assert!(snapshot.styles.iter().any(|style| style.name == "syntax-keyword"));
        assert!(matches!(&indented.presentation, Some(PresentationUpdate::Delta(delta)) if !delta.invalidations.contains(&roe_core::session::Invalidation::Full)));
        session.terminate_workspace().await.unwrap();
    });
}
