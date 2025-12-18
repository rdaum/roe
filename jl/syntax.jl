# Copyright (C) 2025 Ryan Daum <ryan.daum@gmail.com> This program is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License as published by the Free Software Foundation, version 3.
#
# This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details.
#
# You should have received a copy of the GNU General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
#
# Syntax Highlighting API
# This module provides functions for defining faces (styles) and applying
# syntax highlighting spans to buffer content.

using Libdl

# Reuse the handle from buffer_api.jl
# (included in same module, so _get_roe_handle is available)

"""
    define_face(name::String; foreground=nothing, background=nothing,
                bold=false, italic=false, underline=false) -> Bool

Define a named face (style) for syntax highlighting.

# Arguments
- `name`: Unique name for this face (e.g., "keyword", "string", "comment")
- `foreground`: Foreground color as hex string (e.g., "#ff0000") or nothing
- `background`: Background color as hex string or nothing
- `bold`: Whether text should be bold
- `italic`: Whether text should be italic
- `underline`: Whether text should be underlined

# Returns
`true` if face was defined successfully, `false` otherwise.

# Example
```julia
define_face("keyword", foreground="#569cd6", bold=true)
define_face("comment", foreground="#6a9955", italic=true)
define_face("string", foreground="#ce9178")
```
"""
function define_face(name::String;
                     foreground::Union{String,Nothing}=nothing,
                     background::Union{String,Nothing}=nothing,
                     bold::Bool=false,
                     italic::Bool=false,
                     underline::Bool=false)
    handle = _get_roe_handle()

    fg_ptr = foreground === nothing ? C_NULL : pointer(foreground)
    bg_ptr = background === nothing ? C_NULL : pointer(background)

    result = ccall(
        Libdl.dlsym(handle, :roe_define_face),
        Clonglong,
        (Cstring, Cstring, Cstring, Cuchar, Cuchar, Cuchar),
        name, fg_ptr, bg_ptr,
        bold ? 1 : 0,
        italic ? 1 : 0,
        underline ? 1 : 0
    )

    return result == 1
end

"""
    face_exists(name::String) -> Bool

Check if a face with the given name has been defined.

# Example
```julia
if !face_exists("keyword")
    define_face("keyword", foreground="#569cd6")
end
```
"""
function face_exists(name::String)
    handle = _get_roe_handle()
    result = ccall(
        Libdl.dlsym(handle, :roe_face_exists),
        Clonglong,
        (Cstring,),
        name
    )
    return result == 1
end

"""
    add_span(start::Int, stop::Int, face::String) -> Bool

Add a highlight span to the current buffer.

# Arguments
- `start`: Starting character offset (0-indexed, inclusive)
- `stop`: Ending character offset (0-indexed, exclusive)
- `face`: Name of a previously defined face

# Returns
`true` if span was added successfully, `false` otherwise.

# Example
```julia
# Highlight characters 10-15 as a keyword
add_span(10, 15, "keyword")
```
"""
function add_span(start::Int, stop::Int, face::String)
    handle = _get_roe_handle()
    result = ccall(
        Libdl.dlsym(handle, :roe_add_span),
        Clonglong,
        (Clonglong, Clonglong, Cstring),
        start, stop, face
    )
    return result == 1
end

"""
    add_spans(starts::Vector{Int}, stops::Vector{Int}, faces::Vector{String}) -> Int

Add multiple highlight spans at once (more efficient than individual calls).

# Arguments
- `starts`: Vector of starting offsets
- `stops`: Vector of ending offsets
- `faces`: Vector of face names

All vectors must have the same length.

# Returns
Number of spans successfully added.

# Example
```julia
starts = [0, 10, 20]
stops = [5, 15, 25]
faces = ["keyword", "string", "comment"]
add_spans(starts, stops, faces)
```
"""
function add_spans(starts::Vector{Int}, stops::Vector{Int}, faces::Vector{String})
    @assert length(starts) == length(stops) == length(faces) "All vectors must have same length"

    if isempty(starts)
        return 0
    end

    handle = _get_roe_handle()

    # Convert to C-compatible arrays
    starts_c = Clonglong.(starts)
    stops_c = Clonglong.(stops)
    faces_c = [pointer(f) for f in faces]

    result = ccall(
        Libdl.dlsym(handle, :roe_add_spans),
        Clonglong,
        (Ptr{Clonglong}, Ptr{Clonglong}, Ptr{Ptr{Cchar}}, Clonglong),
        starts_c, stops_c, faces_c, length(starts)
    )

    return Int(result)
