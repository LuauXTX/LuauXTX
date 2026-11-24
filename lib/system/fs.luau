local native = ...
local Promise = require("@luauxtx/async/promise")

local fs = {}

function fs.read(path)
    return Promise.new(function(resolve, reject)
        native.fs_read(path, resolve, reject)
    end)
end

function fs.write(path, contents)
    return Promise.new(function(resolve, reject)
        native.fs_write(path, contents, resolve, reject)
    end)
end

function fs.readSync(path)
    return native.fs_read_sync(path)
end

function fs.writeSync(path, contents)
    return native.fs_write_sync(path, contents)
end

function fs.exists(path)
    return native.fs_exists(path)
end

local function async(name, ...)
    local values = table.pack(...)
    return Promise.new(function(resolve, reject)
        values[values.n + 1] = resolve
        values[values.n + 2] = reject
        native[name](table.unpack(values, 1, values.n + 2))
    end)
end

function fs.mkdir(path, recursive) return async("fs_mkdir", path, recursive == true) end
function fs.copy(source, destination, recursive) return async("fs_copy", source, destination, recursive == true) end
function fs.remove(path, recursive) return async("fs_remove", path, recursive == true) end
function fs.rename(source, destination) return async("fs_rename", source, destination) end
function fs.symlink(target, path) return async("fs_symlink", target, path) end
function fs.readDir(path) return async("fs_read_dir", path) end
function fs.stat(path)
    return async("fs_stat", path):andThen(function(is_file, is_directory, size)
        return { isFile = is_file, isDirectory = is_directory, size = size }
    end)
end

return fs
