use super::*;
use serde_json::json;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll, Wake, Waker},
    time::Duration,
};
use winit::{
    application::ApplicationHandler,
    event::{DeviceEvent, DeviceId, ElementState, Ime, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy},
    window::WindowId,
};

struct WakeLoop(EventLoopProxy<()>);
impl Wake for WakeLoop {
    fn wake(self: Arc<Self>) {
        let _ = self.0.send_event(());
    }
    fn wake_by_ref(self: &Arc<Self>) {
        let _ = self.0.send_event(());
    }
}

pub(crate) fn execute(
    lua: &Lua,
    rt: &tokio::runtime::Runtime,
    future: impl Future<Output = Result<()>>,
) -> Result<()> {
    let state = lua
        .app_data_ref::<Rc<State>>()
        .ok_or_else(|| error("graphics host is not registered"))?
        .clone();
    let mut future = Box::pin(future);
    let result = rt.block_on(async {
        tokio::select! {result=&mut future=>Some(result),_=state.notify.notified()=>None}
    });
    if let Some(result) = result {
        return result;
    }
    let request = state
        .request
        .borrow_mut()
        .take()
        .ok_or_else(|| error("missing graphics run request"))?;
    let event_loop = match EventLoop::<()>::with_user_event().build() {
        Ok(el) => el,
        Err(e) => {
            let _ = request.done.send(Err(error(format!(
                "desktop graphics unavailable (a display session is required): {e}"
            ))));
            return rt.block_on(future);
        }
    };
    let waker = Waker::from(Arc::new(WakeLoop(event_loop.create_proxy())));
    let mut app = App {
        state: state.clone(),
        lua,
        rt,
        future: future.as_mut(),
        result: None,
        startup: Some(request.startup),
        done: Some(request.done),
        waker,
        gpu: None,
        sequence: 0,
        started: Instant::now(),
        callbacks: tokio::task::LocalSet::new(),
    };
    let loop_result = event_loop.run_app(&mut app);
    if app.done.is_some() {
        app.finish(Err(error(
            loop_result
                .err()
                .map(|e| format!("graphics event loop failed: {e}"))
                .unwrap_or_else(|| "graphics event loop stopped".into()),
        )));
    }
    let result = app.result.take();
    drop(app);
    state.running.set(false);
    if let Some(result) = result {
        result
    } else {
        rt.block_on(future)
    }
}
struct App<'a> {
    state: Rc<State>,
    lua: &'a Lua,
    rt: &'a tokio::runtime::Runtime,
    future: Pin<&'a mut dyn Future<Output = Result<()>>>,
    result: Option<Result<()>>,
    startup: Option<Function>,
    done: Option<tokio::sync::oneshot::Sender<Result<()>>>,
    waker: Waker,
    gpu: Option<Rc<render::Gpu>>,
    sequence: u64,
    started: Instant,
    callbacks: tokio::task::LocalSet,
}
impl App<'_> {
    fn tick(&mut self, event_loop: &ActiveEventLoop) {
        let _enter = self.rt.enter();
        let mut cx = Context::from_waker(&self.waker);
        // Graphics callbacks can launch async work too. Their LocalSet remains
        // on this main thread and wakes winit when timers or I/O become ready.
        let callbacks_done = Pin::new(&mut self.callbacks).poll(&mut cx).is_ready();
        if self.result.is_none() {
            if let Poll::Ready(result) = self.future.as_mut().poll(&mut cx) {
                self.result = Some(result);
            }
        }
        if self.result.is_some() && callbacks_done {
            event_loop.exit();
        }
    }
    fn call(&self, callback: &Function, args: impl mlua::IntoLuaMulti) -> Result<()> {
        let _runtime = self.rt.enter();
        let _local = self.callbacks.enter();
        callback.call(args)
    }
    fn finish(&mut self, result: Result<()>) {
        for window in self.state.windows.borrow().iter() {
            let mut w = window.borrow_mut();
            w.open = false;
            w.frame = None;
            w.renderer = None;
            w.native = None;
            w.on_frame = None;
            w.on_event = None;
        }
        self.state.windows.borrow_mut().clear();
        self.gpu = None;
        self.state.running.set(false);
        if let Some(done) = self.done.take() {
            let _ = done.send(result);
        }
    }
    fn sync(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        let windows = self.state.windows.borrow().clone();
        for window in windows {
            let mut w = window.borrow_mut();
            if !w.open {
                w.frame = None;
                w.renderer = None;
                w.native = None;
                w.on_frame = None;
                w.on_event = None;
                continue;
            }
            if w.native.is_some() {
                continue;
            }
            let o = &w.options;
            let native = Arc::new(
                event_loop
                    .create_window(
                        Window::default_attributes()
                            .with_title(o.title.clone())
                            .with_inner_size(winit::dpi::LogicalSize::new(
                                o.width as f64,
                                o.height as f64,
                            ))
                            .with_resizable(o.resizable)
                            .with_visible(o.visible),
                    )
                    .map_err(|e| error(format!("cannot create graphics window: {e}")))?,
            );
            let (gpu, surface) = if let Some(gpu) = &self.gpu {
                (
                    gpu.clone(),
                    gpu.instance
                        .create_surface(native.clone())
                        .map_err(|e| error(format!("cannot create graphics surface: {e}")))?,
                )
            } else {
                let (gpu, surface) = render::Gpu::new(event_loop, native.clone())?;
                self.gpu = Some(gpu.clone());
                (gpu, surface)
            };
            let renderer = render::Renderer::new(gpu, surface, native.clone(), o.vsync)?;
            w.scale = native.scale_factor() as f32;
            let size = native.inner_size().to_logical::<f32>(native.scale_factor());
            w.size = [size.width, size.height];
            native.set_cursor(w.cursor);
            native.set_cursor_visible(w.cursor_visible);
            native.set_ime_allowed(true);
            native.request_redraw();
            w.native = Some(native);
            w.renderer = Some(renderer);
        }
        self.state.windows.borrow_mut().retain(|w| w.borrow().open);
        if self.state.windows.borrow().is_empty() && self.startup.is_none() && self.done.is_some() {
            self.finish(Ok(()));
        }
        Ok(())
    }
    fn event(
        &mut self,
        window: &Rc<RefCell<WindowData>>,
        mut event: serde_json::Value,
    ) -> Result<()> {
        self.sequence += 1;
        event["sequence"] = json!(self.sequence);
        event["time"] = json!(self.started.elapsed().as_secs_f64());
        let callback = {
            let mut w = window.borrow_mut();
            if w.events.len() >= 100_000 {
                return Err(error(
                    "graphics input queue exceeds 100000 events; drain pollEvents regularly",
                ));
            }
            w.events.push_back(event.clone());
            w.on_event.clone()
        };
        if let Some(callback) = callback {
            self.call(&callback, json_value(self.lua, &event)?)?;
        }
        Ok(())
    }
    fn window_event_impl(&mut self, id: WindowId, event: WindowEvent) -> Result<()> {
        let window = self
            .state
            .windows
            .borrow()
            .iter()
            .find(|w| w.borrow().native.as_ref().is_some_and(|n| n.id() == id))
            .cloned();
        let Some(window) = window else {
            return Ok(());
        };
        match event{
            WindowEvent::RedrawRequested=>{
                let(callback,dt)={let w=window.borrow();(w.on_frame.clone(),w.last_frame.elapsed().as_secs_f32())};
                if let Some(callback)=callback{
                    window.borrow_mut().in_redraw=true;
                    let result=self.call(&callback,dt);
                    window.borrow_mut().in_redraw=false;
                    result?;
                    if window.borrow().frame.is_some(){return Err(error("onFrame returned with an unfinished frame; call window:endFrame()"));}
                }
            }
            WindowEvent::CloseRequested=>{self.event(&window,json!({"type":"close"}))?;if window.borrow().options.close_on_request{window.borrow_mut().open=false;}}
            WindowEvent::Resized(size)=>{
                let mut w=window.borrow_mut();let logical=size.to_logical::<f32>(w.scale as f64);w.size=[logical.width,logical.height];if let Some(r)=w.renderer.as_mut(){r.resize()?;}if let Some(n)=&w.native{n.request_redraw();}drop(w);
                self.event(&window,json!({"type":"resize","width":logical.width,"height":logical.height}))?;
            }
            WindowEvent::ScaleFactorChanged{scale_factor,..}=>{
                let mut w=window.borrow_mut();w.scale=scale_factor as f32;let size=w.native.as_ref().ok_or_else(||error("missing native window"))?.inner_size().to_logical::<f32>(scale_factor);w.size=[size.width,size.height];drop(w);
                self.event(&window,json!({"type":"scale","scaleFactor":scale_factor,"width":size.width,"height":size.height}))?;
            }
            WindowEvent::Focused(focused)=>{
                if !focused{let mut w=window.borrow_mut();w.capture=CursorGrabMode::None;if let Some(n)=&w.native{let _=n.set_cursor_grab(CursorGrabMode::None);}}
                self.event(&window,json!({"type":"focus","focused":focused}))?;
            }
            WindowEvent::CursorMoved{position,..}=>{let p=position.to_logical::<f64>(window.borrow().scale as f64);self.event(&window,json!({"type":"mouseMove","x":p.x,"y":p.y}))?;}
            WindowEvent::CursorEntered{..}=>self.event(&window,json!({"type":"mouseEnter"}))?,
            WindowEvent::CursorLeft{..}=>self.event(&window,json!({"type":"mouseLeave"}))?,
            WindowEvent::MouseInput{state,button,..}=>self.event(&window,json!({"type":if state==ElementState::Pressed{"mouseDown"}else{"mouseUp"},"button":match button{winit::event::MouseButton::Left=>"left".into(),winit::event::MouseButton::Right=>"right".into(),winit::event::MouseButton::Middle=>"middle".into(),other=>format!("{other:?}")}}))?,
            WindowEvent::MouseWheel{delta,..}=>{let(x,y,unit)=match delta{MouseScrollDelta::LineDelta(x,y)=>(x as f64,y as f64,"lines"),MouseScrollDelta::PixelDelta(p)=>{let p=p.to_logical::<f64>(window.borrow().scale as f64);(p.x,p.y,"pixels")}};self.event(&window,json!({"type":"scroll","x":x,"y":y,"unit":unit}))?;}
            WindowEvent::ModifiersChanged(m)=>{let m=m.state();self.event(&window,json!({"type":"modifiers","shift":m.shift_key(),"control":m.control_key(),"alt":m.alt_key(),"super":m.super_key()}))?;}
            WindowEvent::KeyboardInput{event,..}=>{
                let key=match &event.logical_key{winit::keyboard::Key::Character(text)=>text.to_string(),other=>format!("{other:?}").trim_start_matches("Named(").trim_end_matches(')').to_string()};
                let code=match event.physical_key{winit::keyboard::PhysicalKey::Code(code)=>format!("{code:?}"),other=>format!("{other:?}")};
                self.event(&window,json!({"type":if event.state==ElementState::Pressed{"keyDown"}else{"keyUp"},"key":key,"code":code,"repeat":event.repeat}))?;
                if event.state==ElementState::Pressed{if let Some(text)=event.text{self.event(&window,json!({"type":"textInput","text":text.to_string()}))?;}}
            }
            WindowEvent::Ime(ime)=>{let event=match ime{Ime::Enabled=>json!({"type":"imeEnabled"}),Ime::Disabled=>json!({"type":"imeDisabled"}),Ime::Preedit(text,cursor)=>json!({"type":"imeComposition","text":text,"cursorStart":cursor.map(|p|p.0),"cursorEnd":cursor.map(|p|p.1)}),Ime::Commit(text)=>json!({"type":"textInput","text":text,"ime":true})};self.event(&window,event)?;}
            WindowEvent::Destroyed=>window.borrow_mut().open=false,
            WindowEvent::Occluded(occluded)=>{
                let mut w=window.borrow_mut();w.occluded=occluded;
                if !occluded{w.next_frame=Instant::now();if let Some(n)=&w.native{n.request_redraw();}}
            }
            _=>{}
        }
        Ok(())
    }
}
impl ApplicationHandler<()> for App<'_> {
    fn device_event(&mut self, event_loop: &ActiveEventLoop, _: DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta: (x, y) } = event {
            let captured: Vec<_> = self
                .state
                .windows
                .borrow()
                .iter()
                .filter(|w| {
                    let w = w.borrow();
                    w.open && w.capture != CursorGrabMode::None
                })
                .cloned()
                .collect();
            for window in captured {
                if let Err(e) = self.event(
                    &window,
                    json!({"type":"rawMouseMotion","deltaX":x,"deltaY":y}),
                ) {
                    self.finish(Err(e));
                    break;
                }
            }
            if let Err(e) = self.sync(event_loop) {
                self.finish(Err(e));
            }
            self.tick(event_loop);
        }
    }
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(startup) = self.startup.take() {
            self.state.running.set(true);
            if let Err(e) = self.call(&startup, ()) {
                self.finish(Err(e));
            }
        }
        if let Err(e) = self.sync(event_loop) {
            self.finish(Err(e));
        }
        self.tick(event_loop);
    }
    fn user_event(&mut self, event_loop: &ActiveEventLoop, _: ()) {
        self.tick(event_loop);
        if let Err(e) = self.sync(event_loop) {
            self.finish(Err(e));
        }
    }
    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if let Err(e) = self.window_event_impl(id, event) {
            self.finish(Err(e));
        }
        if let Err(e) = self.sync(event_loop) {
            self.finish(Err(e));
        }
        self.tick(event_loop);
    }
    fn suspended(&mut self, _: &ActiveEventLoop) {
        self.finish(Err(error(
            "desktop graphics session was suspended; windows were released",
        )));
    }
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.tick(event_loop);
        if let Err(e) = self.sync(event_loop) {
            self.finish(Err(e));
        }
        let now = Instant::now();
        let mut deadline = None;
        for w in self.state.windows.borrow().iter() {
            let mut w = w.borrow_mut();
            if w.open
                && w.options.visible
                && !w.occluded
                && w.options.continuous
                && w.on_frame.is_some()
            {
                if let Some(n) = w.native.clone() {
                    let size = n.inner_size();
                    if size.width == 0 || size.height == 0 {
                        continue;
                    }
                    if now >= w.next_frame {
                        n.request_redraw();
                        w.next_frame = now + Duration::from_secs_f32(1. / w.options.fps);
                    }
                    deadline =
                        Some(deadline.map_or(w.next_frame, |d: Instant| d.min(w.next_frame)));
                }
            }
        }
        event_loop.set_control_flow(
            deadline
                .map(ControlFlow::WaitUntil)
                .unwrap_or(ControlFlow::Wait),
        );
    }
}
