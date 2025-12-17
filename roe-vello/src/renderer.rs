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

//! VelloRenderer implementing the roe_core Renderer trait.
//!
//! Note: The Renderer trait is designed for terminal-style incremental rendering.
//! For the Vello backend, we use an immediate-mode approach where we rebuild
//! the entire scene each frame (which is efficient with GPU rendering).
//! The dirty tracking is still useful to know when to request redraws.

use roe_core::renderer::{DirtyRegion, DirtyTracker, Renderer};
use roe_core::Editor;

use crate::theme::VelloTheme;

/// Vello-based renderer for the editor
///
/// This renderer builds a Vello Scene that can be rendered to a GPU surface.
/// Unlike the terminal renderer which writes directly to a device, this renderer
/// produces a Scene that the application event loop will render.
pub struct VelloRenderer {
    /// Dirty region tracking (for knowing when to redraw)
    dirty_tracker: DirtyTracker,
    /// The theme colors
    pub theme: VelloTheme,
    /// Whether a redraw is needed
    needs_redraw: bool,
}

impl Default for VelloRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl VelloRenderer {
    pub fn new() -> Self {
        Self {
            dirty_tracker: DirtyTracker::new(),
            theme: VelloTheme::default(),
            needs_redraw: true,
        }
    }

    pub fn with_theme(theme: VelloTheme) -> Self {
        Self {
            dirty_tracker: DirtyTracker::new(),
            theme,
            needs_redraw: true,
        }
    }

    /// Check if a redraw is needed
    pub fn needs_redraw(&self) -> bool {
        self.needs_redraw || self.dirty_tracker.is_full_screen_dirty()
    }

    /// Mark that a redraw has been performed
    pub fn redraw_complete(&mut self) {
        self.needs_redraw = false;
    }
}

impl Renderer for VelloRenderer {
    type Error = std::io::Error;

    fn mark_dirty(&mut self, region: DirtyRegion) {
        self.dirty_tracker.mark_dirty(region);
        self.needs_redraw = true;
    }

    fn render_incremental(&mut self, _editor: &Editor) -> Result<(), Self::Error> {
        // For Vello, we don't do incremental rendering in the traditional sense.
        // The scene is rebuilt each frame by the application.
        // This method just marks that we've processed the dirty state.
        self.needs_redraw = true;
        Ok(())
    }

    fn render_full(&mut self, _editor: &Editor) -> Result<(), Self::Error> {
        // Same as incremental - the actual rendering happens in the app event loop
        self.needs_redraw = true;
        Ok(())
    }

    fn clear_dirty(&mut self) {
        self.dirty_tracker.clear();
    }
}