end

"""
    clear_spans()

Remove all highlight spans from the current buffer.
"""
function clear_spans()
    handle = _get_roe_handle()
    ccall(Libdl.dlsym(handle, :roe_clear_spans), Cvoid, ())
    return nothing
end

"""
    clear_spans_in_range(start::Int, stop::Int)

Remove highlight spans that overlap with the given range.

# Arguments
- `start`: Starting character offset (0-indexed, inclusive)
- `stop`: Ending character offset (0-indexed, exclusive)
"""
function clear_spans_in_range(start::Int, stop::Int)
    handle = _get_roe_handle()
    ccall(
        Libdl.dlsym(handle, :roe_clear_spans_in_range),
        Cvoid,
        (Clonglong, Clonglong),
        start, stop
    )
    return nothing
end

"""
    has_spans() -> Bool

Check if the current buffer has any highlight spans.
"""
function has_spans()
    handle = _get_roe_handle()
    result = ccall(Libdl.dlsym(handle, :roe_buffer_has_spans), Clonglong, ())
    return result == 1
end

# ============================================
# Common face definitions
# ============================================

"""
    define_standard_faces()

Define a set of standard faces commonly used for syntax highlighting.
These follow a VSCode-like dark theme color scheme.

Defined faces:
- `keyword`: Language keywords (if, else, for, etc.)
- `type`: Type names
- `function`: Function names
- `variable`: Variable names
- `string`: String literals
- `number`: Numeric literals
- `comment`: Comments
- `operator`: Operators
- `punctuation`: Punctuation marks
- `constant`: Constants and special values
- `error`: Error highlighting
- `warning`: Warning highlighting
"""
function define_standard_faces()
    # Keywords (blue)
    define_face("keyword", foreground="#569cd6", bold=true)

    # Types (teal/cyan)
    define_face("type", foreground="#4ec9b0")

    # Functions (yellow)
    define_face("function", foreground="#dcdcaa")

    # Variables (light blue)
    define_face("variable", foreground="#9cdcfe")

    # Strings (orange)
    define_face("string", foreground="#ce9178")

    # Numbers (light green)
    define_face("number", foreground="#b5cea8")

    # Comments (green, italic)
    define_face("comment", foreground="#6a9955", italic=true)

    # Operators (white)
    define_face("operator", foreground="#d4d4d4")

    # Punctuation (gray)
    define_face("punctuation", foreground="#808080")

    # Constants (blue)
    define_face("constant", foreground="#4fc1ff")

    # Error (red, with background)
    define_face("error", foreground="#f44747", underline=true)

    # Warning (yellow)
    define_face("warning", foreground="#cca700", underline=true)

    return nothing
end

# ============================================
# Helper types for building highlights
# ============================================

"""
    Span

Represents a highlight span with position and face information.
"""
struct Span
    start::Int
    stop::Int
    face::String
end

"""
    highlight_matches(text::String, pattern::Regex, face::String; offset::Int=0) -> Vector{Span}

Find all matches of a pattern in text and create spans for them.

# Arguments
- `text`: The text to search
- `pattern`: A regex pattern to match
- `face`: The face name to apply to matches
- `offset`: Offset to add to all positions (useful when highlighting a substring)

# Returns
Vector of `Span` objects for all matches.

# Example
```julia
# Find all numbers and create spans
spans = highlight_matches(line_text, r"\\d+", "number", offset=line_start)
```
"""
function highlight_matches(text::String, pattern::Regex, face::String; offset::Int=0)
    spans = Span[]
    for m in eachmatch(pattern, text)
        push!(spans, Span(m.offset - 1 + offset, m.offset - 1 + length(m.match) + offset, face))
    end
    return spans
end

"""
    apply_spans(spans::Vector{Span}) -> Int

Apply a vector of spans to the current buffer.

# Returns
Number of spans successfully added.
"""
function apply_spans(spans::Vector{Span})
    if isempty(spans)
        return 0
    end

    starts = [s.start for s in spans]
    stops = [s.stop for s in spans]
    faces = [s.face for s in spans]

    return add_spans(starts, stops, faces)
end
