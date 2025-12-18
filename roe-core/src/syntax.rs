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

//! Syntax highlighting infrastructure.
//!
//! This module provides the core types for syntax highlighting:
//! - `Face`: A named style (colors, bold, italic, etc.) - like Emacs faces
//! - `FaceId`: Unique identifier for a face
//! - `HighlightSpan`: A range in the buffer with an associated face
//! - `SpanStore`: Collection of spans that auto-adjusts when the buffer is edited
//!
//! The design follows Emacs' interval tree approach: spans are stored separately
//! from the text, but the SpanStore automatically adjusts span positions when
//! text is inserted or deleted.
//!
//! Highlighters (Rust-native or Julia-defined) produce spans which are stored
//! in the SpanStore. Renderers query the SpanStore when drawing to get the
//! appropriate face for each character.

use slotmap::{new_key_type, SlotMap};
use std::collections::HashMap;
use std::ops::Range;

new_key_type! {
    /// Unique identifier for a Face
    pub struct FaceId;
}

/// A color specification that can be used for foreground or background.
#[derive(Debug, Clone, PartialEq)]
pub enum Color {
    /// RGB color (0-255 for each component)
    Rgb { r: u8, g: u8, b: u8 },
    /// Named color (resolved at render time)
    Named(String),
    /// Inherit from default/parent
    Inherit,
}

impl Color {
    /// Parse a hex color string like "#ff0000" or "#f00"
    pub fn from_hex(hex: &str) -> Option<Self> {
        let hex = hex.trim_start_matches('#');
        match hex.len() {
            6 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                Some(Color::Rgb { r, g, b })
            }
            3 => {
                let r = u8::from_str_radix(&hex[0..1], 16).ok()? * 17;
                let g = u8::from_str_radix(&hex[1..2], 16).ok()? * 17;
                let b = u8::from_str_radix(&hex[2..3], 16).ok()? * 17;
                Some(Color::Rgb { r, g, b })
            }
            _ => None,
        }
    }

    pub fn rgb(r: u8, g: u8, b: u8) -> Self {
        Color::Rgb { r, g, b }
    }

    pub fn named(name: &str) -> Self {
        Color::Named(name.to_string())
    }
}

/// A Face defines a visual style for text.
/// Named after Emacs faces - a face is a collection of visual attributes.
#[derive(Debug, Clone)]
pub struct Face {
    /// Human-readable name for the face (e.g., "font-lock-keyword-face")
    pub name: String,
    /// Foreground color (None = inherit)
    pub foreground: Option<Color>,
    /// Background color (None = inherit)
    pub background: Option<Color>,
    /// Bold text
    pub bold: bool,
    /// Italic text
    pub italic: bool,
    /// Underline text
    pub underline: bool,
    /// Strikethrough text
    pub strikethrough: bool,
}

impl Face {
    /// Create a new face with the given name and default attributes
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            foreground: None,
            background: None,
            bold: false,
            italic: false,
            underline: false,
            strikethrough: false,
        }
    }

    /// Builder method to set foreground color
    pub fn with_foreground(mut self, color: Color) -> Self {
        self.foreground = Some(color);
        self
    }

    /// Builder method to set background color
    pub fn with_background(mut self, color: Color) -> Self {
        self.background = Some(color);
        self
    }

    /// Builder method to set bold
    pub fn with_bold(mut self, bold: bool) -> Self {
        self.bold = bold;
        self
    }

    /// Builder method to set italic
    pub fn with_italic(mut self, italic: bool) -> Self {
        self.italic = italic;
        self
    }

    /// Builder method to set underline
    pub fn with_underline(mut self, underline: bool) -> Self {
        self.underline = underline;
        self
    }
}

/// Registry of all defined faces.
/// Faces are defined globally and referenced by FaceId in spans.
#[derive(Debug, Default)]
pub struct FaceRegistry {
    faces: SlotMap<FaceId, Face>,
    /// Map from face name to FaceId for lookup
    name_to_id: HashMap<String, FaceId>,
}

impl FaceRegistry {
    pub fn new() -> Self {
        let mut registry = Self::default();
        // Register default faces
        registry.register_default_faces();
        registry
    }

