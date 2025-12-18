# Copyright (C) 2025 Ryan Daum <ryan.daum@gmail.com> This program is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License as published by the Free Software Foundation, version 3.
#
# This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details.
#
# You should have received a copy of the GNU General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
#
# Julia Syntax Highlighting Mode
# Uses JuliaSyntax.jl directly for tokenization-based syntax highlighting.

# Track whether JuliaSyntax is available (loaded lazily)
const _julia_syntax_available = Ref{Union{Bool, Nothing}}(nothing)

"""
Try to load JuliaSyntax.jl. Returns true if available, false otherwise.
"""
function _try_load_julia_highlighting()
    # Already checked
    if _julia_syntax_available[] !== nothing
        return _julia_syntax_available[]::Bool
    end

    # Try to load the package - eval in Main to make it globally available
    try
        Core.eval(Main, :(using JuliaSyntax))
        _julia_syntax_available[] = true
        return true
    catch e
        println("[syntax] Failed to load JuliaSyntax: $e")
        _julia_syntax_available[] = false
        return false
    end
end

# Wrapper to call tokenize - works around potential jlrs issues
function _do_tokenize(code::String)
    # Use invokelatest to ensure fresh dispatch
    return Base.invokelatest(Main.JuliaSyntax.tokenize, code)
end

# Wrapper to get kind as a string - does all conversion within Julia
function _get_kind_string(tok)
    kind = Base.invokelatest(Main.JuliaSyntax.kind, tok)
    # Convert Kind to Symbol then to String
    return String(Symbol(kind))
end

# ============================================
# Token kind to face mapping for JuliaSyntax
# ============================================

# Keywords in Julia
const JULIA_KEYWORDS = Set([
    "function", "end", "if", "else", "elseif", "for", "while", "try", "catch",
    "finally", "return", "break", "continue", "begin", "let", "do", "struct",
    "mutable", "abstract", "primitive", "type", "module", "baremodule", "using",
    "import", "export", "const", "global", "local", "macro", "quote", "where",
    "in", "isa", "outer", "public"
])

# Get face for a JuliaSyntax token kind
function _token_kind_to_face(kind_str::String)
    # Comments
    if kind_str == "Comment"
        return "julia-comment"
    end

    # Strings and string delimiters
    if kind_str in ("String", "CmdString", "Char", "\"", "`", "'", "\"\"\"", "```")
        return "julia-string"
    end

    # Numbers
    if kind_str in ("Integer", "BinInt", "OctInt", "HexInt", "Float", "Float32")
        return "julia-number"
    end

    # Keywords (JuliaSyntax returns the keyword itself as kind, e.g. "function", "if")
    if kind_str in JULIA_KEYWORDS
        return "julia-keyword"
    end

    # Operators
    if kind_str in ("+", "-", "*", "/", "^", "%", "\\", "&", "|", "⊻", "~",
                    "==", "!=", "<", ">", "<=", ">=", "===", "!==", "≤", "≥", "≠",
                    "&&", "||", "!", "=", "+=", "-=", "*=", "/=", "^=",
                    "->", "<:", ">:", "::", ".", "..", "...", "?", ":", ";",
                    "@", "\$", "=>", "|>", "<|", "∈", "∉", "⊆", "⊇", "∩", "∪")
        return "julia-operator"
    end

    # Brackets/parens - handled separately for rainbow coloring
    if kind_str in ("(", ")", "[", "]", "{", "}")
        return nothing
    end

    # Macro prefix
    if kind_str == "@"
        return "julia-macro"
    end

    # Boolean and special constants
    if kind_str in ("true", "false")
        return "julia-constant"
    end

    # Identifiers - could be functions, types, variables
    # We'll leave them unhighlighted for now (default color)
    if kind_str == "Identifier"
        return nothing
    end

    return nothing
end

