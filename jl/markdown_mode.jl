# Copyright (C) 2025 Ryan Daum <ryan.daum@gmail.com> This program is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License as published by the Free Software Foundation, version 3.
#
# This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details.
#
# You should have received a copy of the GNU General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
#
# Markdown Mode
# Regex-based syntax highlighting and auto-indentation for Markdown files.

# ============================================
# Face definitions
# ============================================

"""
Define faces for markdown syntax highlighting.
"""
function define_markdown_faces()
    # Headers (blue, bold) - different weights for different levels
    define_face("md-header-1", foreground="#569cd6", bold=true)
    define_face("md-header-2", foreground="#569cd6", bold=true)
    define_face("md-header-3", foreground="#4ec9b0", bold=true)
    define_face("md-header-4", foreground="#4ec9b0")
    define_face("md-header-5", foreground="#9cdcfe")
    define_face("md-header-6", foreground="#9cdcfe")

    # Emphasis
    define_face("md-bold", bold=true)
    define_face("md-italic", italic=true)

    # Code
    define_face("md-code", foreground="#ce9178")
    define_face("md-code-block", foreground="#ce9178")
    define_face("md-code-fence", foreground="#808080")
    define_face("md-code-language", foreground="#4ec9b0")

    # Links and images
    define_face("md-link-text", foreground="#4ec9b0", underline=true)
    define_face("md-image", foreground="#c586c0")

    # Lists
    define_face("md-list-marker", foreground="#dcdcaa", bold=true)

    # Blockquotes
    define_face("md-blockquote-marker", foreground="#6a9955", bold=true)

    # Horizontal rules
    define_face("md-hr", foreground="#808080")

    return nothing
end

# ============================================
# Syntax highlighting (regex-based)
# ============================================

