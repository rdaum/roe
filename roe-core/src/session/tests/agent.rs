// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-only

use super::*;

#[test]
fn production_mica_workspace_always_has_a_distinguished_scratch_buffer() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let mut session =
            test_mica_client(test_editor(), CapabilityGrants::editor_default()).unwrap();
        let scratch: Vec<_> = session
            .workspace
            .editor
            .buffers
            .iter()
            .filter(|(_, buffer)| buffer.kind() == crate::buffer::BufferKind::Scratch)
            .collect();
        assert_eq!(scratch.len(), 1);
        assert_eq!(scratch[0].1.display_name(), "*scratch*");
        assert_eq!(scratch[0].1.visited_file(), None);
        session.terminate_workspace().await.unwrap();
    });
}

async fn start_test_agent(session: &mut DirectSessionClient) -> SessionOutput {
    let meta = LogicalKey::Modifier(crate::keys::KeyModifier::Meta(crate::keys::Side::Left));
    for event in [
        InputEvent::Keys(vec![meta, LogicalKey::AlphaNumeric('x')]),
        InputEvent::Text("agent-chat".to_owned()),
        InputEvent::Keys(vec![LogicalKey::Enter]),
        InputEvent::Text("Inspect this workspace.".to_owned()),
    ] {
        let output = session.dispatch(session.envelope(event)).await.unwrap();
        assert!(
            !output
                .lifecycle
                .iter()
                .any(|event| matches!(event, LifecycleEvent::Error(_))),
            "{output:#?}"
        );
    }
    session
        .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Enter])))
        .await
        .unwrap()
}

async fn await_agent_text(
    session: &mut DirectSessionClient,
    expected: &str,
) -> PresentationSnapshot {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        if let Some(output) = session.poll_output().await.unwrap() {
            assert!(
                !output
                    .lifecycle
                    .iter()
                    .any(|event| matches!(event, LifecycleEvent::Error(_))),
                "{output:#?}"
            );
            if output.presentation.is_some()
                && session.workspace.editor.buffers.values().any(|buffer| {
                    buffer.display_name() == "*Agent*" && buffer.content().contains(expected)
                })
            {
                return snapshot(&output).clone();
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "agent never presented {expected:?}"
        );
        compio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

#[test]
fn agent_tools_use_composed_live_sources_and_bound_globs() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let mut editor = test_editor();
        let original = editor.windows[editor.active_window].active_buffer;
        let root = std::env::current_dir().unwrap().canonicalize().unwrap();
        editor.buffers[original].set_visited_file(Some(root.join("Cargo.toml")));
        editor.buffers[original].insert_pos("UNSAVED_ONLY_AGENT_MARKER\n".to_owned(), 0);
        assert!(editor.buffers[original].is_modified());
        let virtual_file = Buffer::visiting(root.join("agent-virtual/note.txt"));
        virtual_file.insert_pos("virtual file text".to_owned(), 0);
        editor.buffers.insert(virtual_file);
        let mut session = test_mica_client(editor, CapabilityGrants::editor_default()).unwrap();
        let source = r#"
            let read = roe/agent_invoke_tool("read", json_decode("{\"path\":\"Cargo.toml\"}"))
            let grep = roe/agent_invoke_tool("grep", json_decode("{\"pattern\":\"UNSAVED_ONLY_AGENT_MARKER\",\"limit\":1}"))
            let glob = roe/agent_invoke_tool("glob", {:pattern -> "**/*.txt", :path -> "agent-virtual"})
            let listing = roe/agent_invoke_tool("ls", {:path -> "agent-virtual"})
            read[:status] == "complete" || raise E_TEST, "read failed"
            grep[:status] == "complete" || raise E_TEST, "grep failed"
            glob[:content] == "agent-virtual/note.txt" || raise E_TEST, "glob missed virtual file"
            listing[:content] == "file  agent-virtual/note.txt" || raise E_TEST, "ls missed virtual file"
            roe/agent_glob_match("**/*.txt", "root.txt") || raise E_TEST, "globstar missed root"
            roe/agent_glob_match("**/x/*.txt", "a/x/b/x/c.txt") || raise E_TEST, "glob backtracking failed"
            not roe/agent_glob_match("?.txt", "/.txt") || raise E_TEST, "question mark crossed slash"
            not roe/agent_glob_match("*.txt", "dir/root.txt") || raise E_TEST, "star crossed slash"
            roe/agent_invoke_tool("write", {})[:status] == "error" || raise E_TEST, "write allowed"
            return [read[:content], grep[:content]]
        "#;
        let WorkspaceHost { editor, buffer_resources, mica, .. } = &mut session.workspace;
        let result = mica.as_mut().unwrap().evaluate_source(editor, buffer_resources, source.to_owned()).await.unwrap();
        assert_eq!(result.value.matches("UNSAVED_ONLY_AGENT_MARKER").count(), 2, "{result:#?}");
        assert!(editor.buffers[original].is_modified());
        session.terminate_workspace().await.unwrap();
    });
}

