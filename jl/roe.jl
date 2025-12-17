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
