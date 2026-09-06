// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-only

use super::*;
use crate::frontend::{
    FrontendOutputError, FrontendServices, PresentationConsumer, consume_output,
};
use crate::renderer::{PresentationStreamError, PresentationStreamState};

#[derive(Default)]
struct Consumer {
    state: PresentationStreamState,
    full_snapshots: usize,
}

impl PresentationConsumer for Consumer {
    fn accept_presentation(
        &mut self,
        update: &PresentationUpdate,
    ) -> Result<(), PresentationStreamError> {
        self.state.apply(update)?;
        self.full_snapshots += usize::from(matches!(update, PresentationUpdate::Full(_)));
        Ok(())
    }
}

#[derive(Default)]
struct Clipboard {
    text: String,
    calls: usize,
}

impl FrontendServices for Clipboard {
    fn handle(&mut self, request: FrontendServiceRequest) -> FrontendServiceResult {
        self.calls += 1;
        let request_id = request.request_id();
        let result = match request {
            FrontendServiceRequest::ReadClipboard { .. } => {
                FrontendServiceResponse::ClipboardContents(Some(self.text.clone()))
            }
            FrontendServiceRequest::WriteClipboard { contents, .. } => {
                self.text = contents;
                FrontendServiceResponse::Completed
            }
            FrontendServiceRequest::Notify { .. } => FrontendServiceResponse::Completed,
        };
        FrontendServiceResult {
            request_id,
            result: Ok(result),
        }
    }
}

#[test]
fn frontend_recovers_a_missing_delta_through_the_same_session() {
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let mut session = test_session();
        let mut clipboard = Clipboard::default();
        let mut consumer = Consumer::default();
        let output = session.initial_output().await;
        consume_output(&mut session, &mut clipboard, &mut consumer, output)
            .await
            .unwrap();
        session
            .dispatch(session.envelope(InputEvent::Text("x".into())))
            .await
            .unwrap();
        let output = session
            .dispatch(session.envelope(InputEvent::Text("y".into())))
            .await
            .unwrap();
        let before = session.next_sequence();
        assert!(
            !consume_output(&mut session, &mut clipboard, &mut consumer, output)
                .await
                .unwrap()
        );
        assert_eq!(session.next_sequence(), before + 1);
        assert_eq!(consumer.full_snapshots, 2);
        assert_eq!(
            consumer.state.current().unwrap().views[0].visible_text,
            "helloxy"
        );
        assert_eq!(clipboard.calls, 0);
        let output = session.terminate_workspace().await.unwrap();
        assert!(
            consume_output(&mut session, &mut clipboard, &mut consumer, output)
                .await
                .unwrap()
        );
    });
}

#[test]
fn frontend_drains_correlated_services_without_workspace_clipboard_access() {
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let workspace =
            WorkspaceHost::open(test_editor(), CapabilityGrants::editor_default()).unwrap();
        let mut session =
            DirectSessionClient::new(workspace, AttachmentConfiguration::local_frontend(80, 24));
        let mut clipboard = Clipboard {
            text: " paste".into(),
            calls: 0,
        };
        let mut consumer = Consumer::default();
        let output = session.initial_output().await;
        consume_output(&mut session, &mut clipboard, &mut consumer, output)
            .await
            .unwrap();
        session
            .attachment
            .enqueue_frontend_request(PendingFrontendRequest::ReadClipboardForYank, |request_id| {
                FrontendServiceRequest::ReadClipboard { request_id }
            })
            .unwrap();
        let output = session.poll_output().await.unwrap().unwrap();
        consume_output(&mut session, &mut clipboard, &mut consumer, output)
            .await
            .unwrap();
        assert_eq!(clipboard.calls, 1);
        assert!(session.attachment.pending_frontend_requests.is_empty());
        assert_eq!(
            consumer.state.current().unwrap().views[0].visible_text,
            "hello paste"
        );
        session.terminate_workspace().await.unwrap();
    });
}

#[test]
fn frontend_rejects_oversized_service_batches_before_any_service_call() {
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let mut session = test_session();
        let mut clipboard = Clipboard::default();
        let mut consumer = Consumer::default();
        let mut output = session.initial_output().await;
        output.frontend_requests = (0..=MAX_FRONTEND_REQUESTS)
            .map(|id| FrontendServiceRequest::ReadClipboard {
                request_id: FrontendRequestId(id as u64),
            })
            .collect();
        assert!(matches!(
            consume_output(&mut session, &mut clipboard, &mut consumer, output).await,
            Err(FrontendOutputError::Overloaded)
        ));
        assert_eq!(clipboard.calls, 0);
        session.terminate_workspace().await.unwrap();
    });
}
