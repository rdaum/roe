# Copyright (C) 2025 Ryan Daum <ryan.daum@gmail.com> This program is free software: you can redistribute it and/or modify it under terms of the GNU General Public License as published by Free Software Foundation, version 3.
#
# This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details.
#
# You should have received a copy of the GNU General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
#
# Rust Syntax Highlighting Mode
# Uses TreeSitter.jl for robust Rust syntax highlighting

# Track whether TreeSitter is available (loaded lazily)
const _tree_sitter_available = Ref{Union{Bool, Nothing}}(nothing)
const _rust_parser = Ref{Union{Nothing, Any}}(nothing)

"""
Try to load TreeSitter.jl and tree_sitter_rust_jll. Returns true if available, false otherwise.
"""
function _try_load_rust_highlighting()
    # Already checked
    if _tree_sitter_available[] !== nothing
        return _tree_sitter_available[]::Bool
    end

    # Try to load the packages
    try
        Core.eval(Main, :(using TreeSitter))
        Core.eval(Main, :(using tree_sitter_rust_jll))
        _tree_sitter_available[] = true
        return true
    catch e
        println("[rust-syntax] Failed to load TreeSitter: $e")
        _tree_sitter_available[] = false
        return false
    end
end

"""
Get the Rust parser instance (creates one if needed).
"""
function _get_rust_parser()
    if _rust_parser[] !== nothing
        return _rust_parser[]
    end

    if !_try_load_rust_highlighting()
        return nothing
    end

    try
        parser = Base.invokelatest(Main.Parser, Main.tree_sitter_rust_jll)
        _rust_parser[] = parser
        return parser
    catch e
        println("[rust-syntax] Failed to create Rust parser: $e")
        _tree_sitter_available[] = false
        return nothing
    end
end

# ============================================
# Tree-sitter node type to face mapping for Rust
# ============================================

"""
Get face for a tree-sitter node kind (Rust).
"""
function _node_kind_to_face(kind::String)
    # Comments
    if kind == "line_comment"
        return "rust-comment"
    end
    if kind == "block_comment"
        return "rust-comment"
    end

    # Strings
    if kind in ("string_literal", "raw_string_literal")
        return "rust-string"
    end
    if kind == "char_literal"
        return "rust-char"
    end

    # Numbers
    if kind == "float_literal"
        return "rust-float"
    end
    if kind == "integer_literal"
        return "rust-number"
    end

    # Boolean
    if kind == "boolean_literal"
        return "rust-constant"
    end

    # Keywords
    if kind in (
        "fn",
        "let",
        "mut",
        "pub",
        "crate",
        "super",
        "self",
        "Self",
        "struct",
        "enum",
        "union",
        "impl",
        "trait",
        "type",
        "where",
        "use",
        "mod",
        "extern",
        "crate",
        "static",
        "const",
        "move",
        "ref",
        "unsafe",
        "async",
        "await",
        "loop",
        "while",
        "for",
        "in",
        "if",
        "else",
        "match",
        "else if",
        "return",
        "break",
        "continue",
        "type",
        "dyn",
        "becomes",
        "impl",
    )
        return "rust-keyword"
    end

    # Control flow keywords
    if kind in ("break_statement", "continue_statement", "return_expression")
        return "rust-keyword"
    end

    # Modifiers
    if kind in (
        "visibility_modifier",
        "mutable_specifier",
        "const_qualifier",
        "async_qualifier",
        "move_specifier",
        "unsafe_block",
        "unsafe_expression",
        "try_block",
    )
        return "rust-keyword"
    end

    # Functions
    if kind == "function_item"
        return "rust-function"
    end
    if kind == "function_signature_item"
        return "rust-function"
    end
    if kind == "call_expression"
        return "rust-function-call"
    end
    if kind == "macro_invocation"
        return "rust-macro"
    end

    # Types
    if kind in (
        "type_identifier",
        "primitive_type",
        "generic_type",
        "qualified_type",
        "pointer_type",
        "reference_type",
        "array_type",
        "slice_type",
        "tuple_type",
        "tuple_expression",
        "unit_type",
        "never_type",
        "dynamic_type",
    )
        return "rust-type"
    end

    # Traits
    if kind in ("trait", "trait_bounds", "where_clause")
        return "rust-trait"
    end

    # Lifetime
    if kind == "lifetime"
        return "rust-lifetime"
    end

    # Attribute
    if kind == "attribute_item"
        return "rust-attribute"
    end
    if kind == "inner_attribute_item"
        return "rust-attribute"
    end

    # Struct fields, enum variants
    if kind == "field_declaration"
        return "rust-field"
    end
    if kind == "enum_variant"
        return "rust-enum-variant"
    end

    # Pattern matching
    if kind == "match_block"
        return "rust-match"
    end
    if kind == "match_arm"
        return "rust-match-arm"
    end
    if kind == "match_pattern"
        return "rust-pattern"
    end

    # Parameters
    if kind == "parameters"
        return "rust-parameter"
    end
    if kind == "parameter"
        return "rust-parameter"
    end

    # Variable bindings
    if kind in ("identifier", "shorthand_field_identifier", "field_identifier")
        return "rust-identifier"
    end

    # Use/import
    if kind in ("use_declaration", "mod_item")
        return "rust-keyword"
    end

    # Operators
    if kind in (
        "!",
        "&&",
        "||",
        "==",
        "!=",
        "<",
        ">",
        "<=",
        ">=",
        "+",
        "-",
        "*",
        "/",
        "%",
        "^",
        "&",
        "|",
        "=>",
        "->",
        "=",
        "+=",
        "-=",
        "*=",
        "/=",
        "%=",
        "&=",
        "|=",
        "^=",
        "<<",
        ">>",
        "..=",
        "..",
        "?",
        ":",
        ";",
        "::",
        "<-",
        "->",
    )
        return "rust-operator"
    end

    # Punctuation
    if kind in ("(", ")", "[", "]", "{", "}", ",", ".", ";", "::")
        return "rust-punctuation"
    end

    return nothing
