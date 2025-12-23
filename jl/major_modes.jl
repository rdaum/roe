# Copyright (C) 2025 Ryan Daum <ryan.daum@gmail.com> This program is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License as published by the Free Software Foundation, version 3.
#
# This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details.
#
# You should have received a copy of the GNU General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
#
# Major Mode System
# Emacs-style major modes with lifecycle hooks and file extension associations.

"""
    ModeProperties

Bundle of configurable properties for a major mode.
Use `mode_properties()` to create with defaults, then override as needed.
"""
Base.@kwdef struct ModeProperties
    show_gutter::Bool = true
    # Add more properties here as needed:
    # indent_width::Int = 4
    # use_tabs::Bool = false
    # word_wrap::Bool = false
    # etc.
end

"""
    mode_properties(; kwargs...) -> ModeProperties

Create a ModeProperties with the given overrides.
All properties have sensible defaults.

# Example
```julia
mode_properties(show_gutter = false)
```
"""
mode_properties(; kwargs...) = ModeProperties(; kwargs...)

"""
    MajorModeDefinition

Holds the definition of a major mode including its hooks and configuration.
"""
struct MajorModeDefinition
    name::String
    extensions::Vector{String}
    init::Union{Function, Nothing}
    after_change::Union{Function, Nothing}
    properties::ModeProperties
    # Future hooks can be added here:
    # before_save, after_save, on_enter, on_exit, etc.
end

# Registry of major modes: name => MajorModeDefinition
const _major_modes = Dict{String, MajorModeDefinition}()

# Extension to mode name mapping (built from mode definitions)
const _extension_to_mode = Dict{String, String}()

# Default mode for files with no matching extension
const _default_mode = Ref{String}("fundamental-mode")

"""
    define_major_mode(name::String; extensions=String[], init=nothing, after_change=nothing, properties=ModeProperties())

Define a major mode with the given name and configuration.

# Arguments
- `name`: The name of the mode (e.g., "julia-mode", "python-mode")
- `extensions`: File extensions this mode handles (e.g., [".jl", ".julia"])
- `init`: Function called when the mode is activated for a buffer.
          Called with no arguments, should set up faces and initial highlighting.
- `after_change`: Function called after buffer content changes.
                  Called with (start::Int, old_end::Int, new_end::Int) for incremental updates.
- `properties`: Mode properties bundle (use `mode_properties()` to create)

# Example
```julia
define_major_mode("julia-mode",
    extensions = [".jl"],
    properties = mode_properties(show_gutter = true),
    init = () -> begin
        define_julia_faces()
        highlight_julia_buffer()
    end,
    after_change = (start, old_end, new_end) -> begin
        # Re-highlight the changed region
        highlight_julia_region(start, new_end)
    end
)
```
"""
function define_major_mode(name::String;
                           extensions::Vector{String}=String[],
                           init::Union{Function, Nothing}=nothing,
                           after_change::Union{Function, Nothing}=nothing,
                           properties::ModeProperties=ModeProperties())

    # Normalize extensions (ensure they start with .)
    normalized_extensions = String[]
    for ext in extensions
        if !startswith(ext, ".")
            push!(normalized_extensions, "." * ext)
        else
            push!(normalized_extensions, ext)
        end
    end

    # Create mode definition
    mode_def = MajorModeDefinition(name, normalized_extensions, init, after_change, properties)
    _major_modes[name] = mode_def

    # Register extension mappings
    for ext in normalized_extensions
        _extension_to_mode[ext] = name
    end

    return nothing
end

"""
    get_major_mode_for_file(filepath::String) -> String

Get the name of the major mode for a given file path based on its extension.
Returns the default mode if no matching mode is found.
"""
function get_major_mode_for_file(filepath::String)
    # Get file extension (including the dot)
    ext = lowercase(splitext(filepath)[2])

    if haskey(_extension_to_mode, ext)
        return _extension_to_mode[ext]
    end

    return _default_mode[]
end

"""
    call_major_mode_init(mode_name::String) -> Bool

Call the init hook for the given major mode.
Also sets buffer properties like gutter visibility based on the mode's configuration.
Returns true if the hook was called successfully, false otherwise.
"""
function call_major_mode_init(mode_name::String)
    if !haskey(_major_modes, mode_name)
        return false
    end

    mode_def = _major_modes[mode_name]

    # Set gutter visibility based on mode configuration
    buffer_set_show_gutter!(mode_def.properties.show_gutter)

    if mode_def.init === nothing
        return true  # No init hook, but mode exists
    end

    try
        mode_def.init()
        return true
    catch e
        @error "Error in major mode init hook" mode_name exception=(e, catch_backtrace())
        return false
    end
end

"""
    call_major_mode_after_change(mode_name::String, start::Int, old_end::Int, new_end::Int) -> Bool

Call the after_change hook for the given major mode.
This is called after buffer content changes to allow incremental updates.

# Arguments
- `mode_name`: Name of the major mode
- `start`: Starting byte position of the change
- `old_end`: End position before the change
- `new_end`: End position after the change

Returns true if the hook was called successfully, false otherwise.
"""
function call_major_mode_after_change(mode_name::String, start::Int, old_end::Int, new_end::Int)
    if !haskey(_major_modes, mode_name)
        return false
    end

    mode_def = _major_modes[mode_name]
    if mode_def.after_change === nothing
        return true  # No after_change hook, but mode exists
    end

    try
        mode_def.after_change(start, old_end, new_end)
        return true
    catch e
        @error "Error in major mode after_change hook" mode_name exception=(e, catch_backtrace())
        return false
    end
end

"""
    has_major_mode(name::String) -> Bool

Check if a major mode is registered.
"""
function has_major_mode(name::String)
    haskey(_major_modes, name)
end

"""
    list_major_modes() -> Vector{String}

Return a list of all registered major mode names.
"""
function list_major_modes()
    collect(keys(_major_modes))
end

"""
    get_major_mode_extensions(name::String) -> Vector{String}

Get the file extensions associated with a major mode.
"""
function get_major_mode_extensions(name::String)
    if haskey(_major_modes, name)
        return _major_modes[name].extensions
    end
    return String[]
end

"""
    set_default_major_mode(name::String)

Set the default major mode for files with no matching extension.
"""
function set_default_major_mode(name::String)
    _default_mode[] = name
end

# ============================================
# Built-in modes
# ============================================

# fundamental-mode: The default mode with no special behavior.
# Used for files with unrecognized extensions.
define_major_mode("fundamental-mode")
