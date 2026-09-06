// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-only

use super::*;

#[test]
fn ordered_input_produces_monotonic_revisions() {
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let mut session = test_session();
        let initial = session.initial_output().await;
        let first_revision = snapshot(&initial).revision;
        let envelope = session.envelope(InputEvent::Text("!".to_string()));
        let output = session.dispatch(envelope).await.unwrap();
        assert!(snapshot(&output).revision.0 > first_revision.0);
        assert_eq!(snapshot(&output).views[0].visible_text, "hello!");
    });
}

#[test]
fn attachment_lifecycle_preserves_workspace_and_resets_transport_state() {
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let mut session = test_session();
        let first_attachment = session.attachment_id();
        let first_epoch = session.epoch();
        session.initial_output().await;
        let edited = session
            .dispatch(session.envelope(InputEvent::Text("!".to_owned())))
            .await
            .unwrap();
        assert_eq!(edited.acknowledged_input, Some(0));
        assert_eq!(snapshot(&edited).views[0].visible_text, "hello!");

        let detached = session.detach().await.unwrap();
        assert!(
            detached
                .lifecycle
                .contains(&LifecycleEvent::AttachmentDetached {
                    attachment: first_attachment,
                })
        );
        assert!(matches!(
            session.poll_output().await,
            Err(SessionError::AttachmentUnavailable)
        ));

        let resumed = session
            .resume(AttachmentConfiguration::headless(100, 40))
            .await
            .unwrap();
        assert_eq!(session.attachment_id(), first_attachment);
        assert_ne!(session.epoch(), first_epoch);
        assert_eq!(session.next_sequence(), 0);
        assert_eq!(resumed.acknowledged_input, None);
        assert!(matches!(
            resumed.presentation,
            Some(PresentationUpdate::Full(_))
        ));
        assert_eq!(snapshot(&resumed).views[0].visible_text, "hello!");

        let closed = session.close_attachment().await.unwrap();
        assert!(
            closed
                .lifecycle
                .contains(&LifecycleEvent::AttachmentClosed {
                    attachment: first_attachment,
                })
        );
        let workspace = session.into_workspace().unwrap();
        let mut replacement =
            DirectSessionClient::new(workspace, AttachmentConfiguration::headless(80, 24));
        assert_ne!(replacement.attachment_id(), first_attachment);
        let replacement_initial = replacement.initial_output().await;
        assert_eq!(
            snapshot(&replacement_initial).views[0].visible_text,
            "hello!"
        );
        replacement.terminate_workspace().await.unwrap();
    });
}

#[test]
fn workspace_and_attachment_can_be_driven_without_the_direct_client() {
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let mut workspace =
            WorkspaceHost::open(test_editor(), CapabilityGrants::editor_default()).unwrap();
        let mut attachment = workspace.attach(AttachmentConfiguration::headless(80, 24));
        let initial = workspace.initial_output(&mut attachment).await;
        assert_eq!(snapshot(&initial).views[0].visible_text, "hello");
        let envelope = InputEnvelope {
            protocol_version: SESSION_PROTOCOL_VERSION,
            epoch: attachment.epoch(),
            sequence: 0,
            event: InputEvent::Text("!".to_owned()),
        };
        let edited = workspace.dispatch(&mut attachment, envelope).await.unwrap();
        assert_eq!(edited.acknowledged_input, Some(0));
        assert_eq!(snapshot(&edited).views[0].visible_text, "hello!");
        workspace
            .terminate_workspace(&mut attachment)
            .await
            .unwrap();
    });
}

#[test]
fn duplicate_and_gapped_sequences_are_rejected_without_mutation() {
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let mut session = test_session();
        let mut envelope = session.envelope(InputEvent::Heartbeat);
        envelope.sequence += 1;
        assert!(matches!(
            session.dispatch(envelope).await,
            Err(SessionError::Sequence { .. })
        ));
        assert_eq!(session.next_sequence(), 0);
    });
}

