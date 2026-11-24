# Native 2D graphics

`require("@luauxtx/gfx")` supplies desktop graphics primitives. It contains no
widgets, layout, themes, or GUI framework. It uses winit, wgpu, and glyphon's
cosmic-text/Swash types for Unicode shaping, measurement, and rasterization.
Text runs are cached as GPU textures so they support the same affine transforms
and exact polygon clips as other drawing operations.

## Execution model

```luau
local gfx = require("@luauxtx/gfx")
local font = gfx.loadFont(nil, 18)
gfx.run(function()
    local window = gfx.createWindow({ title="Hello", width=640, height=480 })
    window:onFrame(function(dt)
        for _, event in window:pollEvents() do
            print(event.type)
        end
        local frame = window:beginFrame()
        frame:clear(20, 20, 25)
        frame:rect({ x=20, y=20, width=150, height=40, radius=8, color={60,120,240} })
        frame:text({ text="Hello World", x=30, y=27, font=font, color={255,255,255} })
        window:endFrame()
    end)
end)
font:dispose()
```

Call `gfx.run(startup)` from a yieldable script, rather than the synchronous
initialization of a `require` module. It returns when all windows are closed.
Only one run is supported per process, including after an initialization error.
Create additional windows during that run. Native windows are created after the
startup callback returns. Draw only inside that window's `onFrame` callback;
`beginFrame` rejects calls from event handlers, tasks, or another window's
callback. Event handlers update Luau state and request redraws. Focus, mouse
capture, IME controls, and `requestRedraw` become available after native creation.

Winit owns the main thread. Luau, graphics callbacks, and local async tasks also
stay on that thread. A Tokio worker services timers/network readiness; future
wakers notify winit, which sleeps between OS events and scheduled frame times.
Existing async tasks continue running while graphics is active. Callbacks may
launch work with `task.spawn`, `task.defer`, asynchronous filesystem operations,
or Promise handlers. Callbacks themselves must return synchronously: put
`:await()`/`task.wait()` in a task, not directly in `startup`, `onFrame`, or
`onEvent`. Callback errors end the graphics run, release its windows, and become
errors from `gfx.run` (which can be caught with `pcall`).

Set `continuous=false` for static, event-driven content. Call `requestRedraw()`
when your state changes. Such windows do not schedule periodic frames. Continuous
windows are capped by `maxFrameRate` and VSync; no zero-time event-pumping loop is
used. Each callback must finish its frame before returning.

## Coordinates and colors

All positions, sizes, line widths, font sizes, mouse positions, and pixel scroll
deltas use **logical pixels**. The origin is the window's top-left; X increases
rightward and Y downward. Winit's scale factor converts these to display pixels.
Font textures are rasterized at the current display scale. Image intrinsic sizes
are source pixels interpreted as logical dimensions unless overridden.

Colors are `{red, green, blue, alpha?}`, with finite channels in `0..255`.
Alpha defaults to 255. Drawing options default to white; `clear` requires RGB.
The renderer converts sRGB colors to linear color for alpha blending and uses
sRGB render targets when the adapter supports them. All primitives are submitted
in call order, including text and images. There is no depth sorting.

## Module functions

| Function | Behavior |
| --- | --- |
| `gfx.run(startup)` | Own the desktop event loop until the last window closes. |
| `gfx.createWindow(options)` | Return a window handle during an active run. |
| `gfx.loadFont(pathOrNil, size)` | Load a TrueType/OpenType file (first face/family), or system sans-serif with `nil`. Size is logical pixels, `0 < size <= 512`. |
| `gfx.measureText(text, font)` | Return `width, height` in logical pixels. Explicit newlines are supported; no automatic wrapping. Height includes line spacing (1.3 times font size per line). |
| `gfx.loadImage(path)` | Decode a PNG/JPEG into an image handle. |

Resources can be loaded and text measured without creating a display or GPU.
Paths are relative to `process.cwd()` and do not cause downloads or hook
execution. Missing files, unsupported/corrupt fonts or images, and disposed
handles produce Luau errors. A requested font file takes precedence over an
installed font with the same family; other system families supply Unicode
fallback. Use an explicit file for reproducible typography across platforms.

Font/image handles are cached by canonical file path (plus font size). Repeated
loads share a resource while it is live; disposing an alias disposes that shared
resource. Dispose before reloading an edited file. Each font caches up to 128
text runs / 16 MiB with least-recently-used eviction. GPU textures and vertex
buffers are reused between frames; expired GPU textures are reclaimed on the
next frame or when the window closes.

## Window options

| Option | Default | Meaning |
| --- | --- | --- |
| `title` | `"LuauXTX"` | Window title, up to 4096 UTF-8 bytes. |
| `width`, `height` | `800`, `600` | Positive logical dimensions, each <= 16384. GPU physical dimension limits also apply. |
| `resizable` | `true` | Permit OS-driven resizing. |
| `visible` | `true` | Show the native window. OS redraw callbacks may be suppressed for hidden windows. |
| `vsync` | `true` | Use automatic VSync; false requests non-VSync with backend fallback. |
| `continuous` | `true` | Schedule frames automatically. |
| `maxFrameRate` | `60` | Continuous frame cap, positive and <= 1000. |
| `closeOnRequest` | `true` | Automatically close after reporting the OS close request. Use false to handle/cancel it in Luau. |

