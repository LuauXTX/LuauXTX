local gfx = require("@luauxtx/gfx")
local font = gfx.loadFont(nil, 16)
local image = gfx.loadImage("pixel.png")
local backgroundRan = false
local callbackRan = false
local callbackFileRan = false
local frameCount, resizeCount, ordered, presented = 0, 0, 0, 0
local retainedFrame
local second
task.delay(0.05, function() backgroundRan = true end)
gfx.run(function()
    print("graphics startup callback")
    assert(not pcall(function() gfx.createWindow({ width=0, height=100 }) end))
    assert(not pcall(function() gfx.createWindow({ width=100, height=0/0 }) end))
    local window = gfx.createWindow({ title = "LuauXTX native graphics test", width = 320, height = 240, visible = true, vsync = false })
    second = gfx.createWindow({ title = "Second window", width = 200, height = 120, visible = true, continuous = false })
    local hidden = gfx.createWindow({ title="Background lifecycle", width=120, height=80, visible=false })
    task.delay(0.05, function() hidden:close() end)
    second:onFrame(function()
        local frame = second:beginFrame()
        frame:clear(1, 2, 3)
        second:endFrame()
    end)
    window:onEvent(function(event)
        assert(not pcall(function() window:beginFrame() end), "drawing must happen inside onFrame")
        assert(event.sequence > ordered)
        ordered = event.sequence
        if event.type == "resize" then resizeCount += 1 end
    end)
    window:onFrame(function(dt)
        frameCount += 1
        print("graphics frame", frameCount)
        if frameCount == 1 then
            local unbalanced = window:beginFrame()
            unbalanced:pushTransform({x=1})
            assert(not pcall(function() window:endFrame() end))
            assert(not pcall(function() unbalanced:clear(0,0,0) end))
        end
        assert(dt >= 0 and window:getScaleFactor() > 0)
        if retainedFrame then assert(not pcall(function() retainedFrame:rect({ width = 1, height = 1 }) end)) end
        local frame = window:beginFrame()
        retainedFrame = frame
        assert(frame.deltaTime >= 0)
        assert(not pcall(function() window:beginFrame() end))
        assert(not pcall(function() frame:popClip() end))
        assert(not pcall(function() frame:popTransform() end))
        assert(not pcall(function() frame:rect({ width = -1, height = 2 }) end))
        assert(not pcall(function() frame:clear(300, 0, 0) end))
        frame:clear(20, 20, 25)
        frame:rect({ x=5, y=5, width=80, height=40, color={200,20,40,128}, radius=8 })
        frame:rect({ x=5, y=60, width=80, height=40, outline=true, lineWidth=3, radius=8 })
        frame:line({ x1=0, y1=0, x2=100, y2=100, width=3 })
        frame:pushTransform({ x=120, y=20, rotation=0.2, scaleX=1.2 })
        frame:pushClip({ width=120, height=100 })
        frame:text({ text="Unicode: héllo 世界", x=0, y=0, font=font })
        frame:image({ image=image, x=0, y=30, width=50, height=50 })
        frame:popClip()
        frame:popTransform()
        if frameCount == 12 then
            image:dispose()
            font:dispose()
            assert(not pcall(function() frame:image({image=image}) end))
            assert(not pcall(function() frame:text({text="disposed",font=font}) end))
        end
        if window:endFrame() then presented += 1 end
        assert(not pcall(function() window:endFrame() end))
        window:pollEvents()
        second:pollEvents()
        if frameCount == 1 then
            task.delay(0.05, function() callbackRan = true end)
            task.spawn(function()
                fs.write("callback.tmp", "graphics async I/O"):await()
                callbackFileRan = fs.read("callback.tmp"):await() == "graphics async I/O"
            end)
            window:setTitle("Resizing")
            window:setSize(400, 300)
            window:setCursor("pointer")
            window:setCursorVisible(false)
            window:setCursorVisible(true)
            window:setImeEnabled(true)
            window:setImePosition(10, 10, 30, 20)
        end
        if frameCount >= 12 then
            assert(backgroundRan, "Tokio tasks must progress while gfx.run is active")
            assert(callbackRan, "async work launched by frame callbacks must progress")
            assert(callbackFileRan, "async filesystem work launched by frame callbacks must progress")
            window:close()
            second:close()
        end
    end)
end)
assert(frameCount >= 12 and not second:isOpen())
assert(not pcall(function() second:onFrame(function() end) end))
assert(not pcall(function() second:onEvent(function() end) end))
assert(not pcall(function() second:setCursor("pointer") end))
assert(presented > 0, "no native surface frame was presented")
assert(resizeCount > 0, "native resize event was not delivered")
assert(not pcall(function() retainedFrame:clear(0,0,0) end))
image:dispose()
font:dispose()
assert(not pcall(function() gfx.run(function() end) end))
print("native multi-window lifecycle passed")
