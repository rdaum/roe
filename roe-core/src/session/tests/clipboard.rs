// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-only

use super::*;

#[test]
fn frontend_clipboard_requests_are_correlated_and_attachment_local() {
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let workspace =
            WorkspaceHost::open(test_editor(), CapabilityGrants::editor_default()).unwrap();
        let mut session =
            DirectSessionClient::new(workspace, AttachmentConfiguration::local_frontend(80, 24));
        session.workspace.editor.kill_ring.kill("copied".to_owned());
        let mut lifecycle = Vec::new();
        {
            let DirectSessionClient {
                workspace,
                attachment,
            } = &mut session;
            workspace.write_kill_ring_to_frontend(attachment, "copy_region", &mut lifecycle);
        }
        let write = session.poll_output().await.unwrap().unwrap();
        let FrontendServiceRequest::WriteClipboard {
            request_id,
            contents,
        } = &write.frontend_requests[0]
        else {
            panic!("expected a clipboard write request");
        };
        assert_eq!(contents, "copied");
        let write_complete = session
            .complete_frontend_request(FrontendServiceResult {
                request_id: *request_id,
                result: Ok(FrontendServiceResponse::Completed),
            })
            .await
            .unwrap();
        assert_eq!(write_complete.acknowledged_input, None);

        let read_id;
        {
            let attachment = &mut session.attachment;
            attachment
                .enqueue_frontend_request(
                    PendingFrontendRequest::ReadClipboardForYank,
                    |request_id| FrontendServiceRequest::ReadClipboard { request_id },
                )
                .unwrap();
            read_id = attachment.frontend_requests.front().unwrap().request_id();
        }
        let read = session.poll_output().await.unwrap().unwrap();
        assert!(matches!(
            read.frontend_requests.as_slice(),
            [FrontendServiceRequest::ReadClipboard { request_id }] if *request_id == read_id
        ));
        let yank = session
            .complete_frontend_request(FrontendServiceResult {
                request_id: read_id,
                result: Ok(FrontendServiceResponse::ClipboardContents(Some(
                    " pasted".to_owned(),
                ))),
            })
            .await
            .unwrap();
        assert_eq!(snapshot(&yank).views[0].visible_text, "hello pasted");
        session.terminate_workspace().await.unwrap();
    });
}
