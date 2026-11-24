-- Static windows sleep until OS input, explicit redraw, or an asynchronous task.
local gfx = require("@luauxtx/gfx")
local font = gfx.loadFont(nil, 20)
gfx.run(function()
    for index=1,2 do
        local window = gfx.createWindow({ title="Event-driven window " .. index, width=480, height=240, continuous=false })
        local lastEvent = "Resize, focus, or move the mouse."
        window:onEvent(function(event)
            lastEvent = event.type
            -- The queue is separate from onEvent notifications; drain it too.
            window:pollEvents()
            if window:isOpen() then window:requestRedraw() end
        end)
        window:onFrame(function()
            local frame = window:beginFrame()
            frame:clear(25,30,40)
            frame:text({ text=lastEvent, x=20, y=20, font=font })
            frame:pushTransform({ x=150, y=120, rotation=0.25 })
            frame:pushClip({ x=-80, y=-25, width=160, height=50 })
            frame:rect({ x=-100, y=-100, width=200, height=200, color={80,200,140} })
            frame:popClip()
            frame:popTransform()
            window:endFrame()
        end)
    end
end)
font:dispose()
