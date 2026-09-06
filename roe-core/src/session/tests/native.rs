// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-only

use super::*;

#[test]
fn native_completion_payloads_are_bounded() {
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "roe-session-output-{}-{unique}",
            std::process::id()
        ));
        std::fs::write(&path, vec![b'x'; MAX_NATIVE_RESULT_BYTES + 1]).unwrap();
        let mut session = test_session_with_grants(CapabilityGrants::new([Capability::FileRead]));
        let output = session
            .dispatch(session.envelope(InputEvent::NativeRequest {
                request_id: RequestId(23),
                operation: NativeOperation::ReadFile { path: path.clone() },
            }))
            .await
            .unwrap();
        assert!(output.native_completions[0].result.is_err());
        assert!(
            output
                .lifecycle
                .iter()
                .any(|event| matches!(event, LifecycleEvent::Overloaded { .. }))
        );
        std::fs::remove_file(path).unwrap();
    });
}

#[test]
fn native_watch_changes_surface_through_session_lifecycle() {
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("roe-session-watch-{}-{unique}", std::process::id()));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("watched.txt");
        std::fs::write(&path, "before").unwrap();

        let mut session = test_session();
        let initial = session.initial_output().await;
        let resource = snapshot(&initial).views[0].resource;
        let registered = session
            .dispatch(session.envelope(InputEvent::NativeRequest {
                request_id: RequestId(31),
                operation: NativeOperation::RegisterWatch {
                    resource,
                    path: path.clone(),
                },
            }))
            .await
            .unwrap();
        assert!(matches!(
            registered.native_completions[0].result,
            Ok(NativeResult::WatchRegistered)
        ));

        std::fs::write(&path, "after").unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            let output = session.poll_output().await.unwrap();
            let Some(output) = output else {
                continue;
            };
            if output.lifecycle.iter().any(|event| {
                matches!(
                    event,
                    LifecycleEvent::ResourceChanged {
                        resource: changed,
                        ..
                    } if *changed == resource
                )
            }) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "session did not surface native watch notification"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        session.terminate_workspace().await.unwrap();
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(directory).unwrap();
    });
}

#[test]
fn host_reports_cleanup_warning_only_after_resource_is_revoked() {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "roe-session-revoke-{}-{unique}",
        std::process::id()
    ));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("watched.txt");
    std::fs::write(&path, "content").unwrap();

    let mut session = test_session();
    let (buffer, resource) = session
        .workspace
        .buffer_resources
        .iter()
        .next()
        .map(|(buffer, resource)| (*buffer, *resource))
        .unwrap();
    session
        .workspace
        .kernel
        .lock()
        .unwrap()
        .execute(NativeOperation::RegisterWatch {
            resource,
            path: path.clone(),
        })
        .unwrap();
    session
        .workspace
        .kernel
        .lock()
        .unwrap()
        .force_backend_unwatch_for_test(&path)
        .unwrap();
    session.workspace.editor.buffers.remove(buffer);

    let (invalidated, warnings) = session.workspace.synchronize_identities();
    assert_eq!(invalidated, vec![resource]);
    assert!(
        warnings
            .iter()
            .any(|warning| warning.contains("cleanup failed"))
    );
    assert!(matches!(
        session.workspace.kernel.lock().unwrap().snapshot(resource),
        Err(KernelError::StaleResource(id)) if id == resource
    ));

    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir(directory).unwrap();
}