end

"""
    define_rust_faces()

Define all faces used for Rust syntax highlighting.
Uses a color scheme inspired by rust-analyzer/VSCode.
"""
function define_rust_faces()
    # Keywords (purple/magenta)
    define_face("rust-keyword", foreground = "#c586c0", bold = true)

    # Functions (yellow)
    define_face("rust-function", foreground = "#dcdcaa")
    define_face("rust-function-call", foreground = "#dcdcaa")

    # Types (teal)
    define_face("rust-type", foreground = "#4ec9b0")
    define_face("rust-trait", foreground = "#4ec9b0")

    # Lifetimes (cyan)
    define_face("rust-lifetime", foreground = "#4fc1ff")

    # Macros (pink/purple)
    define_face("rust-macro", foreground = "#c586c0")

    # Strings (orange)
    define_face("rust-string", foreground = "#ce9178")
    define_face("rust-char", foreground = "#ce9178")

    # Numbers (light green)
    define_face("rust-number", foreground = "#b5cea8")
    define_face("rust-float", foreground = "#b5cea8")

    # Constants (blue)
    define_face("rust-constant", foreground = "#569cd6")

    # Identifiers (white/light gray)
    define_face("rust-identifier", foreground = "#9cdcfe")

    # Struct fields, parameters (light blue)
    define_face("rust-field", foreground = "#9cdcfe")
    define_face("rust-parameter", foreground = "#9cdcfe")

    # Enum variants (yellow)
    define_face("rust-enum-variant", foreground = "#dcdcaa")

    # Attributes (gray)
    define_face("rust-attribute", foreground = "#808080")

    # Comments (green, italic)
    define_face("rust-comment", foreground = "#6a9955", italic = true)

    # Operators (light gray)
    define_face("rust-operator", foreground = "#d4d4d4")

    # Punctuation (gray)
    define_face("rust-punctuation", foreground = "#808080")

    # Pattern matching
    define_face("rust-pattern", foreground = "#4ec9b0")
    define_face("rust-match", foreground = "#c586c0", bold = true)
    define_face("rust-match-arm", foreground = "#c586c0")

    return nothing
end

"""
    highlight_rust(code::String; offset::Int=0) -> Int

Apply Rust syntax highlighting to the given code using TreeSitter tokenization.
Returns the number of spans applied.

# Arguments
- `code`: The Rust source code to highlight
- `offset`: Character offset to add to all positions (for highlighting part of a buffer)
"""
function highlight_rust(code::String; offset::Int = 0)
    parser = _get_rust_parser()
    if parser === nothing
        return 0
    end

    # Access TreeSitter module
    TS = Main.TreeSitter

    # Parse the code
    local tree
    try
        tree = Base.invokelatest(TS.parse, parser, code)
    catch e
        println("[rust-syntax] Parse error: $e")
        return 0
    end

    # Define arrays to collect results
    starts = Int[]
    stops = Int[]
    faces = String[]

    # Create visitor closure
    visitor = function(node, enter)
        if !enter
            return
        end

        kind = Base.invokelatest(TS.node_type, node)
        face = _node_kind_to_face(kind)

        if face !== nothing
            br = Base.invokelatest(TS.byte_range, node)
            # byte_range returns 1-indexed inclusive, convert to 0-indexed exclusive
            push!(starts, br[1] - 1 + offset)
            push!(stops, br[2] + offset)  # inclusive->exclusive cancels 1-indexed->0-indexed
            push!(faces, face)
        end
    end

    # Call traverse with the visitor (callback is first argument)
    try
        Base.invokelatest(TS.traverse, visitor, tree)
    catch e
        println("[rust-syntax] Tree traversal error: $e")
        return 0
    end

    if !isempty(starts)
        return add_spans(starts, stops, faces)
    end

    return 0
