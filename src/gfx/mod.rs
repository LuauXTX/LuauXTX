//! Low-level desktop graphics. Winit owns the main thread; no widgets or layout.
mod geometry;
mod render;
mod resources;
mod runtime;
use geometry::*;
use mlua::{AnyUserData, Function, Lua, Result, Table, UserData, UserDataFields, UserDataMethods};
use resources::*;
pub(crate) use runtime::execute;
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    rc::{Rc, Weak},
    sync::Arc,
    time::Instant,
};
use winit::window::{CursorGrabMode, CursorIcon, Window};

pub(super) struct RunRequest {
    startup: Function,
    done: tokio::sync::oneshot::Sender<Result<()>>,
}
pub(crate) struct State {
    windows: RefCell<Vec<Rc<RefCell<WindowData>>>>,
    resources: RefCell<Resources>,
    request: RefCell<Option<RunRequest>>,
    notify: tokio::sync::Notify,
    used: Cell<bool>,
    running: Cell<bool>,
}
#[derive(Clone)]
struct Options {
    title: String,
    width: f32,
    height: f32,
    fps: f32,
    vsync: bool,
    continuous: bool,
    visible: bool,
    resizable: bool,
    close_on_request: bool,
}
struct WindowData {
    options: Options,
    native: Option<Arc<Window>>,
    renderer: Option<render::Renderer>,
    open: bool,
    size: [f32; 2],
    scale: f32,
    events: VecDeque<serde_json::Value>,
    epoch: u64,
    frame: Option<DrawFrame>,
    on_frame: Option<Function>,
    on_event: Option<Function>,
    last_frame: Instant,
    next_frame: Instant,
    cursor: CursorIcon,
    cursor_visible: bool,
}
pub(super) struct Draw {
    vertices: Vec<Vertex>,
    size: [f32; 2],
    color: Color,
    radius: f32,
    border: f32,
    texture: Option<Rc<Pixels>>,
}
struct DrawFrame {
    clear: Color,
    draws: Vec<Draw>,
    transforms: Vec<Transform>,
    clips: Vec<Option<Vec<Point>>>,
    dt: f32,
    size: [f32; 2],
}
impl DrawFrame {
    fn new(size: [f32; 2], dt: f32) -> Self {
        Self {
            clear: [0., 0., 0., 1.],
            draws: vec![],
            transforms: vec![Transform::IDENTITY],
            clips: vec![None],
            dt,
            size,
        }
    }
    fn add(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: Color,
        radius: f32,
        border: f32,
        texture: Option<Rc<Pixels>>,
    ) -> Result<()> {
        if self.draws.len() >= 100_000 {
            return Err(error("frame exceeds 100000 draw operations"));
        }
        let mut vertices = quad(
            *self
                .transforms
                .last()
                .ok_or_else(|| error("invalid transform stack"))?,
            x,
            y,
            w,
            h,
        );
        if let Some(Some(polygon)) = self.clips.last() {
            vertices = clip(vertices, polygon);
        }
        if vertices.len() >= 3 {
            self.draws.push(Draw {
                vertices,
                size: [w, h],
                color,
                radius,
                border,
                texture,
            });
        }
        Ok(())
    }
}
#[derive(Clone)]
struct WindowHandle {
    window: Rc<RefCell<WindowData>>,
}
struct FrameHandle {
    window: Weak<RefCell<WindowData>>,
    epoch: u64,
}
impl FrameHandle {
    fn access<T>(&self, f: impl FnOnce(&mut DrawFrame) -> Result<T>) -> Result<T> {
        let window = self
            .window
            .upgrade()
            .ok_or_else(|| error("frame window was disposed"))?;
        let mut window = window
            .try_borrow_mut()
            .map_err(|_| error("window is busy"))?;
        if !window.open || window.epoch != self.epoch {
            return Err(error("frame is no longer active"));
        }
        f(window
            .frame
            .as_mut()
            .ok_or_else(|| error("frame is no longer active"))?)
    }
}
impl WindowHandle {
    fn native(&self) -> Result<Arc<Window>> {
        let w = self.window.borrow();
        if !w.open {
            return Err(error("window is closed"));
        }
        w.native
            .clone()
            .ok_or_else(|| error("window is not ready until its first event/frame callback"))
    }
}
fn rect_options(t: &Table) -> Result<(f32, f32, f32, f32, Color)> {
    Ok((
        number(t, "x", 0.)?,
        number(t, "y", 0.)?,
        positive(number(t, "width", 0.)?, "width", 1e7)?,
        positive(number(t, "height", 0.)?, "height", 1e7)?,
        color(t.get("color")?)?,
    ))
}
impl UserData for FrameHandle {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("deltaTime", |_, this| this.access(|f| Ok(f.dt)));
    }
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method(
            "clear",
            |lua, this, (r, g, b, a): (f32, f32, f32, Option<f32>)| {
                let t = lua.create_sequence_from([r, g, b, a.unwrap_or(255.)])?;
                let c = color(Some(t))?;
                this.access(|f| {
                    f.clear = c;
                    f.draws.clear();
                    Ok(())
                })
            },
        );
        m.add_method("rect", |_, this, t: Table| {
            let (x, y, w, h, c) = rect_options(&t)?;
            let radius = number(&t, "radius", 0.)?;
            let line = positive(number(&t, "lineWidth", 1.)?, "lineWidth", 1e7)?;
            if radius < 0. {
                return Err(error("radius must be non-negative"));
            }
            let border = if t.get::<Option<bool>>("outline")?.unwrap_or(false) {
                line.min(w.min(h) / 2.)
            } else {
                0.
            };
            this.access(|f| f.add(x, y, w, h, c, radius.min(w.min(h) / 2.), border, None))
        });
        m.add_method("line", |_, this, t: Table| {
            let x1 = number(&t, "x1", 0.)?;
            let y1 = number(&t, "y1", 0.)?;
            let x2 = number(&t, "x2", 0.)?;
            let y2 = number(&t, "y2", 0.)?;
            let width = positive(number(&t, "width", 1.)?, "line width", 1e7)?;
            let c = color(t.get("color")?)?;
            let length = (x2 - x1).hypot(y2 - y1);
            let (s, cosa) = (y2 - y1).atan2(x2 - x1).sin_cos();
            this.access(|f| {
                let parent = *f
                    .transforms
                    .last()
                    .ok_or_else(|| error("invalid transform stack"))?;
                f.transforms
                    .push(parent.then(Transform([cosa, s, -s, cosa, x1, y1])));
                let result = f.add(
                    -width / 2.,
                    -width / 2.,
                    length + width,
                    width,
                    c,
                    width / 2.,
                    0.,
                    None,
                );
                f.transforms.pop();
                result
            })
        });
        m.add_method("image", |_, this, t: Table| {
            let handle = t
                .get::<AnyUserData>("image")?
                .borrow::<ImageHandle>()?
                .clone();
            let pixels = handle
                .0
                .borrow()
                .as_ref()
                .cloned()
                .ok_or_else(|| error("image is disposed"))?;
            let x = number(&t, "x", 0.)?;
            let y = number(&t, "y", 0.)?;
            let w = positive(number(&t, "width", pixels.width as f32)?, "width", 1e7)?;
            let h = positive(number(&t, "height", pixels.height as f32)?, "height", 1e7)?;
            let c = color(t.get("color")?)?;
            this.access(|f| f.add(x, y, w, h, c, 0., 0., Some(pixels)))
        });
        m.add_method("text", |_, this, t: Table| {
            // Validate the frame before allocating or shaping text.
            this.access(|_| Ok(()))?;
            let text = t.get::<String>("text")?;
            let font = t
                .get::<AnyUserData>("font")?
                .borrow::<FontHandle>()?
                .clone();
            let x = number(&t, "x", 0.)?;
            let y = number(&t, "y", 0.)?;
            let c = color(t.get("color")?)?;
            let scale = this
                .window
                .upgrade()
                .ok_or_else(|| error("window disposed"))?
                .borrow()
                .scale;
            let (pixels, pad) = font
                .0
                .borrow_mut()
                .as_mut()
                .ok_or_else(|| error("font is disposed"))?
                .raster(&text, scale)?;
            let (w, h) = (pixels.width as f32 / scale, pixels.height as f32 / scale);
            this.access(|f| f.add(x - pad, y - pad, w, h, c, 0., 0., Some(pixels)))
        });
        m.add_method("pushTransform", |_, this, t: Table| {
            let t = Transform::from_options(&t)?;
            this.access(|f| {
                if f.transforms.len() >= 64 {
                    return Err(error("transform stack exceeds 64 entries"));
                }
                let combined = f.transforms[f.transforms.len() - 1].then(t);
                for value in combined.0 {
                    finite(value, "composed transform")?;
                }
                f.transforms.push(combined);
                Ok(())
            })
        });
        m.add_method("popTransform", |_, this, ()| {
            this.access(|f| {
                if f.transforms.len() <= 1 {
                    return Err(error("transform stack underflow"));
                }
                f.transforms.pop();
                Ok(())
            })
        });
        m.add_method("pushClip", |_, this, t: Table| {
            let (x, y, w, h, _) = rect_options(&t)?;
            this.access(|f| {
                if f.clips.len() >= 64 {
                    return Err(error("clip stack exceeds 64 entries"));
                }
                let mut polygon = quad(f.transforms[f.transforms.len() - 1], x, y, w, h);
                if let Some(Some(current)) = f.clips.last() {
                    polygon = clip(polygon, current);
                }
                f.clips
                    .push(Some(polygon.iter().map(|v| v.position).collect()));
                Ok(())
            })
        });
        m.add_method("popClip", |_, this, ()| {
            this.access(|f| {
                if f.clips.len() <= 1 {
                    return Err(error("clip stack underflow"));
                }
                f.clips.pop();
                Ok(())
            })
        });
    }
}
impl UserData for WindowHandle {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("isOpen", |_, this, ()| Ok(this.window.borrow().open));
        m.add_method("close", |_, this, ()| {
            let mut w = this.window.borrow_mut();
            w.open = false;
            w.frame = None;
            if let Some(n) = &w.native {
                n.request_redraw();
            }
            Ok(())
        });
        m.add_method("setTitle", |_, this, title: String| {
            if title.len() > 4096 {
                return Err(error("window title exceeds 4096 bytes"));
            }
            let mut w = this.window.borrow_mut();
            if !w.open {
                return Err(error("window is closed"));
            }
            if let Some(n) = &w.native {
                n.set_title(&title);
            }
            w.options.title = title;
            Ok(())
        });
        m.add_method("getSize", |_, this, ()| {
            let w = this.window.borrow();
            Ok((w.size[0], w.size[1]))
        });
        m.add_method("setSize", |_, this, (width, height): (f32, f32)| {
            positive(width, "width", 16384.)?;
            positive(height, "height", 16384.)?;
            let mut w = this.window.borrow_mut();
            if !w.open {
                return Err(error("window is closed"));
            }
            w.size = [width, height];
            w.options.width = width;
            w.options.height = height;
            if let Some(n) = &w.native {
                let _ =
                    n.request_inner_size(winit::dpi::LogicalSize::new(width as f64, height as f64));
            }
            Ok(())
        });
        m.add_method("getScaleFactor", |_, this, ()| {
            Ok(this.window.borrow().scale)
        });
        m.add_method("requestRedraw", |_, this, ()| {
            this.native()?.request_redraw();
            Ok(())
        });
        m.add_method("onFrame", |_, this, callback: Option<Function>| {
            this.window.borrow_mut().on_frame = callback;
            Ok(())
        });
        m.add_method("onEvent", |_, this, callback: Option<Function>| {
            this.window.borrow_mut().on_event = callback;
            Ok(())
        });
        m.add_method("pollEvents", |lua, this, ()| {
            let events: Vec<_> = this.window.borrow_mut().events.drain(..).collect();
            lua.create_sequence_from(
                events
                    .iter()
                    .map(|e| json_value(lua, e))
                    .collect::<Result<Vec<_>>>()?,
            )
        });
        m.add_method("setCursor", |_, this, name: String| {
            let icon = cursor(&name)?;
            let mut w = this.window.borrow_mut();
            w.cursor = icon;
            if let Some(n) = &w.native {
                n.set_cursor(icon);
            }
            Ok(())
        });
        m.add_method("setCursorVisible", |_, this, visible: bool| {
            let mut w = this.window.borrow_mut();
            w.cursor_visible = visible;
            if let Some(n) = &w.native {
                n.set_cursor_visible(visible);
            }
            Ok(())
        });
        m.add_method("setMouseCapture", |_, this, capture: bool| {
            let n = this.native()?;
            let result = if capture {
                n.set_cursor_grab(CursorGrabMode::Locked)
                    .or_else(|_| n.set_cursor_grab(CursorGrabMode::Confined))
            } else {
                n.set_cursor_grab(CursorGrabMode::None)
            };
            result.map_err(|e| error(format!("mouse capture unavailable: {e}")))
        });
        m.add_method("focus", |_, this, ()| {
            this.native()?.focus_window();
            Ok(())
        });
        m.add_method("setImeEnabled", |_, this, enabled: bool| {
            this.native()?.set_ime_allowed(enabled);
            Ok(())
        });
        m.add_method(
            "setImePosition",
            |_, this, (x, y, w, h): (f32, f32, f32, f32)| {
                finite(x, "x")?;
                finite(y, "y")?;
                positive(w, "width", 1e7)?;
                positive(h, "height", 1e7)?;
                this.native()?.set_ime_cursor_area(
                    winit::dpi::LogicalPosition::new(x as f64, y as f64),
                    winit::dpi::LogicalSize::new(w as f64, h as f64),
                );
                Ok(())
            },
        );
        m.add_method("beginFrame", |_, this, ()| {
            let mut w = this.window.borrow_mut();
            if !w.open || w.native.is_none() {
                return Err(error("window is closed or not ready"));
            }
            if w.frame.is_some() {
                return Err(error("a frame is already active"));
            }
            let now = Instant::now();
            let dt = now.duration_since(w.last_frame).as_secs_f32();
            w.last_frame = now;
            w.epoch += 1;
            w.frame = Some(DrawFrame::new(w.size, dt));
            Ok(FrameHandle {
                window: Rc::downgrade(&this.window),
                epoch: w.epoch,
            })
        });
        m.add_method("endFrame", |_, this, ()| {
            let mut w = this.window.borrow_mut();
            if !w.open {
                return Err(error("window is closed"));
            }
            let frame = w.frame.take().ok_or_else(|| error("no frame is active"))?;
            if frame.transforms.len() != 1 || frame.clips.len() != 1 {
                return Err(error("unbalanced transform or clipping stack at endFrame"));
            }
            w.renderer
                .as_mut()
                .ok_or_else(|| error("window is not ready"))?
                .draw(frame)
        });
    }
}
fn cursor(name: &str) -> Result<CursorIcon> {
    Ok(match name {
        "default" => CursorIcon::Default,
        "pointer" => CursorIcon::Pointer,
        "text" => CursorIcon::Text,
        "crosshair" => CursorIcon::Crosshair,
        "move" => CursorIcon::Move,
        "grab" => CursorIcon::Grab,
        "grabbing" => CursorIcon::Grabbing,
        "resize-horizontal" => CursorIcon::EwResize,
        "resize-vertical" => CursorIcon::NsResize,
        "not-allowed" => CursorIcon::NotAllowed,
        "wait" => CursorIcon::Wait,
        _ => return Err(error("unknown cursor shape")),
    })
}
fn json_value(lua: &Lua, v: &serde_json::Value) -> Result<mlua::Value> {
    use serde_json::Value as J;
    Ok(match v {
        J::Null => mlua::Value::Nil,
        J::Bool(b) => mlua::Value::Boolean(*b),
        J::Number(n) => mlua::Value::Number(n.as_f64().unwrap_or(0.)),
        J::String(s) => mlua::Value::String(lua.create_string(s)?),
        J::Array(a) => mlua::Value::Table(
            lua.create_sequence_from(
                a.iter()
                    .map(|x| json_value(lua, x))
                    .collect::<Result<Vec<_>>>()?,
            )?,
        ),
        J::Object(o) => {
            let t = lua.create_table()?;
            for (k, v) in o {
                t.set(k.as_str(), json_value(lua, v)?)?;
            }
            mlua::Value::Table(t)
        }
    })
}
pub(crate) fn module(lua: &Lua) -> Result<Table> {
    let state = Rc::new(State {
        windows: RefCell::new(vec![]),
        resources: RefCell::new(Resources::default()),
        request: RefCell::new(None),
        notify: tokio::sync::Notify::new(),
        used: Cell::new(false),
        running: Cell::new(false),
    });
    lua.set_app_data(state.clone());
    let t = lua.create_table()?;
    let s = state.clone();
    t.set("run",lua.create_async_function(move|_,startup:Function|{let s=s.clone();async move{
        if s.used.replace(true){return Err(error("gfx.run may be called only once per process; nested runs are not supported"));}
        let(tx,rx)=tokio::sync::oneshot::channel();*s.request.borrow_mut()=Some(RunRequest{startup,done:tx});s.notify.notify_one();
        rx.await.map_err(|_|error("graphics event loop stopped"))?
    }})?)?;
    let s = state.clone();
    t.set(
        "createWindow",
        lua.create_function(move |_, options: Option<Table>| {
            if !s.running.get() {
                return Err(error(
                    "createWindow must be called inside gfx.run startup/event/frame callbacks",
                ));
            }
            let options = options.ok_or_else(|| error("createWindow expects an options table"))?;
            let o = Options {
                title: options
                    .get::<Option<String>>("title")?
                    .unwrap_or_else(|| "LuauXTX".into()),
                width: positive(number(&options, "width", 800.)?, "width", 16384.)?,
                height: positive(number(&options, "height", 600.)?, "height", 16384.)?,
                fps: positive(
                    number(&options, "maxFrameRate", 60.)?,
                    "maxFrameRate",
                    1000.,
                )?,
                vsync: options.get::<Option<bool>>("vsync")?.unwrap_or(true),
                continuous: options.get::<Option<bool>>("continuous")?.unwrap_or(true),
                visible: options.get::<Option<bool>>("visible")?.unwrap_or(true),
                resizable: options.get::<Option<bool>>("resizable")?.unwrap_or(true),
                close_on_request: options
                    .get::<Option<bool>>("closeOnRequest")?
                    .unwrap_or(true),
            };
            if o.title.len() > 4096 {
                return Err(error("window title exceeds 4096 bytes"));
            }
            let mut windows = s.windows.borrow_mut();
            if windows.iter().filter(|w| w.borrow().open).count() >= 16 {
                return Err(error("at most 16 windows are supported"));
            }
            let w = Rc::new(RefCell::new(WindowData {
                size: [o.width, o.height],
                options: o,
                native: None,
                renderer: None,
                open: true,
                scale: 1.,
                events: VecDeque::new(),
                epoch: 0,
                frame: None,
                on_frame: None,
                on_event: None,
                last_frame: Instant::now(),
                next_frame: Instant::now(),
                cursor: CursorIcon::Default,
                cursor_visible: true,
            }));
            windows.push(w.clone());
            Ok(WindowHandle { window: w })
        })?,
    )?;
    let s = state.clone();
    t.set(
        "loadFont",
        lua.create_function(move |_, (path, size): (Option<String>, f32)| {
            s.resources.borrow_mut().font(path, size)
        })?,
    )?;
    let s = state.clone();
    t.set(
        "loadImage",
        lua.create_function(move |_, path: String| s.resources.borrow_mut().image(path))?,
    )?;
    t.set(
        "measureText",
        lua.create_function(|_, (text, font): (String, AnyUserData)| {
            let handle = font.borrow::<FontHandle>()?.clone();
            handle
                .0
                .borrow_mut()
                .as_mut()
                .ok_or_else(|| error("font is disposed"))?
                .measure(&text)
        })?,
    )?;
    Ok(t)
}
