# Roe / ᚱᛟ / Ryan's Own Emacs

A minimalistic console text editor in the spirit of the Emacs family of editors, built in Rust.

This editor follows the Emacs tradition in three key ways: (a) it's buffer-oriented rather than
file-oriented, (b) it uses the default GNU Emacs keybinding set, and (c) it's being architected with
full programmability in mind (though not yet implemented). Unlike the current trend toward "modal"
editors, this is a direct manipulation editor and proud of it.

Currently, all behavior is hard-wired in Rust rather than implemented in a scripting language like
Lisp. However, the architecture has been designed from the ground up to delegate the bulk of editor
logic to an embedded scripting system. The likely outcome will be to embed "Steel" Scheme, a Scheme
interpreter written in Rust.

## Screenshot

![Screenshot of Roe editor](screenshot.png)

## Features

- **Emacs-style keybindings**: Familiar keyboard shortcuts for Emacs users
- **Buffer-oriented editing**: Work with buffers as the primary unit, not just files like other
  editors. Just like GNU emacs.
  - Even the command entry window is a buffer.
- **Window management**: Split windows horizontally/vertically, switch between them, same as emacs.
- **Mouse support**: Click to position cursor, drag window borders to resize, click to switch
  windows
- **Modular architecture**: Extensible mode system for different editing behaviors
- **Terminal-based**: Lightweight, runs in your terminal
- **Fast rendering**: Uses crossterm for efficient terminal manipulation

## Key Bindings

_Note: The keybindings are currently hard-coded and cannot be redefined. There are also no macros
yet (so how can you call it an Emacs!). However, the architecture has been designed to support both
customizable keybindings and macro recording/playback in the future._

### Cursor Movement

#### Basic Movement

- Arrow keys or `C-f/b/n/p`: Move right/left/down/up
- `C-a`: Beginning of line
- `C-e`: End of line
- `Home/End`: Beginning/end of line

#### Word Movement

- `M-f` or `C-Right`: Move forward by word
- `M-b` or `C-Left`: Move backward by word

#### Paragraph Movement

- `M-{`: Move backward by paragraph
- `M-}`: Move forward by paragraph

#### Page Movement

- `C-v` or `Page Down`: Page down
- `M-v` or `Page Up`: Page up
- `M-Up`: Page up (alternative)
- `M-Down`: Page down (alternative)

#### Buffer Movement

- `C-Home`: Beginning of buffer
- `C-End`: End of buffer

### Window Management

- `C-x 2`: Split window horizontally
- `C-x 3`: Split window vertically
- `C-x o`: Switch to other window
- `C-x 0`: Delete current window
- `C-x 1`: Delete all other windows

### Buffer Management

- `C-x b`: Switch to another buffer
- `C-x k`: Kill (close) a buffer

### Mouse Operations

- **Click**: Position cursor at click location
- **Click in window**: Switch to clicked window
- **Drag window borders**: Resize windows by dragging their borders
- **Mouse events in modes**: Mouse events are forwarded to modes for future extensibility

### File Operations

- `C-x C-f`: Find file
- `C-x C-s`: Save file

### Editing

- Type to insert text
- `<Backspace>`: Delete character before cursor
- `<Delete>`: Delete character at cursor
- `<Enter>`: Insert newline

### Region Selection & Kill Ring

#### Region Selection

- `C-Space`: Set mark at cursor (start region selection)

#### Kill Ring Operations

- `C-w`: Kill (cut) region between mark and cursor
- `M-w`: Copy region to kill ring without deleting
- `C-k`: Kill (cut) from cursor to end of line
- `C-y`: Yank (paste) most recent kill
- `C-S-y`: Yank from kill-ring index 0

### Command & Control

- `M-x`: Command mode (interactive command execution)
- `C-g`: Cancel current operation (e.g., clear region selection)
- `C-x C-c`: Quit
- `Esc`: Escape

## Building and Running

```bash
# Build the project
cargo build --release

# Run the editor
cargo run
```

## Architecture

Roe is built with a clean separation of concerns:

- **Buffer**: Text storage using `ropey` for efficient editing
- **Window**: View into a buffer with cursor and scroll position
- **Mode**: Defines behavior and keybindings for different editing contexts
- **Editor**: Coordinates buffers, windows, and modes
- **Frame**: Represents the terminal screen real estate

## Current Status

This is a work-in-progress editor. Currently implemented:

- **Text editing**: Basic insertion, deletion, cursor movement
- **Advanced movement**: Word-wise, paragraph-wise, and page navigation with Emacs key bindings
- **Window management**: Split windows horizontally/vertically, switch between windows
- **Buffer management**: Multiple buffers, switching, killing with interactive selection
- **Region selection**: Mark system with visual highlighting
- **Kill ring**: Cut, copy, paste with kill ring history
- **Command mode**: Interactive command execution (M-x) with completion
- **File operations**: Open and save files
- **Mouse integration**: Click-to-position cursor, window switching, border dragging for resizing
- **Terminal UI**: Efficient rendering with borders, modelines, and echo area
- **Auto-clearing messages**: Timed echo message clearing for better UX

## Next steps / not yet implemented

- **Customizable keybindings**: Allow users to redefine key mappings
- **Macro system**: Record and playback keystroke sequences
- **Search and replace**: Interactive search, query-replace functionality
- **Scripting support**: Embed "Steel" Scheme (a scheme interpreter written in Rust) or something
  similar, and rewrite the basic text handling modes in it
- **Syntax highlighting**: TreeSitter integration for language-aware editing
- **LSP integration**: Language server protocol support for modern development features
- **Advanced editing**: Multiple cursors, rectangular selections, etc.

## Contributing & Feedback

This editor is very much a work-in-progress and almost certainly has bugs. It also probably won't
meet your real editing needs yet. However, feedback and bug reports are very welcome!

If you encounter issues or have suggestions, please file them in the project's issue tracker. Even
if the editor isn't ready for daily use, your input helps guide development priorities and catch
problems early.

**Please report:**

- Crashes or unexpected behavior
- Missing features that are essential for your workflow
- Performance issues
- Ideas for improvements or missing Emacs functionality
