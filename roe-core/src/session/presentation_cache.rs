// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-only

//! Bounded presentation data caches. No attachment, policy, kernel, or renderer access.

use super::{MAX_PRESENTATION_CHARS, MAX_SESSION_VIEWS};
use crate::buffer::BufferKind;
use crate::{Buffer, BufferId};
use ropey::Rope;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// Shared immutable text, serialized as an ordinary string.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PresentationText(Arc<str>);

impl From<String> for PresentationText {
    fn from(text: String) -> Self {
        Self(Arc::from(text))
    }
}
impl From<&str> for PresentationText {
    fn from(text: &str) -> Self {
        Self(Arc::from(text))
    }
}
impl std::ops::Deref for PresentationText {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}
impl AsRef<str> for PresentationText {
    fn as_ref(&self) -> &str {
        &self.0
    }
}
impl PartialEq<&str> for PresentationText {
    fn eq(&self, text: &&str) -> bool {
        self.as_ref() == *text
    }
}
impl PartialEq<String> for PresentationText {
    fn eq(&self, text: &String) -> bool {
        self.as_ref() == text
    }
}
impl std::fmt::Display for PresentationText {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, formatter)
    }
}

#[derive(Clone, Copy)]
struct Metrics {
    revision: u64,
    max_line_chars: usize,
}

/// One read-lock observation, including the persistent Rope root.
#[derive(Clone)]
pub(super) struct BufferFrame {
    pub rope: Rope,
    pub revision: u64,
    pub saved_revision: u64,
    pub name: String,
    pub kind: BufferKind,
    pub path: Option<PathBuf>,
    pub read_only: bool,
    pub mark: Option<usize>,
    pub show_gutter: bool,
    pub max_line_chars: usize,
}

impl BufferFrame {
    pub fn cursor_coordinates(&self, cursor: usize) -> (usize, usize) {
        let cursor = cursor.min(self.rope.len_chars());
        let line = self.rope.char_to_line(cursor);
        (cursor - self.rope.line_to_char(line), line)
    }
    pub fn modified(&self) -> bool {
        self.revision != self.saved_revision
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct SliceKey {
    buffer: BufferId,
    revision: u64,
    start: usize,
    end: usize,
}

#[derive(Default)]
pub(super) struct PresentationCache {
    metrics: HashMap<BufferId, Metrics>,
    frames: HashMap<BufferId, BufferFrame>,
    previous_slices: HashMap<SliceKey, PresentationText>,
    slices: HashMap<SliceKey, PresentationText>,
}

impl PresentationCache {
    pub fn scroll_limits(&mut self, id: BufferId, buffer: &Buffer) -> (usize, usize) {
        // Input can follow a mutation since the last presentation capture.
        self.frames.remove(&id);
        let frame = self.buffer(id, buffer);
        (
            frame.rope.len_lines().saturating_sub(1),
            frame.max_line_chars,
        )
    }

    pub fn begin_frame(&mut self) {
        self.frames.clear();
        self.previous_slices = std::mem::take(&mut self.slices);
    }

    pub fn buffer(&mut self, id: BufferId, buffer: &Buffer) -> BufferFrame {
        if let Some(frame) = self.frames.get(&id) {
            return frame.clone();
        }
        let mut frame = buffer.with_read(|inner| BufferFrame {
            rope: inner.buffer.clone(),
            revision: inner.text_revision,
            saved_revision: inner.last_saved_revision,
            name: inner.display_name.clone(),
            kind: inner.kind,
            path: inner.visited_file.clone(),
            read_only: inner.read_only,
            mark: inner.mark,
            show_gutter: inner.show_gutter,
            max_line_chars: 0,
        });
        frame.max_line_chars = self
            .metrics
            .get(&id)
            .filter(|metrics| metrics.revision == frame.revision)
            .map(|metrics| metrics.max_line_chars)
            .unwrap_or_else(|| {
                frame
                    .rope
                    .lines()
                    .map(|line| {
                        let len = line.len_chars();
                        len.saturating_sub(usize::from(len > 0 && line.char(len - 1) == '\n'))
                    })
                    .max()
                    .unwrap_or(0)
            });
        // Only displayed buffers enter the cache. Overflow computes without retention.
        if self.frames.len() < MAX_SESSION_VIEWS {
            self.metrics.insert(
                id,
                Metrics {
                    revision: frame.revision,
                    max_line_chars: frame.max_line_chars,
                },
            );
            self.frames.insert(id, frame.clone());
        }
        frame
    }

    pub fn visible_text(
        &mut self,
        id: BufferId,
        frame: &BufferFrame,
        start_line: usize,
        end_line: usize,
    ) -> (usize, usize, PresentationText) {
        let start = frame
            .rope
            .line_to_char(start_line.min(frame.rope.len_lines().saturating_sub(1)));
        let end = if end_line < frame.rope.len_lines() {
            frame.rope.line_to_char(end_line)
        } else {
            frame.rope.len_chars()
        };
        let end = end
            .min(start.saturating_add(MAX_PRESENTATION_CHARS))
            .max(start);
        let key = SliceKey {
            buffer: id,
            revision: frame.revision,
            start,
            end,
        };
        let text = self
            .slices
            .get(&key)
            .cloned()
            .or_else(|| self.previous_slices.remove(&key))
            .unwrap_or_else(|| frame.rope.slice(start..end).to_string().into());
        if self.slices.len() < MAX_SESSION_VIEWS {
            self.slices.insert(key, text.clone());
        }
        (start, end, text)
    }

    pub fn finish_frame(&mut self) {
        self.metrics.retain(|id, _| self.frames.contains_key(id));
        self.previous_slices.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unchanged_views_share_slices_and_changed_text_refreshes_metrics() {
        let id = BufferId::default();
        let buffer = Buffer::named("test", BufferKind::Ordinary);
        buffer.load_str("short\nlonger line\n");
        let mut cache = PresentationCache::default();
        cache.begin_frame();
        let frame = cache.buffer(id, &buffer);
        assert_eq!(frame.max_line_chars, 11);
        let (_, _, first) = cache.visible_text(id, &frame, 1, 2);
        let (_, _, second_view) = cache.visible_text(id, &frame, 1, 2);
        assert!(Arc::ptr_eq(&first.0, &second_view.0));
        cache.finish_frame();
        cache.begin_frame();
        let frame = cache.buffer(id, &buffer);
        let (_, _, next_frame) = cache.visible_text(id, &frame, 1, 2);
        assert!(Arc::ptr_eq(&first.0, &next_frame.0));
        cache.finish_frame();
        buffer.load_str("λ");
        cache.begin_frame();
        let frame = cache.buffer(id, &buffer);
        assert_eq!(frame.max_line_chars, 1);
        let (start, end, changed) = cache.visible_text(id, &frame, 0, 1);
        assert_eq!((start, end), (0, 1));
        assert_eq!(changed, "λ");
        assert!(!Arc::ptr_eq(&first.0, &changed.0));
        cache.finish_frame();
        cache.begin_frame();
        cache.finish_frame();
        assert!(cache.metrics.is_empty());
        assert!(cache.slices.is_empty());
    }
}
