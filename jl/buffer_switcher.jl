# Copyright (C) 2025 Ryan Daum <ryan.daum@gmail.com> This program is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License as published by the Free Software Foundation, version 3.
#
# This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details.
#
# You should have received a copy of the GNU General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
#
# Buffer Switcher Mode (Julia implementation)
# Handles C-x b (switch-buffer) and C-x k (kill-buffer) interactions.

define_mode("julia-buffer-switcher") do action, state
    action_type = get(action, "type", "")

    # Initialize state on first call or explicit init
    if !haskey(state, "initialized") || action_type == "init"
        state["initialized"] = true
        state["input"] = ""
        state["selected"] = 1
        # Get purpose from action (switch or kill)
        state["purpose"] = get(action, "purpose", "switch")
        # Get buffer list from action (JSON array of {index, name})
        buffers_json = get(action, "buffers", "[]")
        state["buffers"] = _parse_buffer_list(buffers_json)
        state["filtered_buffers"] = copy(state["buffers"])
        # Get preselected index if provided
        preselect = get(action, "preselect", nothing)
        if preselect !== nothing
            idx = findfirst(b -> b["index"] == parse(Int, preselect), state["filtered_buffers"])
            if idx !== nothing
                state["selected"] = idx
            end
        end

        # For init action, immediately return the rendered content
        if action_type == "init"
            return _buffer_switcher_render(state)
        end
    end

    if action_type == "alphanumeric"
        # Add character to filter input
        state["input"] *= action["char"]
        _buffer_switcher_filter!(state)
        return _buffer_switcher_render(state)

    elseif action_type == "backspace"
        # Remove last character from input
        if !isempty(state["input"])
            state["input"] = state["input"][1:end-1]
            _buffer_switcher_filter!(state)
        end
        return _buffer_switcher_render(state)

    elseif action_type == "cursor"
        dir = get(action, "direction", "")
        if dir == "up" && state["selected"] > 1
            state["selected"] -= 1
        elseif dir == "down" && state["selected"] < length(state["filtered_buffers"])
            state["selected"] += 1
        end
        return _buffer_switcher_render(state)

    elseif action_type == "tab"
        # Cycle through matches
        if !isempty(state["filtered_buffers"])
            state["selected"] = mod1(state["selected"] + 1, length(state["filtered_buffers"]))
        end
        return _buffer_switcher_render(state)

    elseif action_type == "enter"
        # Select buffer
        if !isempty(state["filtered_buffers"]) && state["selected"] <= length(state["filtered_buffers"])
            selected_buffer = state["filtered_buffers"][state["selected"]]
            buffer_index = selected_buffer["index"]
            if state["purpose"] == "kill"
                return Dict(
                    "result" => "consumed",
                    "actions" => [KillBufferAction(buffer_index)]
                )
            else
                return Dict(
                    "result" => "consumed",
                    "actions" => [SwitchBufferAction(buffer_index)]
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

# Helper: Parse buffer list JSON
function _parse_buffer_list(json_str::String)
    buffers = Vector{Dict{String, Any}}()
    # Simple JSON array parsing: [{"index":0,"name":"foo"},...]
    # Strip brackets and split by },{
    content = strip(json_str)
    if startswith(content, "[") && endswith(content, "]")
        content = content[2:end-1]  # Remove [ ]
    end
    if isempty(content)
        return buffers
    end

    # Split by "},{" being careful about the pattern
    parts = split(content, "},{")
    for (i, part) in enumerate(parts)
        # Add back braces that were removed by split
        if i == 1
            part = rstrip(part, '}')
        elseif i == length(parts)
            part = lstrip(part, '{')
        end
        part = "{" * strip(part, ['{', '}']) * "}"

        # Parse simple JSON object
        buf = Dict{String, Any}()
        # Extract "index":N
        m_idx = match(r"\"index\"\s*:\s*(\d+)", part)
        if m_idx !== nothing
            buf["index"] = parse(Int, m_idx.captures[1])
        end
        # Extract "name":"..."
        m_name = match(r"\"name\"\s*:\s*\"([^\"]*)\"", part)
        if m_name !== nothing
            buf["name"] = m_name.captures[1]
        end
        if haskey(buf, "index") && haskey(buf, "name")
            push!(buffers, buf)
        end
    end
    return buffers
end

# Helper: Filter buffers based on input
function _buffer_switcher_filter!(state)
    input = lowercase(state["input"])
    if isempty(input)
        state["filtered_buffers"] = copy(state["buffers"])
    else
        state["filtered_buffers"] = filter(b -> contains(lowercase(b["name"]), input), state["buffers"])
    end
    # Reset selection if out of bounds
    if state["selected"] > length(state["filtered_buffers"])
        state["selected"] = max(1, length(state["filtered_buffers"]))
    end
end

# Helper: Render the buffer switcher buffer content
function _buffer_switcher_render(state)
    lines = String[]
    purpose = state["purpose"]
    header = purpose == "kill" ? "Kill buffer:" : "Switch to buffer:"

    # Header
    push!(lines, header)

    # Show filter if any
    has_filter = !isempty(state["input"])
    if has_filter
        push!(lines, "Filter: $(state["input"])")
    end

    push!(lines, "")

    # Calculate max visible items (same as file selector)
    max_visible = 4

    selected = state["selected"]
    buffers = state["filtered_buffers"]
    n_items = length(buffers)

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
        push!(lines, prefix * buffers[i]["name"])
    end

    # Status line
    push!(lines, "")
    action_hint = purpose == "kill" ? "kill" : "switch"
    push!(lines, "[$(n_items) buffers] Arrow keys to navigate, Enter to $(action_hint)")

    content = join(lines, "\n")

    return Dict(
        "result" => "consumed",
        "actions" => [
            ClearTextAction(),
            InsertTextModeAction(content, "start")
        ]
    )
end
