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

use crate::{BufferId, WindowId};
use std::collections::{HashMap, HashSet};

/// Represents a dirty region in logical buffer coordinates
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirtyRegion {
    /// Single line needs redrawing
    Line { buffer_id: BufferId, line: usize },
    /// Range of lines need redrawing
    #[allow(dead_code)]
    LineRange {
        buffer_id: BufferId,
        start_line: usize,
        end_line: usize,
    },
    /// Specific character range (for highlighting, cursors, etc.)
    #[allow(dead_code)]
    CharRange {
        buffer_id: BufferId,
        start_char: usize,
        end_char: usize,
    },
    /// Entire buffer needs redrawing
    Buffer { buffer_id: BufferId },
    /// Window chrome (borders, modeline) needs redrawing
    #[allow(dead_code)]
    WindowChrome { window_id: WindowId },
    /// Specific modeline component needs updating
    Modeline {
        window_id: WindowId,
        component: ModelineComponent,
    },
    /// Entire screen needs redrawing (layout changes, etc.)
    FullScreen,
}

/// Components of the modeline that can be updated independently
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ModelineComponent {
    /// Cursor position (line:col)
    CursorPosition,
    /// Buffer name/object
    #[allow(dead_code)]
    BufferName,
    /// Mode name
    #[allow(dead_code)]
    ModeName,
    /// All components (equivalent to WindowChrome)
    All,
}

/// Represents a dirty span within a single line
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineSpan {
    pub start_col: usize,
    pub end_col: usize,
}

impl LineSpan {
    pub fn new(start_col: usize, end_col: usize) -> Self {
        Self { start_col, end_col }
    }

    pub fn full_line() -> Self {
        Self {
            start_col: 0,
            end_col: usize::MAX,
        }
    }

    /// Merge with another span, returning the combined span
    pub fn merge(&self, other: &LineSpan) -> LineSpan {
        LineSpan {
            start_col: self.start_col.min(other.start_col),
            end_col: self.end_col.max(other.end_col),
        }
    }

    /// Check if this span overlaps with another
    pub fn overlaps(&self, other: &LineSpan) -> bool {
        self.start_col <= other.end_col && other.start_col <= self.end_col
    }
}

/// Scanline-based dirty tracking for terminal rendering
#[derive(Debug, Clone)]
pub struct DirtyTracker {
    /// Dirty spans per line, indexed by line number
    /// None means line is clean, Some(span) means that span needs redrawing
    dirty_lines: Vec<Option<LineSpan>>,
    /// Buffers that need full redraw
    dirty_buffers: HashSet<BufferId>,
    /// Windows whose chrome needs redraw
    dirty_window_chrome: HashSet<WindowId>,
    /// Modeline components that need redraw, keyed by window_id
    dirty_modeline_components: HashMap<WindowId, HashSet<ModelineComponent>>,
    /// Whether full screen needs redraw
    full_screen_dirty: bool,
}

impl Default for DirtyTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl DirtyTracker {
    pub fn new() -> Self {
        Self {
            dirty_lines: Vec::new(),
            dirty_buffers: HashSet::new(),
            dirty_window_chrome: HashSet::new(),
            dirty_modeline_components: HashMap::new(),
            full_screen_dirty: false,
        }
    }

    /// Mark a region as dirty
    pub fn mark_dirty(&mut self, region: DirtyRegion) {
        match region {
            DirtyRegion::Line { buffer_id: _, line } => {
                self.ensure_line_capacity(line + 1);
                self.dirty_lines[line] = Some(LineSpan::full_line());
            }
            DirtyRegion::LineRange {
                buffer_id: _,
                start_line,
                end_line,
            } => {
                self.ensure_line_capacity(end_line + 1);
                for line in start_line..=end_line {
                    self.dirty_lines[line] = Some(LineSpan::full_line());
                }
            }
            DirtyRegion::CharRange {
                buffer_id,
                start_char: _,
                end_char: _,
            } => {
                // For now, mark the entire buffer dirty
                // TODO: Convert char range to line ranges
                self.dirty_buffers.insert(buffer_id);
            }
            DirtyRegion::Buffer { buffer_id } => {
                self.dirty_buffers.insert(buffer_id);
            }
            DirtyRegion::WindowChrome { window_id } => {
                self.dirty_window_chrome.insert(window_id);
            }
            DirtyRegion::Modeline {
                window_id,
                component,
            } => {
                self.dirty_modeline_components
                    .entry(window_id)
                    .or_default()
                    .insert(component);
            }
            DirtyRegion::FullScreen => {
                self.full_screen_dirty = true;
            }
        }
    }

    /// Mark a specific span within a line as dirty
    pub fn mark_line_span_dirty(&mut self, line: usize, span: LineSpan) {
        self.ensure_line_capacity(line + 1);

        match &self.dirty_lines[line] {
            None => {
                // Line is clean, mark this span as dirty
                self.dirty_lines[line] = Some(span);
            }
            Some(existing_span) => {
                // Line already has dirty span, merge them
                self.dirty_lines[line] = Some(existing_span.merge(&span));
            }
        }
    }

    /// Check if a line is dirty
    pub fn is_line_dirty(&self, line: usize) -> bool {
        if self.full_screen_dirty {
            return true;
        }

        if line < self.dirty_lines.len() {
            self.dirty_lines[line].is_some()
        } else {
            false
        }
    }

    /// Get the dirty span for a line, if any
    pub fn get_line_dirty_span(&self, line: usize) -> Option<&LineSpan> {
        if line < self.dirty_lines.len() {
            self.dirty_lines[line].as_ref()
        } else {
            None
        }
    }