"""
    define_julia_faces()

Define all faces used for Julia syntax highlighting.
Uses a color scheme inspired by VSCode's Julia extension.
"""
function define_julia_faces()
    # Keywords (purple/magenta)
    define_face("julia-keyword", foreground="#c586c0", bold=true)

    # Functions (yellow)
    define_face("julia-function", foreground="#dcdcaa")
    define_face("julia-function-def", foreground="#dcdcaa", bold=true)
    define_face("julia-builtin", foreground="#4ec9b0")
    define_face("julia-macro", foreground="#c586c0")

    # Types (teal)
    define_face("julia-type", foreground="#4ec9b0")

    # Strings (orange)
    define_face("julia-string", foreground="#ce9178")
    define_face("julia-regex", foreground="#d16969")

    # Numbers (light green)
    define_face("julia-number", foreground="#b5cea8")

    # Constants (blue)
    define_face("julia-constant", foreground="#569cd6")

    # Symbols (light blue)
    define_face("julia-symbol", foreground="#9cdcfe")

    # Comments (green, italic)
    define_face("julia-comment", foreground="#6a9955", italic=true)

    # Operators (light gray)
    define_face("julia-operator", foreground="#d4d4d4")
    define_face("julia-broadcast", foreground="#d4d4d4", bold=true)

    # Errors (red)
    define_face("julia-error", foreground="#f44747", background="#3c1e1e")

    # Rainbow delimiters (6 colors that cycle)
    define_face("julia-paren-1", foreground="#ffd700")  # Gold
    define_face("julia-paren-2", foreground="#da70d6")  # Orchid
    define_face("julia-paren-3", foreground="#87cefa")  # Light sky blue
    define_face("julia-paren-4", foreground="#98fb98")  # Pale green
    define_face("julia-paren-5", foreground="#ff6347")  # Tomato
    define_face("julia-paren-6", foreground="#00ced1")  # Dark turquoise

    return nothing
end

"""
    highlight_julia(code::String; offset::Int=0) -> Int

Apply Julia syntax highlighting to the given code using JuliaSyntax tokenization.
Returns the number of spans applied.

# Arguments
- `code`: The Julia source code to highlight
- `offset`: Character offset to add to all positions (for highlighting part of a buffer)
"""
function highlight_julia(code::String; offset::Int=0)
    if !_try_load_julia_highlighting()
        return 0
    end

    # Tokenize using wrapper function
    local tokens
    try
        tokens = collect(_do_tokenize(code))
    catch e
        println("[syntax] Tokenization error: $e")
        return 0
    end

    # Convert tokens to spans
    starts = Int[]
    stops = Int[]
    faces = String[]

    # Track bracket depth for rainbow parens
    paren_depth = 0
    bracket_depth = 0
    curly_depth = 0
    rainbow_colors = ["julia-paren-1", "julia-paren-2", "julia-paren-3",
                      "julia-paren-4", "julia-paren-5", "julia-paren-6"]

    for tok in tokens
        kind_str = _get_kind_string(tok)

        # Get byte range from token - tokens store range as UnitRange
        # Token structure: Token(head, start:stop)
        tok_range = tok.range
        start_byte = first(tok_range)
        end_byte = last(tok_range)

        # Convert byte positions to character positions
        # For now assume ASCII/simple encoding (byte == char position)
        # Note: ranges are 1-indexed in Julia, we need 0-indexed
        start_pos = start_byte - 1 + offset
        end_pos = end_byte + offset  # end_byte is inclusive, our end is exclusive

        # Handle rainbow brackets
        if kind_str == "("
            face = rainbow_colors[(paren_depth % 6) + 1]
            paren_depth += 1
            push!(starts, start_pos)
            push!(stops, end_pos)
            push!(faces, face)
            continue
        elseif kind_str == ")"
            paren_depth = max(0, paren_depth - 1)
            face = rainbow_colors[(paren_depth % 6) + 1]
            push!(starts, start_pos)
            push!(stops, end_pos)
            push!(faces, face)
            continue
        elseif kind_str == "["
            face = rainbow_colors[(bracket_depth % 6) + 1]
            bracket_depth += 1
            push!(starts, start_pos)
            push!(stops, end_pos)
            push!(faces, face)
            continue
        elseif kind_str == "]"
            bracket_depth = max(0, bracket_depth - 1)
            face = rainbow_colors[(bracket_depth % 6) + 1]
            push!(starts, start_pos)
            push!(stops, end_pos)
            push!(faces, face)
            continue
        elseif kind_str == "{"
            face = rainbow_colors[(curly_depth % 6) + 1]
            curly_depth += 1
            push!(starts, start_pos)
            push!(stops, end_pos)
            push!(faces, face)
            continue
        elseif kind_str == "}"
            curly_depth = max(0, curly_depth - 1)
            face = rainbow_colors[(curly_depth % 6) + 1]
            push!(starts, start_pos)
            push!(stops, end_pos)
            push!(faces, face)
            continue
        end

        # Look up face for this token kind
        face = _token_kind_to_face(kind_str)
        if face !== nothing
            push!(starts, start_pos)
            push!(stops, end_pos)
            push!(faces, face)
        end
    end

    if !isempty(starts)
        return add_spans(starts, stops, faces)
    end

    return 0
