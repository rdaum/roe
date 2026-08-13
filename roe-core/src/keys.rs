// Copyright (C) 2025 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Renderer-neutral input vocabulary and native editing mechanisms.
//!
//! Key binding, chord, command, and mode policy live in Mica. Rust only keeps
//! the normalized physical key vocabulary and the small set of native actions
//! selected by Mica.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum KeyAction {
    AlphaNumeric(char),
    Cursor(CursorDirection),
    CursorSelect(CursorDirection),
    Delete,
    Backspace,
    Enter,
    Tab,
    MarkStart,
    KillRegion(bool),
    KillLine(bool),
    Yank(Option<usize>),
    Escape,
    Cancel,
    Undo,
    Redo,
    DeleteWord,
    BackspaceWord,
    Redraw,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub enum CursorDirection {
    Left,
    Right,
    Up,
    Down,
    LineEnd,
    LineStart,
    BufferStart,
    BufferEnd,
    PageUp,
    PageDown,
    WordForward,
    WordBackward,
    ParagraphForward,
    ParagraphBackward,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub enum Side {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub enum LogicalKey {
    Left,
    Right,
    Up,
    Down,
    PageUp,
    PageDown,
    Function(u8),
    AlphaNumeric(char),
    Backspace,
    Enter,
    Home,
    End,
    Insert,
    Tab,
    Delete,
    Unmapped,
    CapsLock,
    ScrollLock,
    Esc,
    Modifier(KeyModifier),
}

impl LogicalKey {
    pub fn as_display_string(&self) -> String {
        match self {
            Self::Left => "←".to_owned(),
            Self::Right => "→".to_owned(),
            Self::Up => "↑".to_owned(),
            Self::Down => "↓".to_owned(),
            Self::PageUp => "PgUp".to_owned(),
            Self::PageDown => "PgDn".to_owned(),
            Self::Function(number) => format!("F{number}"),
            Self::AlphaNumeric(character) => character.to_string(),
            Self::Backspace => "⌫".to_owned(),
            Self::Enter => "⏎".to_owned(),
            Self::Home => "Home".to_owned(),
            Self::End => "End".to_owned(),
            Self::Insert => "Ins".to_owned(),
            Self::Tab => "Tab".to_owned(),
            Self::Delete => "Del".to_owned(),
            Self::Unmapped => "Unmapped".to_owned(),
            Self::CapsLock => "Caps".to_owned(),
            Self::ScrollLock => "Scroll".to_owned(),
            Self::Esc => "Esc".to_owned(),
            Self::Modifier(KeyModifier::Hyper(_)) => "H".to_owned(),
            Self::Modifier(KeyModifier::Super(_) | KeyModifier::Shift(_)) => "S".to_owned(),
            Self::Modifier(KeyModifier::Meta(_)) => "M".to_owned(),
            Self::Modifier(KeyModifier::Control(_)) => "C".to_owned(),
            Self::Modifier(KeyModifier::Alt(_)) => "A".to_owned(),
            Self::Modifier(KeyModifier::Unmapped) => "?".to_owned(),
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub enum KeyModifier {
    Hyper(Side),
    Super(Side),
    Meta(Side),
    Control(Side),
    Shift(Side),
    Alt(Side),
    Unmapped,
}
