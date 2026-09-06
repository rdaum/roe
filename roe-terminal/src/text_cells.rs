// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-only

//! Terminal-only text realization. Native text positions remain character indices.

use std::collections::VecDeque;
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

const TAB_WIDTH: usize = 8;
const MAX_GRAPHEME_CHARS: usize = 256;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Glyph {
    pub text: String,
    pub characters: Range<usize>,
    pub cell: usize,
    pub width: usize,
}

#[derive(Debug)]
pub(crate) struct CellRow {
    pub glyphs: Vec<Glyph>,
    pub width: usize,
    pub truncated: bool,
    start: usize,
}

fn escaped_control(character: char) -> Option<String> {
    match character {
        '\0'..='\u{1f}' => Some(format!("^{}", char::from(character as u8 + b'@'))),
        '\u{7f}' => Some("^?".into()),
        character
            if character.is_control()
                || matches!(character, '\u{61c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}') =>
        {
            Some(format!("\\u{{{:x}}}", u32::from(character)))
        }
        _ => None,
    }
}

/// Stream graphemes without retaining the complete input or complete line.
fn glyphs(text: &str) -> impl Iterator<Item = Glyph> + '_ {
    let mut character = 0;
    let mut cell = 0usize;
    text.graphemes(true).map(move |grapheme| {
        let start = character;
        let count = grapheme.chars().count();
        character += count;
        let text = if count > MAX_GRAPHEME_CHARS {
            "�".into()
        } else if grapheme == "\t" {
            " ".repeat(TAB_WIDTH - cell % TAB_WIDTH)
        } else {
            let mut safe = String::new();
            for character in grapheme.chars() {
                match escaped_control(character) {
                    Some(escaped) => safe.push_str(&escaped),
                    None => safe.push(character),
                }
            }
            if safe.width() == 0 {
                safe.insert(0, '◌');
            }
            safe
        };
        let width = text.width();
        let glyph = Glyph {
            text,
            characters: start..character,
            cell,
            width,
        };
        cell = cell.saturating_add(width);
        glyph
    })
}

impl CellRow {
    /// Clip whole graphemes. Partial tabs become spaces; partial wide glyphs stay blank.
    pub fn new(text: &str, start: usize, columns: usize) -> Self {
        let mut row = Self {
            glyphs: Vec::new(),
            width: 0,
            truncated: false,
            start,
        };
        for mut glyph in glyphs(text) {
            if glyph.characters.end <= start {
                continue;
            }
            if row.width == columns {
                row.truncated = true;
                break;
            }
            let remaining = columns - row.width;
            if glyph.width > remaining {
                row.truncated = true;
                if !glyph.text.chars().all(|character| character == ' ') {
                    break;
                }
                glyph.text.truncate(remaining);
                glyph.width = remaining;
            }
            glyph.cell = row.width;
            row.width += glyph.width;
            row.glyphs.push(glyph);
            if row.truncated {
                break;
            }
        }
        row
    }

    pub fn text(&self) -> String {
        self.glyphs
            .iter()
            .map(|glyph| glyph.text.as_str())
            .collect()
    }

    pub fn character_at_cell(&self, cell: usize) -> usize {
        self.glyphs
            .iter()
            .find(|glyph| cell < glyph.cell + glyph.width)
            .map(|glyph| glyph.characters.start)
            .unwrap_or_else(|| {
                self.glyphs
                    .last()
                    .map_or(self.start, |glyph| glyph.characters.end)
            })
    }

    pub fn cell_at_character(&self, character: usize) -> Option<usize> {
        if character < self.start {
            return None;
        }
        for glyph in &self.glyphs {
            if character == glyph.characters.start {
                return Some(glyph.cell);
            }
            if character <= glyph.characters.end {
                return Some(glyph.cell + glyph.width);
            }
        }
        let end = self
            .glyphs
            .last()
            .map_or(self.start, |glyph| glyph.characters.end);
        (character == end).then_some(self.width)
    }
}