end

"""
    highlight_julia_buffer()

Highlight the entire current buffer as Julia code.
Call this from a command to apply syntax highlighting.
"""
function highlight_julia_buffer()
    if !_try_load_julia_highlighting()
        return 0
    end

    code = buffer_content()
    if isempty(code)
        return 0
    end

    # Clear existing spans
    clear_spans()

    # Do all tokenization and processing in one place to avoid jlrs boundary issues
    starts, stops, faces = _tokenize_and_extract_spans(code)

    if !isempty(starts)
        return add_spans(starts, stops, faces)
    end

    return 0
end

# Pre-cached Kind constants (set after JuliaSyntax is loaded)
const _kind_cache = Ref{Union{Nothing, NamedTuple}}(nothing)

"""
Get cached Kind constants, loading JuliaSyntax if needed.
"""
function _get_kind_constants()
    if _kind_cache[] !== nothing
        return _kind_cache[]
    end

    if !_try_load_julia_highlighting()
        return nothing
    end

    # Eval the Kind constants from JuliaSyntax
    # These need @eval because K"..." is a string macro
    kinds = @eval Main begin
        (
            Comment = JuliaSyntax.K"Comment",
            String = JuliaSyntax.K"String",
            CmdString = JuliaSyntax.K"CmdString",
            Char = JuliaSyntax.K"Char",
            Integer = JuliaSyntax.K"Integer",
            BinInt = JuliaSyntax.K"BinInt",
            OctInt = JuliaSyntax.K"OctInt",
            HexInt = JuliaSyntax.K"HexInt",
            Float = JuliaSyntax.K"Float",
            Float32 = JuliaSyntax.K"Float32",
            Identifier = JuliaSyntax.K"Identifier",
            LParen = JuliaSyntax.K"(",
            RParen = JuliaSyntax.K")",
            LBracket = JuliaSyntax.K"[",
            RBracket = JuliaSyntax.K"]",
            LBrace = JuliaSyntax.K"{",
            RBrace = JuliaSyntax.K"}",
            StringDelim = JuliaSyntax.K"\"",
            CmdDelim = JuliaSyntax.K"`",
            CharDelim = JuliaSyntax.K"'",
            TripleString = JuliaSyntax.K"\"\"\"",
            TripleCmd = JuliaSyntax.K"```",
            true_kw = JuliaSyntax.K"true",
            false_kw = JuliaSyntax.K"false",
            At = JuliaSyntax.K"@",
        )
    end

    _kind_cache[] = kinds
    return kinds
end

