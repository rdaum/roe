// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-only

use super::*;
use crate::editor::OpenType;
use crate::mica_host::{MicaEvent, MicaHostAction};
use std::time::{Duration, Instant};

async fn open(
    session: &mut DirectSessionClient,
    path: &std::path::Path,
    kind: OpenType,
) -> Vec<LifecycleEvent> {
    let mut events = MicaEventBatch::default();
    events.push(MicaEvent::Host(MicaHostAction::OpenFile {
        path: path.to_string_lossy().into_owned(),
        kind,
    }));
    super::bridge::apply(session, events).await
}

#[test]
fn production_file_open_rejects_directory_without_replacing_the_buffer() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let mut session = test_mica_client(test_editor(), CapabilityGrants::editor_default()).unwrap();
        let window = session.workspace.editor.active_window;
        let original = session.workspace.editor.windows[window].active_buffer;
        for kind in [OpenType::New, OpenType::Visit] {
            let lifecycle = open(&mut session, directory.path(), kind).await;
            assert!(lifecycle.iter().any(|event| matches!(event, LifecycleEvent::Error(message) if message.contains("failed to open"))), "{lifecycle:?}");
            assert_eq!(session.workspace.editor.windows[window].active_buffer, original);
            assert!(session.workspace.editor.buffers.contains_key(original));
        }
        session.terminate_workspace().await.unwrap();
    });
}

#[test]
fn production_file_visit_preserves_shared_buffers_and_cleans_up_watches() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("visited.txt");
        std::fs::write(&path, "visited").unwrap();
        let mut editor = test_editor();
        let original = editor.windows[editor.active_window].active_buffer;
        editor.split_horizontal();
        let target = editor.active_window;
        let other = editor
            .windows
            .keys()
            .find(|window| *window != target)
            .unwrap();
        let mut session = test_mica_client(editor, CapabilityGrants::editor_default()).unwrap();
        assert!(open(&mut session, &path, OpenType::Visit).await.is_empty());
        let visited = session.workspace.editor.windows[target].active_buffer;
        assert_ne!(visited, original);
        assert_eq!(
            session.workspace.editor.windows[other].active_buffer,
            original
        );
        assert!(session.workspace.editor.buffers.contains_key(original));
        assert!(
            session
                .workspace
                .editor
                .file_watcher
                .get_sync_state(visited)
                .is_some()
        );
        std::fs::write(&path, "external").unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while session.workspace.editor.buffers[visited].content() != "external"
            && Instant::now() < deadline
        {
            session.poll_output().await.unwrap();
            compio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            session.workspace.editor.buffers[visited].content(),
            "external"
        );
        session.terminate_workspace().await.unwrap();
        assert!(
            session
                .workspace
                .editor
                .file_watcher
                .get_sync_state(visited)
                .is_none()
        );
    });
}