    /// Register the default faces used for syntax highlighting
    fn register_default_faces(&mut self) {
        // Keywords (if, else, for, while, fn, etc.)
        self.define_face(
            Face::new("keyword")
                .with_foreground(Color::from_hex("#569cd6").unwrap())
                .with_bold(true),
        );

        // Strings
        self.define_face(Face::new("string").with_foreground(Color::from_hex("#ce9178").unwrap()));

        // Comments
        self.define_face(
            Face::new("comment")
                .with_foreground(Color::from_hex("#6a9955").unwrap())
                .with_italic(true),
        );

        // Function names
        self.define_face(
            Face::new("function").with_foreground(Color::from_hex("#dcdcaa").unwrap()),
        );

        // Types
        self.define_face(Face::new("type").with_foreground(Color::from_hex("#4ec9b0").unwrap()));

        // Constants/numbers
        self.define_face(
            Face::new("constant").with_foreground(Color::from_hex("#b5cea8").unwrap()),
        );

        // Variables
        self.define_face(
            Face::new("variable").with_foreground(Color::from_hex("#9cdcfe").unwrap()),
        );

        // Operators
        self.define_face(
            Face::new("operator").with_foreground(Color::from_hex("#d4d4d4").unwrap()),
        );

        // Punctuation
        self.define_face(
            Face::new("punctuation").with_foreground(Color::from_hex("#d4d4d4").unwrap()),
        );

        // Error highlighting
        self.define_face(
            Face::new("error")
                .with_foreground(Color::from_hex("#f44747").unwrap())
                .with_underline(true),
        );

        // Warning highlighting
        self.define_face(
            Face::new("warning")
                .with_foreground(Color::from_hex("#cca700").unwrap())
                .with_underline(true),
        );
    }

    /// Define a new face and return its ID
    pub fn define_face(&mut self, face: Face) -> FaceId {
        let name = face.name.clone();
        let id = self.faces.insert(face);
        self.name_to_id.insert(name, id);
        id
    }

    /// Look up a face by ID
    pub fn get(&self, id: FaceId) -> Option<&Face> {
        self.faces.get(id)
    }

    /// Look up a face ID by name
    pub fn get_id(&self, name: &str) -> Option<FaceId> {
        self.name_to_id.get(name).copied()
    }

    /// Look up a face by name
    pub fn get_by_name(&self, name: &str) -> Option<&Face> {
        self.get_id(name).and_then(|id| self.get(id))
    }

    /// Update an existing face
    pub fn update_face(&mut self, id: FaceId, face: Face) -> bool {
        if let Some(existing) = self.faces.get_mut(id) {
            // Update name mapping if name changed
            if existing.name != face.name {
                self.name_to_id.remove(&existing.name);
                self.name_to_id.insert(face.name.clone(), id);
            }
            *existing = face;
            true
        } else {
            false
        }
    }

    /// Iterate over all faces
    pub fn iter(&self) -> impl Iterator<Item = (FaceId, &Face)> {
        self.faces.iter()
    }
}

/// A span of highlighted text in a buffer.
/// Spans use character offsets (not byte offsets) to match ropey's indexing.
#[derive(Debug, Clone, PartialEq)]
pub struct HighlightSpan {
    /// Start position (character offset, inclusive)
    pub start: usize,
    /// End position (character offset, exclusive)
    pub end: usize,
    /// Face to apply to this span
    pub face_id: FaceId,
}

impl HighlightSpan {
    pub fn new(start: usize, end: usize, face_id: FaceId) -> Self {
        Self {
            start,
            end,
            face_id,
        }
    }

    /// Check if this span overlaps with a range
    pub fn overlaps(&self, range: &Range<usize>) -> bool {
        self.start < range.end && self.end > range.start
    }

    /// Check if this span contains a position
    pub fn contains(&self, pos: usize) -> bool {
        pos >= self.start && pos < self.end
    }

    /// Get the length of this span
    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    /// Check if span is empty
    pub fn is_empty(&self) -> bool {
        self.start >= self.end
    }
}

