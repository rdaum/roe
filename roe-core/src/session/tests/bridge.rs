// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-only

use super::*;
use crate::mica_host::{MicaEvent, MicaHostAction, MicaNativeAction, MicaPolicyFact};

pub(super) async fn apply(
    session: &mut DirectSessionClient,
    events: MicaEventBatch,
) -> Vec<LifecycleEvent> {
    let mut lifecycle = Vec::new();
    session
        .workspace
        .apply_mica_events(
            &mut session.attachment,
            events,
            &mut lifecycle,
            &mut Vec::new(),
        )
        .await;
    lifecycle
}

async fn evaluate(session: &mut DirectSessionClient, source: &str) -> MicaEventBatch {
    session.workspace.synchronize_identities();
    let source = format!(
        "let exactly {{:session -> session, :actor -> actor}} = roe/SessionActor(?session, ?actor)\n{source}"
    );
    let WorkspaceHost {
        editor,
        buffer_resources,
        mica,
        ..
    } = &mut session.workspace;
    mica.as_mut()
        .unwrap()
        .evaluate_source(editor, buffer_resources, source)
        .await
        .unwrap()
        .events
}

#[test]
fn bridge_host_selection_precedes_native_edit() {
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let editor = test_editor();
        let original = editor.windows[editor.active_window].active_buffer;
        let mut session = attach_test_workspace(
            WorkspaceHost::open(editor, CapabilityGrants::editor_default()).unwrap(),
        );
        let target = session
            .workspace
            .editor
            .create_buffer("*target*".into(), String::new());
        let mut batch = MicaEventBatch::default();
        batch.push(MicaEvent::Host(MicaHostAction::SelectBuffer {
            buffer: target,
        }));
        batch.push(MicaEvent::Native(MicaNativeAction::InsertText(
            "ordered".into(),
        )));
        let lifecycle = apply(&mut session, batch).await;
        assert!(
            !lifecycle
                .iter()
                .any(|event| matches!(event, LifecycleEvent::Error(_))),
            "{lifecycle:?}"
        );
        assert_eq!(
            session.workspace.editor.buffers[original].content(),
            "hello"
        );
        assert_eq!(
            session.workspace.editor.buffers[target].content(),
            "ordered"
        );
        session.terminate_workspace().await.unwrap();
    });
}

#[test]
fn bridge_tab_requires_native_text_write_grant() {
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let mut session = test_session_with_grants(CapabilityGrants::new([Capability::TextRead]));
        let buffer =
            session.workspace.editor.windows[session.workspace.editor.active_window].active_buffer;
        let mut batch = MicaEventBatch::default();
        batch.push(MicaEvent::Native(MicaNativeAction::Tab));
        let lifecycle = apply(&mut session, batch).await;
        assert_eq!(session.workspace.editor.buffers[buffer].content(), "hello");
        assert!(lifecycle.iter().any(
            |event| matches!(event, LifecycleEvent::Error(message) if message.contains("TextWrite"))
        ));
        session.terminate_workspace().await.unwrap();
    });
}

#[test]
fn mica_bridge_rejects_malformed_and_oversized_policy_without_losing_projection() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let mut session = test_mica_client(test_editor(), CapabilityGrants::editor_default()).unwrap();
        let mut baseline = MicaEventBatch::default();
        baseline.push(MicaEvent::Policy(vec![MicaPolicyFact::Configuration { key: "retained".into(), value: "yes".into() }]));
        apply(&mut session, baseline).await;
        let revision = session.workspace.policy.revision;
        for source in [
            r#"emit(session, {:kind -> :policy_snapshot, :facts -> [{:kind -> :configuration_policy, :key -> :retained}]})"#,
            r#"
                let facts = []
                let index = 0
                while index < 257
                  facts = [@facts, {:kind -> :configuration_policy, :key -> :retained, :value -> index}]
                  index = index + 1
                end
                emit(session, {:kind -> :policy_snapshot, :facts -> facts})
            "#,
            r#"emit(session, {:kind -> :host_action, :action -> :echo})"#,
            r#"emit(session, {:kind -> :native_action, :action -> :insert_text})"#,
            r#"emit(session, {:kind -> :unknown_effect})"#,
        ] {
            let batch = evaluate(&mut session, source).await;
            let lifecycle = apply(&mut session, batch).await;
            assert!(lifecycle.iter().any(|event| matches!(event, LifecycleEvent::Error(_))), "{source}: {lifecycle:?}");
            assert_eq!(session.workspace.policy.revision, revision);
            assert_eq!(session.workspace.policy.configuration["retained"], "yes");
        }
        session.terminate_workspace().await.unwrap();
    });
}

