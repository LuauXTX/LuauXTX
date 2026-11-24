-- luauxtx examples/graphics.luau [optional-image.png]
local gfx = require("@luauxtx/gfx")
local font = gfx.loadFont(nil, 18)
local image = if process.argv[3] then gfx.loadImage(process.argv[3]) else nil
local x, y, seconds = 0, 0, 0
local input = "Move the mouse; type Unicode text; Escape closes the window."
gfx.run(function()
    local window = gfx.createWindow({ title="LuauXTX graphics primitives", width=800, height=600, closeOnRequest=false })
    window:onFrame(function(dt)
        seconds += dt
        for _, event in window:pollEvents() do
            if event.type == "close" or (event.type == "keyDown" and event.key == "Escape") then window:close(); return end
            if event.type == "mouseMove" then x, y = event.x, event.y end
            if event.type == "textInput" then input = "Text input: " .. event.text end
            if event.type == "imeComposition" then input = "IME preedit: " .. event.text end
        end
        window:setCursor("crosshair")
        window:setImePosition(20, 80, 300, 24)
        local width, height = window:getSize()
        local frame = window:beginFrame()
        frame:clear(20, 20, 25)
        frame:rect({ x=20, y=20, width=180, height=40, radius=8, color={60,120,240} })
        frame:text({ text="Hello World", x=30, y=27, font=font })
        frame:text({ text=input, x=20, y=80, font=font })
        frame:rect({ x=20, y=130, width=math.max(1,width-40), height=math.max(1,height-160), outline=true, lineWidth=2, radius=16, color={100,110,130} })
        frame:pushClip({ x=22, y=132, width=math.max(1,width-44), height=math.max(1,height-164) })
        frame:pushTransform({ x=width/2, y=height/2, rotation=seconds*0.3 })
        frame:rect({ x=-110, y=-50, width=220, height=100, radius=20, color={240,80,90,180} })
        frame:text({ text="Transformed text", x=-85, y=-12, font=font })
        if image then frame:image({ image=image, x=-50, y=65, width=100, height=100 }) end
        frame:popTransform()
        frame:line({ x1=0, y1=y, x2=width, y2=y, color={70,180,220,150} })
        frame:line({ x1=x, y1=0, x2=x, y2=height, color={70,180,220,150} })
        frame:popClip()
        window:endFrame()
    end)
end)
font:dispose()
if image then image:dispose() end