"""
Internal function that does all JuliaSyntax processing in one place.
Returns (starts, stops, faces) arrays.
"""
function _tokenize_and_extract_spans(code::String)
    starts = Int[]
    stops = Int[]
    faces = String[]

    # Get cached Kind constants
    K = _get_kind_constants()
    if K === nothing
        return (starts, stops, faces)
    end

    # Get the JuliaSyntax module
    JS = Main.JuliaSyntax

    # Tokenize - use invokelatest since JuliaSyntax was loaded dynamically
    tokens = collect(Base.invokelatest(JS.tokenize, code))

    # Track bracket depth for rainbow parens
    paren_depth = 0
    bracket_depth = 0
    curly_depth = 0
    rainbow_colors = ["julia-paren-1", "julia-paren-2", "julia-paren-3",
                      "julia-paren-4", "julia-paren-5", "julia-paren-6"]

    for tok in tokens
        # Get kind - use invokelatest for dynamic method dispatch
        kind = Base.invokelatest(JS.kind, tok)

        # Get byte range from token
        tok_range = tok.range
        start_byte = Int(first(tok_range))
        end_byte = Int(last(tok_range))

        # Convert byte positions to character positions (0-indexed)
        start_pos = start_byte - 1
        end_pos = end_byte  # end_byte is inclusive, our end is exclusive

        # Handle rainbow brackets
        if kind == K.LParen
            face = rainbow_colors[(paren_depth % 6) + 1]
            paren_depth += 1
            push!(starts, start_pos)
            push!(stops, end_pos)
            push!(faces, face)
            continue
        elseif kind == K.RParen
            paren_depth = max(0, paren_depth - 1)
            face = rainbow_colors[(paren_depth % 6) + 1]
            push!(starts, start_pos)
            push!(stops, end_pos)
            push!(faces, face)
            continue
        elseif kind == K.LBracket
            face = rainbow_colors[(bracket_depth % 6) + 1]
            bracket_depth += 1
            push!(starts, start_pos)
            push!(stops, end_pos)
            push!(faces, face)
            continue
        elseif kind == K.RBracket
            bracket_depth = max(0, bracket_depth - 1)
            face = rainbow_colors[(bracket_depth % 6) + 1]
            push!(starts, start_pos)
            push!(stops, end_pos)
            push!(faces, face)
            continue
        elseif kind == K.LBrace
            face = rainbow_colors[(curly_depth % 6) + 1]
            curly_depth += 1
            push!(starts, start_pos)
            push!(stops, end_pos)
            push!(faces, face)
            continue
        elseif kind == K.RBrace
            curly_depth = max(0, curly_depth - 1)
            face = rainbow_colors[(curly_depth % 6) + 1]
            push!(starts, start_pos)
            push!(stops, end_pos)
            push!(faces, face)
            continue
        end

        # Determine face based on kind
        face = nothing

        # Comments
        if kind == K.Comment
            face = "julia-comment"
        # Strings and string delimiters
        elseif kind == K.String || kind == K.CmdString || kind == K.Char ||
               kind == K.StringDelim || kind == K.CmdDelim || kind == K.CharDelim ||
               kind == K.TripleString || kind == K.TripleCmd
            face = "julia-string"
        # Numbers
        elseif kind == K.Integer || kind == K.BinInt || kind == K.OctInt ||
               kind == K.HexInt || kind == K.Float || kind == K.Float32
            face = "julia-number"
        # Boolean constants
        elseif kind == K.true_kw || kind == K.false_kw
            face = "julia-constant"
        # Macro prefix
        elseif kind == K.At
            face = "julia-macro"
        # Use predicates for broader categories
        elseif Base.invokelatest(JS.is_keyword, kind)
            face = "julia-keyword"
        elseif Base.invokelatest(JS.is_operator, kind)
            face = "julia-operator"
        end

        if face !== nothing
            push!(starts, start_pos)
            push!(stops, end_pos)
            push!(faces, face)
        end
    end

    return (starts, stops, faces)
end

"""
    highlight_julia_region(start::Int, stop::Int)

Highlight a region of the buffer as Julia code.
Useful for incremental highlighting.

# Arguments
- `start`: Starting character offset (0-indexed)
- `stop`: Ending character offset (0-indexed, exclusive)
"""
function highlight_julia_region(start::Int, stop::Int)
    code = buffer_substring(start, stop)
    if isempty(code)
        return 0
    end

    # Clear spans in this region and apply new ones
    clear_spans_in_range(start, stop)
    return highlight_julia(code, offset=start)
end

# ============================================
# Command registration
# ============================================

"""
Register the Julia highlighting command.
"""
function _register_julia_highlighting_command()
    define_command(
        "highlight-julia",
        "Apply Julia syntax highlighting to the current buffer"
    ) do ctx
        if !_try_load_julia_highlighting()
            return EchoAction("JuliaSyntaxHighlighting.jl not available. Install with: using Pkg; Pkg.add(\"JuliaSyntaxHighlighting\")")
        end

        # Define faces if not already done
        if !face_exists("julia-keyword")
            define_julia_faces()
        end

        count = highlight_julia_buffer()
        return EchoAction("Applied $count syntax highlights")
    end
end

# Auto-register when loaded
_register_julia_highlighting_command()

# ============================================
# Major Mode Registration
# ============================================

