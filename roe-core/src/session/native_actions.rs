// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-only

//! Native action realization sees only editor mechanisms and effective policy.
//! Attachment services, driver invocation, and lifecycle handling stay with the coordinator.

use super::policy::PolicyProjection;
use crate::editor::{ActionPosition, ChromeAction};
use crate::keys::CursorDirection;
use crate::mica_host::MicaNativeAction;
use crate::{BufferId, Editor};

pub(super) enum ActionError {
    Policy(String),
    Mechanism(std::io::Error),
}

pub(super) struct NativeActions<'a> {
    pub editor: &'a mut Editor,
    pub policy: &'a PolicyProjection,
}

impl NativeActions<'_> {
    pub async fn apply(
        &mut self,
        action: MicaNativeAction,
    ) -> Result<Vec<ChromeAction>, ActionError> {
        match action {
            MicaNativeAction::InsertText(text) => {
                Ok(self.editor.insert_text(text, &ActionPosition::Cursor))
            }
            MicaNativeAction::Move {
                direction:
                    direction @ (CursorDirection::WordForward | CursorDirection::WordBackward),
                selecting,
            } => self
                .mica_word_boundary(direction == CursorDirection::WordForward)
                .map(|position| self.editor.move_cursor_to(position, selecting))
                .map_err(ActionError::Policy),
            MicaNativeAction::KillWord { forward } => {
                self.mica_kill_word(forward).map_err(ActionError::Policy)
            }
            other => self
                .editor
                .perform_native_action(
                    other
                        .into_key_action()
                        .expect("special native actions were handled"),
                )
                .await
                .map_err(ActionError::Mechanism),
        }
    }
    fn mica_kill_word(&mut self, forward: bool) -> Result<Vec<ChromeAction>, String> {
        let window = &self.editor.windows[self.editor.active_window];
        let buffer_id = window.active_buffer;
        let cursor = window.cursor;
        let text: Vec<char> = self.editor.buffers[buffer_id].content().chars().collect();
        let syntax = self.mica_word_syntax(buffer_id)?;
        let is_word = |character: char| syntax.contains(character);
        let boundary = word_boundary(&text, cursor, forward, is_word);
        let count = if forward {
            isize::try_from(boundary.saturating_sub(cursor)).unwrap_or(isize::MAX)
        } else {
            -isize::try_from(cursor.saturating_sub(boundary)).unwrap_or(isize::MAX)
        };
        Ok(self
            .editor
            .kill_text(&crate::editor::ActionPosition::Cursor, count))
    }

    fn mica_word_boundary(&self, forward: bool) -> Result<usize, String> {
        let window = &self.editor.windows[self.editor.active_window];
        let buffer_id = window.active_buffer;
        let cursor = window.cursor;
        let text: Vec<char> = self.editor.buffers[buffer_id].content().chars().collect();
        let syntax = self.mica_word_syntax(buffer_id)?;
        Ok(word_boundary(&text, cursor, forward, |character| {
            syntax.contains(character)
        }))
    }

    fn mica_word_syntax(&self, buffer_id: BufferId) -> Result<SyntaxClass, String> {
        let word_rules: Vec<_> = self
            .policy
            .syntax
            .get(&buffer_id)
            .into_iter()
            .flatten()
            .filter(|rule| rule.kind == "word")
            .collect();
        let precedence = word_rules
            .iter()
            .map(|rule| rule.precedence)
            .max()
            .ok_or_else(|| "Mica has no effective word syntax rule".to_owned())?;
        let mut patterns: Vec<_> = word_rules
            .into_iter()
            .filter(|rule| rule.precedence == precedence)
            .map(|rule| rule.pattern.as_str())
            .collect();
        patterns.sort_unstable();
        patterns.dedup();
        if patterns.len() != 1 {
            return Err(format!(
                "Mica word syntax is ambiguous at precedence {precedence}"
            ));
        }
        SyntaxClass::parse(patterns[0])
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SyntaxClassAtom {
    Alnum,
    Alpha,
    Digit,
    Lower,
    Upper,
    Space,
    Blank,
    HexDigit,
    Literal(char),
    Range(char, char),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SyntaxClass {
    negated: bool,
    atoms: Vec<SyntaxClassAtom>,
}

impl SyntaxClass {
    fn parse(pattern: &str) -> Result<Self, String> {
        let Some(inner) = pattern
            .strip_prefix('[')
            .and_then(|value| value.strip_suffix(']'))
        else {
            return Err(format!(
                "unsupported Mica word syntax pattern {pattern:?}: expected a character class"
            ));
        };
        let mut characters: Vec<char> = inner.chars().collect();
        let negated = characters.first() == Some(&'^');
        if negated {
            characters.remove(0);
        }
        let mut atoms = Vec::new();
        let mut index = 0;
        while index < characters.len() {
            if characters.get(index) == Some(&'[') && characters.get(index + 1) == Some(&':') {
                let Some(end) = (index + 2..characters.len().saturating_sub(1)).find(|candidate| {
                    characters.get(*candidate) == Some(&':')
                        && characters.get(candidate + 1) == Some(&']')
                }) else {
                    return Err(format!(
                        "invalid POSIX class in Mica syntax pattern {pattern:?}"
                    ));
                };
                let name: String = characters[index + 2..end].iter().collect();
                atoms.push(match name.as_str() {
                    "alnum" => SyntaxClassAtom::Alnum,
                    "alpha" => SyntaxClassAtom::Alpha,
                    "digit" => SyntaxClassAtom::Digit,
                    "lower" => SyntaxClassAtom::Lower,
                    "upper" => SyntaxClassAtom::Upper,
                    "space" => SyntaxClassAtom::Space,
                    "blank" => SyntaxClassAtom::Blank,
                    "xdigit" => SyntaxClassAtom::HexDigit,
                    _ => {
                        return Err(format!(
                            "unsupported POSIX class {name:?} in Mica syntax pattern"
                        ));
                    }
                });
                index = end + 2;
                continue;
            }
            let start = if characters[index] == '\\' {
                index += 1;
                *characters
                    .get(index)
                    .ok_or_else(|| format!("trailing escape in Mica syntax pattern {pattern:?}"))?
            } else {
                characters[index]
            };
            if characters.get(index + 1) == Some(&'-')
                && let Some(end) = characters.get(index + 2).copied()
            {
                atoms.push(SyntaxClassAtom::Range(start, end));
                index += 3;
            } else {
                atoms.push(SyntaxClassAtom::Literal(start));
                index += 1;
            }
        }
        if atoms.is_empty() {
            return Err("Mica word syntax character class is empty".to_owned());
        }
        Ok(Self { negated, atoms })
    }

    fn contains(&self, character: char) -> bool {
        let matched = self.atoms.iter().any(|atom| match atom {
            SyntaxClassAtom::Alnum => character.is_alphanumeric(),
            SyntaxClassAtom::Alpha => character.is_alphabetic(),
            SyntaxClassAtom::Digit => character.is_numeric(),
            SyntaxClassAtom::Lower => character.is_lowercase(),
            SyntaxClassAtom::Upper => character.is_uppercase(),
            SyntaxClassAtom::Space => character.is_whitespace(),
            SyntaxClassAtom::Blank => matches!(character, ' ' | '\t'),
            SyntaxClassAtom::HexDigit => character.is_ascii_hexdigit(),
            SyntaxClassAtom::Literal(expected) => character == *expected,
            SyntaxClassAtom::Range(start, end) => *start <= character && character <= *end,
        });
        matched != self.negated
    }
}

fn word_boundary(
    text: &[char],
    cursor: usize,
    forward: bool,
    is_word: impl Fn(char) -> bool,
) -> usize {
    if forward {
        let mut position = cursor.min(text.len());
        while position < text.len() && !is_word(text[position]) {
            position += 1;
        }
        while position < text.len() && is_word(text[position]) {
            position += 1;
        }
        while position < text.len() && !is_word(text[position]) {
            position += 1;
        }
        position
    } else {
        let mut position = cursor.min(text.len());
        while position > 0 && !is_word(text[position - 1]) {
            position -= 1;
        }
        while position > 0 && is_word(text[position - 1]) {
            position -= 1;
        }
        position
    }
}
