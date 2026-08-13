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

//! Vello redraw state and revisioned session-presentation gate.

use roe_core::renderer::{
    DirtyRegion, DirtyTracker, PresentationStreamError, PresentationStreamState,
};
use roe_core::session::PresentationUpdate;

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
    session_presentation: PresentationStreamState,
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
            session_presentation: PresentationStreamState::default(),
        }
    }

    pub fn apply_session_presentation(
        &mut self,
        update: &PresentationUpdate,
    ) -> Result<(), PresentationStreamError> {
        self.session_presentation.apply(update)
    }

    pub fn session_presentation(&self) -> &PresentationStreamState {
        &self.session_presentation
    }

    pub fn with_theme(theme: VelloTheme) -> Self {
        Self {
            dirty_tracker: DirtyTracker::new(),
            theme,
            needs_redraw: true,
            session_presentation: PresentationStreamState::default(),
        }
    }

    /// Check if a redraw is needed
    pub fn needs_redraw(&self) -> bool {
        self.needs_redraw || self.dirty_tracker.is_dirty()
    }

    /// Invalidate part of the production Vello presentation.
    pub fn invalidate(&mut self, region: DirtyRegion) {
        self.dirty_tracker.mark_dirty(region);
        self.needs_redraw = true;
    }

    /// Mark that a redraw has been performed
    pub fn redraw_complete(&mut self) {
        self.dirty_tracker.clear();
        self.needs_redraw = false;
    }
}
