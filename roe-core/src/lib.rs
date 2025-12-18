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

use slotmap::new_key_type;

pub mod buffer;
pub mod buffer_host;
pub mod buffer_switch_mode;
pub mod command_mode;
pub mod command_registry;
pub mod editor;
pub mod file_selector_mode;
pub mod julia_runtime;
pub mod keys;
pub mod kill_ring;
pub mod mode;
pub mod renderer;
pub mod scripted_mode;
pub mod selection_menu;
pub mod syntax;
pub mod window;

new_key_type! {
    pub struct WindowId;
}

new_key_type! {
    pub struct BufferId;
}

new_key_type! {
    pub struct ModeId;
}

pub use buffer::Buffer;
pub use editor::{Editor, Frame, Window};
pub use keys::{Bindings, ConfigurableBindings, KeyState};
pub use mode::{FileMode, Mode};
pub use renderer::Renderer;
pub use syntax::{Color, Face, FaceId, FaceRegistry, HighlightSpan, SpanStore};