#[test]
fn agent_presents_partial_streams_without_stealing_focus_and_cancels_on_close() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        use mica_driver::{Symbol, Value};
        struct StreamDrop(Arc<AtomicBool>);
        impl Drop for StreamDrop {
            fn drop(&mut self) {
                self.0.store(true, AtomicOrdering::SeqCst);
            }
        }
        let release = Arc::new(AtomicBool::new(false));
        let dropped = Arc::new(AtomicBool::new(false));
        let handler: mica_driver::ExternalStreamRequestHandler = {
            let release = Arc::clone(&release);
            let dropped = Arc::clone(&dropped);
            Arc::new(move |_, _, emitter| {
                let release = Arc::clone(&release);
                let dropped = Arc::clone(&dropped);
                Box::pin(async move {
                    let _guard = StreamDrop(dropped);
                    emitter
                        .emit(Value::map([
                            (
                                Value::symbol(Symbol::intern("type")),
                                Value::symbol(Symbol::intern("text_delta")),
                            ),
                            (
                                Value::symbol(Symbol::intern("delta")),
                                Value::string("First chunk"),
                            ),
                        ]))
                        .await
                        .unwrap();
                    while !release.load(AtomicOrdering::SeqCst) {
                        compio::time::sleep(std::time::Duration::from_millis(10)).await;
                    }
                    emitter
                        .emit(Value::map([
                            (
                                Value::symbol(Symbol::intern("type")),
                                Value::symbol(Symbol::intern("text_delta")),
                            ),
                            (
                                Value::symbol(Symbol::intern("delta")),
                                Value::string(" and second chunk"),
                            ),
                        ]))
                        .await
                        .unwrap();
                    std::future::pending::<Value>().await
                })
            })
        };
        let mut session = test_mica_client_with_stream_handler(
            test_editor(),
            CapabilityGrants::editor_default(),
            handler,
        )
        .unwrap();
        start_test_agent(&mut session).await;
        let first = await_agent_text(&mut session, "First chunk").await;
        let agent = first
            .views
            .iter()
            .find(|view| view.name == "*Agent*")
            .unwrap();
        assert!(agent.visible_text.contains("First chunk"));
        assert!(agent.read_only);
        assert!(agent.modeline.contains("(agent)"), "{agent:#?}");
        assert!(!release.load(AtomicOrdering::SeqCst));
        assert!(!dropped.load(AtomicOrdering::SeqCst));

        // Leave the stream running while the user switches back to editing.
        for event in [
            InputEvent::Keys(vec![control(), LogicalKey::AlphaNumeric('x')]),
            InputEvent::Keys(vec![LogicalKey::AlphaNumeric('b')]),
            InputEvent::Text("*test*".to_owned()),
            InputEvent::Keys(vec![LogicalKey::Enter]),
        ] {
            session.dispatch(session.envelope(event)).await.unwrap();
        }
        let window = session.workspace.editor.active_window;
        let buffer = session.workspace.editor.windows[window].active_buffer;
        assert_eq!(
            session.workspace.editor.buffers[buffer].display_name(),
            "*test*"
        );
        let cursor = session.workspace.editor.windows[window].cursor;
        release.store(true, AtomicOrdering::SeqCst);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            if let Some(output) = session.poll_output().await.unwrap() {
                assert!(
                    !output
                        .lifecycle
                        .iter()
                        .any(|event| matches!(event, LifecycleEvent::Error(_))),
                    "{output:#?}"
                );
            }
            if session.workspace.editor.buffers.values().any(|buffer| {
                buffer.display_name() == "*Agent*" && buffer.content().contains("second chunk")
            }) {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            compio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(session.workspace.editor.active_window, window);
        assert_eq!(
            session.workspace.editor.windows[window].active_buffer,
            buffer
        );
        assert_eq!(session.workspace.editor.windows[window].cursor, cursor);
        session.terminate_workspace().await.unwrap();
        assert!(
            dropped.load(AtomicOrdering::SeqCst),
            "endpoint close did not cancel its HTTP future"
        );
    });
}