/// Storage for highlight spans with automatic adjustment on edits.
///
/// This is a simplified interval store that maintains spans and adjusts
/// them when the underlying buffer is edited. It follows the Emacs model
/// where spans automatically shift/grow/shrink as text is modified.
#[derive(Debug, Default, Clone)]
pub struct SpanStore {
    /// Spans sorted by start position
    spans: Vec<HighlightSpan>,
    /// Whether spans are currently sorted (optimization)
    sorted: bool,
}

impl SpanStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a new span to the store
    pub fn add_span(&mut self, span: HighlightSpan) {
        if !span.is_empty() {
            self.spans.push(span);
            self.sorted = false;
        }
    }

    /// Add multiple spans at once
    pub fn add_spans(&mut self, spans: impl IntoIterator<Item = HighlightSpan>) {
        for span in spans {
            if !span.is_empty() {
                self.spans.push(span);
            }
        }
        self.sorted = false;
    }

    /// Clear all spans
    pub fn clear(&mut self) {
        self.spans.clear();
        self.sorted = true;
    }

    /// Clear spans in a specific range (for incremental re-highlighting)
    pub fn clear_range(&mut self, range: Range<usize>) {
        self.spans.retain(|span| !span.overlaps(&range));
    }

    /// Ensure spans are sorted by start position
    fn ensure_sorted(&mut self) {
        if !self.sorted {
            self.spans.sort_by_key(|s| s.start);
            self.sorted = true;
        }
    }

    /// Get all spans that overlap with the given range
    pub fn spans_in_range(&mut self, range: Range<usize>) -> Vec<&HighlightSpan> {
        self.ensure_sorted();
        self.spans
            .iter()
            .filter(|span| span.overlaps(&range))
            .collect()
    }

    /// Get all spans (sorted by start position)
    pub fn all_spans(&mut self) -> &[HighlightSpan] {
        self.ensure_sorted();
        &self.spans
    }

    /// Get the face at a specific position (returns the last matching face)
    pub fn face_at(&mut self, pos: usize) -> Option<FaceId> {
        self.ensure_sorted();
        // Return the last face that contains this position
        // (later spans override earlier ones)
        self.spans
            .iter()
            .rev()
            .find(|span| span.contains(pos))
            .map(|span| span.face_id)
    }

    /// Adjust spans after an insertion at `pos` with length `len`.
    ///
    /// Spans that:
    /// - End before pos: unchanged
    /// - Start after pos: shift right by len
    /// - Contain pos: extend by len (end moves right)
    pub fn adjust_for_insert(&mut self, pos: usize, len: usize) {
        if len == 0 {
            return;
        }

        for span in &mut self.spans {
            if span.start >= pos {
                // Span starts at or after insertion point - shift it
                span.start += len;
                span.end += len;
            } else if span.end > pos {
                // Span contains the insertion point - extend it
                span.end += len;
            }
            // Spans ending before pos are unchanged
        }
    }

    /// Adjust spans after a deletion from `start` to `end`.
    ///
    /// Spans that:
    /// - End before start: unchanged
    /// - Start after end: shift left by (end - start)
    /// - Overlap with deleted range: shrink or remove
    pub fn adjust_for_delete(&mut self, start: usize, end: usize) {
        if start >= end {
            return;
        }

        let delete_len = end - start;

        self.spans.retain_mut(|span| {
            if span.end <= start {
                // Span is entirely before deletion - unchanged
                true
            } else if span.start >= end {
                // Span is entirely after deletion - shift left
                span.start -= delete_len;
                span.end -= delete_len;
                true
            } else if span.start >= start && span.end <= end {
                // Span is entirely within deletion - remove it
                false
            } else if span.start < start && span.end > end {
                // Deletion is entirely within span - shrink it
                span.end -= delete_len;
                true
            } else if span.start < start {
                // Span overlaps start of deletion - truncate end
                span.end = start;
                !span.is_empty()
            } else {
                // Span overlaps end of deletion - truncate start
                span.start = start;
                span.end -= end - span.start.max(start);
                span.end = span.end.max(span.start);
                !span.is_empty()
            }
        });
    }

    /// Number of spans in the store
    pub fn len(&self) -> usize {
        self.spans.len()
    }

    /// Check if store is empty
    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_color_from_hex() {
        assert_eq!(
            Color::from_hex("#ff0000"),
            Some(Color::Rgb { r: 255, g: 0, b: 0 })
        );
        assert_eq!(
            Color::from_hex("#00ff00"),
            Some(Color::Rgb { r: 0, g: 255, b: 0 })
        );
        assert_eq!(
            Color::from_hex("0000ff"),
            Some(Color::Rgb { r: 0, g: 0, b: 255 })
        );
        assert_eq!(
            Color::from_hex("#f00"),
            Some(Color::Rgb { r: 255, g: 0, b: 0 })
        );
    }

    #[test]
    fn test_face_registry() {
        let mut registry = FaceRegistry::new();

        // Default faces should be registered
        assert!(registry.get_id("keyword").is_some());
        assert!(registry.get_id("string").is_some());
        assert!(registry.get_id("comment").is_some());

        // Custom face
        let custom_id = registry
            .define_face(Face::new("my-custom-face").with_foreground(Color::rgb(100, 100, 100)));

        assert_eq!(registry.get_id("my-custom-face"), Some(custom_id));
        let face = registry.get(custom_id).unwrap();
        assert_eq!(face.name, "my-custom-face");
    }

    #[test]
    fn test_span_store_basic() {
        let mut store = SpanStore::new();
        let face_id = FaceId::default();

        store.add_span(HighlightSpan::new(0, 10, face_id));
        store.add_span(HighlightSpan::new(15, 20, face_id));

        assert_eq!(store.len(), 2);

        let spans = store.spans_in_range(5..17);
        assert_eq!(spans.len(), 2);

        let spans = store.spans_in_range(0..5);
        assert_eq!(spans.len(), 1);
    }

    #[test]
    fn test_span_store_insert_adjustment() {
        let mut store = SpanStore::new();
        let face_id = FaceId::default();

        // Span from 10-20
        store.add_span(HighlightSpan::new(10, 20, face_id));

        // Insert 5 chars at position 5 (before span)
        store.adjust_for_insert(5, 5);
        let spans = store.all_spans();
        assert_eq!(spans[0].start, 15); // Shifted right by 5
        assert_eq!(spans[0].end, 25);
    }

    #[test]
    fn test_span_store_insert_within_span() {
        let mut store = SpanStore::new();
        let face_id = FaceId::default();

        // Span from 10-20
        store.add_span(HighlightSpan::new(10, 20, face_id));

        // Insert 5 chars at position 15 (within span)
        store.adjust_for_insert(15, 5);
        let spans = store.all_spans();
        assert_eq!(spans[0].start, 10); // Start unchanged
        assert_eq!(spans[0].end, 25); // End extended
    }

    #[test]
    fn test_span_store_delete_adjustment() {
        let mut store = SpanStore::new();
        let face_id = FaceId::default();

        // Span from 10-20
        store.add_span(HighlightSpan::new(10, 20, face_id));

        // Delete from 0-5 (before span)
        store.adjust_for_delete(0, 5);
        let spans = store.all_spans();
        assert_eq!(spans[0].start, 5); // Shifted left by 5
        assert_eq!(spans[0].end, 15);
    }

    #[test]
    fn test_span_store_delete_within_span() {
        let mut store = SpanStore::new();
        let face_id = FaceId::default();

        // Span from 10-20
        store.add_span(HighlightSpan::new(10, 20, face_id));

        // Delete from 12-15 (within span)
        store.adjust_for_delete(12, 15);
        let spans = store.all_spans();
        assert_eq!(spans[0].start, 10); // Start unchanged
        assert_eq!(spans[0].end, 17); // End shrunk by 3
    }

    #[test]
    fn test_span_store_delete_removes_span() {
        let mut store = SpanStore::new();
        let face_id = FaceId::default();

        // Span from 10-20
        store.add_span(HighlightSpan::new(10, 20, face_id));

        // Delete from 5-25 (encompasses entire span)
        store.adjust_for_delete(5, 25);
        assert!(store.is_empty());
    }
}
