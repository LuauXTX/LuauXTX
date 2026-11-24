local gfx = require("@luauxtx/gfx")
local ok, err = pcall(function() gfx.run(function() end) end)
if not ok then
    assert(tostring(err):find("desktop graphics unavailable", 1, true), tostring(err))
end
print("graphics startup/display errors are catchable")
