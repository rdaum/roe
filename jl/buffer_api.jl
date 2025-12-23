# Copyright (C) 2025 Ryan Daum <ryan.daum@gmail.com> This program is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License as published by the Free Software Foundation, version 3.
#
# This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details.
#
# You should have received a copy of the GNU General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
#
# Buffer Access API (calls back into Rust)
# These functions access the current buffer during command execution.
# They use ccall to invoke Rust functions in the main roe binary.

using Libdl

# Get handle to current process for ccall
const _roe_handle = Ref{Ptr{Cvoid}}(C_NULL)

function _get_roe_handle()
    if _roe_handle[] == C_NULL
        # Get handle to current process (the roe binary)
        # Empty string opens the main executable
        _roe_handle[] = Libdl.dlopen("", Libdl.RTLD_NOW | Libdl.RTLD_GLOBAL)
    end
    return _roe_handle[]
end

"""
    buffer_content() -> String

Get the entire content of the current buffer.
"""
function buffer_content()
    handle = _get_roe_handle()
    ptr = ccall(Libdl.dlsym(handle, :roe_buffer_content), Ptr{Cchar}, ())
    if ptr == C_NULL
        return ""
    end
    result = unsafe_string(ptr)
    ccall(Libdl.dlsym(handle, :roe_free_string), Cvoid, (Ptr{Cchar},), ptr)
    return result
end

"""
    buffer_line(line_idx::Int) -> String

Get a single line from the buffer (0-indexed).
"""
function buffer_line(line_idx::Int)
    handle = _get_roe_handle()
    ptr = ccall(Libdl.dlsym(handle, :roe_buffer_line), Ptr{Cchar}, (Clonglong,), line_idx)
    if ptr == C_NULL
        return ""
    end
    result = unsafe_string(ptr)
    ccall(Libdl.dlsym(handle, :roe_free_string), Cvoid, (Ptr{Cchar},), ptr)
    return result
end

"""
    buffer_line_count() -> Int

Get the number of lines in the current buffer.
"""
function buffer_line_count()
    handle = _get_roe_handle()
    return ccall(Libdl.dlsym(handle, :roe_buffer_line_count), Clonglong, ())
end

"""
    buffer_char_count() -> Int

Get the number of characters in the current buffer.
"""
function buffer_char_count()
    handle = _get_roe_handle()
    return ccall(Libdl.dlsym(handle, :roe_buffer_char_count), Clonglong, ())
end

"""
    buffer_substring(start::Int, stop::Int) -> String

Get a substring from the buffer (start inclusive, stop exclusive, 0-indexed).
"""
function buffer_substring(start::Int, stop::Int)
    handle = _get_roe_handle()
    ptr = ccall(Libdl.dlsym(handle, :roe_buffer_substring), Ptr{Cchar}, (Clonglong, Clonglong), start, stop)
    if ptr == C_NULL
        return ""
    end
    result = unsafe_string(ptr)
    ccall(Libdl.dlsym(handle, :roe_free_string), Cvoid, (Ptr{Cchar},), ptr)
    return result
end

"""
    buffer_insert!(pos::Int, text::String)

Insert text at the given position in the buffer (0-indexed).
This directly modifies the buffer.
"""
function buffer_insert!(pos::Int, text::String)
    handle = _get_roe_handle()
    ccall(Libdl.dlsym(handle, :roe_buffer_insert), Cvoid, (Clonglong, Cstring), pos, text)
    return nothing
end

"""
    buffer_delete!(start::Int, stop::Int)

Delete text from start to stop (exclusive, 0-indexed).
This directly modifies the buffer.
"""
function buffer_delete!(start::Int, stop::Int)
    handle = _get_roe_handle()
    ccall(Libdl.dlsym(handle, :roe_buffer_delete), Cvoid, (Clonglong, Clonglong), start, stop)
    return nothing
end

"""
    buffer_major_mode() -> Union{String, Nothing}

Get the major mode of the current buffer, or nothing if no mode is set.
"""
function buffer_major_mode()
    handle = _get_roe_handle()
    ptr = ccall(Libdl.dlsym(handle, :roe_buffer_major_mode), Ptr{Cchar}, ())
    if ptr == C_NULL
        return nothing
    end
    result = unsafe_string(ptr)
    ccall(Libdl.dlsym(handle, :roe_free_string), Cvoid, (Ptr{Cchar},), ptr)
    return result
end

"""
    buffer_show_gutter() -> Bool

Check if the gutter (line numbers, status indicators) should be shown for the current buffer.
"""
function buffer_show_gutter()
    handle = _get_roe_handle()
    result = ccall(Libdl.dlsym(handle, :roe_buffer_show_gutter), Clonglong, ())
    return result != 0
end

"""
    buffer_set_show_gutter!(show::Bool)

Set whether the gutter should be shown for the current buffer.
"""
function buffer_set_show_gutter!(show::Bool)
    handle = _get_roe_handle()
    ccall(Libdl.dlsym(handle, :roe_buffer_set_show_gutter), Cvoid, (Clonglong,), show ? 1 : 0)
    return nothing
end