"""
Parse markdown and extract all highlight spans using regex.
"""
function _parse_and_highlight(code::String)
    starts = Int[]
    stops = Int[]
    faces = String[]

    # First, find all fenced code block ranges to exclude from other highlighting
    fenced_ranges = Tuple{Int, Int}[]
    for m in eachmatch(r"^(`{3,}|~{3,})(\w*)\n(.*?)\n\1"ms, code)
        push!(fenced_ranges, (m.offset - 1, m.offset - 1 + length(m.match)))
    end

    # Helper to check if a position is inside a fenced code block
    function in_fenced_block(pos::Int)
        for (start, stop) in fenced_ranges
            if start <= pos < stop
                return true
            end
        end
        return false
    end

    # Headers (include the # markers and full line)
    for m in eachmatch(r"^(#{1,6})\s+(.*)$"m, code)
        if !in_fenced_block(m.offset - 1)
            level = length(m.captures[1])
            face = level <= 6 ? "md-header-$level" : "md-header-6"
            push!(starts, m.offset - 1)
            push!(stops, m.offset - 1 + length(m.match))
            push!(faces, face)
        end
    end

    # Bold (**text** or __text__)
    for m in eachmatch(r"\*\*[^*]+\*\*|__[^_]+__", code)
        if !in_fenced_block(m.offset - 1)
            push!(starts, m.offset - 1)
            push!(stops, m.offset - 1 + length(m.match))
            push!(faces, "md-bold")
        end
    end

    # Italic (*text* or _text_)
    for m in eachmatch(r"(?<!\*)\*(?!\*)[^*]+(?<!\*)\*(?!\*)|(?<!_)_(?!_)[^_]+(?<!_)_(?!_)", code)
        if !in_fenced_block(m.offset - 1)
            push!(starts, m.offset - 1)
            push!(stops, m.offset - 1 + length(m.match))
            push!(faces, "md-italic")
        end
    end

    # Inline code - double backticks (``code``)
    for m in eachmatch(r"``[^`]+``", code)
        if !in_fenced_block(m.offset - 1)
            push!(starts, m.offset - 1)
            push!(stops, m.offset - 1 + length(m.match))
            push!(faces, "md-code")
        end
    end

    # Inline code - single backticks (`code`)
    for m in eachmatch(r"(?<!`)`(?!`)[^`\n]+`(?!`)", code)
        if !in_fenced_block(m.offset - 1)
            push!(starts, m.offset - 1)
            push!(stops, m.offset - 1 + length(m.match))
            push!(faces, "md-code")
        end
    end

    # Links [text](url)
    for m in eachmatch(r"\[([^\]]+)\]\(([^)]+)\)", code)
        if !in_fenced_block(m.offset - 1)
            push!(starts, m.offset - 1)
            push!(stops, m.offset - 1 + length(m.match))
            push!(faces, "md-link-text")
        end
    end

    # Images ![alt](url)
    for m in eachmatch(r"!\[([^\]]*)\]\(([^)]+)\)", code)
        if !in_fenced_block(m.offset - 1)
            push!(starts, m.offset - 1)
            push!(stops, m.offset - 1 + length(m.match))
            push!(faces, "md-image")
        end
    end

    # Blockquote markers (>) at start of lines
    for m in eachmatch(r"^(>+)"m, code)
        if !in_fenced_block(m.offset - 1)
            push!(starts, m.offset - 1)
            push!(stops, m.offset - 1 + length(m.match))
            push!(faces, "md-blockquote-marker")
        end
    end

    # List markers at start of lines
    for m in eachmatch(r"^(\s*)([-*+]|\d+[.)])\s"m, code)
        if !in_fenced_block(m.offset - 1)
            marker_start = m.offset - 1 + length(m.captures[1])
            marker_end = marker_start + length(m.captures[2])
            push!(starts, marker_start)
            push!(stops, marker_end)
            push!(faces, "md-list-marker")
        end
    end

    # Horizontal rules (---, ***, ___)
    for m in eachmatch(r"^([-*_])\1{2,}\s*$"m, code)
        if !in_fenced_block(m.offset - 1)
            push!(starts, m.offset - 1)
            push!(stops, m.offset - 1 + length(m.match))
            push!(faces, "md-hr")
        end
    end

    # Fenced code blocks
    for m in eachmatch(r"^(`{3,}|~{3,})(\w*)\n(.*?)\n\1"ms, code)
        block_start = m.offset - 1
        fence_chars = m.captures[1]
        lang = m.captures[2]
        content = m.captures[3]

        # Opening fence
        push!(starts, block_start)
        push!(stops, block_start + length(fence_chars))
        push!(faces, "md-code-fence")

        # Language identifier
        if lang !== nothing && !isempty(lang)
            lang_start = block_start + length(fence_chars)
            push!(starts, lang_start)
            push!(stops, lang_start + length(lang))
            push!(faces, "md-code-language")
        end

        # Content
        content_start = block_start + length(fence_chars) + (lang !== nothing ? length(lang) : 0) + 1
        content_end = content_start + length(content)
        if content_end > content_start
            push!(starts, content_start)
            push!(stops, content_end)
            push!(faces, "md-code-block")
        end

        # Closing fence
        closing_start = content_end + 1
        push!(starts, closing_start)
        push!(stops, closing_start + length(fence_chars))
        push!(faces, "md-code-fence")
    end

    return starts, stops, faces
end

"""
Apply markdown highlighting to the current buffer.
"""
function highlight_markdown_buffer()
    code = buffer_content()
    if isempty(code)
        return 0
    end

    clear_spans()

    starts, stops, faces = _parse_and_highlight(code)

    if !isempty(starts)
        return add_spans(starts, stops, faces)
    end

    return 0
end

# ============================================
# Indentation
# ============================================

const MARKDOWN_INDENT_SIZE = 2

"""
Get the list marker and its indentation from a line.
Returns (indent, marker_length) or nothing if not a list item.
"""
function _get_list_info(line::AbstractString)
    m = match(r"^(\s*)([-*+]|\d+[.)])\s", line)
    if m !== nothing
        indent = length(m.captures[1])
        marker_len = length(m.captures[2])
        return (indent, marker_len)
    end
    return nothing
end

"""
Count leading blockquote markers on a line.
"""
function _count_blockquote_markers(line::AbstractString)
    count = 0
    for c in line
        if c == '>'
            count += 1
        elseif c == ' '
            continue
        else
            break
        end
    end
    return count
end