#[test]
fn agent_recovers_after_an_oversized_tool_response() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        use mica_driver::{Symbol, Value};
        let count = Arc::new(AtomicUsize::new(0));
        let handler: mica_driver::ExternalStreamRequestHandler = {
            let count = Arc::clone(&count);
            Arc::new(move |_, request, emitter| {
                let turn = count.fetch_add(1, AtomicOrdering::SeqCst);
                Box::pin(async move {
                    if turn == 0 {
                        for index in 0..17 {
                            emitter
                                .emit(Value::map([
                                    (
                                        Value::symbol(Symbol::intern("type")),
                                        Value::symbol(Symbol::intern("tool_call_ready")),
                                    ),
                                    (
                                        Value::symbol(Symbol::intern("call_id")),
                                        Value::string(format!("call-{index}")),
                                    ),
                                    (Value::symbol(Symbol::intern("name")), Value::string("read")),
                                    (
                                        Value::symbol(Symbol::intern("arguments")),
                                        Value::string("{}"),
                                    ),
                                ]))
                                .await
                                .unwrap();
                        }
                    } else {
                        let input = request
                            .payload
                            .map_get(&Value::symbol(Symbol::intern("input")))
                            .unwrap();
                        assert!(
                            input
                                .with_list(|items| items.iter().all(|item| {
                                    item.map_get(&Value::symbol(Symbol::intern("type")))
                                        != Some(Value::string("function_call"))
                                }))
                                .unwrap(),
                            "failed response left orphaned tool calls"
                        );
                        emitter
                            .emit(Value::map([
                                (
                                    Value::symbol(Symbol::intern("type")),
                                    Value::symbol(Symbol::intern("text_delta")),
                                ),
                                (
                                    Value::symbol(Symbol::intern("delta")),
                                    Value::string("Recovered successfully"),
                                ),
                            ]))
                            .await
                            .unwrap();
                    }
                    let _ = emitter
                        .emit(Value::map([(
                            Value::symbol(Symbol::intern("type")),
                            Value::symbol(Symbol::intern("completed")),
                        )]))
                        .await;
                    Value::bool(true)
                })
            })
        };
        let mut session = test_mica_client_with_stream_handler(
            test_editor(),
            CapabilityGrants::editor_default(),
            handler,
        )
        .unwrap();
        start_test_agent(&mut session).await;
        await_agent_text(&mut session, "exceeds 16 tool calls").await;
        start_test_agent(&mut session).await;
        await_agent_text(&mut session, "Recovered successfully").await;
        assert_eq!(count.load(AtomicOrdering::SeqCst), 2);
        session.terminate_workspace().await.unwrap();
    });
}

