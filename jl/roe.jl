# Copyright (C) 2025 Ryan Daum <ryan.daum@gmail.com> This program is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License as published by the Free Software Foundation, version 3.
#
# This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details.
#
# You should have received a copy of the GNU General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
#
# Roe Editor Julia API
# This module provides the interface for extending Roe from Julia

module Roe

export define_command, call_command, CommandContext, define_key, define_keys, undefine_key,
       # Action types
       EchoAction, NoAction, InsertAction, DeleteAction, ReplaceAction,
       SetCursorAction, SetMarkAction, ClearMarkAction, SetContentAction,
       # Buffer access functions
       buffer_content, buffer_line, buffer_line_count, buffer_char_count,
       buffer_substring, buffer_insert!, buffer_delete!,
       # Mode API
       define_mode, mode_perform, has_mode, reset_mode_state,
       ClearTextAction, InsertTextModeAction, OpenFileAction, ExecuteCommandAction,
       CursorUpAction, CursorDownAction, CursorLeftAction, CursorRightAction,
       SwitchBufferAction, KillBufferAction

# Get the directory containing this file
const _module_dir = @__DIR__

# Include sub-modules in dependency order
include(joinpath(_module_dir, "buffer_api.jl"))
include(joinpath(_module_dir, "commands.jl"))
include(joinpath(_module_dir, "keybindings.jl"))
include(joinpath(_module_dir, "modes.jl"))
include(joinpath(_module_dir, "file_selector.jl"))
include(joinpath(_module_dir, "buffer_switcher.jl"))

end # module Roe