"""
Check if we're inside a fenced code block at the given line.
"""
function _in_fenced_code_block(lines::Vector{<:AbstractString}, line_num::Int)
    fence_pattern = r"^(`{3,}|~{3,})"
    in_fence = false
    fence_char = nothing

    for i in 1:(line_num - 1)
        m = match(fence_pattern, lines[i])
        if m !== nothing
            if !in_fence
                in_fence = true
                fence_char = m.captures[1][1]
            elseif lines[i][1] == fence_char
                in_fence = false
                fence_char = nothing
            end
        end
    end

    return in_fence
end

"""
Calculate the correct indentation for a markdown line.
"""
function calculate_markdown_indent(code::String, line_num::Int)
    lines = split(code, '\n', keepempty=true)

    if line_num < 1 || line_num > length(lines)
        return 0
    end

    if line_num == 1
        return 0
    end

    current_line = String(lines[line_num])
    prev_line = String(lines[line_num - 1])

    # If in fenced code block, preserve existing indentation
    if _in_fenced_code_block(lines, line_num)
        m = match(r"^(\s*)", current_line)
        return m !== nothing ? length(m.captures[1]) : 0
    end

    # Check if previous line was a list item
    prev_list = _get_list_info(prev_line)
    if prev_list !== nothing
        indent, marker_len = prev_list
        return indent + marker_len + 1
    end

    return 0
end

# ============================================
# Mode registration
# ============================================

function _markdown_mode_init()
    if !face_exists("md-header-1")
        define_markdown_faces()
    end

    register_indent_command("markdown-mode", "markdown-indent-line")
    register_newline_indent_command("markdown-mode", "markdown-newline-and-indent")

    highlight_markdown_buffer()
end

function _markdown_mode_after_change(start::Int, old_end::Int, new_end::Int)
    highlight_markdown_buffer()
end

# Register commands
function _register_markdown_commands()
    define_command(
        "markdown-indent-line",
        "Re-indent the current line in markdown"
    ) do ctx
        code = buffer_content()
        target_indent = calculate_markdown_indent(code, ctx.current_line)
        return IndentLineAction(ctx.current_line - 1, target_indent)
    end

    define_command(
        "markdown-newline-and-indent",
        "Insert newline and indent in markdown"
    ) do ctx
        code = buffer_content()
        cursor = ctx.cursor_pos
        line_num = ctx.current_line

        lines = split(code, '\n', keepempty=true)
        if line_num >= 1 && line_num <= length(lines)
            current_line = String(lines[line_num])

            # Check if current line is an empty list item
            list_info = _get_list_info(current_line)
            if list_info !== nothing
                trimmed = strip(current_line)
                if match(r"^[-*+]$|^\d+[.)]$", trimmed) !== nothing
                    return InsertAction(cursor, "\n")
                end

                # Continue the list
                m = match(r"^(\s*)([-*+]|\d+)([.)])\s", current_line)
                if m !== nothing
                    prefix_spaces = m.captures[1]
                    marker = m.captures[2]
                    suffix = m.captures[3]

                    if marker in ["-", "*", "+"]
                        new_marker = "$(prefix_spaces)$(marker)$(suffix) "
                    else
                        num = parse(Int, marker) + 1
                        new_marker = "$(prefix_spaces)$(num)$(suffix) "
                    end

                    return InsertAction(cursor, "\n" * new_marker)
                end
            end

            # Check for blockquote continuation
            bq_count = _count_blockquote_markers(current_line)
            if bq_count > 0
                prefix = repeat("> ", bq_count)
                return InsertAction(cursor, "\n" * prefix)
            end
        end

        return InsertAction(cursor, "\n")
    end

    define_command(
        "highlight-markdown",
        "Apply markdown syntax highlighting to the current buffer"
    ) do ctx
        if !face_exists("md-header-1")
            define_markdown_faces()
        end

        count = highlight_markdown_buffer()
        return EchoAction("Applied $count markdown highlights")
    end
end

_register_markdown_commands()

# Register the major mode
define_major_mode("markdown-mode",
    extensions = [".md", ".markdown", ".mkd", ".mdown"],
    init = _markdown_mode_init,
    after_change = _markdown_mode_after_change
)
