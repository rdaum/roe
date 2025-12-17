# Copyright (C) 2025 Ryan Daum <ryan.daum@gmail.com> This program is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License as published by the Free Software Foundation, version 3.
#
# This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details.
#
# You should have received a copy of the GNU General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
#
# Mode API (for scripted modes like file-selector, buffer-switcher)

# Store registered modes: name => (handler_function, state_dict)
# Handler receives (action_dict, state) and returns a result dict
const _modes = Dict{String, Tuple{Function, Dict{String, Any}}}()

# Mode action types (returned in the actions array)
struct ClearTextAction end

struct InsertTextModeAction
    text::String
    position::String  # "start", "end", "cursor"
end

struct OpenFileAction
    path::String
    open_type::String  # "new" or "visit"
end

struct ExecuteCommandAction
    command::String
end

struct CursorUpAction end
struct CursorDownAction end
struct CursorLeftAction end
struct CursorRightAction end

struct SwitchBufferAction
    buffer_index::Int  # 0-based index into the buffer list
end

struct KillBufferAction
    buffer_index::Int  # 0-based index into the buffer list
end

# Convert mode actions to dicts for Rust
function mode_action_to_dict(a::ClearTextAction)
    Dict("type" => "clear_text")
end

function mode_action_to_dict(a::InsertTextModeAction)
    Dict("type" => "insert_text", "text" => a.text, "position" => a.position)
end

function mode_action_to_dict(a::OpenFileAction)
    Dict("type" => "open_file", "path" => a.path, "open_type" => a.open_type)
end

function mode_action_to_dict(a::ExecuteCommandAction)
    Dict("type" => "execute_command", "command" => a.command)
end

function mode_action_to_dict(::CursorUpAction)
    Dict("type" => "cursor_up")
end

function mode_action_to_dict(::CursorDownAction)
    Dict("type" => "cursor_down")
end

function mode_action_to_dict(::CursorLeftAction)
    Dict("type" => "cursor_left")
end

function mode_action_to_dict(::CursorRightAction)
    Dict("type" => "cursor_right")
end

function mode_action_to_dict(a::SwitchBufferAction)
    Dict("type" => "switch_buffer", "buffer_index" => a.buffer_index)
end

function mode_action_to_dict(a::KillBufferAction)
    Dict("type" => "kill_buffer", "buffer_index" => a.buffer_index)
end

"""
    define_mode(name::String, handler::Function)

Register a mode that handles key events.

The handler receives `(action::Dict, state::Dict)` where:
- `action` has "type" (e.g., "alphanumeric", "cursor", "enter", "escape")
- `action` may have additional keys like "char", "direction"
- `state` is a mutable Dict that persists across calls

The handler should return a Dict with:
- `"result"` => "consumed", "annotated", or "ignored"
- `"actions"` => Vector of action dicts (optional)
"""
function define_mode(handler::Function, name::String)
    _modes[name] = (handler, Dict{String, Any}())
    return nothing
end

# Also support name-first syntax
function define_mode(name::String, handler::Function)
    define_mode(handler, name)
end

"""
    mode_perform(mode_name::String, action::Dict) -> Dict

Called by Rust to invoke a mode's key handler.
Returns a Dict with "result" and optionally "actions".
"""
function mode_perform(mode_name::String, action::Dict)
    if !haskey(_modes, mode_name)
        return Dict("result" => "ignored")
    end

    handler, state = _modes[mode_name]

    try
        result = handler(action, state)

        # Convert any mode action objects to dicts
        if haskey(result, "actions") && result["actions"] isa Vector
            result["actions"] = [
                a isa Dict ? a : mode_action_to_dict(a)
                for a in result["actions"]
            ]
        end

        return result
    catch e
        @error "Mode error" mode_name exception=(e, catch_backtrace())
        return Dict("result" => "ignored")
    end
end

"""
    has_mode(name::String) -> Bool

Check if a mode is registered.
"""
function has_mode(name::String)
    haskey(_modes, name)
end

"""
    reset_mode_state(name::String)

Reset a mode's state to empty.
"""
function reset_mode_state(name::String)
    if haskey(_modes, name)
        handler, _ = _modes[name]
        _modes[name] = (handler, Dict{String, Any}())
    end
end
