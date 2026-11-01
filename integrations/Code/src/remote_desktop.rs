//! Native, in-process RFB viewer. Observe is the default; input is gated until
//! the owning Ninja's takeover barrier has completed. No desktop data is saved.
use super::*;
use rv_core::{ConnectRequest, EncryptionMode, Keyboard, QualityPreset};
use rv_session::{SessionEvent, SessionHandle};
use std::{cell::Cell, rc::Rc, sync::Arc};

pub(super) struct RemoteDesktop {
    host: String,
    name: String,
    user: Entity<TextInput>,
    password: Entity<TextInput>,
    focus: FocusHandle,
    session: Option<Arc<SessionHandle>>,
    picture: Option<Arc<RenderImage>>,
    generation: u64,
    dimensions: (u16, u16),
    bounds: Rc<Cell<Bounds<Pixels>>>,
    status: String,
    controls: bool,
    taking_over: bool,
    takeover_epoch:u64,
    tint:u32,
    keys: Keyboard,
    buttons: u8,
    last_pointer: (u16, u16),
    visible: bool,
}
impl RemoteDesktop {
    pub fn new(
        host: String,
        name: String,
        username: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let user = cx.new(|cx| TextInput::new(window, cx));
        user.update(cx, |i, cx| {
            i.set_text(&username, cx);
            i.set_placeholder("Mac username", cx);
        });
        let password = cx.new(|cx| {
            let mut i = TextInput::new(window, cx);
            i.set_secret(true, cx);
            i.set_placeholder("Mac password", cx);
            i
        });
        for input in [&user, &password] {
            cx.subscribe(input, |view, _, event, cx| {
                if matches!(event, crate::input::InputEvent::Submit) {
                    view.connect(cx);
                }
            })
            .detach();
        }
        let focus = cx.focus_handle();
        cx.on_blur(&focus, window, |view, _, _| view.release())
            .detach();
        cx.observe_window_activation(window, |view, w, _| {
            if !w.is_window_active() {
                view.release();
            }
        })
        .detach();
        Self {
            host,
            name,
            user,
            password,
            focus,
            session: None,
            picture: None,
            generation: 0,
            dimensions: (0, 0),
            bounds: Rc::new(Cell::new(Bounds::default())),
            status: "Sign in to this Mac to watch its desktop".into(),
            controls: false,
            taking_over:false,
            takeover_epoch:0,
            tint:0x74d6a0,
            keys: Keyboard::new(),
            buttons: 0,
            last_pointer: (0, 0),
            visible: true,
        }
    }
    pub fn controlling(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if !enabled {
            self.release();
        }
        self.controls = enabled;
        self.taking_over=false;
        self.takeover_epoch=self.takeover_epoch.wrapping_add(1);
        cx.notify();
    }
    pub(super) fn connected(&self)->bool{self.session.is_some() && self.picture.is_some() && self.generation>1}
    pub(super) fn is_controlling(&self)->bool{self.controls}
    pub(super) fn takeover_pending(&mut self,tint:u32,cx:&mut Context<Self>)->u64{self.tint=tint;self.taking_over=true;self.takeover_epoch=self.takeover_epoch.wrapping_add(1);self.status="Pausing Ninja and waiting for its tools…".into();cx.notify();self.takeover_epoch}
    pub(super) fn takeover_error(&mut self,error:String,cx:&mut Context<Self>){self.taking_over=false;self.status=error;cx.notify();}
    pub(super) fn takeover_finished(&mut self,epoch:u64,result:Result<(),String>,cx:&mut Context<Self>){if epoch!=self.takeover_epoch || !self.taking_over{return} match result{Ok(())=>{self.status="Connected".into();self.controlling(true,cx)},Err(e)=>self.takeover_error(e,cx)}}
    fn release(&mut self) {
        let keys = self.keys.release_all();
        if let Some(s) = &self.session {
            for (key, down) in keys {
                s.key(key, down);
            }
            if self.buttons != 0 {
                s.pointer(self.last_pointer.0, self.last_pointer.1, 0);
            }
        }
        self.buttons = 0;
    }
    fn connect(&mut self, cx: &mut Context<Self>) {
        self.controlling(false,cx);
        self.session.take();
        if let Some(old)=self.picture.take(){cx.drop_image(old,None);}
        self.generation = 0;
        let request = ConnectRequest {
            connection_id: None,
            name: self.name.clone(),
            host: self.host.clone(),
            port: 5900,
            username: Some(self.user.read(cx).text()),
            password: Some(self.password.read(cx).text()),
            encryption: EncryptionMode::Off,
            quality: QualityPreset::Auto,
            view_only: false,
            shared: true,
        };
        self.password.update(cx, |i, cx| i.set_text("", cx));
        self.status = "Connecting…".into();
        let handle = Arc::new(SessionHandle::spawn(request));
        self.session = Some(handle.clone());
        cx.spawn(async move |this, cx| {
            loop {
                let h = handle.clone();
                // Drain/coalesce and copy the latest pixels off the UI thread.
                let (events, frame) = cx
                    .background_executor()
                    .spawn(async move {
                        let events = h.drain();
                        let latest = events
                            .iter()
                            .any(|e| matches!(e, SessionEvent::FrameReady { .. }));
                        let frame = if latest {
                            let fb = h.framebuffer.lock().ok();
                            fb.filter(|fb| fb.width > 0 && fb.height > 0)
                                .and_then(|fb| {
                                    image::RgbaImage::from_raw(
                                        fb.width as u32,
                                        fb.height as u32,
                                        fb.pixels.clone(),
                                    )
                                    .map(|pixels| {
                                        (
                                            fb.generation,
                                            fb.width,
                                            fb.height,
                                            Arc::new(RenderImage::new(smallvec::smallvec![
                                                image::Frame::new(pixels)
                                            ])),
                                        )
                                    })
                                })
                        } else {
                            None
                        };
                        (events, frame)
                    })
                    .await;
                let delay = this
                    .update(cx, |view, cx| {
                        if !view
                            .session
                            .as_ref()
                            .is_some_and(|s| Arc::ptr_eq(s, &handle))
                        {
                            return None;
                        }
                        let mut changed = false;
                        let mut ended = false;
                        for event in events {
                            match event {
                                SessionEvent::Status(text) => {
                                    view.status = text;
                                    changed = true
                                }
                                SessionEvent::Connected { width, height } => {
                                    view.dimensions = (width, height);
                                    view.status = "Connected".into();
                                    changed = true
                                }
                                SessionEvent::Error(text) => {
                                    view.status = text;
                                    changed = true;
                                    ended = true
                                }
                                SessionEvent::Disconnected => {
                                    if view.status == "Connected" {
                                        view.status = "Disconnected".into();
                                    }
                                    changed = true;
                                    ended = true
                                }
                                _ => {}
                            }
                        }
                        if let Some((generation, w, h, image)) = frame {
                            if generation != view.generation {
                                view.generation = generation;
                                view.dimensions = (w, h);
                                if let Some(old) = view.picture.replace(image) {
                                    cx.drop_image(old, None);
                                }
                                changed = true;
                            }
                        }
                        if changed {
                            cx.notify();
                        }
                        if ended {
                            view.controlling(false,cx);
                            view.session = None;
                            return None;
                        }
                        Some(if view.visible { 16 } else { 120 })
                    })
                    .ok()
                    .flatten();
                let Some(delay) = delay else { break };
                Timer::after(Duration::from_millis(delay)).await;
            }
        })
        .detach();
        cx.notify();
    }
    fn pointer(&mut self, position: Point<Pixels>) {
        if !self.controls {
            return;
        }
        let b = self.bounds.get();
        if let Some((x, y)) = map_pointer(
            (
                f32::from(position.x - b.origin.x),
                f32::from(position.y - b.origin.y),
            ),
            (f32::from(b.size.width), f32::from(b.size.height)),
            self.dimensions,
        ) {
            if let Some(s) = &self.session {
                self.last_pointer = (x, y);
                s.pointer(x, y, self.buttons);
            }
        }
    }
    fn key(&mut self, event: &Keystroke, down: bool, held: bool) {
        if !self.controls {
            return;
        }
        if let Some(s) = &self.session {
            for (key, d) in self.keys.set_modifiers(
                event.modifiers.control,
                event.modifiers.alt,
                event.modifiers.platform,
            ) {
                s.key(key, d);
            }
            let events = if down {
                self.keys.key_down(
                    &event.key,
                    event.key_char.as_deref(),
                    event.modifiers.shift,
                    held,
                )
            } else {
                self.keys.key_up(&event.key)
            };
            for (key, d) in events {
                s.key(key, d);
            }
        }
    }
}
impl Drop for RemoteDesktop {
    fn drop(&mut self) {
        self.release();
        if let Some(s) = self.session.take() {
            s.close();
        }
    }
}
impl Render for RemoteDesktop {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.visible = window.is_window_active();
        if !self.visible && self.buttons != 0 {
            self.release();
        }
        let bounds = self.bounds.clone();
        let body=div()
            .id("remote-desktop-surface")
            .debug_selector(||"remote-desktop-surface".into())
            .size_full()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .gap_2()
            .occlude()
            .child(div().flex().items_center().gap_2().py_1().child(icon(if self.controls{"lock-keyhole-open"}else{"machine-desktop"},14.,if self.controls{self.tint}else{MUTED})).child(if self.controls{"You’re in control · Ninja paused"}else if self.taking_over{"Preparing takeover…"}else{"Watch · input disabled"}).text_size(theme::ui_px(12.)).text_color(rgb(if self.controls{self.tint}else{TEXT})))
            .on_key_down(cx.listener(|v, e: &KeyDownEvent, _, cx| {
                if v.controls {
                    v.key(&e.keystroke, true, e.is_held);
                    cx.stop_propagation();
                }
            }))
            .on_key_up(cx.listener(|v, e: &KeyUpEvent, _, cx| {
                if v.controls {
                    v.key(&e.keystroke, false, false);
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .text_size(theme::ui_px(11.))
                    .text_color(rgb(MUTED))
                    .child(format!(
                        "{} · {} · {}",
                        self.name,
                        if self.controls {
                            "Controlling"
                        } else {
                            "Watching · input disabled"
                        },
                        self.status
                    )),
            )
            .when(self.session.is_none(), |v| {
                v.child(
                    div()
                        .flex()
                        .gap_2()
                        .items_center()
                        .child(div().id("remote-username").debug_selector(||"remote-username".into()).w(px(150.)).h(px(38.)).px_3().flex().items_center().rounded_lg().bg(rgba(GLASS_HOVER)).child(self.user.clone()))
                        .child(div().id("remote-password").debug_selector(||"remote-password".into()).w(px(180.)).h(px(38.)).px_3().flex().items_center().rounded_lg().bg(rgba(GLASS_HOVER)).child(self.password.clone()))
                        .child(
                            div()
                                .id("desktop-connect")
                                .px_3()
                                .py_2()
                                .rounded_lg()
                                .bg(rgba(GLASS_HOVER))
                                .cursor_pointer()
                                .child("Connect")
                                .on_click(cx.listener(|v, _, _, cx| v.connect(cx))),
                        ),
                )
            })
            .child(
                div()
                    .id("remote-desktop-canvas")
                    .debug_selector(||"remote-desktop-canvas".into())
                    .relative()
                    .flex_1()
                    .min_h(px(100.))
                    .overflow_hidden()
                    .rounded_lg()
                    .border_2().border_color(rgba(if self.controls{(self.tint<<8)|0xb0}else{0xffffff12}))
                    .track_focus(&self.focus)
                    .occlude()
                    .bg(rgb(0x090b0e))
                    .child(
                        canvas(
                            move |b, _, _| {
                                bounds.set(b);
                            },
                            |_, _, _, _| {},
                        )
                        .size_full()
                        .absolute(),
                    )
                    .when_some(self.picture.clone(), |v, p| {
                        v.child(img(p).size_full().object_fit(ObjectFit::Contain))
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|v, e: &MouseDownEvent, w, cx| {
                            v.focus.focus(w);
                            if v.controls {
                                v.buttons |= 1;
                                v.pointer(e.position);
                                cx.stop_propagation();
                            }
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|v, e: &MouseUpEvent, _, cx| {
                            if v.controls {
                                v.buttons &= !1;
                                v.pointer(e.position);
                                cx.stop_propagation();
                            }
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|v, e: &MouseDownEvent, _, cx| {
                            if v.controls {
                                v.buttons |= 4;
                                v.pointer(e.position);
                                cx.stop_propagation();
                            }
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Right,
                        cx.listener(|v, e: &MouseUpEvent, _, cx| {
                            if v.controls {
                                v.buttons &= !4;
                                v.pointer(e.position);
                                cx.stop_propagation();
                            }
                        }),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(|v, _: &MouseUpEvent, _, _| v.release()),
                    )
                    .on_mouse_up_out(
                        MouseButton::Right,
                        cx.listener(|v, _: &MouseUpEvent, _, _| v.release()),
                    )
                    .on_mouse_move(cx.listener(|v, e: &MouseMoveEvent, _, _| v.pointer(e.position)))
                    .on_scroll_wheel(cx.listener(|v, e: &ScrollWheelEvent, _, cx| {
                        if v.controls {
                            let d = e.delta.pixel_delta(px(20.));
                            let bit = if d.y > px(0.) { 8 } else { 16 };
                            v.buttons |= bit;
                            v.pointer(e.position);
                            v.buttons &= !bit;
                            v.pointer(e.position);
                            cx.stop_propagation();
                        }
                    })),
            );
        if self.controls && !native::reduced_motion() && window.is_window_active(){
            let tint=self.tint;body.with_animation("remote-control-halo",Animation::new(Duration::from_secs(2)).repeat(),move|v,t|v.border_1().border_color(rgba((tint<<8)|(30.+30.*(t*std::f32::consts::TAU).sin().abs()) as u32)).rounded_xl().p_2()).into_any_element()
        }else{body.border_1().border_color(rgba(0xffffff12)).rounded_xl().p_2().into_any_element()}
    }
}
fn map_pointer(point: (f32, f32), box_: (f32, f32), screen: (u16, u16)) -> Option<(u16, u16)> {
    if screen.0 == 0 || screen.1 == 0 || box_.0 <= 0. || box_.1 <= 0. {
        return None;
    }
    let scale = (box_.0 / screen.0 as f32).min(box_.1 / screen.1 as f32);
    let x = (point.0 - (box_.0 - screen.0 as f32 * scale) / 2.) / scale;
    let y = (point.1 - (box_.1 - screen.1 as f32 * scale) / 2.) / scale;
    (x >= 0. && y >= 0. && x < screen.0 as f32 && y < screen.1 as f32)
        .then_some((x as u16, y as u16))
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    #[gpui::test]
    fn viewer_password_click_stays_in_the_viewer_and_control_canvas_sends_real_input(cx:&mut TestAppContext){
        let (app,cx)=crate::ui_tests::fixture(cx);
        let desktop=cx.update(|w,cx|app.update(cx,|app,cx|{
            app.open_ninjas(Some("local".into()),None,w,cx);app.ninjas.panel.as_mut().unwrap().profile.revision=1;
            let desktop=cx.new(|cx|RemoteDesktop::new("127.0.0.1".into(),"Fixture Mac".into(),"test-user".into(),w,cx));app.ninjas.desktop=Some(desktop.clone());desktop
        }));
        crate::ui_tests::draw_frame(&app,cx);
        let password=cx.debug_bounds("remote-password").unwrap();assert!(password.origin.x>px(200.),"Viewer escaped its chat column");
        cx.simulate_click(password.center(),Modifiers::none());cx.simulate_input("fixture-password");
        cx.update(|_,cx|{assert_eq!(desktop.read(cx).password.read(cx).text(),"fixture-password");assert!(app.read(cx).github.panel.is_none(),"Password click opened the underlying GitHub panel");});
        let(session,mut peer)=SessionHandle::test_pair();
        desktop.update(cx,|v,cx|{v.session=Some(Arc::new(session));v.dimensions=(100,100);v.picture=Some(Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(image::RgbaImage::new(100,100))])));v.controlling(true,cx);});
        crate::ui_tests::draw_frame(&app,cx);
        let canvas=cx.debug_bounds("remote-desktop-canvas").unwrap();cx.simulate_click(canvas.center(),Modifiers::none());cx.simulate_keystrokes("a");
        let mut commands=Vec::new();while let Ok(c)=peer.commands.try_recv(){commands.push(format!("{c:?}"));}
        assert!(commands.iter().any(|s|s.contains("Pointer")),"Canvas click never reached the RFB session: {commands:?}");assert!(commands.iter().any(|s|s.contains("Key")),"Canvas keystroke never reached the RFB session: {commands:?}");
        desktop.update(cx,|v,cx|{let epoch=v.takeover_pending(0xff0000,cx);v.controlling(false,cx);v.takeover_finished(epoch,Ok(()),cx);assert!(!v.controls,"A stale takeover re-enabled input after Watch");});
    }
    #[gpui::test]
    fn observe_never_sends_input_and_watch_releases_control(cx: &mut TestAppContext) {
        let (_, cx) = crate::ui_tests::fixture(cx);
        cx.update(|w, cx| {
            let view = cx
                .new(|cx| RemoteDesktop::new("127.0.0.1".into(), "Mock".into(), "".into(), w, cx));
            let (session, mut peer) = SessionHandle::test_pair();
            view.update(cx, |v, cx| {
                v.session = Some(Arc::new(session));
                v.dimensions = (100, 100);
                v.bounds.set(Bounds {
                    origin: point(px(0.), px(0.)),
                    size: size(px(100.), px(100.)),
                });
                v.pointer(point(px(30.), px(40.)));
                v.key(&Keystroke::parse("a").unwrap(), true, false);
                v.release();
                assert!(peer.commands.try_recv().is_err(), "Observing emitted input");
                v.controlling(true, cx);
                v.pointer(point(px(30.), px(40.)));
                v.key(&Keystroke::parse("a").unwrap(), true, false);
                assert!(peer.commands.try_recv().is_ok());
                v.controlling(false, cx);
                while peer.commands.try_recv().is_ok() {}
                v.pointer(point(px(10.), px(10.)));
                v.key(&Keystroke::parse("b").unwrap(), true, false);
                v.release();
                assert!(
                    peer.commands.try_recv().is_err(),
                    "Watching retained control"
                );
            });
        });
    }
    #[test]
    #[ignore = "Reads host login from stdin and watches an explicitly selected real desktop"]
    fn live_authorized_desktop_stream() {
        use std::io::BufRead;
        let host = std::env::var("KAI_REMOTE_DESKTOP_HOST").expect("Explicit test host");
        let username = std::env::var("KAI_REMOTE_DESKTOP_USER").expect("Explicit test username");
        let control=std::env::var("KAI_REMOTE_CONTROL_POINT").ok().map(|s|{let(x,y)=s.split_once(',').expect("Isolated fixture coordinates");(x.parse::<u16>().unwrap(),y.parse::<u16>().unwrap())});
        let mut password = String::new();
        std::io::stdin().lock().read_line(&mut password).unwrap();
        let password = password.trim_end().to_owned();
        let handle = SessionHandle::spawn(ConnectRequest {
            connection_id: None,
            name: "Isolated viewer validation".into(),
            host,
            port: std::env::var("KAI_REMOTE_DESKTOP_PORT").ok().map(|s|s.parse().unwrap()).unwrap_or(5900),
            username: Some(username),
            password: Some(password),
            encryption: EncryptionMode::Off,
            quality: QualityPreset::Auto,
            view_only: control.is_none(),
            shared: true,
        });
        let until = Instant::now() + Duration::from_secs(35);
        let mut frame = false;
        while Instant::now() < until {
            for e in handle.drain() {
                match e {
                    SessionEvent::FrameReady { .. } => {
                        let fb = handle.framebuffer.lock().unwrap();
                        if fb.generation<=1 {continue} // Resize alone is not a received desktop.
                        if let Some((x,y))=control {
                            let i=((usize::from(y)+60)*usize::from(fb.width)+usize::from(x))*4;
                            assert!(x<fb.width && usize::from(y)+60<usize::from(fb.height),"Fixture outside desktop");
                            if fb.pixels[i..i+3].iter().all(|v|*v==0){continue}
                        }
                        assert!(fb.width > 0 && fb.height > 0);
                        assert_eq!(fb.pixels.len(), fb.width as usize * fb.height as usize * 4);
                        eprintln!(
                            "DESKTOP_FRAME {}x{}; no input sent, no pixels saved",
                            fb.width, fb.height
                        );
                        frame = true;
                    }
                    SessionEvent::Error(e) => panic!("{e}"),
                    _ => {}
                }
            }
            if frame {
                break;
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        assert!(frame, "No authenticated desktop frame received");
        if let Some((x,y))=control {
            // This opt-in path is only for the named, disposable validation
            // window. No arbitrary command, clipboard, or UI text is accepted.
            handle.pointer(x,y,0);std::thread::sleep(Duration::from_millis(120));
            handle.pointer(x,y,1);std::thread::sleep(Duration::from_millis(120));handle.pointer(x,y,0);std::thread::sleep(Duration::from_millis(500));
            for c in "kai-control-ok".bytes(){handle.key(u32::from(c),true);std::thread::sleep(Duration::from_millis(60));handle.key(u32::from(c),false);std::thread::sleep(Duration::from_millis(60));}
            std::thread::sleep(Duration::from_millis(2000));for e in handle.drain(){if let SessionEvent::Error(e)=e{panic!("Remote input transport: {e}")}}eprintln!("DESKTOP_CONTROL_FIXTURE_INPUT_SENT; no pixels saved");
        }
        handle.close();
    }
    #[test]
    fn mapping_respects_letterboxing_and_edges() {
        assert_eq!(
            map_pointer((500., 250.), (1000., 500.), (1920, 1080)),
            Some((960, 540))
        );
        assert_eq!(map_pointer((0., 0.), (1000., 500.), (1920, 1080)), None);
        assert_eq!(map_pointer((1., 1.), (0., 0.), (1, 1)), None);
    }
}