"""
    _julia_mode_init()

Initialization hook for julia-mode.
Sets up faces and applies initial syntax highlighting.
"""
function _julia_mode_init()
    # Define faces if not already done
    if !face_exists("julia-keyword")
        define_julia_faces()
    end

    # Register mode-specific indent commands
    register_indent_command("julia-mode", "julia-indent-line")
    register_newline_indent_command("julia-mode", "julia-newline-and-indent")

    # Apply initial highlighting
    highlight_julia_buffer()
end

"""
    _julia_mode_after_change(start::Int, old_end::Int, new_end::Int)

After-change hook for julia-mode.
Re-highlights the entire buffer for now (incremental highlighting is complex
due to bracket depth tracking).
"""
function _julia_mode_after_change(start::Int, old_end::Int, new_end::Int)
    # For now, re-highlight the entire buffer
    # A smarter implementation could track bracket depths and only
    # re-highlight affected regions
    highlight_julia_buffer()
end

# ============================================
# Indentation Support
# ============================================

# Keywords that should dedent (line starts with these = one less indent level)
const JULIA_DEDENT_KEYWORDS = Set(["else", "elseif", "catch", "finally", "end"])

# Indent size in spaces
const JULIA_INDENT_SIZE = 4

"""
Get byte positions where each line starts in the code.
"""
function _line_byte_positions(code::String)
    positions = [1]
    for (i, c) in enumerate(code)
        if c == '\n'
            push!(positions, i + 1)
        end
    end
    return positions
end

"""
Count indent level at a byte position by walking the parse tree.
Handles both block constructs (function, if, for) and continuations (unclosed parens).
"""
function _indent_at_byte(tree, byte_pos::Int, code::String)
    if !_try_load_julia_highlighting()
        return 0
    end

    JS = Main.JuliaSyntax
    indent = 0

    # Node kinds that introduce block indentation (cumulative)
    # Note: K"block" is NOT here - it's a generic container, the actual constructs provide indent
    # Note: elseif/else/catch/finally are NOT here - they continue existing blocks, not new indent
    block_kinds = Set([
        (@eval Main JuliaSyntax.K"function"),
        (@eval Main JuliaSyntax.K"macro"),
        (@eval Main JuliaSyntax.K"for"),
        (@eval Main JuliaSyntax.K"while"),
        (@eval Main JuliaSyntax.K"if"),
        (@eval Main JuliaSyntax.K"let"),
        (@eval Main JuliaSyntax.K"try"),
        (@eval Main JuliaSyntax.K"struct"),
        (@eval Main JuliaSyntax.K"module"),
        (@eval Main JuliaSyntax.K"begin"),
        (@eval Main JuliaSyntax.K"do"),
        (@eval Main JuliaSyntax.K"quote"),
    ])

    # Node kinds that introduce continuation indentation (cumulative for nested structures)
    continuation_kinds = Set([
        (@eval Main JuliaSyntax.K"call"),
        (@eval Main JuliaSyntax.K"tuple"),
        (@eval Main JuliaSyntax.K"vect"),
        (@eval Main JuliaSyntax.K"braces"),
        (@eval Main JuliaSyntax.K"parens"),
    ])

    # Get line number for a byte position
    function line_for_byte(pos)
        line = 1
        for (i, c) in enumerate(code)
            if i >= pos
                break
            end
            if c == '\n'
                line += 1
            end
        end
        return line
    end

    target_line = line_for_byte(byte_pos)
    block_lines_seen = Set{Int}()  # Track which lines we've already counted for blocks
    continuation_lines_seen = Set{Int}()  # Track which lines we've already counted for continuations

    function walk(node)
        fb = Base.invokelatest(JS.first_byte, node)
        lb = Base.invokelatest(JS.last_byte, node)
        k = Base.invokelatest(JS.kind, node)

        if fb <= byte_pos <= lb
            # Block constructs: only indent if we're INSIDE the block (started on earlier line)
            # AND we haven't already counted a block from that line
            if k in block_kinds
                block_start_line = line_for_byte(fb)
                if block_start_line < target_line && !(block_start_line in block_lines_seen)
                    push!(block_lines_seen, block_start_line)
                    indent += 1
                end
            # Continuation constructs: add indent if construct spans multiple lines
            # AND started on an earlier line than target
            # AND we haven't already counted a continuation from that line
            elseif k in continuation_kinds
                # Skip leading whitespace to find actual content start
                content_start = fb
                while content_start <= lb && content_start <= length(code) && code[content_start] in " \t\n\r"
                    content_start += 1
                end

                # Skip if content starts with a comment
                starts_with_comment = content_start <= length(code) && code[content_start] == '#'

                construct_start_line = line_for_byte(content_start)
                construct_end_line = line_for_byte(lb)
                spans_multiple_lines = construct_end_line > construct_start_line

                if !starts_with_comment && construct_start_line < target_line && spans_multiple_lines && !(construct_start_line in continuation_lines_seen)
                    push!(continuation_lines_seen, construct_start_line)
                    indent += 1
                end
            end

            if Base.invokelatest(JS.haschildren, node)
                for child in Base.invokelatest(JS.children, node)
                    walk(child)
                end
            end
        end
    end

    walk(tree)
    return indent
