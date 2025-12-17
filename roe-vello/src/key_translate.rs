// Copyright (C) 2025 Ryan Daum <ryan.daum@gmail.com> This program is free
// software: you can redistribute it and/or modify it under the terms of the GNU
// General Public License as published by the Free Software Foundation, version
// 3.
//
// This program is distributed in the hope that it will be useful, but WITHOUT
// ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS
// FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License along with
// this program. If not, see <https://www.gnu.org/licenses/>.
//

//! Translate winit key events to roe's LogicalKey.

use roe_core::keys::{KeyModifier, LogicalKey, Side};
use winit::event::KeyEvent;
use winit::keyboard::{Key, ModifiersState, NamedKey};

/// Translate a winit KeyEvent into a sequence of LogicalKeys
pub fn translate_key_event(event: &KeyEvent, modifiers: ModifiersState) -> Vec<LogicalKey> {
    let mut keys = Vec::new();

    // Add modifiers first (in consistent order)
    if modifiers.control_key() {
        keys.push(LogicalKey::Modifier(KeyModifier::Control(Side::Left)));
    }
    if modifiers.alt_key() {
        // Alt maps to Meta in Emacs terminology
        keys.push(LogicalKey::Modifier(KeyModifier::Meta(Side::Left)));
    }
    if modifiers.shift_key() {
        keys.push(LogicalKey::Modifier(KeyModifier::Shift(Side::Left)));
    }
    if modifiers.super_key() {
        keys.push(LogicalKey::Modifier(KeyModifier::Super(Side::Left)));
    }

    // Translate the main key
    let logical_key = translate_key(&event.logical_key);
    if logical_key != LogicalKey::Unmapped {
        // Don't add modifier keys as the main key if they're already in the modifiers
        match logical_key {
            LogicalKey::Modifier(_) => {
                // Only add if it's the only thing (modifier key press by itself)
                if keys.is_empty() {
                    keys.push(logical_key);
                }
            }
            _ => keys.push(logical_key),
        }
    }

    keys
}

/// Translate a winit Key to a LogicalKey
fn translate_key(key: &Key) -> LogicalKey {
    match key {
        Key::Named(named) => translate_named_key(named),
        Key::Character(s) => {
            // Get the first character
            if let Some(c) = s.chars().next() {
                // For single character strings, return the character
                if s.chars().count() == 1 {
                    LogicalKey::AlphaNumeric(c.to_ascii_lowercase())
                } else {
                    LogicalKey::Unmapped
                }
            } else {
                LogicalKey::Unmapped
            }
        }
        Key::Unidentified(_) => LogicalKey::Unmapped,
        Key::Dead(_) => LogicalKey::Unmapped,
    }
}

/// Translate a winit NamedKey to a LogicalKey
fn translate_named_key(key: &NamedKey) -> LogicalKey {
    match key {
        // Arrow keys
        NamedKey::ArrowLeft => LogicalKey::Left,
        NamedKey::ArrowRight => LogicalKey::Right,
        NamedKey::ArrowUp => LogicalKey::Up,
        NamedKey::ArrowDown => LogicalKey::Down,

        // Navigation
        NamedKey::Home => LogicalKey::Home,
        NamedKey::End => LogicalKey::End,
        NamedKey::PageUp => LogicalKey::PageUp,
        NamedKey::PageDown => LogicalKey::PageDown,
        NamedKey::Insert => LogicalKey::Insert,

        // Editing
        NamedKey::Backspace => LogicalKey::Backspace,
        NamedKey::Delete => LogicalKey::Delete,
        NamedKey::Enter => LogicalKey::Enter,
        NamedKey::Tab => LogicalKey::Tab,
        NamedKey::Escape => LogicalKey::Esc,
        NamedKey::Space => LogicalKey::AlphaNumeric(' '),

        // Function keys
        NamedKey::F1 => LogicalKey::Function(1),
        NamedKey::F2 => LogicalKey::Function(2),
        NamedKey::F3 => LogicalKey::Function(3),
        NamedKey::F4 => LogicalKey::Function(4),
        NamedKey::F5 => LogicalKey::Function(5),
        NamedKey::F6 => LogicalKey::Function(6),
        NamedKey::F7 => LogicalKey::Function(7),
        NamedKey::F8 => LogicalKey::Function(8),
        NamedKey::F9 => LogicalKey::Function(9),
        NamedKey::F10 => LogicalKey::Function(10),
        NamedKey::F11 => LogicalKey::Function(11),
        NamedKey::F12 => LogicalKey::Function(12),

        // Lock keys
        NamedKey::CapsLock => LogicalKey::CapsLock,
        NamedKey::ScrollLock => LogicalKey::ScrollLock,

        // Modifiers (these are usually handled via ModifiersState, but include for completeness)
        NamedKey::Control => LogicalKey::Modifier(KeyModifier::Control(Side::Left)),
        NamedKey::Alt => LogicalKey::Modifier(KeyModifier::Alt(Side::Left)),
        NamedKey::Shift => LogicalKey::Modifier(KeyModifier::Shift(Side::Left)),
        NamedKey::Super => LogicalKey::Modifier(KeyModifier::Super(Side::Left)),
        NamedKey::Meta => LogicalKey::Modifier(KeyModifier::Meta(Side::Left)),

        // Everything else
        _ => LogicalKey::Unmapped,
    }
}
