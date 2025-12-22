# Copyright (C) 2025 Ryan Daum <ryan.daum@gmail.com> This program is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License as published by the Free Software Foundation, version 3.
#
# This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details.
#
# You should have received a copy of the GNU General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
#
# Roe Editor Julia API
# This module provides the interface for extending Roe from Julia

# Activate the project environment so packages can be found
# This is needed because jlrs doesn't automatically use --project
import Pkg
const _project_dir = dirname(@__DIR__)
if isfile(joinpath(_project_dir, "Project.toml"))
    Pkg.activate(_project_dir; io=devnull)
end

module Roe

export define_command, call_command, CommandContext, define_key, define_keys, undefine_key,
       # Action types
       EchoAction, NoAction, InsertAction, DeleteAction, ReplaceAction,
       SetCursorAction, SetMarkAction, ClearMarkAction, SetContentAction, IndentLineAction,
       ExecuteCommandAction,
       # Buffer access functions
       buffer_content, buffer_line, buffer_line_count, buffer_char_count,
       buffer_substring, buffer_insert!, buffer_delete!, buffer_major_mode,
       # Indentation registration
       register_indent_command, register_newline_indent_command,
       # Minor mode API (key handlers)
       define_mode, mode_perform, has_mode, reset_mode_state,
       ClearTextAction, InsertTextModeAction, OpenFileAction, ExecuteCommandAction,
       CursorUpAction, CursorDownAction, CursorLeftAction, CursorRightAction,
       SwitchBufferAction, KillBufferAction,
       # Major mode API (file type associations)
       define_major_mode, get_major_mode_for_file, call_major_mode_init,
       call_major_mode_after_change, has_major_mode, list_major_modes,
       get_major_mode_extensions, set_default_major_mode,
       # Syntax highlighting API
       define_face, face_exists, add_span, add_spans, clear_spans,
       clear_spans_in_range, has_spans, define_standard_faces,
       Span, highlight_matches, apply_spans,
       # Julia syntax highlighting
       define_julia_faces, highlight_julia, highlight_julia_buffer,
       highlight_julia_region,
       # Rust syntax highlighting
       define_rust_faces, highlight_rust, highlight_rust_buffer,
       highlight_rust_region

# Get the directory containing this file
const _module_dir = @__DIR__

# Include sub-modules in dependency order
include(joinpath(_module_dir, "buffer_api.jl"))
include(joinpath(_module_dir, "syntax.jl"))
include(joinpath(_module_dir, "commands.jl"))
include(joinpath(_module_dir, "keybindings.jl"))
include(joinpath(_module_dir, "modes.jl"))
include(joinpath(_module_dir, "file_selector.jl"))
include(joinpath(_module_dir, "buffer_switcher.jl"))
# Major mode system - defines define_major_mode() etc.
include(joinpath(_module_dir, "major_modes.jl"))
# Julia highlighting depends on commands.jl and major_modes.jl
include(joinpath(_module_dir, "julia_highlighting.jl"))
# Rust highlighting uses TreeSitter.jl
include(joinpath(_module_dir, "rust_highlighting.jl"))

end # module Roe