## Window methods

| Method | Behavior |
| --- | --- |
| `window:close()` | Idempotently close and invalidate any active frame. Native resources are released at the next event-loop synchronization. |
| `window:isOpen()` | Return whether the handle is open. |
| `window:setTitle(title)` | Update the title. |
| `window:getSize()` | Return logical `width, height` (last reported size after closing). |
| `window:setSize(width, height)` | Request a size; native resize confirmation is asynchronous. |
| `window:getScaleFactor()` | Return the current display scale (1 before native creation). |
| `window:onFrame(callbackOrNil)` | Set/remove `function(deltaSeconds)` for redraw requests. |
| `window:onEvent(callbackOrNil)` | Set/remove `function(event)` called in OS event order. |
| `window:requestRedraw()` | Request one frame; multiple OS requests may be coalesced. |
| `window:pollEvents()` | Drain ordered queued input/window events into an array. It does not pump the OS event loop. |
| `window:beginFrame()` | Start one frame and return a frame handle. Nested frames are errors. |
| `window:endFrame()` | Finish/present the active frame; return true if presented, false if minimized, occluded, or recovering. |
| `window:setCursor(name)` | Set one of the cursor names below. |
| `window:setCursorVisible(bool)` | Show/hide the cursor while it is over the window. |
| `window:setMouseCapture(bool)` | Request locked capture, falling back to confinement; release with false. Return `locked`, `confined`, or `none`. Unsupported requests produce an error. Capture is released on focus loss. |
| `window:focus()` | Request keyboard focus (the OS may refuse it). |
| `window:setImeEnabled(bool)` | Allow/disallow IME. Enabled by default. |
| `window:setImePosition(x,y,width,height)` | Set the logical candidate/composition area. |

Cursor names: `default`, `pointer`, `text`, `crosshair`, `move`, `grab`,
`grabbing`, `resize-horizontal`, `resize-vertical`, `not-allowed`, `wait`.
Focus and capture depend on the window system and foreground permissions;
Wayland/macOS can restrict them. The runtime does not bypass those restrictions.

`onEvent` notifications do **not** consume the polling queue. Drain it regularly
even if using callbacks. Events remain queued across frames; the runtime never
silently drops old events. Exceeding 100000 queued events per window ends the
run with a clear error. Close events are delivered before automatic closure.
To consume them from a frame callback, choose `closeOnRequest=false` and call
`window:close()` yourself.

## Frame methods

Frames are valid only between `beginFrame` and `endFrame`, and only for their
own window. Retaining a frame does not extend its lifetime. Invalid, closed,
expired, and unbalanced operations produce errors rather than unsafe GPU access.

| Method/property | Parameters |
| --- | --- |
| `frame.deltaTime` | Seconds since this window's previous `beginFrame`. |
| `frame:clear(r,g,b,a?)` | Clear the entire window; erase previous draws in this frame, ignoring transform/clip state. |
| `frame:rect(options)` | `x=0, y=0, width, height, color=white, radius=0, outline=false, lineWidth=1`. Stroke is inside the bounds; radius/stroke are clamped to half the smaller dimension. |
| `frame:line(options)` | `x1=0, y1=0, x2=0, y2=0, width=1, color=white`. Round-ended line. |
| `frame:image(options)` | `image, x=0, y=0, width=imageWidth, height=imageHeight, color=white`. Color tints the image. |
| `frame:text(options)` | `text, font, x=0, y=0, color=white`. Position is the top-left of the line layout, with glyph overhang preserved. |
| `frame:pushClip(options)` | `x=0, y=0, width, height`. Transform this rectangle now and intersect with the previous clip. |
| `frame:popClip()` | Restore the previous clip. |
| `frame:pushTransform(options)` | `x=0, y=0, rotation=0, scaleX=1, scaleY=1`. Compose with the previous affine transform. |
| `frame:popTransform()` | Restore the previous transform. |

Rotation is in radians, clockwise in the Y-down coordinate system. A new
transform scales, then rotates, then translates local points, then applies the
parent transform. Clips retain the transform from when they were pushed;
subsequent transform changes do not move them. Nested rotated/scaled clips are
intersected as convex polygons, including when clipping images and text.
Balance both stacks before `endFrame` (maximum depth 64 each).

Positive drawing dimensions/line widths are required; general coordinates and
transform values must be finite and within +/-10000000. A frame is limited to
100000 draw operations / 1000000 generated vertices. Text is bounded to 64 KiB
per run; rasterized runs to 8192 pixels per dimension / 16 MiB. Font files are
bounded to 16 MiB; images to 8192 pixels per dimension / 64 MiB decoded; encoded
image files to 64 MiB. Up to 16 live windows, 64 fonts, and 256 images are allowed.

