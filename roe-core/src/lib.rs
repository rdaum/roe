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
pub mod editor;
pub mod file_watcher;
pub mod gutter;
pub mod keys;
pub mod kill_ring;
pub mod mica_host;
pub mod native_kernel;
pub mod native_services;
pub mod renderer;
pub mod session;
pub mod undo;
pub mod window;

new_key_type! {
    pub struct WindowId;
}

new_key_type! {
    pub struct BufferId;
}

pub use buffer::Buffer;
pub use editor::{Editor, Frame, Window};
pub use gutter::{
    GutterConfig, GutterLine, LineStatus, calculate_gutter_width, format_line_number,
    get_line_status,
};
