# Red

A minimalistic text editor with Emacs-like keybindings and buffer management, built in Rust.

## Features

- **Emacs-style keybindings**: Familiar keyboard shortcuts for Emacs users
- **Buffer-oriented editing**: Work with buffers as the primary unit, not files
- **Window management**: Split windows horizontally/vertically, switch between them
- **Modular architecture**: Extensible mode system for different editing behaviors
- **Terminal-based**: Lightweight, runs in your terminal
- **Fast rendering**: Uses crossterm for efficient terminal manipulation

## Key Bindings

### Cursor Movement
- Arrow keys or `C-f/b/n/p`: Move right/left/down/up
- `C-a`: Beginning of line
- `C-e`: End of line
- `C-<Home>`: Beginning of buffer
- `C-<End>`: End of buffer
- `C-v`: Page down
- `<Page Up/Down>`: Page up/down

### Window Management
- `C-x 2`: Split window horizontally
- `C-x 3`: Split window vertically
- `C-x o`: Switch to other window
- `C-x 0`: Delete current window
- `C-x 1`: Delete all other windows

### File Operations
- `C-x C-f`: Find file (not yet implemented)
- `C-x C-s`: Save file (not yet implemented)

### Editing
- Type to insert text
- `<Backspace>`: Delete character before cursor
- `<Delete>`: Delete character at cursor
- `<Enter>`: Insert newline
- `C-k`: Kill (cut) from cursor to end of line
- `C-w`: Kill region (when region is selected)
- `C-y`: Yank (paste) most recent kill
- `C-S-y`: Yank from kill-ring index 0

### Other
- `C-x C-c`: Quit
- `M-x`: Command mode (not yet implemented)
- `<Esc>`: Escape

## Building and Running

```bash
# Build the project
cargo build --release

# Run the editor
cargo run
```

## Architecture

Red is built with a clean separation of concerns:

- **Buffer**: Text storage using `ropey` for efficient editing
- **Window**: View into a buffer with cursor and scroll position
- **Mode**: Defines behavior and keybindings for different editing contexts
- **Editor**: Coordinates buffers, windows, and modes
- **Frame**: Represents the terminal screen real estate

### Components

- `buffer.rs`: Text buffer implementation with cursor movement APIs
- `editor.rs`: Main editor state and window management
- `keys.rs`: Keyboard input handling and key binding system
- `mode.rs`: Mode system for extensible editor behaviors
- `window.rs`: Window positioning and rendering utilities
- `main.rs`: Terminal setup, event loop, and rendering

The editor uses a tree-based window layout system similar to Emacs, allowing for complex window arrangements through recursive splitting.

## Current Status

This is a work-in-progress editor. Currently implemented:
- Basic text editing and cursor movement
- Window splitting and management  
- Emacs-style keybindings
- Terminal rendering with borders and modelines
- Kill-ring (cut/copy/paste system)

Not yet implemented:
- File I/O
- Syntax highlighting
- Search and replace
- Undo/redo
- Configuration system
- Region selection (mark system)

## Contributing

Red is designed to be clean and extensible. The codebase follows Rust best practices and includes comprehensive tests for core functionality.