#[test]
fn full_resync_snapshot_is_self_contained_and_stable() {
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let mut session = test_session();
        let first = session
            .dispatch(session.envelope(InputEvent::RequestSnapshot { after: None }))
            .await
            .unwrap();
        let first_snapshot = snapshot(&first).clone();
        let second = session
            .dispatch(session.envelope(InputEvent::RequestSnapshot {
                after: Some(first_snapshot.revision),
            }))
            .await
            .unwrap();
        assert_eq!(snapshot(&second).views, first_snapshot.views);
        assert!(snapshot(&second).revision.0 > first_snapshot.revision.0);
    });
}

#[test]
fn native_capability_denial_is_a_typed_completion_not_endpoint_failure() {
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let mut session = test_session_with_grants(CapabilityGrants::new([]));
        let output = session
            .dispatch(session.envelope(InputEvent::NativeRequest {
                request_id: RequestId(7),
                operation: NativeOperation::ReadClockMillis,
            }))
            .await
            .unwrap();
        assert_eq!(output.native_completions[0].request_id, RequestId(7));
        assert!(
            output.native_completions[0]
                .result
                .as_ref()
                .unwrap_err()
                .contains("was not granted")
        );
        assert!(output.lifecycle.is_empty());
    });
}

#[test]
fn workspace_termination_is_idempotently_terminal() {
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let mut session = test_session();
        let close = session.terminate_workspace().await.unwrap();
        assert!(close.presentation.is_none());
        assert!(
            close
                .lifecycle
                .contains(&LifecycleEvent::WorkspaceTerminated)
        );
        assert!(
            close
                .lifecycle
                .iter()
                .any(|event| matches!(event, LifecycleEvent::ResourceInvalidated { .. }))
        );
        assert!(matches!(
            session
                .dispatch(session.envelope(InputEvent::Heartbeat))
                .await,
            Err(SessionError::WorkspaceTerminated)
        ));
        let repeated = session.terminate_workspace().await.unwrap();
        assert!(
            repeated
                .lifecycle
                .contains(&LifecycleEvent::WorkspaceTerminated)
        );
    });
}

#[test]
fn overload_and_cancellation_are_explicit_lifecycle_results() {
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let mut session = test_session();
        let oversized = session
            .dispatch(session.envelope(InputEvent::Keys(vec![
                LogicalKey::AlphaNumeric('x');
                MAX_KEYS_PER_INPUT + 1
            ])))
            .await
            .unwrap();
        assert!(matches!(
            oversized.lifecycle.as_slice(),
            [LifecycleEvent::Overloaded { .. }]
        ));
        assert_eq!(session.next_sequence(), 1);

        let cancel = session
            .dispatch(session.envelope(InputEvent::Cancel {
                request_id: RequestId(19),
            }))
            .await
            .unwrap();
        assert!(
            cancel
                .lifecycle
                .contains(&LifecycleEvent::RequestCancelled {
                    request_id: RequestId(19),
                    was_pending: false,
                })
        );
    });
}

#[test]
fn idle_heartbeat_does_not_advance_presentation_revision() {
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let mut session = test_session();
        let initial = session.initial_output().await;
        let revision = snapshot(&initial).revision;
        let heartbeat = session
            .dispatch(session.envelope(InputEvent::Heartbeat))
            .await
            .unwrap();
        assert!(heartbeat.presentation.is_none());
        let resync = session
            .dispatch(session.envelope(InputEvent::RequestSnapshot {
                after: Some(revision),
            }))
            .await
            .unwrap();
        assert_eq!(snapshot(&resync).revision.0, revision.0 + 1);
    });
}

#[test]
fn headless_transcript_replays_the_same_ordered_presentation_path() {
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let mut session = test_session();
        let transcript = SessionTranscript {
            events: vec![
                InputEvent::Text("!".to_string()),
                InputEvent::Text("?".to_string()),
                InputEvent::RequestSnapshot { after: None },
            ],
        };
        let outputs = session.replay(&transcript).await.unwrap();
        assert_eq!(snapshot(&outputs[0]).views[0].visible_text, "hello!");
        assert_eq!(snapshot(&outputs[1]).views[0].visible_text, "hello!?");
        assert_eq!(snapshot(&outputs[2]).views[0].visible_text, "hello!?");
        assert!(
            outputs
                .windows(2)
                .all(|pair| snapshot(&pair[0]).revision.0 < snapshot(&pair[1]).revision.0)
        );
    });
}