#[test]
fn agent_chat_streams_into_a_special_buffer_and_reads_unsaved_roe_text() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let request_count = Arc::new(AtomicUsize::new(0));
        let saw_unsaved_text = Arc::new(AtomicBool::new(false));
        let stream_handler: mica_driver::ExternalStreamRequestHandler = {
            let request_count = Arc::clone(&request_count);
            let saw_unsaved_text = Arc::clone(&saw_unsaved_text);
            Arc::new(move |_, request, emitter| {
                let turn = request_count.fetch_add(1, AtomicOrdering::SeqCst);
                let saw_unsaved_text = Arc::clone(&saw_unsaved_text);
                Box::pin(async move {
                    let input = request
                        .payload
                        .map_get(&mica_driver::Value::symbol(mica_driver::Symbol::intern(
                            "input",
                        )))
                        .and_then(|value| value.with_list(<[mica_driver::Value]>::to_vec))
                        .unwrap_or_default();
                    if turn == 0 {
                        emitter
                            .emit(mica_driver::Value::map([
                                (
                                    mica_driver::Value::symbol(mica_driver::Symbol::intern(
                                        "type",
                                    )),
                                    mica_driver::Value::symbol(mica_driver::Symbol::intern(
                                        "tool_call_ready",
                                    )),
                                ),
                                (
                                    mica_driver::Value::symbol(mica_driver::Symbol::intern(
                                        "call_id",
                                    )),
                                    mica_driver::Value::string("read_unsaved"),
                                ),
                                (
                                    mica_driver::Value::symbol(mica_driver::Symbol::intern(
                                        "name",
                                    )),
                                    mica_driver::Value::string("read"),
                                ),
                                (
                                    mica_driver::Value::symbol(mica_driver::Symbol::intern(
                                        "arguments",
                                    )),
                                    mica_driver::Value::string("{\"path\":\"Cargo.toml\"}"),
                                ),
                            ]))
                            .await
                            .unwrap();
                        emitter
                            .emit(mica_driver::Value::map([(
                                mica_driver::Value::symbol(mica_driver::Symbol::intern("type")),
                                mica_driver::Value::symbol(mica_driver::Symbol::intern(
                                    "completed",
                                )),
                            )]))
                            .await
                            .unwrap();
                    } else {
                        let output = input
                            .iter()
                            .find_map(|value| {
                                (value.map_get(&mica_driver::Value::symbol(
                                    mica_driver::Symbol::intern("type"),
                                )) == Some(mica_driver::Value::string("function_call_output")))
                                .then(|| {
                                    value.map_get(&mica_driver::Value::symbol(
                                        mica_driver::Symbol::intern("output"),
                                    ))
                                })
                                .flatten()
                            })
                            .and_then(|value| value.with_str(str::to_owned))
                            .unwrap_or_default();
                        let found = output.contains("UNSAVED_ONLY_AGENT_MARKER");
                        saw_unsaved_text.store(found, AtomicOrdering::SeqCst);
                        let reply = if found {
                            "I read the unsaved Roe buffer."
                        } else {
                            "The unsaved Roe buffer was not visible."
                        };
                        emitter
                            .emit(mica_driver::Value::map([
                                (
                                    mica_driver::Value::symbol(mica_driver::Symbol::intern(
                                        "type",
                                    )),
                                    mica_driver::Value::symbol(mica_driver::Symbol::intern(
                                        "text_delta",
                                    )),
                                ),
                                (
                                    mica_driver::Value::symbol(mica_driver::Symbol::intern(
                                        "delta",
                                    )),
                                    mica_driver::Value::string(reply),
                                ),
                            ]))
                            .await
                            .unwrap();
                        emitter
                            .emit(mica_driver::Value::map([(
                                mica_driver::Value::symbol(mica_driver::Symbol::intern("type")),
                                mica_driver::Value::symbol(mica_driver::Symbol::intern(
                                    "completed",
                                )),
                            )]))
                            .await
                            .unwrap();
                    }
                    mica_driver::Value::map([(
                        mica_driver::Value::symbol(mica_driver::Symbol::intern("started")),
                        mica_driver::Value::bool(true),
                    )])
                }) as mica_driver::ExternalRequestFuture
            })
        };

        let editor = test_editor();
        let buffer = editor.windows[editor.active_window].active_buffer;
        editor.buffers[buffer].load_str("UNSAVED_ONLY_AGENT_MARKER\n");
        editor.buffers[buffer].set_visited_file(Some(
            std::env::current_dir()
                .unwrap()
                .join("Cargo.toml")
                .canonicalize()
                .unwrap(),
        ));
        let mut session = test_mica_client_with_stream_handler(
            editor,
            CapabilityGrants::editor_default(),
            stream_handler,
        )
        .unwrap();
        let meta =
            LogicalKey::Modifier(crate::keys::KeyModifier::Meta(crate::keys::Side::Left));

        session
            .dispatch(
                session.envelope(InputEvent::Keys(vec![meta, LogicalKey::AlphaNumeric('x')])),
            )
            .await
            .unwrap();
        session
            .dispatch(session.envelope(InputEvent::Text("agent-chat".to_owned())))
            .await
            .unwrap();
        let argument_prompt = session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Enter])))
            .await
            .unwrap();
        assert!(
            snapshot(&argument_prompt)
                .views
                .iter()
                .any(|view| view.command_view && view.visible_text.starts_with("Ask agent")),
            "agent-chat did not open its free-form prompt: {argument_prompt:#?}"
        );
        session
            .dispatch(session.envelope(InputEvent::Text(
                "Read Cargo.toml before answering.".to_owned(),
            )))
            .await
            .unwrap();
        let started = session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Enter])))
            .await
            .unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut final_typeout = None;
        let mut diagnostics = vec![format!("started: {started:#?}")];
        loop {
            let output = session.poll_output().await.unwrap();
            if let Some(output) = output {
                if !output.lifecycle.is_empty() || !snapshot(&output).echo_area.is_empty() {
                    diagnostics.push(format!("poll: {output:#?}"));
                }
                final_typeout = snapshot(&output)
                    .views
                    .iter()
                    .find(|view| view.name == "*Agent*")
                    .and_then(|view| view.typeout.clone())
                    .or(final_typeout);
            }
            let agent_text = session
                .workspace
                .editor
                .buffers
                .values()
                .find(|buffer| buffer.display_name() == "*Agent*")
                .map(Buffer::content)
                .unwrap_or_default();
            if agent_text.contains("I read the unsaved Roe buffer.") {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "agent response did not finish; requests={}; text={agent_text:?}; diagnostics={diagnostics:#?}",
                request_count.load(AtomicOrdering::SeqCst),
            );
            compio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        assert!(saw_unsaved_text.load(AtomicOrdering::SeqCst));
        assert_eq!(request_count.load(AtomicOrdering::SeqCst), 2);
        let typeout = final_typeout.expect("agent read should surface tool activity");
        assert_eq!(typeout.kind, "agent_tool");
        assert_eq!(typeout.title, "Agent tool: read");
        assert!(typeout.visible_text.contains("read Cargo.toml"));
        assert!(typeout.visible_text.contains("complete"));

        session.terminate_workspace().await.unwrap();
    });
}