#[test]
fn mica_policy_builder_overflow_retains_the_last_complete_projection() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let mut session =
            test_mica_client(test_editor(), CapabilityGrants::editor_default()).unwrap();
        session.initial_output().await;
        let revision = session.workspace.policy.revision;
        let faces = session.workspace.policy.faces.clone();
        assert!(!faces.is_empty());
        // Start at the admission boundary to exercise the real publisher's failure path.
        let source = include_str!("../../../../mica/roe-first-wave.mica").replacen(
            "let fact_count = 0",
            "let fact_count = 256",
            1,
        );
        session
            .workspace
            .replace_mica_unit("roe/first-wave", source)
            .await
            .unwrap();
        let output = session.initial_output().await;
        assert!(
            output.lifecycle.iter().any(|event| matches!(event,
            LifecycleEvent::Error(message) if message.contains("256"))),
            "{:?}",
            output.lifecycle
        );
        assert_eq!(session.workspace.policy.revision, revision);
        assert_eq!(session.workspace.policy.faces, faces);
        session.terminate_workspace().await.unwrap();
    });
}

#[test]
fn mica_bridge_prompt_close_and_update_retain_emission_order_without_command_kinds() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let mut session = test_mica_client(test_editor(), CapabilityGrants::editor_default()).unwrap();
        let update = r#"emit(session, {:kind -> :prompt_update, :prefix -> "Custom interaction > ", :query -> "value", :selected -> 0, :candidates -> []})"#;
        let close = r#"emit(session, {:kind -> :prompt_close})"#;
        let batch = evaluate(&mut session, &format!("{update}\n{close}")).await;
        assert!(apply(&mut session, batch).await.is_empty());
        assert!(session.workspace.editor.find_command_window().is_none());
        let batch = evaluate(&mut session, &format!("{close}\n{update}")).await;
        assert!(apply(&mut session, batch).await.is_empty());
        let view = session.workspace.editor.find_command_window().unwrap();
        let buffer = session.workspace.editor.windows[view].active_buffer;
        assert_eq!(session.workspace.editor.buffers[buffer].content(), "Custom interaction > value");
        session.terminate_workspace().await.unwrap();
    });
}

#[test]
fn mica_word_selection_uses_effective_syntax_and_keeps_policy_revision() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let mut editor = test_editor();
        let window = editor.active_window;
        let buffer = editor.windows[window].active_buffer;
        editor.buffers[buffer].load_str("foo-bar baz");
        editor.windows[window].cursor = 0;
        let mut session = test_mica_client(editor, CapabilityGrants::editor_default()).unwrap();
        let shift = LogicalKey::Modifier(crate::keys::KeyModifier::Shift(crate::keys::Side::Left));
        let selected = session
            .dispatch(session.envelope(InputEvent::Keys(vec![control(), shift, LogicalKey::Right])))
            .await
            .unwrap();
        assert_eq!(snapshot(&selected).views[0].cursor, 4);
        assert_eq!(session.workspace.editor.buffers[buffer].get_mark(), Some(0));
        let revision = session.workspace.policy.revision;
        let moved = session
            .dispatch(session.envelope(InputEvent::Keys(vec![
                LogicalKey::Modifier(crate::keys::KeyModifier::Meta(crate::keys::Side::Left)),
                LogicalKey::AlphaNumeric('f'),
            ])))
            .await
            .unwrap();
        assert_eq!(snapshot(&moved).views[0].cursor, 8);
        assert_eq!(session.workspace.editor.buffers[buffer].get_mark(), None);
        assert_eq!(session.workspace.policy.revision, revision);
        session.terminate_workspace().await.unwrap();
    });
}

#[test]
fn mica_kill_supplies_its_replacement_and_rejects_missing_identity() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let mut editor = test_editor();
        let window = editor.active_window;
        let buffer = editor.windows[window].active_buffer;
        editor.buffers[buffer].set_display_name("zzz-victim");
        let expected = editor.create_buffer("aaa-replacement".into(), String::new());
        let mut session = test_mica_client(editor, CapabilityGrants::editor_default()).unwrap();
        let batch = evaluate(&mut session, r#"
            let exactly {:buffer -> buffer} = roe/BufferName(?buffer, "zzz-victim")
            emit(session, {:kind -> :host_action, :action -> :kill_buffer_selected, :buffer -> buffer})
        "#).await;
        assert!(apply(&mut session, batch).await.iter().any(|event| matches!(event, LifecycleEvent::Error(_))));
        assert!(session.workspace.editor.buffers.contains_key(buffer));
        let batch = evaluate(&mut session, r#"
            let exactly {:buffer -> buffer} = roe/BufferName(?buffer, "zzz-victim")
            roe/request_kill_buffer(actor, session, buffer)
        "#).await;
        // Mica prefers scratch to the ordinary candidates. SlotMap order does
        // not choose the replacement.
        let replacement = batch.into_events().find_map(|event| match event {
            MicaEvent::Host(MicaHostAction::KillBuffer { buffer: killed, replacement }) if killed == buffer => Some(replacement),
            _ => None,
        }).unwrap();
        assert_ne!(replacement, buffer);
        assert_ne!(replacement, expected);
        assert_eq!(session.workspace.editor.buffers[replacement].kind(), crate::buffer::BufferKind::Scratch);
        let mut batch = MicaEventBatch::default();
        batch.push(MicaEvent::Host(MicaHostAction::KillBuffer { buffer, replacement }));
        apply(&mut session, batch).await;
        assert!(!session.workspace.editor.buffers.contains_key(buffer));
        assert_eq!(session.workspace.editor.windows[window].active_buffer, replacement);
        session.terminate_workspace().await.unwrap();
    });
}