    /// Check if a buffer is completely dirty
    pub fn is_buffer_dirty(&self, buffer_id: BufferId) -> bool {
        self.full_screen_dirty || self.dirty_buffers.contains(&buffer_id)
    }

    /// Check if window chrome is dirty
    pub fn is_window_chrome_dirty(&self, window_id: WindowId) -> bool {
        self.full_screen_dirty || self.dirty_window_chrome.contains(&window_id)
    }

    /// Check if specific modeline components are dirty
    pub fn is_modeline_component_dirty(
        &self,
        window_id: WindowId,
        component: &ModelineComponent,
    ) -> bool {
        if self.full_screen_dirty || self.dirty_window_chrome.contains(&window_id) {
            return true;
        }

        if let Some(dirty_components) = self.dirty_modeline_components.get(&window_id) {
            dirty_components.contains(component)
                || dirty_components.contains(&ModelineComponent::All)
        } else {
            false
        }
    }

    /// Get all dirty modeline components for a window
    pub fn get_dirty_modeline_components(
        &self,
        window_id: WindowId,
    ) -> Option<&HashSet<ModelineComponent>> {
        self.dirty_modeline_components.get(&window_id)
    }

    /// Check if full screen redraw is needed
    pub fn is_full_screen_dirty(&self) -> bool {
        self.full_screen_dirty
    }

    /// Clear all dirty state (call after rendering)
    pub fn clear(&mut self) {
        self.dirty_lines.clear();
        self.dirty_buffers.clear();
        self.dirty_window_chrome.clear();
        self.dirty_modeline_components.clear();
        self.full_screen_dirty = false;
    }

    /// Get all dirty lines as an iterator
    pub fn dirty_lines_iter(&self) -> impl Iterator<Item = (usize, &LineSpan)> {
        self.dirty_lines
            .iter()
            .enumerate()
            .filter_map(|(line_num, span)| span.as_ref().map(|s| (line_num, s)))
    }

    fn ensure_line_capacity(&mut self, needed_lines: usize) {
        if self.dirty_lines.len() < needed_lines {
            self.dirty_lines.resize(needed_lines, None);
        }
    }
}

/// Trait for pluggable rendering backends
pub trait Renderer {
    type Error;

    /// Mark a region as needing redraw
    fn mark_dirty(&mut self, region: DirtyRegion);

    /// Perform incremental rendering of dirty regions
    fn render_incremental(&mut self, editor: &crate::Editor) -> Result<(), Self::Error>;

    /// Force a full screen redraw
    fn render_full(&mut self, editor: &crate::Editor) -> Result<(), Self::Error>;

    /// Clear all dirty state (called after successful render)
    fn clear_dirty(&mut self);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BufferId, WindowId};
    use slotmap::SlotMap;

    fn test_buffer_id() -> BufferId {
        let mut buffers = SlotMap::with_key();
        buffers.insert(())
    }

    fn _test_window_id() -> WindowId {
        let mut windows = SlotMap::with_key();
        windows.insert(())
    }

    #[test]
    fn test_line_span_merge() {
        let span1 = LineSpan::new(5, 10);
        let span2 = LineSpan::new(8, 15);
        let merged = span1.merge(&span2);
        assert_eq!(merged, LineSpan::new(5, 15));
    }

    #[test]
    fn test_line_span_overlaps() {
        let span1 = LineSpan::new(5, 10);
        let span2 = LineSpan::new(8, 15);
        let span3 = LineSpan::new(20, 25);

        assert!(span1.overlaps(&span2));
        assert!(span2.overlaps(&span1));
        assert!(!span1.overlaps(&span3));
        assert!(!span3.overlaps(&span1));
    }

    #[test]
    fn test_dirty_tracker_line_marking() {
        let mut tracker = DirtyTracker::new();

        // Initially clean
        assert!(!tracker.is_line_dirty(0));
        assert!(!tracker.is_line_dirty(5));

        // Mark line 5 dirty
        tracker.mark_dirty(DirtyRegion::Line {
            buffer_id: test_buffer_id(),
            line: 5,
        });
        assert!(tracker.is_line_dirty(5));
        assert!(!tracker.is_line_dirty(4));
        assert!(!tracker.is_line_dirty(6));

        // Clear and check
        tracker.clear();
        assert!(!tracker.is_line_dirty(5));
    }

    #[test]
    fn test_dirty_tracker_line_range() {
        let mut tracker = DirtyTracker::new();

        tracker.mark_dirty(DirtyRegion::LineRange {
            buffer_id: test_buffer_id(),
            start_line: 2,
            end_line: 5,
        });

        assert!(!tracker.is_line_dirty(1));
        assert!(tracker.is_line_dirty(2));
        assert!(tracker.is_line_dirty(3));
        assert!(tracker.is_line_dirty(4));
        assert!(tracker.is_line_dirty(5));
        assert!(!tracker.is_line_dirty(6));
    }

    #[test]
    fn test_dirty_tracker_full_screen() {
        let mut tracker = DirtyTracker::new();

        tracker.mark_dirty(DirtyRegion::FullScreen);
        assert!(tracker.is_full_screen_dirty());
        assert!(tracker.is_line_dirty(0));
        assert!(tracker.is_line_dirty(100));
        assert!(tracker.is_buffer_dirty(test_buffer_id()));
    }

    #[test]
    fn test_line_span_merging_in_tracker() {
        let mut tracker = DirtyTracker::new();

        // Mark two overlapping spans on same line
        tracker.mark_line_span_dirty(3, LineSpan::new(5, 10));
        tracker.mark_line_span_dirty(3, LineSpan::new(8, 15));

        let span = tracker.get_line_dirty_span(3).unwrap();
        assert_eq!(span, &LineSpan::new(5, 15));
    }
}
