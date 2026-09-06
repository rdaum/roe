// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-only

//! Attachment-local transport, presentation, and service lifecycle. No workspace access.

use super::protocol::*;
use crate::WindowId;
use crate::editor::BorderInfo;
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_EPOCH: AtomicU64 = AtomicU64::new(1);
static NEXT_ATTACHMENT_ID: AtomicU64 = AtomicU64::new(1);

/// State belonging to one frontend attachment. None of these values survive a
/// closed attachment or grant authority to workspace resources.
pub struct Attachment {
    pub(super) id: AttachmentId,
    pub(super) epoch: SessionEpoch,
    pub(super) next_sequence: u64,
    pub(super) revision: Revision,
    pub(super) viewport: AttachmentViewport,
    pub(super) focused: bool,
    pub(super) status: AttachmentStatus,
    pub(super) frontend_capabilities: BTreeSet<FrontendCapability>,
    pub(super) view_scroll: HashMap<WindowId, ViewScroll>,
    pub(super) presented_cursors: HashMap<WindowId, usize>,
    pub(super) typeout_page: Option<(TypeoutId, usize)>,
    pub(super) pointer_selection: Option<(WindowId, usize)>,
    pub(super) pending_pointer_drag: Option<(BorderInfo, WindowId, (u16, u16))>,
    pub(super) next_frontend_request: u64,
    pub(super) pending_frontend_requests: HashMap<FrontendRequestId, PendingFrontendRequest>,
    pub(super) frontend_requests: VecDeque<FrontendServiceRequest>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PendingFrontendRequest {
    ReadClipboardForYank,
    WriteClipboard,
}

impl Attachment {
    pub fn id(&self) -> AttachmentId {
        self.id
    }

    pub fn epoch(&self) -> SessionEpoch {
        self.epoch
    }

    pub fn status(&self) -> AttachmentStatus {
        self.status
    }

    pub(super) fn enqueue_frontend_request(
        &mut self,
        pending: PendingFrontendRequest,
        request: impl FnOnce(FrontendRequestId) -> FrontendServiceRequest,
    ) -> Result<(), String> {
        if self.pending_frontend_requests.len() >= MAX_FRONTEND_REQUESTS {
            return Err(format!(
                "frontend request limit of {MAX_FRONTEND_REQUESTS} reached"
            ));
        }
        let request_id = FrontendRequestId(self.next_frontend_request);
        self.next_frontend_request = self.next_frontend_request.saturating_add(1);
        self.pending_frontend_requests.insert(request_id, pending);
        self.frontend_requests.push_back(request(request_id));
        Ok(())
    }
}

impl Attachment {
    pub(super) fn new(configuration: AttachmentConfiguration) -> Self {
        Self {
            id: AttachmentId(NEXT_ATTACHMENT_ID.fetch_add(1, Ordering::Relaxed)),
            epoch: SessionEpoch(NEXT_EPOCH.fetch_add(1, Ordering::Relaxed)),
            next_sequence: 0,
            revision: Revision(0),
            viewport: configuration.viewport,
            focused: true,
            status: AttachmentStatus::Attached,
            frontend_capabilities: configuration.frontend_capabilities,
            view_scroll: HashMap::new(),
            presented_cursors: HashMap::new(),
            typeout_page: None,
            pointer_selection: None,
            pending_pointer_drag: None,
            next_frontend_request: 1,
            pending_frontend_requests: HashMap::new(),
            frontend_requests: VecDeque::new(),
        }
    }
    pub(super) fn validate_envelope(&self, envelope: &InputEnvelope) -> Result<(), SessionError> {
        if self.status != AttachmentStatus::Attached {
            return Err(SessionError::AttachmentUnavailable);
        }
        if envelope.protocol_version != SESSION_PROTOCOL_VERSION {
            return Err(SessionError::ProtocolVersion {
                received: envelope.protocol_version,
                expected: SESSION_PROTOCOL_VERSION,
            });
        }
        if envelope.epoch != self.epoch {
            return Err(SessionError::StaleEpoch {
                received: envelope.epoch,
                expected: self.epoch,
            });
        }
        if envelope.sequence != self.next_sequence {
            return Err(SessionError::Sequence {
                received: envelope.sequence,
                expected: self.next_sequence,
            });
        }
        Ok(())
    }

    pub(super) fn detach(&mut self) -> Result<(), SessionError> {
        if self.status != AttachmentStatus::Attached {
            return Err(SessionError::AttachmentUnavailable);
        }
        self.status = AttachmentStatus::Detached;
        self.clear_pending();
        Ok(())
    }

    pub(super) fn resume(
        &mut self,
        configuration: AttachmentConfiguration,
    ) -> Result<(), SessionError> {
        if self.status != AttachmentStatus::Detached {
            return Err(SessionError::AttachmentUnavailable);
        }
        self.epoch = SessionEpoch(NEXT_EPOCH.fetch_add(1, Ordering::Relaxed));
        self.next_sequence = 0;
        self.revision = Revision(1);
        self.viewport = configuration.viewport;
        self.frontend_capabilities = configuration.frontend_capabilities;
        self.focused = true;
        self.status = AttachmentStatus::Attached;
        self.presented_cursors.clear();
        Ok(())
    }

    pub(super) fn close(&mut self) {
        self.status = AttachmentStatus::Closed;
        self.clear_pending();
        self.view_scroll.clear();
        self.presented_cursors.clear();
        self.typeout_page = None;
    }

    fn clear_pending(&mut self) {
        self.pointer_selection = None;
        self.pending_pointer_drag = None;
        self.pending_frontend_requests.clear();
        self.frontend_requests.clear();
    }
}
