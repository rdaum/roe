// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-only

use super::*;
use crate::startup::StartupConfiguration;

#[test]
fn startup_mica_chooses_the_same_two_file_layout_for_both_viewports() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let files: Vec<_> = ["first.txt", "second.txt", "third.txt"]
            .map(|name| directory.path().join(name))
            .into();
        for (index, path) in files.iter().enumerate() {
            std::fs::write(path, format!("file {index}")).unwrap();
        }
        let configuration = StartupConfiguration {
            file_paths: files.clone(),
            recovery: vec![],
        };
        for (columns, rows) in [(80, 23), (120, 40)] {
            let editor = configuration
                .create_editor(Frame::new(columns, rows))
                .await
                .unwrap();
            let workspace =
                WorkspaceHost::open_with_mica(editor, CapabilityGrants::editor_default()).unwrap();
            let mut client = DirectSessionClient::new(
                workspace,
                AttachmentConfiguration::headless(columns, rows),
            );
            let output = client.initial_output().await;
            assert!(
                !output.lifecycle.iter().any(|event| matches!(
                    event,
                    LifecycleEvent::Error(_) | LifecycleEvent::Fatal(_)
                )),
                "{:?}",
                output.lifecycle
            );
            let presentation = snapshot(&output);
            assert_eq!(presentation.views.len(), 2);
            assert_eq!(presentation.views[0].visited_file.as_ref(), Some(&files[0]));
            assert_eq!(presentation.views[1].visited_file.as_ref(), Some(&files[1]));
            assert!(presentation.views[0].active);
            assert!(presentation.views[1].geometry.y > presentation.views[0].geometry.y);
            assert_eq!(client.workspace.editor.startup_buffers.len(), 0);
            assert!(
                client
                    .workspace
                    .editor
                    .buffers
                    .values()
                    .any(|buffer| buffer.visited_file().as_ref() == Some(&files[2]))
            );
            assert_eq!(snapshot(&client.initial_output().await).views.len(), 2);
            client.terminate_workspace().await.unwrap();
        }
    });
}

#[test]
fn startup_layout_changes_through_mica_replacement_without_a_rust_fallback() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let configuration = StartupConfiguration {
            file_paths: vec![
                directory.path().join("new-first.txt"),
                directory.path().join("new-second.txt"),
            ],
            recovery: vec![],
        };
        let editor = configuration
            .create_editor(Frame::new(80, 23))
            .await
            .unwrap();
        let mut workspace =
            WorkspaceHost::open_with_mica(editor, CapabilityGrants::editor_default()).unwrap();
        let policy = include_str!("../../../../mica/roe-first-wave.mica").replacen(
            "if count > 1",
            "if count > 64",
            1,
        );
        workspace.replace_mica_first_wave(policy).await.unwrap();
        let mut client =
            DirectSessionClient::new(workspace, AttachmentConfiguration::headless(80, 23));
        let output = client.initial_output().await;
        assert!(
            !output
                .lifecycle
                .iter()
                .any(|event| matches!(event, LifecycleEvent::Error(_))),
            "{:?}",
            output.lifecycle
        );
        assert_eq!(snapshot(&output).views.len(), 1);
        assert_eq!(
            snapshot(&output).views[0].visited_file.as_ref(),
            Some(&configuration.file_paths[0])
        );
        client.terminate_workspace().await.unwrap();
    });
}
