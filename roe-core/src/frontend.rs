// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-only

//! Attachment-local services and bounded output processing shared by local frontends.
//! The workspace never constructs a clipboard or owns this state.

use crate::renderer::PresentationStreamError;
use crate::session::*;
use std::collections::VecDeque;

/// Renderer realization only. There is no editor or policy access.
pub trait PresentationConsumer {
    fn accept_presentation(
        &mut self,
        update: &PresentationUpdate,
    ) -> Result<(), PresentationStreamError>;
    fn redraw_presentation(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub trait FrontendServices {
    fn handle(&mut self, request: FrontendServiceRequest) -> FrontendServiceResult;
}

#[derive(Debug, thiserror::Error)]
pub enum FrontendOutputError {
    #[error("session output failed: {0}")]
    Session(#[from] SessionError),
    #[error("presentation resynchronization failed: {0}")]
    Presentation(#[from] PresentationStreamError),
    #[error("frontend output failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("frontend output cascade exceeds its bound")]
    Overloaded,
    #[error("snapshot recovery returned no full snapshot")]
    MissingSnapshot,
}

const MAX_OUTPUT_CASCADE: usize = 64;

/// Drain correlated service responses in order. A rejected delta requests a full snapshot.
/// Both the pending queue and the total follow-up work have explicit limits.
pub async fn consume_output(
    session: &mut impl SessionClient,
    services: &mut impl FrontendServices,
    consumer: &mut impl PresentationConsumer,
    output: SessionOutput,
) -> Result<bool, FrontendOutputError> {
    let mut outputs = VecDeque::from([output]);
    let mut quit = false;
    let mut applied = 0;
    let mut recovered_through = None;
    while let Some(mut output) = outputs.pop_front() {
        applied += 1;
        if applied > MAX_OUTPUT_CASCADE || output.frontend_requests.len() > MAX_FRONTEND_REQUESTS {
            return Err(FrontendOutputError::Overloaded);
        }
        quit |= report_lifecycle(&output.lifecycle);
        if let Some(update) = output.presentation.take() {
            let stale_after_recovery = match (&update, recovered_through) {
                (PresentationUpdate::Delta(delta), Some((epoch, revision))) => {
                    delta.epoch == epoch && delta.revision.0 <= revision
                }
                _ => false,
            };
            if !stale_after_recovery {
                match consumer.accept_presentation(&update) {
                    Ok(()) => consumer.redraw_presentation()?,
                    Err(error) if matches!(update, PresentationUpdate::Delta(_)) => {
                        tracing::warn!(%error, "requesting full presentation snapshot");
                        let envelope =
                            session.envelope(InputEvent::RequestSnapshot { after: None });
                        let mut recovery = session.dispatch(envelope).await?;
                        quit |= report_lifecycle(&recovery.lifecycle);
                        let Some(PresentationUpdate::Full(snapshot)) = recovery.presentation.take()
                        else {
                            return Err(FrontendOutputError::MissingSnapshot);
                        };
                        recovered_through = Some((snapshot.epoch, snapshot.revision.0));
                        consumer.accept_presentation(&PresentationUpdate::Full(snapshot))?;
                        consumer.redraw_presentation()?;
                        if recovery.frontend_requests.len() > MAX_FRONTEND_REQUESTS {
                            return Err(FrontendOutputError::Overloaded);
                        }
                        // Retain independently issued requests from the recovery response.
                        recovery.lifecycle.clear();
                        outputs.push_front(recovery);
                    }
                    Err(error) => return Err(error.into()),
                }
            }
        }
        for request in output.frontend_requests {
            if outputs.len() >= MAX_FRONTEND_REQUESTS {
                return Err(FrontendOutputError::Overloaded);
            }
            let completion = services.handle(request);
            outputs.push_back(session.complete_frontend_request(completion).await?);
        }
    }
    Ok(quit)
}

pub fn report_lifecycle(events: &[LifecycleEvent]) -> bool {
    let mut quit = false;
    for event in events {
        match event {
            LifecycleEvent::QuitRequested
            | LifecycleEvent::AttachmentClosed { .. }
            | LifecycleEvent::WorkspaceTerminated => quit = true,
            LifecycleEvent::Warning(message) => tracing::warn!(%message, "session warning"),
            LifecycleEvent::Error(message) => tracing::error!(%message, "session error"),
            LifecycleEvent::Fatal(message) => {
                tracing::error!(%message, "fatal session error");
                quit = true;
            }
            LifecycleEvent::Overloaded { detail } => tracing::warn!(%detail, "session overload"),
            LifecycleEvent::RecoveryResult { operation, result } => match result {
                Ok(_) => tracing::info!(%operation, "Mica recovery operation completed"),
                Err(error) => tracing::error!(%operation, %error, "Mica recovery operation failed"),
            },
            _ => {}
        }
    }
    quit
}

pub struct LocalFrontendServices {
    clipboard: Option<arboard::Clipboard>,
    clipboard_error: Option<String>,
}

impl LocalFrontendServices {
    pub fn new() -> Self {
        match arboard::Clipboard::new() {
            Ok(clipboard) => Self {
                clipboard: Some(clipboard),
                clipboard_error: None,
            },
            Err(error) => Self {
                clipboard: None,
                clipboard_error: Some(error.to_string()),
            },
        }
    }
}

impl FrontendServices for LocalFrontendServices {
    fn handle(&mut self, request: FrontendServiceRequest) -> FrontendServiceResult {
        let request_id = request.request_id();
        let unavailable = self
            .clipboard_error
            .clone()
            .unwrap_or_else(|| "frontend clipboard is unavailable".to_owned());
        let result = match request {
            FrontendServiceRequest::WriteClipboard { ref contents, .. }
                if contents.chars().count() > MAX_FRONTEND_TEXT_CHARS =>
            {
                Err("frontend clipboard write exceeds the text limit".to_owned())
            }
            FrontendServiceRequest::ReadClipboard { .. } => self
                .clipboard
                .as_mut()
                .ok_or_else(|| unavailable.clone())
                .and_then(|clipboard| clipboard.get_text().map_err(|error| error.to_string()))
                .and_then(|contents| {
                    if contents.chars().count() > MAX_FRONTEND_TEXT_CHARS {
                        Err("frontend clipboard read exceeds the text limit".to_owned())
                    } else {
                        Ok(FrontendServiceResponse::ClipboardContents(Some(contents)))
                    }
                }),
            FrontendServiceRequest::WriteClipboard { contents, .. } => self
                .clipboard
                .as_mut()
                .ok_or(unavailable)
                .and_then(|clipboard| {
                    clipboard
                        .set_text(contents)
                        .map(|()| FrontendServiceResponse::Completed)
                        .map_err(|error| error.to_string())
                }),
            FrontendServiceRequest::Notify { .. } => {
                Err("frontend notifications are not available".to_owned())
            }
        };
        FrontendServiceResult { request_id, result }
    }
}

impl Default for LocalFrontendServices {
    fn default() -> Self {
        Self::new()
    }
}
