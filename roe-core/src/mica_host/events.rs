// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-only

//! Validated native messages. Command and binding policy remain in Mica.

use super::{MicaPresentationEffect, MicaPromptUpdate, MicaSearchFinish, MicaSearchUpdate};
use crate::editor::{OpenType, SplitDirection};
use crate::keys::{CursorDirection, KeyAction};
use crate::native_kernel::Capability;
use crate::{BufferId, WindowId};

pub(crate) const MAX_BATCH_EVENTS: usize = 256;
pub(crate) const MAX_BATCH_BYTES: usize = 4 * 1_048_576;
pub(crate) const MAX_POLICY_FACTS: usize = 256;

#[derive(Debug, Clone, PartialEq)]
pub enum MicaHostAction {
    AgentOpen {
        view: WindowId,
        name: String,
        text: String,
    },
    AgentUpdate {
        buffer: BufferId,
        text: String,
    },
    StartAgent,
    ShowTypeout {
        view: WindowId,
        buffer: BufferId,
        kind: String,
        title: String,
        text: String,
    },
    DismissTypeout,
    PageTypeout {
        forward: bool,
    },
    Echo(String),
    Quit,
    Redraw,
    Split {
        view: WindowId,
        direction: SplitDirection,
    },
    SelectView {
        view: WindowId,
    },
    DeleteView {
        view: WindowId,
    },
    CollapseToView {
        view: WindowId,
    },
    BeginLayoutDrag {
        view: WindowId,
    },
    PointerDown {
        view: WindowId,
        position: usize,
        anchor: usize,
    },
    PointerMove {
        view: WindowId,
        position: usize,
        anchor: usize,
    },
    PointerUp,
    Scroll {
        view: WindowId,
        line: usize,
        column: usize,
    },
    SplitRatio {
        path: Vec<usize>,
        ratio: f32,
    },
    InvalidateSyntax {
        view: WindowId,
    },
    Save {
        buffer: BufferId,
    },
    SaveAs {
        buffer: BufferId,
        path: String,
    },
    CreateBuffer {
        view: WindowId,
        name: String,
    },
    EvalRegion {
        buffer: BufferId,
        view: WindowId,
    },
    EvalBuffer {
        buffer: BufferId,
        unit: String,
    },
    SelectBuffer {
        buffer: BufferId,
    },
    KillBuffer {
        buffer: BufferId,
        replacement: BufferId,
    },
    OpenFile {
        path: String,
        kind: OpenType,
    },
}