end

"""
    highlight_rust_buffer()

Highlight the entire current buffer as Rust code.
Call this from a command to apply syntax highlighting.
"""
function highlight_rust_buffer()
    parser = _get_rust_parser()
    if parser === nothing
        return 0
    end

    code = buffer_content()
    if isempty(code)
        return 0
    end

    # Clear existing spans
    clear_spans()

    # Parse and highlight
    return highlight_rust(code)
end

"""
    highlight_rust_region(start::Int, stop::Int)

Highlight a region of the buffer as Rust code.
Useful for incremental highlighting.

# Arguments
- `start`: Starting character offset (0-indexed)
- `stop`: Ending character offset (0-indexed, exclusive)
"""
function highlight_rust_region(start::Int, stop::Int)
    code = buffer_substring(start, stop)
    if isempty(code)
        return 0
    end

    # Clear spans in this region and apply new ones
    clear_spans_in_range(start, stop)
    return highlight_rust(code, offset = start)
end

# ============================================
# Command registration
# ============================================

"""
Register the Rust highlighting command.
"""
function _register_rust_highlighting_command()
    define_command(
        "highlight-rust",
        "Apply Rust syntax highlighting to the current buffer",
    ) do ctx
        if !_try_load_rust_highlighting()
            return EchoAction(
                "TreeSitter.jl or tree_sitter_rust_jll not available. Install with: using Pkg; Pkg.add([\"TreeSitter\", \"tree_sitter_rust_jll\"])",
            )
        end

        # Define faces if not already done
        if !face_exists("rust-keyword")
            define_rust_faces()
        end

        count = highlight_rust_buffer()
        return EchoAction("Applied $count syntax highlights")
    end
end

# Auto-register when loaded
_register_rust_highlighting_command()

# ============================================
# Indentation Support
# ============================================

# Indent size in spaces
const RUST_INDENT_SIZE = 4

# Node kinds that introduce block indentation
const RUST_BLOCK_KINDS = Set([
    "function_item",
    "if_expression",
    "else_clause",
    "match_expression",
    "match_arm",
    "loop_expression",
    "while_expression",
    "for_expression",
    "impl_item",
    "struct_item",
    "enum_item",
    "trait_item",
    "mod_item",
    "block",
    "unsafe_block",
    "async_block",
    "const_block",
    "closure_expression",
    "declaration_list",
    "field_declaration_list",
    "enum_variant_list",
    "use_list",
])

# Node kinds for continuation indentation (unclosed parens, etc.)
const RUST_CONTINUATION_KINDS = Set([
    "arguments",
    "parameters",
    "type_parameters",
    "tuple_expression",
    "tuple_pattern",
    "array_expression",
    "tuple_type",
    "macro_invocation",
])

"""
Get byte positions where each line starts in the code.
"""
function _rust_line_byte_positions(code::String)
    positions = [1]
    for (i, c) in enumerate(code)
        if c == '\n'
            push!(positions, i + 1)
        end
    end
    return positions
end

