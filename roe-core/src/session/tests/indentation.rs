// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-only

use super::*;

#[test]
fn checked_native_edits_reject_denial_stale_generation_revision_and_read_only() {
    let mut kernel = NativeKernel::new(CapabilityGrants::editor_default());
    let buffer = Buffer::new();
    buffer.load_str("λ text");
    let resource = kernel.register_buffer(buffer.clone()).unwrap();
    let revision = buffer.text_revision();
    let edit = NativeOperation::ReplaceAtRevision {
        resource,
        revision,
        start: 2,
        end: 6,
        text: "value".into(),
    };
    buffer.set_read_only(true);
    assert!(matches!(
        kernel.execute(edit.clone()),
        Err(KernelError::ReadOnly(_))
    ));
    buffer.set_read_only(false);
    buffer.insert_pos("!".into(), 6);
    assert!(matches!(
        kernel.execute(edit.clone()),
        Err(KernelError::StaleRevision { .. })
    ));
    assert_eq!(buffer.content(), "λ text!");
    kernel
        .execute(NativeOperation::CloseResource { resource })
        .unwrap();
    assert!(matches!(
        kernel.execute(edit),
        Err(KernelError::StaleResource(_))
    ));
    let mut denied = NativeKernel::new(CapabilityGrants::new([]));
    assert!(matches!(
        denied.execute(NativeOperation::ReplaceAtRevision {
            resource,
            revision,
            start: 0,
            end: 0,
            text: "x".into()
        }),
        Err(KernelError::CapabilityDenied(Capability::TextWrite))
    ));
    assert!(matches!(
        denied.observe_text(resource),
        Err(KernelError::CapabilityDenied(Capability::TextRead))
    ));
}

#[test]
fn checked_native_edit_is_atomic_undoable_and_maps_marks() {
    let mut kernel = NativeKernel::new(CapabilityGrants::editor_default());
    let buffer = Buffer::new();
    buffer.load_str("  let λ = 1;");
    buffer.set_mark(6);
    let resource = kernel.register_buffer(buffer.clone()).unwrap();
    kernel
        .execute(NativeOperation::ReplaceAtRevision {
            resource,
            revision: buffer.text_revision(),
            start: 0,
            end: 2,
            text: "    ".into(),
        })
        .unwrap();
    assert_eq!(buffer.content(), "    let λ = 1;");
    assert_eq!(buffer.get_mark(), Some(8));
    buffer.undo();
    assert_eq!(buffer.content(), "  let λ = 1;");
    buffer.redo();
    assert_eq!(buffer.content(), "    let λ = 1;");
    let revision = buffer.text_revision();
    kernel
        .execute(NativeOperation::ReplaceAtRevision {
            resource,
            revision,
            start: 0,
            end: 4,
            text: "    ".into(),
        })
        .unwrap();
    assert_eq!(buffer.text_revision(), revision);
    assert!(matches!(
        kernel.execute(NativeOperation::ReplaceAtRevision {
            resource,
            revision,
            start: 50,
            end: 51,
            text: "".into()
        }),
        Err(KernelError::InvalidRange { .. })
    ));
    assert_eq!(buffer.text_revision(), revision);
}