impl MicaHostAction {
    fn heap_bytes(&self) -> usize {
        match self {
            Self::AgentOpen { name, text, .. } => name.len() + text.len(),
            Self::AgentUpdate { text, .. } | Self::Echo(text) => text.len(),
            Self::ShowTypeout {
                kind, title, text, ..
            } => kind.len() + title.len() + text.len(),
            Self::SaveAs { path, .. } | Self::OpenFile { path, .. } => path.len(),
            Self::CreateBuffer { name, .. } => name.len(),
            Self::EvalBuffer { unit, .. } => unit.len(),
            Self::SplitRatio { path, .. } => path.len() * size_of::<usize>(),
            _ => 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MicaNativeAction {
    InsertText(String),
    Move {
        direction: CursorDirection,
        selecting: bool,
    },
    Backspace,
    Delete,
    Enter,
    Tab,
    KillLine,
    KillRegion,
    CopyRegion,
    Yank,
    KillWord {
        forward: bool,
    },
    SetMark,
    MarkWholeBuffer,
    Cancel,
    Undo,
    Redo,
}

impl MicaNativeAction {
    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::InsertText(_) => "insert_text",
            Self::Move { .. } => "move_cursor",
            Self::Backspace => "backspace",
            Self::Delete => "delete",
            Self::Enter => "enter",
            Self::Tab => "tab",
            Self::KillLine => "kill_line",
            Self::KillRegion => "kill_region",
            Self::CopyRegion => "copy_region",
            Self::Yank => "yank",
            Self::KillWord { forward: true } => "kill_word",
            Self::KillWord { forward: false } => "backward_kill_word",
            Self::SetMark => "set_mark",
            Self::MarkWholeBuffer => "mark_whole_buffer",
            Self::Cancel => "cancel",
            Self::Undo => "undo",
            Self::Redo => "redo",
        }
    }

    pub(crate) fn decode(name: &str, text: Option<String>) -> Result<Self, String> {
        let movement = |direction, selecting| Self::Move {
            direction,
            selecting,
        };
        Ok(match name {
            "insert_text" => Self::InsertText(text.ok_or("Mica insert_text requires text")?),
            "cursor_left" => movement(CursorDirection::Left, false),
            "cursor_right" => movement(CursorDirection::Right, false),
            "cursor_up" => movement(CursorDirection::Up, false),
            "cursor_down" => movement(CursorDirection::Down, false),
            "cursor_line_start" => movement(CursorDirection::LineStart, false),
            "cursor_line_end" => movement(CursorDirection::LineEnd, false),
            "cursor_buffer_start" => movement(CursorDirection::BufferStart, false),
            "cursor_buffer_end" => movement(CursorDirection::BufferEnd, false),
            "cursor_page_up" => movement(CursorDirection::PageUp, false),
            "cursor_page_down" => movement(CursorDirection::PageDown, false),
            "cursor_word_forward" => movement(CursorDirection::WordForward, false),
            "cursor_word_backward" => movement(CursorDirection::WordBackward, false),
            "cursor_paragraph_forward" => movement(CursorDirection::ParagraphForward, false),
            "cursor_paragraph_backward" => movement(CursorDirection::ParagraphBackward, false),
            "cursor_left_select" => movement(CursorDirection::Left, true),
            "cursor_right_select" => movement(CursorDirection::Right, true),
            "cursor_up_select" => movement(CursorDirection::Up, true),
            "cursor_down_select" => movement(CursorDirection::Down, true),
            "cursor_line_start_select" => movement(CursorDirection::LineStart, true),
            "cursor_line_end_select" => movement(CursorDirection::LineEnd, true),
            "cursor_buffer_start_select" => movement(CursorDirection::BufferStart, true),
            "cursor_buffer_end_select" => movement(CursorDirection::BufferEnd, true),
            "cursor_page_up_select" => movement(CursorDirection::PageUp, true),
            "cursor_page_down_select" => movement(CursorDirection::PageDown, true),
            "cursor_word_forward_select" => movement(CursorDirection::WordForward, true),
            "cursor_word_backward_select" => movement(CursorDirection::WordBackward, true),
            "backspace" => Self::Backspace,
            "delete" => Self::Delete,
            "enter" => Self::Enter,
            "tab" => Self::Tab,
            "kill_line" => Self::KillLine,
            "kill_region" => Self::KillRegion,
            "copy_region" => Self::CopyRegion,
            "yank" => Self::Yank,
            "kill_word" => Self::KillWord { forward: true },
            "backward_kill_word" => Self::KillWord { forward: false },
            "set_mark" => Self::SetMark,
            "mark_whole_buffer" => Self::MarkWholeBuffer,
            "cancel" | "escape" => Self::Cancel,
            "undo" => Self::Undo,
            "redo" => Self::Redo,
            _ => return Err(format!("unknown Mica native action: {name}")),
        })
    }

    pub(crate) fn capabilities(&self) -> &'static [Capability] {
        match self {
            Self::InsertText(_)
            | Self::Backspace
            | Self::Delete
            | Self::Enter
            | Self::Tab
            | Self::KillLine
            | Self::KillRegion
            | Self::Yank
            | Self::KillWord { .. }
            | Self::Undo
            | Self::Redo => &[Capability::TextWrite],
            Self::CopyRegion | Self::SetMark | Self::MarkWholeBuffer | Self::Move { .. } => {
                &[Capability::TextRead]
            }
            Self::Cancel => &[],
        }
    }

    pub(crate) fn writes_clipboard(&self) -> bool {
        matches!(
            self,
            Self::KillLine | Self::KillRegion | Self::CopyRegion | Self::KillWord { .. }
        )
    }

