// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-only

use super::*;

#[test]
fn mica_prompt_content_keeps_context_below_the_selected_candidate() {
    let update = MicaPromptUpdate {
        prefix: "M-x ".to_owned(),
        query: String::new(),
        selected: 9,
        candidates: (0..12).map(|index| format!("command-{index}")).collect(),
    };

    let prompt = mica_prompt_content(&update);
    let lines: Vec<_> = prompt.content.lines().collect();
    assert_eq!(prompt.cursor, "M-x ".chars().count());
    assert_eq!(lines.len(), MICA_PROMPT_HEIGHT.saturating_sub(2) as usize);
    assert_eq!(lines[1], "command-5");
    assert_eq!(lines[5], "command-9");
    assert_eq!(lines[7], "command-11");
    assert_eq!(prompt.selected_line, Some(5));
    assert!(!prompt.content.contains('>'));

    let mut at_top = update;
    at_top.selected = 0;
    let prompt = mica_prompt_content(&at_top);
    assert_eq!(prompt.selected_line, Some(1));
    assert!(prompt.content.contains("\ncommand-0\n"));
    assert!(prompt.content.contains("\ncommand-6"));
    assert!(!prompt.content.contains("command-7"));
}

#[test]
fn typeout_pages_are_attachment_local_and_the_final_page_closes() {
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let editor = test_editor();
        let mut session = attach_test_workspace(
            WorkspaceHost::open(editor, CapabilityGrants::editor_default()).unwrap(),
        );
        let view = session.workspace.editor.active_window;
        let buffer = session.workspace.editor.windows[view].active_buffer;
        let text = (0..40)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        session
            .workspace
            .show_typeout(
                &mut session.attachment,
                view,
                buffer,
                "information".to_owned(),
                "Paged output".to_owned(),
                text,
            )
            .unwrap();

        let first = session
            .workspace
            .capture_snapshot(&mut session.attachment)
            .views
            .into_iter()
            .find(|presented| presented.active)
            .unwrap()
            .typeout
            .unwrap();
        assert_eq!(first.first_visible_line, 0);
        assert!(first.more_after);

        let (_, closed) = session
            .workspace
            .page_typeout(&mut session.attachment, true)
            .unwrap();
        assert!(!closed);
        let second = session
            .workspace
            .capture_snapshot(&mut session.attachment)
            .views
            .into_iter()
            .find(|presented| presented.active)
            .unwrap()
            .typeout
            .unwrap();
        assert!(second.first_visible_line > 0);
        assert!(second.more_before);

        while session.workspace.typeout.is_some() {
            session
                .workspace
                .page_typeout(&mut session.attachment, true)
                .unwrap();
        }
        assert!(session.attachment.typeout_page.is_none());
    });
}
