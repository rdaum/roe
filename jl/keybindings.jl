# Copyright (C) 2025 Ryan Daum <ryan.daum@gmail.com> This program is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License as published by the Free Software Foundation, version 3.
#
# This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details.
#
# You should have received a copy of the GNU General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
#
# Keybinding API and default bindings

# Store keybindings: key_sequence => action
# Key sequences are strings like "C-x C-c", "M-x", "C-p"
# Actions are either:
#   - Command name strings (e.g., "quit", "save-buffer")
#   - Special action strings prefixed with ":" (e.g., ":cursor-up", ":kill-line")
const _keybindings = Dict{String, String}()

"""
    define_key(key_sequence::String, action::String)

Define a keybinding that maps a key sequence to an action.

Key sequence format (Emacs-style):
- `C-x` = Control-x
- `M-x` = Meta/Alt-x
- `S-<key>` = Shift-key (for special keys)
- Space-separated for chords: `C-x C-c`

Action can be:
- A command name: `"quit"`, `"save-buffer"`, `"my-custom-command"`
- A special action prefixed with `:`: `":cursor-up"`, `":kill-line"`

Special actions available:
- `:cursor-up`, `:cursor-down`, `:cursor-left`, `:cursor-right`
- `:cursor-line-start`, `:cursor-line-end`
- `:cursor-buffer-start`, `:cursor-buffer-end`
- `:cursor-word-forward`, `:cursor-word-backward`
- `:cursor-page-up`, `:cursor-page-down`
- `:cursor-*-select` variants (e.g., `:cursor-left-select`) - move and select
- `:delete`, `:backspace`, `:enter`, `:tab`
- `:kill-line`, `:kill-region`, `:copy-region`
- `:yank`, `:mark-start`, `:cancel`
- `:undo`, `:redo`, `:redraw`

# Example
```julia
# Map C-s to save (instead of default C-x C-s)
define_key("C-s", "save-buffer")

# Map F5 to a custom command
define_key("F5", "my-build-command")

# Remap cursor movement
define_key("C-j", ":cursor-down")
```
"""
function define_key(key_sequence::String, action::String)
    _keybindings[key_sequence] = action
    return nothing
end

"""
    define_keys(bindings::Pair{String, String}...)

Define multiple keybindings at once.

# Example
```julia
define_keys(
    "C-s" => "save-buffer",
    "C-q" => "quit",
    "F5" => "my-build-command"
)
```
"""
function define_keys(bindings::Pair{String, String}...)
    for (key, action) in bindings
        define_key(key, action)
    end
    return nothing
end

"""
    undefine_key(key_sequence::String)

Remove a keybinding.
"""
function undefine_key(key_sequence::String)
    delete!(_keybindings, key_sequence)
    return nothing
end

"""
    list_keybindings() -> Vector{Tuple{String, String}}

Return list of (key_sequence, action) for all registered keybindings.
Called by Rust to query user-defined keybindings.
"""
function list_keybindings()
    [(seq, action) for (seq, action) in _keybindings]
end

"""
    has_keybinding(key_sequence::String) -> Bool

Check if a keybinding is defined for the given key sequence.
"""
function has_keybinding(key_sequence::String)
    haskey(_keybindings, key_sequence)
end

"""
    get_keybinding(key_sequence::String) -> Union{String, Nothing}

Get the action for a keybinding, or nothing if not defined.
"""
function get_keybinding(key_sequence::String)
    get(_keybindings, key_sequence, nothing)
end

# ============================================
# Default Keybindings (Emacs-style)
# ============================================
# These are loaded by default. Users can override or add to these in their config.

# --- Basic cursor movement ---
define_key("Left", ":cursor-left")
define_key("Right", ":cursor-right")
define_key("Up", ":cursor-up")
define_key("Down", ":cursor-down")
define_key("Home", ":cursor-line-start")
define_key("End", ":cursor-line-end")
define_key("PageUp", ":cursor-page-up")
define_key("PageDown", ":cursor-page-down")

# Emacs cursor movement
define_key("C-p", ":cursor-up")
define_key("C-n", ":cursor-down")
define_key("C-f", ":cursor-right")
define_key("C-b", ":cursor-left")
define_key("C-a", ":cursor-line-start")
define_key("C-e", ":cursor-line-end")
define_key("C-v", ":cursor-page-down")
define_key("M-v", ":cursor-page-up")

# Word movement
define_key("M-f", ":cursor-word-forward")
define_key("M-b", ":cursor-word-backward")
define_key("C-Left", ":cursor-word-backward")
define_key("C-Right", ":cursor-word-forward")

# Paragraph movement
define_key("M-{", ":cursor-paragraph-backward")
define_key("M-}", ":cursor-paragraph-forward")

# Buffer start/end
define_key("C-Home", ":cursor-buffer-start")
define_key("C-End", ":cursor-buffer-end")
define_key("M-<", ":cursor-buffer-start")
define_key("M->", ":cursor-buffer-end")

# --- Basic text manipulation ---
define_key("Backspace", ":backspace")
define_key("Delete", ":delete")
define_key("C-d", ":delete")
define_key("Enter", ":enter")
define_key("Tab", ":tab")

# --- Kill/yank ---
define_key("C-k", ":kill-line")
define_key("C-w", ":kill-region")
define_key("M-w", ":copy-region")
define_key("C-y", ":yank")

# --- Mark ---
define_key("C-Space", ":set-mark")

# --- Cancel/escape ---
define_key("C-g", ":cancel")
define_key("Escape", ":escape")

# --- Display ---
define_key("C-l", ":redraw")

# --- Undo/redo ---
define_key("C-/", ":undo")
define_key("C-_", ":undo")
define_key("C-x u", ":undo")
define_key("M-/", ":redo")
# Terminal sends Ctrl+/ as Ctrl+7 (ASCII control code limitation)
define_key("C-7", ":undo")

# --- Commands (C-x prefix) ---
define_key("C-x C-c", "quit")
define_key("C-x C-s", "save-buffer")
define_key("C-x C-f", "find-file")
define_key("C-x C-v", "visit-file")

# Window management
define_key("C-x 2", "split-window-horizontally")
define_key("C-x 3", "split-window-vertically")
define_key("C-x o", "other-window")
define_key("C-x 0", "delete-window")
define_key("C-x 1", "delete-other-windows")

# Buffer management
define_key("C-x b", "switch-to-buffer")
define_key("C-x k", "kill-buffer")

# --- M-x command mode ---
define_key("M-x", "command-mode")

# --- Page up/down with Meta ---
define_key("M-Up", ":cursor-page-up")
define_key("M-Down", ":cursor-page-down")

# --- CUA-style shift-arrow selection ---
define_key("S-Left", ":cursor-left-select")
define_key("S-Right", ":cursor-right-select")
define_key("S-Up", ":cursor-up-select")
define_key("S-Down", ":cursor-down-select")
define_key("S-Home", ":cursor-line-start-select")
define_key("S-End", ":cursor-line-end-select")
define_key("S-PageUp", ":cursor-page-up-select")
define_key("S-PageDown", ":cursor-page-down-select")

# Shift+Ctrl for word selection
define_key("C-S-Left", ":cursor-word-backward-select")
define_key("C-S-Right", ":cursor-word-forward-select")

# Shift+Ctrl for buffer start/end selection
define_key("C-S-Home", ":cursor-buffer-start-select")
define_key("C-S-End", ":cursor-buffer-end-select")