    /// Word operations require the effective Mica syntax at their call site.
    pub(crate) fn into_key_action(self) -> Option<KeyAction> {
        Some(match self {
            Self::Move {
                direction: CursorDirection::WordForward | CursorDirection::WordBackward,
                ..
            }
            | Self::KillWord { .. }
            | Self::InsertText(_) => return None,
            Self::Move {
                direction,
                selecting: true,
            } => KeyAction::CursorSelect(direction),
            Self::Move {
                direction,
                selecting: false,
            } => KeyAction::Cursor(direction),
            Self::Backspace => KeyAction::Backspace,
            Self::Delete => KeyAction::Delete,
            Self::Enter => KeyAction::Enter,
            Self::Tab => KeyAction::Tab,
            Self::KillLine => KeyAction::KillLine,
            Self::KillRegion => KeyAction::KillRegion(true),
            Self::CopyRegion => KeyAction::KillRegion(false),
            Self::Yank => KeyAction::Yank(None),
            Self::SetMark => KeyAction::MarkStart,
            Self::MarkWholeBuffer => KeyAction::MarkWholeBuffer,
            Self::Cancel => KeyAction::Cancel,
            Self::Undo => KeyAction::Undo,
            Self::Redo => KeyAction::Redo,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MicaPolicyFact {
    Mode {
        buffer: BufferId,
        name: String,
    },
    Face {
        name: String,
        attribute: String,
        value: String,
    },
    Configuration {
        key: String,
        value: String,
    },
    Syntax {
        buffer: BufferId,
        kind: String,
        pattern: String,
        precedence: i64,
    },
    Highlight {
        mode: String,
        capture: String,
        face: String,
        precedence: i64,
    },
}

impl MicaPolicyFact {
    pub(crate) fn heap_bytes(&self) -> usize {
        match self {
            Self::Mode { name, .. } => name.len(),
            Self::Face {
                name,
                attribute,
                value,
            } => name.len() + attribute.len() + value.len(),
            Self::Configuration { key, value } => key.len() + value.len(),
            Self::Syntax { kind, pattern, .. } => kind.len() + pattern.len(),
            Self::Highlight {
                mode,
                capture,
                face,
                ..
            } => mode.len() + capture.len() + face.len(),
        }
    }
}

#[derive(Debug)]
pub enum MicaEvent {
    Presentation(MicaPresentationEffect),
    Host(MicaHostAction),
    Native(MicaNativeAction),
    Policy(Vec<MicaPolicyFact>),
    Prompt(MicaPromptUpdate),
    PromptClosed,
    Search(MicaSearchUpdate),
    SearchFinished(MicaSearchFinish),
    Error(String),
    TaskCancelled(u64),
    SubscriptionReady(u64),
    Overloaded,
}

impl MicaEvent {
    fn retained_bytes(&self) -> usize {
        size_of::<Self>()
            + match self {
                Self::Host(action) => action.heap_bytes(),
                Self::Native(MicaNativeAction::InsertText(text)) | Self::Error(text) => text.len(),
                Self::Policy(facts) => facts
                    .iter()
                    .map(|fact| size_of::<MicaPolicyFact>() + fact.heap_bytes())
                    .sum(),
                Self::Prompt(prompt) => {
                    prompt.prefix.len()
                        + prompt.query.len()
                        + prompt
                            .candidates
                            .iter()
                            .map(|name| size_of::<String>() + name.len())
                            .sum::<usize>()
                }
                Self::Search(search) => search.matches.len() * size_of::<(usize, usize)>(),
                Self::SearchFinished(search) => search.query.len(),
                _ => 0,
            }
    }
}

/// One ordered admission budget, including events accumulated while an invocation runs.
#[derive(Debug, Default)]
pub struct MicaEventBatch {
    events: Vec<MicaEvent>,
    bytes: usize,
    overloaded: bool,
}

impl MicaEventBatch {
    pub(crate) fn push(&mut self, event: MicaEvent) -> bool {
        if self.overloaded {
            return false;
        }
        let bytes = self.bytes.saturating_add(event.retained_bytes());
        if self.events.len() >= MAX_BATCH_EVENTS || bytes > MAX_BATCH_BYTES {
            self.overloaded = true;
            return false;
        }
        self.bytes = bytes;
        self.events.push(event);
        true
    }

    pub(crate) fn into_events(self) -> impl Iterator<Item = MicaEvent> {
        self.events
            .into_iter()
            .chain(self.overloaded.then_some(MicaEvent::Overloaded))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_requires_write_and_malformed_insert_is_rejected() {
        assert_eq!(
            MicaNativeAction::decode("tab", None)
                .unwrap()
                .capabilities(),
            &[Capability::TextWrite]
        );
        assert!(MicaNativeAction::decode("insert_text", None).is_err());
        assert!(MicaNativeAction::decode("unknown", None).is_err());
    }

    #[test]
    fn admission_retains_cross_category_order_and_reports_overflow_once() {
        let mut batch = MicaEventBatch::default();
        batch.push(MicaEvent::Host(MicaHostAction::Echo("first".into())));
        batch.push(MicaEvent::Native(MicaNativeAction::Tab));
        batch.push(MicaEvent::PromptClosed);
        for _ in 3..MAX_BATCH_EVENTS + 20 {
            batch.push(MicaEvent::TaskCancelled(42));
        }
        let events: Vec<_> = batch.into_events().collect();
        assert!(
            matches!(&events[0], MicaEvent::Host(MicaHostAction::Echo(text)) if text == "first")
        );
        assert!(matches!(
            events[1],
            MicaEvent::Native(MicaNativeAction::Tab)
        ));
        assert!(matches!(events[2], MicaEvent::PromptClosed));
        assert_eq!(events.len(), MAX_BATCH_EVENTS + 1);
        assert!(matches!(events.last(), Some(MicaEvent::Overloaded)));
    }

    #[test]
    fn byte_budget_rejects_a_whole_policy_publication() {
        let mut batch = MicaEventBatch::default();
        batch.push(MicaEvent::Policy(vec![MicaPolicyFact::Configuration {
            key: "large".into(),
            value: "x".repeat(MAX_BATCH_BYTES),
        }]));
        let events: Vec<_> = batch.into_events().collect();
        assert!(matches!(events.as_slice(), [MicaEvent::Overloaded]));
    }
}
