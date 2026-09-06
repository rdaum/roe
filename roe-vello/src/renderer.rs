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

use roe_core::renderer::{PresentationStreamError, PresentationStreamState};
use roe_core::session::PresentationUpdate;

use crate::theme::VelloTheme;

/// Vello-based renderer for the editor
///
/// This renderer builds a Vello Scene that can be rendered to a GPU surface.
/// Unlike the terminal renderer which writes directly to a device, this renderer
/// produces a Scene that the application event loop will render.
pub struct VelloRenderer {
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
            theme,
            needs_redraw: true,
            session_presentation: PresentationStreamState::default(),
        }
    }

    /// Check if a redraw is needed
    pub fn needs_redraw(&self) -> bool {
        self.needs_redraw
    }

    /// Invalidate part of the production Vello presentation.
    pub fn invalidate(&mut self) {
        self.needs_redraw = true;
    }

    /// Mark that a redraw has been performed
    pub fn redraw_complete(&mut self) {
        self.needs_redraw = false;
    }
}

impl roe_core::frontend::PresentationConsumer for VelloRenderer {
    fn accept_presentation(
        &mut self,
        update: &PresentationUpdate,
    ) -> Result<(), PresentationStreamError> {
        self.apply_session_presentation(update)?;
        self.invalidate();
        Ok(())
    }
}
