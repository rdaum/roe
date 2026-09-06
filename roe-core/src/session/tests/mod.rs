// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-only

mod bridge;
mod coordinates;
mod files;
use super::*;
use crate::Buffer;
use crate::editor::{Frame, SplitDirection, WindowNode};
use crate::native_kernel::TextSelection;
use slotmap::SlotMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};

static MICA_TEST_LOCK: Mutex<()> = Mutex::new(());

struct FixedNativeClock(u64);

impl NativeClock for FixedNativeClock {
    fn unix_millis(&self) -> u64 {
        self.0
    }
}

fn test_editor() -> Editor {
    let buffer = Buffer::named("*test*", crate::buffer::BufferKind::Ordinary);
    buffer.load_str("hello");
    let mut editor = Editor::new(buffer, Frame::new(80, 23));
    editor.move_cursor_to(5, false);
    editor
}

fn attach_test_workspace(workspace: WorkspaceHost) -> DirectSessionClient {
    DirectSessionClient::new(workspace, AttachmentConfiguration::headless(80, 24))
}

fn test_mica_client(
    editor: Editor,
    grants: CapabilityGrants,
) -> Result<DirectSessionClient, MicaHostError> {
    WorkspaceHost::open_with_mica(editor, grants).map(attach_test_workspace)
}

fn test_mica_client_with_clock(
    editor: Editor,
    grants: CapabilityGrants,
    clock: Arc<dyn NativeClock>,
) -> Result<DirectSessionClient, MicaHostError> {
    WorkspaceHost::open_with_mica_clock(editor, grants, clock).map(attach_test_workspace)
}

fn test_mica_client_with_stream_handler(
    editor: Editor,
    grants: CapabilityGrants,
    stream_handler: mica_driver::ExternalStreamRequestHandler,
) -> Result<DirectSessionClient, MicaHostError> {
    WorkspaceHost::open_with_mica_stream_handler(editor, grants, stream_handler)
        .map(attach_test_workspace)
}

fn test_session_with_grants(grants: CapabilityGrants) -> DirectSessionClient {
    attach_test_workspace(WorkspaceHost::open(test_editor(), grants).unwrap())
}

fn test_session() -> DirectSessionClient {
    test_session_with_grants(CapabilityGrants::editor_default())
}

fn control() -> LogicalKey {
    LogicalKey::Modifier(crate::keys::KeyModifier::Control(crate::keys::Side::Left))
}

fn snapshot(output: &SessionOutput) -> &PresentationSnapshot {
    match output.presentation.as_ref().unwrap() {
        PresentationUpdate::Full(snapshot) => snapshot,
        PresentationUpdate::Delta(delta) => &delta.snapshot,
    }
}

mod agent;
mod authority;
mod background;
mod buffers;
mod clipboard;
mod commands;
mod file_prompts;
mod layout;
mod native;
mod pointer;
mod policy;
mod presentation;
mod protocol;
mod recovery;
mod sources;

mod frontend;
mod startup;
