// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-only

//! Realize an explicitly targeted Mica indentation effect with a checked edit.

use super::*;
use crate::syntax::IndentRequest;

pub(super) struct IndentationSettings {
    pub newline: bool,
    pub width: usize,
    pub tab_width: usize,
}

impl WorkspaceHost {
    pub(super) fn apply_indentation(
        &mut self,
        view: WindowId,
        buffer: BufferId,
        revision: u64,
        settings: IndentationSettings,
        invalidations: &mut Vec<Invalidation>,
    ) -> Result<(), String> {
        let IndentationSettings {
            newline,
            width,
            tab_width,
        } = settings;
        // Check write authority before even obtaining the source for planning.
        self.kernel
            .lock()
            .unwrap()
            .authorize(Capability::TextWrite)
            .map_err(|error| error.to_string())?;
        let window = self
            .editor
            .windows
            .get(view)
            .ok_or("indentation targeted a stale view")?;
        if window.active_buffer != buffer {
            return Err("indentation targeted a stale view/buffer association".into());
        }
        let cursor = window.cursor;
        let resource = *self
            .buffer_resources
            .get(&buffer)
            .ok_or("indentation targeted a stale buffer")?;
        let (actual, rope) = self
            .kernel
            .lock()
            .unwrap()
            .observe_text(resource)
            .map_err(|error| error.to_string())?;
        if revision != actual {
            return Err(KernelError::StaleRevision {
                expected: revision,
                actual,
            }
            .to_string());
        }
        let plan = self
            .policy
            .modes
            .get(&buffer)
            .and_then(|mode| self.policy.parsers.get(mode))
            .ok_or("Mica has no syntax parser for this buffer")?;
        let edit = self.syntax.indent_edit(
            buffer,
            revision,
            &rope,
            plan,
            IndentRequest {
                cursor,
                newline,
                width,
                tab_width,
            },
        )?;
        let inserted = edit.text.chars().count();
        self.kernel
            .lock()
            .unwrap()
            .execute(NativeOperation::ReplaceAtRevision {
                resource,
                revision,
                start: edit.start,
                end: edit.end,
                text: edit.text,
            })
            .map_err(|error| error.to_string())?;
        for (id, window) in &mut self.editor.windows {
            if window.active_buffer == buffer {
                window.cursor = if id == view {
                    edit.cursor
                } else {
                    crate::native_kernel::map_edit_position(
                        window.cursor,
                        edit.start,
                        edit.end,
                        inserted,
                    )
                };
                if let Some(id) = self.view_ids.get(&id) {
                    invalidations.push(Invalidation::View(*id));
                }
            }
        }
        tracing::trace!(
            ?view,
            ?buffer,
            revision,
            newline,
            "applied syntax indentation"
        );
        Ok(())
    }
}