## Events

Every event has `type`, `sequence` (globally increasing within this run), and
`time` (seconds since the event loop started). Polling preserves emission order.

| Type | Additional fields |
| --- | --- |
| `close` | OS close request. |
| `resize` | `width`, `height` in logical pixels (may be zero while minimized). |
| `focus` | `focused` boolean. |
| `scale` | `scaleFactor`, logical `width`, `height`. |
| `mouseMove` | Logical `x`, `y`. |
| `rawMouseMotion` | While capturing: unscaled device `deltaX`, `deltaY`. These are raw device units, not display pixels; useful for locked capture, which can suppress absolute movement. |
| `mouseDown`, `mouseUp` | `button`: `left`, `right`, `middle`, or winit's named/other button representation. |
| `mouseEnter`, `mouseLeave` | No additional fields. |
| `scroll` | `x`, `y`, `unit` (`lines` or logical `pixels`). |
| `keyDown`, `keyUp` | `key` (layout-aware character or named key, e.g. `Escape`), `code` (physical winit KeyCode name, e.g. `KeyA`), `repeat`. |
| `modifiers` | `shift`, `control`, `alt`, `super` booleans. |
| `textInput` | Unicode `text`; `ime=true` for an IME commit. |
| `imeComposition` | Preedit `text`; optional `cursorStart`, `cursorEnd` UTF-8 byte offsets. Empty text clears the composition. |
| `imeEnabled`, `imeDisabled` | IME availability notification. |

Use `textInput` rather than interpreting key codes as text. Key-down/up describe
keyboard actions; IME preedit is temporary composition, not committed input.

## Resource disposal and recovery

`font:dispose()`, `image:dispose()`, and `window:close()` are idempotent.
`font:isDisposed()` / `image:isDisposed()` report disposal; `image:getSize()`
returns source `width, height`. GC also releases resources when handles/caches
no longer own them. Draw commands retain their resource snapshots through the
current frame; disposing a handle prevents new use without creating dangling
GPU references. Closed windows release surfaces, pipelines, textures, and
callbacks, even if a Luau window handle is retained.

Zero-sized/minimized windows skip presentation. Outdated/suboptimal surfaces are
reconfigured; lost surfaces are recreated and a redraw requested. Fatal device
errors end the run cleanly. Desktop-session suspension ends the run and releases
its windows; transparent resumption and GPU-device recreation are not provided.
`endFrame=false` indicates a skipped/recovery frame; persistent static content
can explicitly request another redraw when needed.
Hidden, minimized, and OS-reported occluded windows do not schedule periodic
frames. Use tasks/events rather than frame callbacks for background lifecycle
work; OS redraw delivery for invisible windows is not guaranteed.

## Platforms and tests

Native desktop Windows, Linux (X11/Wayland), and macOS are supported where a
window-system session and compatible wgpu adapter are available. Software
adapters may work, with lower performance. Requiring the module or loading CPU
resources does not initialize winit/wgpu. An absent display/GPU produces a
catchable error from `gfx.run`. The portable/browser host reports that this
module requires the native desktop CLI. Mobile/web windowing is not supported.
Linux requires `libxkbcommon` (including compose support), and X11 additionally
requires `libxkbcommon-x11`. On Debian/Ubuntu these are `libxkbcommon0` and
`libxkbcommon-x11-0`. Missing keyboard libraries are reported before winit's
window-system initialization; the runtime never installs them automatically.

Default native backends are Direct3D 12 on Windows, Metal on macOS, and
Vulkan/OpenGL on Linux. `WGPU_BACKEND` can select another compiled backend
(for example `vulkan`, `dx12`, `metal`, or `gl`). `WGPU_ADAPTER_NAME` selects a
case-insensitive adapter-name substring; an unmatched name is a catchable error.
`WGPU_POWER_PREF` overrides the default high-performance preference. Set
`LUAUXTX_GFX_DIAGNOSTICS=1` to print the chosen adapter/backend. These switches
are useful on hybrid-GPU systems and when diagnosing driver compatibility.

Text uses cached raster textures, so extreme transform magnification can blur
it; use an appropriate font size. There are no arbitrary paths, rich-text
layout, custom shaders, text editing, widgets, or layout engines in this API.
Build those policies in external Luau libraries.

```bash
cargo test --locked -p luauxtx
cargo test --locked -p luauxtx --test graphics -- --ignored
luauxtx examples/graphics.luau optional-image.png
luauxtx examples/graphics-events.luau
```

The first command tests geometry, GPU pixel readback when an adapter exists,
resource validation, and existing runtime/package behavior. The second requires
a desktop or Xvfb and tests actual windows, multiple surfaces, resizing, async
task integration, stale frames, disposal, and cleanup. Hardware-dependent tests
report their availability; an unavailable adapter is not evidence of rendering
correctness. Set `LUAUXTX_GFX_SNAPSHOT` to a PNG path to save the readback fixture.
