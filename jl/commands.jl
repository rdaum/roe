# Copyright (C) 2025 Ryan Daum <ryan.daum@gmail.com> This program is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License as published by the Free Software Foundation, version 3.
#
# This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details.
#
# You should have received a copy of the GNU General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
#
# Command API and built-in commands

using Dates

# Store registered commands: name => (description, function)
const _commands = Dict{String, Tuple{String, Function}}()

# Command context passed from Rust
# Contains lightweight metadata - buffer content accessed via API functions
struct CommandContext
    buffer_name::String
    buffer_modified::Bool
    cursor_pos::Int
    current_line::Int
    current_column::Int
    line_count::Int
    char_count::Int
    mark_pos::Union{Int, Nothing}  # Mark position (nothing if not set)
end

# Convert a Dict (from Rust) to CommandContext
function CommandContext(d::Dict)
    mark = get(d, "mark_pos", nothing)
    # Convert -1 to nothing for mark_pos
    mark_pos = (mark === nothing || mark < 0) ? nothing : mark

    CommandContext(
        get(d, "buffer_name", ""),
        get(d, "buffer_modified", false),
        get(d, "cursor_pos", 0),
        get(d, "current_line", 0),
        get(d, "current_column", 0),
        get(d, "line_count", 0),
        get(d, "char_count", 0),
        mark_pos
    )
end

# Action types that can be returned to Rust
abstract type Action end

struct EchoAction <: Action
    message::String
end

struct NoAction <: Action end

# Buffer manipulation actions
struct InsertAction <: Action
    pos::Int
    text::String
end

struct DeleteAction <: Action
    start_pos::Int
    end_pos::Int
end

struct ReplaceAction <: Action
    start_pos::Int
    end_pos::Int
    text::String
end

struct SetCursorAction <: Action
    pos::Int
end

struct SetMarkAction <: Action
    pos::Int
end

struct ClearMarkAction <: Action end

struct SetContentAction <: Action
    content::String
end

# Convert action to Dict for Rust consumption
function action_to_dict(a::EchoAction)
    Dict("type" => "echo", "message" => a.message)
end

function action_to_dict(a::NoAction)
    Dict("type" => "none")
end

function action_to_dict(::Nothing)
    Dict("type" => "none")
end

function action_to_dict(a::InsertAction)
    Dict("type" => "insert", "pos" => a.pos, "text" => a.text)
end

function action_to_dict(a::DeleteAction)
    Dict("type" => "delete", "start" => a.start_pos, "end" => a.end_pos)
end

function action_to_dict(a::ReplaceAction)
    Dict("type" => "replace", "start" => a.start_pos, "end" => a.end_pos, "text" => a.text)
end

function action_to_dict(a::SetCursorAction)
    Dict("type" => "set_cursor", "pos" => a.pos)
end

function action_to_dict(a::SetMarkAction)
    Dict("type" => "set_mark", "pos" => a.pos)
end

function action_to_dict(a::ClearMarkAction)
    Dict("type" => "clear_mark")
end

function action_to_dict(a::SetContentAction)
    Dict("type" => "set_content", "content" => a.content)
end

"""
    define_command(name::String, description::String, func::Function)

Register a command that can be invoked from the editor.

The function receives a `CommandContext` and should return an `Action` or `nothing`.

# Example
```julia
define_command("my-greeting", "Say hello") do ctx
    EchoAction("Hello from Julia! Buffer: \$(ctx.buffer_name)")
end
```
"""
function define_command(name::String, description::String, func::Function)
    _commands[name] = (description, func)
    return nothing
end

# Version for do-block syntax (func comes first with do ... end)
function define_command(func::Function, name::String, description::String)
    define_command(name, description, func)
end

# Convenience macro for defining commands
macro defcmd(name, description, body)
    quote
        define_command($(esc(name)), $(esc(description)), $(esc(body)))
    end
end