end

"""
Get the first word on a line (for detecting dedent keywords).
"""
function _get_first_word(line::AbstractString)
    m = match(r"^\s*(\w+)", line)
    return m === nothing ? "" : m.captures[1]
end

"""
    calculate_julia_indent(code::String, line_num::Int) -> Int

Calculate the correct indentation level (in spaces) for a given line number.
Uses JuliaSyntax parse tree for accurate block detection.
"""
function calculate_julia_indent(code::String, line_num::Int)
    if !_try_load_julia_highlighting()
        return 0
    end

    JS = Main.JuliaSyntax

    lines = split(code, '\n')
    if line_num < 1 || line_num > length(lines)
        return 0
    end

    # Parse the code
    local tree
    try
        tree = Base.invokelatest(JS.parseall, JS.SyntaxNode, code)
    catch e
        # Parse error - fall back to simple indent
        return 0
    end

    line_starts = _line_byte_positions(code)
    if line_num > length(line_starts)
        return 0
    end

    byte_pos = line_starts[line_num]

    # Get base indent from tree
    indent = _indent_at_byte(tree, byte_pos, code)

    # Check if line starts with dedenting keyword
    first_word = _get_first_word(lines[line_num])
    if first_word in JULIA_DEDENT_KEYWORDS
        indent = max(0, indent - 1)
    end

    # Check if line starts with closing bracket (dedent for continuation)
    line_trimmed = lstrip(lines[line_num])
    if !isempty(line_trimmed) && line_trimmed[1] in ")]}"
        indent = max(0, indent - 1)
    end

    return indent * JULIA_INDENT_SIZE
end

# Register indent commands
function _register_julia_indent_commands()
    define_command(
        "julia-indent-line",
        "Re-indent the current line (Tab behavior)"
    ) do ctx
        code = buffer_content()
        line_num = ctx.current_line  # 1-indexed from Rust

        # Calculate correct indent using parse tree
        target_indent = calculate_julia_indent(code, line_num)

        # Return IndentLineAction with 0-indexed line for Rust
        return IndentLineAction(ctx.current_line - 1, target_indent)
    end

    define_command(
        "julia-newline-and-indent",
        "Insert newline and indent (Enter behavior)"
    ) do ctx
        code = buffer_content()
        cursor = ctx.cursor_pos  # 0-indexed from Rust
        line_num = ctx.current_line  # Already 1-indexed from Rust

        # Simulate what the buffer will look like after newline insertion
        # to calculate the correct indent for the new line
        # Convert 0-indexed cursor to 1-indexed Julia string position
        before = cursor > 0 ? code[1:cursor] : ""
        after = cursor < length(code) ? code[cursor+1:end] : ""
        new_code = before * "\n" * after
        new_line_num = line_num + 1

        target_indent = calculate_julia_indent(new_code, new_line_num)

        # Insert newline + indentation as a single insert
        indent_str = " " ^ target_indent
        return InsertAction(cursor, "\n" * indent_str)
    end
end

_register_julia_indent_commands()

# Register julia-mode as a major mode
define_major_mode("julia-mode",
    extensions = [".jl"],
    init = _julia_mode_init,
    after_change = _julia_mode_after_change
)