"""
Count indent level at a byte position by walking the TreeSitter parse tree.
"""
function _rust_indent_at_byte(tree, byte_pos::Int, code::String)
    if !_try_load_rust_highlighting()
        return 0
    end

    TS = Main.TreeSitter
    indent = 0

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
    block_lines_seen = Set{Int}()
    continuation_lines_seen = Set{Int}()

    # Visitor to walk the tree
    function visitor(node, enter)
        if !enter
            return
        end

        br = Base.invokelatest(TS.byte_range, node)
        # TreeSitter byte_range is 1-indexed
        fb = br[1]
        lb = br[2]
        kind = Base.invokelatest(TS.node_type, node)

        if fb <= byte_pos <= lb
            # Block constructs: indent if we're inside (started on earlier line)
            if kind in RUST_BLOCK_KINDS
                block_start_line = line_for_byte(fb)
                if block_start_line < target_line && !(block_start_line in block_lines_seen)
                    push!(block_lines_seen, block_start_line)
                    indent += 1
                end
            # Continuation constructs: add indent if spans multiple lines
            elseif kind in RUST_CONTINUATION_KINDS
                construct_start_line = line_for_byte(fb)
                construct_end_line = line_for_byte(lb)
                spans_multiple_lines = construct_end_line > construct_start_line

                if construct_start_line < target_line && spans_multiple_lines && !(construct_start_line in continuation_lines_seen)
                    push!(continuation_lines_seen, construct_start_line)
                    indent += 1
                end
            end
        end
    end

    try
        Base.invokelatest(TS.traverse, visitor, tree)
    catch e
        println("[rust-indent] Tree traversal error: $e")
        return 0
    end

    return indent
end

"""
    calculate_rust_indent(code::String, line_num::Int) -> Int

Calculate the correct indentation level (in spaces) for a given line number.
Uses TreeSitter parse tree for accurate block detection.
"""
function calculate_rust_indent(code::String, line_num::Int)
    parser = _get_rust_parser()
    if parser === nothing
        return 0
    end

    TS = Main.TreeSitter

    lines = split(code, '\n')
    if line_num < 1 || line_num > length(lines)
        return 0
    end

    # Parse the code
    local tree
    try
        tree = Base.invokelatest(TS.parse, parser, code)
    catch e
        # Parse error - fall back to no indent
        return 0
    end

    line_starts = _rust_line_byte_positions(code)
    if line_num > length(line_starts)
        return 0
    end

    byte_pos = line_starts[line_num]

    # Get base indent from tree
    indent = _rust_indent_at_byte(tree, byte_pos, code)

    # Check if line starts with closing brace (dedent)
    line_trimmed = lstrip(lines[line_num])
    if !isempty(line_trimmed) && line_trimmed[1] in "})]"
        indent = max(0, indent - 1)
    end

    return indent * RUST_INDENT_SIZE
end

# Register indent commands
function _register_rust_indent_commands()
    define_command(
        "rust-indent-line",
        "Re-indent the current line (Tab behavior)"
    ) do ctx
        code = buffer_content()
        line_num = ctx.current_line  # 1-indexed from Rust

        # Calculate correct indent using parse tree
        target_indent = calculate_rust_indent(code, line_num)

        # Return IndentLineAction with 0-indexed line for Rust
        return IndentLineAction(ctx.current_line - 1, target_indent)
    end

    define_command(
        "rust-newline-and-indent",
        "Insert newline and indent (Enter behavior)"
    ) do ctx
        code = buffer_content()
        cursor = ctx.cursor_pos  # 0-indexed from Rust
        line_num = ctx.current_line  # Already 1-indexed from Rust

        # Simulate what the buffer will look like after newline insertion
        before = cursor > 0 ? code[1:cursor] : ""
        after = cursor < length(code) ? code[cursor+1:end] : ""
        new_code = before * "\n" * after
        new_line_num = line_num + 1

        target_indent = calculate_rust_indent(new_code, new_line_num)

        # Insert newline + indentation as a single insert
        indent_str = " " ^ target_indent
        return InsertAction(cursor, "\n" * indent_str)
    end
end

_register_rust_indent_commands()

# ============================================
# Major Mode Registration
# ============================================

"""
    _rust_mode_init()

Initialization hook for rust-mode.
Sets up faces and applies initial syntax highlighting.
"""
function _rust_mode_init()
    # Define faces if not already done
    if !face_exists("rust-keyword")
        define_rust_faces()
    end

    # Register mode-specific indent commands
    register_indent_command("rust-mode", "rust-indent-line")
    register_newline_indent_command("rust-mode", "rust-newline-and-indent")

    # Apply initial highlighting
    highlight_rust_buffer()
end

"""
    _rust_mode_after_change(start::Int, old_end::Int, new_end::Int)

After-change hook for rust-mode.
Re-highlights the entire buffer for simplicity.
"""
function _rust_mode_after_change(start::Int, old_end::Int, new_end::Int)
    # For now, re-highlight the entire buffer
    # Tree-sitter supports incremental re-parsing, which could be used for
    # more efficient updates in the future
    highlight_rust_buffer()
end

# Register rust-mode as a major mode
define_major_mode("rust-mode", extensions = [".rs"], init = _rust_mode_init, after_change = _rust_mode_after_change)
