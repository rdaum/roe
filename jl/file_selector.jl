# Copyright (C) 2025 Ryan Daum <ryan.daum@gmail.com> This program is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License as published by the Free Software Foundation, version 3.
#
# This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details.
#
# You should have received a copy of the GNU General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
#
# File Selector Mode (Julia implementation)
# Handles C-x C-f (find-file) and C-x C-v (visit-file) interactions.

define_mode("julia-file-selector") do action, state
    action_type = get(action, "type", "")

    # Initialize state on first call or explicit init
    if !haskey(state, "initialized") || action_type == "init"
        state["initialized"] = true
        state["input"] = ""
        state["current_dir"] = pwd()
        state["items"] = String[]
        state["paths"] = String[]
        state["selected"] = 1
        # Get open_type from action if provided (for init)
        state["open_type"] = get(action, "open_type", get(state, "open_type", "new"))
        _file_selector_load_dir!(state)

        # For init action, immediately return the rendered content
        if action_type == "init"
            return _file_selector_render(state)
        end
    end

    if action_type == "alphanumeric"
        # Add character to filter input
        state["input"] *= action["char"]
        _file_selector_filter!(state)
        return _file_selector_render(state)

    elseif action_type == "backspace"
        # Remove last character from input
        if !isempty(state["input"])
            state["input"] = state["input"][1:end-1]
            _file_selector_filter!(state)
        end
        return _file_selector_render(state)

    elseif action_type == "cursor"
        dir = get(action, "direction", "")
        if dir == "up" && state["selected"] > 1
            state["selected"] -= 1
        elseif dir == "down" && state["selected"] < length(state["filtered_items"])
            state["selected"] += 1
        end
        return _file_selector_render(state)

    elseif action_type == "tab"
        # Cycle through matches
        if !isempty(state["filtered_items"])
            state["selected"] = mod1(state["selected"] + 1, length(state["filtered_items"]))
        end
        return _file_selector_render(state)

    elseif action_type == "enter"
        # Select item
        if !isempty(state["filtered_paths"]) && state["selected"] <= length(state["filtered_paths"])
            selected_path = state["filtered_paths"][state["selected"]]
            if isdir(selected_path)
                # Navigate into directory
                state["current_dir"] = selected_path
                state["input"] = ""
                state["selected"] = 1
                _file_selector_load_dir!(state)
                return _file_selector_render(state)
            else
                # Open file
                open_type = state["open_type"]
                return Dict(
                    "result" => "consumed",
                    "actions" => [
                        OpenFileAction(selected_path, open_type)
                    ]
                )
            end
        end
        return Dict("result" => "consumed", "actions" => [])

    elseif action_type == "escape" || action_type == "cancel"
        # Let editor handle cancel
        return Dict("result" => "ignored")

    else
        return Dict("result" => "ignored")
    end
end

# Helper: Load directory contents into state
function _file_selector_load_dir!(state)
    dir = state["current_dir"]
    items = String[]
    paths = String[]

    # Add parent directory if not at root
    parent = dirname(dir)
    if parent != dir
        push!(items, "../")
        push!(paths, parent)
    end

    # Read directory
    try
        entries = readdir(dir)
        dirs = String[]
        files = String[]
        dir_paths = String[]
        file_paths = String[]

        for entry in entries
            full_path = joinpath(dir, entry)
            if isdir(full_path)
                push!(dirs, entry * "/")
                push!(dir_paths, full_path)
            else
                push!(files, entry)
                push!(file_paths, full_path)
            end
        end

        # Sort and add dirs first, then files
        perm_dirs = sortperm(dirs)
        perm_files = sortperm(files)

        for i in perm_dirs
            push!(items, dirs[i])
            push!(paths, dir_paths[i])
        end
        for i in perm_files
            push!(items, files[i])
            push!(paths, file_paths[i])
        end
    catch e
        @warn "Failed to read directory" dir exception=e
    end

    state["items"] = items
    state["paths"] = paths
    state["filtered_items"] = copy(items)
    state["filtered_paths"] = copy(paths)
end

# Helper: Filter items based on input
function _file_selector_filter!(state)
    input = lowercase(state["input"])
    if isempty(input)
        state["filtered_items"] = copy(state["items"])
        state["filtered_paths"] = copy(state["paths"])
    else
        filtered_items = String[]
        filtered_paths = String[]
        for (item, path) in zip(state["items"], state["paths"])
            if contains(lowercase(item), input)
                push!(filtered_items, item)
                push!(filtered_paths, path)
            end
        end
        state["filtered_items"] = filtered_items
        state["filtered_paths"] = filtered_paths
    end
    # Reset selection if out of bounds
    if state["selected"] > length(state["filtered_items"])
        state["selected"] = max(1, length(state["filtered_items"]))
    end
end

# Helper: Render the file selector buffer content
function _file_selector_render(state)
    lines = String[]

    # Header with current directory
    push!(lines, "Directory: $(state["current_dir"])")

    # Show filter if any
    has_filter = !isempty(state["input"])
    if has_filter
        push!(lines, "Filter: $(state["input"])")
    end

    push!(lines, "")

    # Calculate max visible items
    max_visible = 4

    selected = state["selected"]
    items = state["filtered_items"]
    n_items = length(items)

    # Calculate visible window
    if n_items <= max_visible
        start_idx = 1
        end_idx = n_items
    else
        half = div(max_visible, 2)
        if selected <= half
            start_idx = 1
            end_idx = max_visible
        elseif selected > n_items - half
            start_idx = n_items - max_visible + 1
            end_idx = n_items
        else
            start_idx = selected - half
            end_idx = selected + half - 1
        end
    end

    for i in start_idx:end_idx
        prefix = i == selected ? "> " : "  "
        push!(lines, prefix * items[i])
    end

    # Status line
    push!(lines, "")
    push!(lines, "[$(n_items) items] Arrow keys to navigate, Enter to select, Tab to cycle")

    content = join(lines, "\n")

    return Dict(
        "result" => "consumed",
        "actions" => [
            ClearTextAction(),
            InsertTextModeAction(content, "start")
        ]
    )
end