"""
    call_command(name::String, context::Dict) -> Dict

Called by Rust to invoke a registered command.
Returns a Dict describing the action to take.
"""
function call_command(name::String, context::Dict)
    if !haskey(_commands, name)
        return Dict("type" => "error", "message" => "Unknown command: $name")
    end

    desc, func = _commands[name]
    ctx = CommandContext(context)

    try
        result = func(ctx)
        return action_to_dict(result)
    catch e
        return Dict("type" => "error", "message" => "Command error: $(sprint(showerror, e))")
    end
end

"""
    list_commands() -> Vector{Tuple{String, String}}

Return list of (name, description) for all registered Julia commands.
"""
function list_commands()
    [(name, desc) for (name, (desc, _)) in _commands]
end

"""
    has_command(name::String) -> Bool

Check if a command is registered.
"""
function has_command(name::String)
    haskey(_commands, name)
end

# ============================================
# Built-in commands
# ============================================

define_command("describe-buffer-jl", "Describe current buffer (Julia)") do ctx
    modified = ctx.buffer_modified ? " [modified]" : ""
    EchoAction("$(ctx.buffer_name)$(modified) - Line $(ctx.current_line), Col $(ctx.current_column) ($(ctx.line_count) lines, $(ctx.char_count) chars)")
end

define_command("hello-julia", "Test Julia command integration") do ctx
    EchoAction("Hello from Julia! You're editing $(ctx.buffer_name)")
end

define_command("cursor-info", "Show cursor position info") do ctx
    EchoAction("Position: $(ctx.cursor_pos), Line: $(ctx.current_line), Column: $(ctx.current_column)")
end

define_command("insert-timestamp", "Insert current timestamp at cursor") do ctx
    timestamp = Dates.format(Dates.now(), "yyyy-mm-dd HH:MM:SS")
    InsertAction(ctx.cursor_pos, timestamp)
end

define_command("insert-hello", "Insert 'Hello, World!' at cursor") do ctx
    InsertAction(ctx.cursor_pos, "Hello, World!")
end

define_command("goto-start", "Move cursor to start of buffer") do ctx
    SetCursorAction(0)
end

define_command("goto-end", "Move cursor to end of buffer") do ctx
    SetCursorAction(ctx.char_count)
end

define_command("set-mark-here", "Set mark at cursor position") do ctx
    SetMarkAction(ctx.cursor_pos)
end

define_command("clear-mark-jl", "Clear mark (Julia version)") do ctx
    ClearMarkAction()
end

# Buffer access test commands (using FFI)

define_command("buffer-test-read", "Test direct buffer read via FFI") do ctx
    # Test the ccall-based buffer access
    content = buffer_content()
    lines = buffer_line_count()
    chars = buffer_char_count()

    # Get first line if buffer is not empty
    first_line = lines > 0 ? buffer_line(0) : "<empty>"
    first_line_preview = length(first_line) > 30 ? first_line[1:30] * "..." : first_line

    EchoAction("FFI read: $(chars) chars, $(lines) lines. Line 0: \"$(first_line_preview)\"")
end

define_command("buffer-test-insert", "Test direct buffer insert via FFI") do ctx
    # Insert text directly using FFI
    buffer_insert!(ctx.cursor_pos, "[INSERTED]")
    EchoAction("Inserted text at position $(ctx.cursor_pos) via FFI")
end

define_command("buffer-test-delete", "Test direct buffer delete via FFI") do ctx
    # Delete 5 characters at cursor if possible
    chars = buffer_char_count()
    end_pos = min(ctx.cursor_pos + 5, chars)
    if ctx.cursor_pos < end_pos
        deleted = buffer_substring(ctx.cursor_pos, end_pos)
        buffer_delete!(ctx.cursor_pos, end_pos)
        EchoAction("Deleted \"$(deleted)\" via FFI")
    else
        EchoAction("Nothing to delete")
    end
end