/// Find a character offset that leaves a visible terminal cell for the cursor.
pub(crate) fn start_for_cursor(
    text: &str,
    requested: usize,
    columns: usize,
    cursor: usize,
) -> usize {
    if cursor <= requested || columns == 0 {
        return cursor;
    }
    let mut start = requested;
    let mut width = 0usize;
    let mut retained = VecDeque::new();
    for glyph in glyphs(text) {
        if glyph.characters.end <= requested {
            continue;
        }
        if glyph.characters.start >= cursor {
            break;
        }
        width = width.saturating_add(glyph.width);
        retained.push_back((glyph.characters.end, glyph.width));
        while width >= columns {
            let Some((end, count)) = retained.pop_front() else {
                break;
            };
            start = end;
            width -= count;
        }
    }
    start
}

/// Safe one-line text with cell-based ellipsis and padding.
pub(crate) fn fitted(text: &str, columns: usize, fill: char) -> String {
    let row = CellRow::new(text, 0, columns);
    let (mut output, width) = if row.truncated {
        let dots = columns.min(3);
        let clipped = CellRow::new(text, 0, columns - dots);
        let mut output = clipped.text();
        output.extend(std::iter::repeat_n('.', dots));
        (output, clipped.width + dots)
    } else {
        (row.text(), row.width)
    };
    output.extend(std::iter::repeat_n(fill, columns.saturating_sub(width)));
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controls_never_reach_terminal_output() {
        let row = CellRow::new("a\x1b[2J\r\x07\u{85}\u{202e}z", 0, 80);
        let text = row.text();
        assert_eq!(text, "a^[[2J^M^G\\u{85}\\u{202e}z");
        assert!(!text.chars().any(char::is_control));
    }

    #[test]
    fn widths_and_maps_cover_tabs_wide_and_combining_text() {
        let row = CellRow::new("a\t界e\u{301}", 0, 20);
        assert_eq!(row.width, 11);
        assert_eq!(row.text(), "a       界e\u{301}");
        assert_eq!(row.cell_at_character(2), Some(8));
        assert_eq!(row.character_at_cell(7), 1);
        assert_eq!(row.character_at_cell(9), 2);
        assert_eq!(row.character_at_cell(10), 3);
        assert_eq!(row.cell_at_character(5), Some(11));
    }

    #[test]
    fn clipping_keeps_wide_graphemes_whole_and_tabs_inside_the_row() {
        let row = CellRow::new("a界z", 0, 2);
        assert_eq!(row.text(), "a");
        assert!(row.truncated);
        let tab = CellRow::new("\tx", 0, 3);
        assert_eq!(tab.text(), "   ");
        assert_eq!(tab.width, 3);
        assert!(tab.truncated);
        let shifted = CellRow::new("a\t界", 2, 2);
        assert_eq!(shifted.text(), "界");
        assert_eq!(shifted.character_at_cell(1), 2);
    }

    #[test]
    fn isolated_combining_marks_have_a_visible_base() {
        assert_eq!(CellRow::new("\u{301}", 0, 4).text(), "◌\u{301}");
    }

    #[test]
    fn fitted_text_counts_cells_and_escapes_every_surface() {
        assert_eq!(fitted("界", 4, ' '), "界  ");
        assert_eq!(fitted("界界界", 5, ' '), "界...");
        assert_eq!(fitted("\x1b", 4, '─'), "^[──");
        assert_eq!(fitted("abcdef", 2, ' '), "..");
        assert_eq!(fitted("x", 0, ' '), "");
    }

    #[test]
    fn cursor_scroll_uses_cell_width_without_truncating_character_indices() {
        assert_eq!(start_for_cursor("界界界界", 0, 5, 4), 2);
        let text = "a".repeat(70_000);
        assert_eq!(start_for_cursor(&text, 65_536, 80, 70_000), 69_921);
        let row = CellRow::new(&text, 69_921, 80);
        assert_eq!(row.cell_at_character(70_000), Some(79));
    }
}
