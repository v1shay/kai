//! A demand-created native window over the existing Orbit entity. The panel has
//! its own input geometry, but shares drafts, agents, history and runtime state.
use super::*;
use global_hotkey::{GlobalHotkey, Shortcut};

pub struct Controller {
    orbit: Entity<Orbit>,
    window: Option<WindowHandle<QuickChat>>,
    hotkey: Option<GlobalHotkey>,
}
impl gpui::Global for Controller {}
impl Controller {
    pub fn new(orbit: Entity<Orbit>) -> Self {
        Self {
            orbit,
            window: None,
            hotkey: None,
        }
    }
    /// Install after placing this Controller in App globals. Rebinding preserves
    /// a working registration if the candidate conflicts with another app.
    pub fn bind(&mut self, shortcut: Shortcut, cx: &mut App) -> anyhow::Result<()> {
        if let Some(hotkey) = &mut self.hotkey {
            return hotkey.rebind(shortcut);
        }
        let hotkey = GlobalHotkey::new(shortcut)?;
        let events = hotkey.events.clone();
        self.hotkey = Some(hotkey);
        cx.spawn(async move |cx| {
            while events.recv().await.is_ok() {
                let alive = cx.update(|cx| {
                    if !cx.has_global::<Controller>() {
                        return false;
                    }
                    cx.update_global::<Controller, _>(|service, cx| {
                        let recorder = service.orbit.read(cx).shortcut_recorder.clone();
                        let recording = recorder.as_ref().is_some_and(|recorder| {
                            cx.active_window().is_some_and(|window| {
                                window
                                    .update(cx, |_, window, cx| {
                                        recorder.read(cx).is_capturing(window)
                                    })
                                    .unwrap_or(false)
                            })
                        });
                        if let Some(recorder) = recorder.filter(|_| recording) {
                            // Carbon consumes the already registered chord before
                            // NSEvent reaches the recorder. Recording that same
                            // chord confirms it instead of opening another window.
                            recorder.update(cx, |recorder, cx| recorder.record_current(cx));
                            return;
                        }
                        if let Err(error) = service.toggle(cx) {
                            service.orbit.update(cx, |app, cx| {
                                app.notice = Some(format!("Quick Chat: {error}"));
                                cx.notify();
                            });
                        }
                    });
                    true
                });
                if !matches!(alive, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
        Ok(())
    }
    pub fn toggle(&mut self, cx: &mut App) -> anyhow::Result<()> {
        unified::quick_prepare();
        if let Some(window) = self.window {
            if window
                .update(cx, |panel, window, cx| {
                    if panel.visible && native_panel::visible(window) && window.is_window_active() {
                        panel.hide(window, cx);
                    } else {
                        panel.show(window, cx);
                    }
                })
                .is_ok()
            {
                return Ok(());
            }
            self.window = None;
        }
        self.open(cx)
    }
    pub(super) fn dismiss(&mut self,cx:&mut App){
        self.dismiss_at(None,cx);
    }
    pub(super) fn dismiss_at(&mut self,issued_at:Option<f64>,cx:&mut App){
        if let Some(window)=self.window{let _=window.update(cx,|panel,window,cx|panel.hide_if_current(issued_at,window,cx));}
    }
    pub fn open(&mut self, cx: &mut App) -> anyhow::Result<()> {
        unified::quick_prepare();
        let result=self.open_surface(cx);
        if result.is_err(){unified::quick_visibility(false);}
        result
    }
    fn open_surface(&mut self,cx:&mut App)->anyhow::Result<()>{
        if let Some(window) = self.window
            && window
                .update(cx, |panel, window, cx| panel.show(window, cx))
                .is_ok()
        {
            return Ok(());
        }
        let orbit = self.orbit.clone();
        let bounds = Bounds::centered(None, size(px(680.), px(520.)), cx);
        let window = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(540.), px(360.))),
                titlebar: None,
                kind: WindowKind::Floating,
                is_minimizable: false,
                is_resizable: true,
                window_background: WindowBackgroundAppearance::Blurred,
                app_id: Some("dev.orbit.agents.quick-chat".into()),
                ..Default::default()
            },
            move |window, cx| {
                native::configure(window);
                native_panel::configure(window);
                cx.new(|cx| QuickChat::new(orbit, window, cx))
            },
        )?;
        self.window = Some(window);
        window.update(cx, |panel, window, cx| panel.show(window, cx))?;
        Ok(())
    }
}

pub struct QuickChat {
    orbit: Entity<Orbit>,
    pub(super) composer: Entity<TextInput>,
    visible: bool,
    show_generation: u64,
    shown_at:f64,
    transcript_scroll: ListState,
    transcript_follow:bool,
    transcript_revision: Option<(Option<String>, usize, Option<u64>,u64)>,
    _subscriptions: Vec<Subscription>,
}
impl QuickChat {
    pub(super) fn new(orbit: Entity<Orbit>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let main_composer = orbit.read(cx).composer.clone();
        let draft = main_composer.read(cx).text();
        let composer = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_placeholder("Ask, build, or explore…", cx);
            input.set_image_paste(true);
            input.set_text(&draft, cx);
            input
        });
        let input_changes = cx.subscribe_in(&composer, window, |this, _, event, window, cx| {
            if !this.visible {
                return;
            }
            match event {
                InputEvent::Changed | InputEvent::CursorChanged => {
                    this.sync_to_main(cx);
                }
                InputEvent::Submit => this.submit(window, cx),
                InputEvent::ImagePasted(bytes) => this
                    .orbit
                    .update(cx, |app, cx| app.add_clipboard_image(bytes.clone(), cx)),
                InputEvent::ClipboardError(error) => this.orbit.update(cx, |app, cx| {
                    app.notice = Some(error.clone());
                    cx.notify();
                }),
                InputEvent::DictationCancelled | InputEvent::LimitReached => {
                    this.orbit.update(cx, |app, cx| {
                        app.stop_dictation(cx);
                        if matches!(event, InputEvent::LimitReached) {
                            app.notice = Some(
                                "Keep the prompt under 64 KB. Reference large files by path."
                                    .into(),
                            );
                            cx.notify();
                        }
                    })
                }
                InputEvent::CommandPrevious => {
                    this.orbit.update(cx, |app, cx| app.move_slash(-1, cx))
                }
                InputEvent::CommandNext => this.orbit.update(cx, |app, cx| app.move_slash(1, cx)),
                InputEvent::CommandAccept => this
                    .orbit
                    .update(cx, |app, cx| app.accept_slash(false, window, cx)),
                InputEvent::CommandComplete => this
                    .orbit
                    .update(cx, |app, cx| app.accept_slash(true, window, cx)),
                _ => {}
            }
        });
        let main_changes = cx.observe(&main_composer, |this, _, cx| {
            if this.visible {
                this.sync_from_main(cx);
            }
        });
        let orbit_changes = cx.observe(&orbit, |this, _, cx| {
            if this.visible {
                cx.notify();
            }
        });
        let activation=cx.observe_window_activation(window,|this,window,cx|{
            if !window.is_window_active(){this.orbit.update(cx,|app,cx|{app.dismiss_controls_for_window(window.window_handle().window_id());cx.notify();});}
        });
        let background = orbit.read(cx).background.clone();
        let background_changes = cx.observe(&background, |this, _, cx| {
            if this.visible {
                cx.notify();
            }
        });
        let weak = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            let _ = weak.update(cx, |this, cx| this.hide(window, cx));
            false // Closing this surface never tears down an agent/runtime.
        });
        let transcript_scroll=ListState::new(0,ListAlignment::Bottom,px(400.));
        transcript_scroll.set_scroll_handler(cx.listener(|this,event:&ListScrollEvent,_,cx|{
            this.transcript_follow=!event.is_scrolled;let earlier=event.is_scrolled;let start=event.visible_range.start;let generation=this.orbit.read(cx).generation;let weak=this.orbit.downgrade();
            if earlier{cx.defer(move|cx|{let _=weak.update(cx,|app,cx|{if generation==app.generation && app.history_near_top(start){app.load_history_page(true,cx);}});});}
        }));
        Self {
            orbit,
            composer,
            visible: false,
            show_generation: 0,
            shown_at:0.,
            transcript_scroll,transcript_follow:true,
            transcript_revision: None,
            _subscriptions: vec![
                input_changes,
                main_changes,
                orbit_changes,
                background_changes,
                activation,
            ],
        }
    }
    #[cfg(test)]
    pub(super) fn is_visible(&self) -> bool {
        self.visible
    }
    fn sync_to_main(&mut self, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).text();
        let caret=self.composer.read(cx).command_caret();
        let main = self.orbit.read(cx).composer.clone();
        if main.read(cx).text() != text {
            main.update(cx, |input, cx| input.set_text(&text, cx));
        }
        main.update(cx,|input,cx|input.follow_command_caret(caret,cx));
    }
    fn sync_from_main(&mut self, cx: &mut Context<Self>) {
        let main = self.orbit.read(cx).composer.clone();
        let text = main.read(cx).text();
        let follow = main.read(cx).is_dictating();
        self.composer
            .update(cx, |input, cx| input.follow_mirrored_dictation(follow, cx));
        let navigation=self.orbit.read(cx).slash.as_ref().is_some_and(|p|!p.closing);
        self.composer.update(cx,|input,_|input.set_command_navigation(navigation));
        if self.composer.read(cx).text() != text {
            self.composer
                .update(cx, |input, cx| input.set_text(&text, cx));
        }
    }
    pub(super) fn show(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.shown_at=workspace_motion::now();
        unified::quick_prepare();
        self.sync_from_main(cx);
        self.visible = true;
        self.show_generation = self.show_generation.wrapping_add(1);
        self.orbit.update(cx, |app, _| {
            let id=window.window_handle().window_id();
            if let Some(previous)=app.overlay_window.filter(|previous|*previous!=id){app.dismiss_controls_for_window(previous);}
            app.overlay_window = Some(id)
        });
        native::apply_theme(window, &self.orbit.read(cx).resolved_theme);
        native::apply_appearance(window, &self.orbit.read(cx).effective_appearance());
        native_panel::show(window);
        unified::quick_visibility(true);
        window.activate_window();
        cx.activate(true);
        self.composer.read(cx).focus_handle.focus(window);
        cx.notify();
    }
    pub(super) fn hide(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A delayed/repeated dismissal must not write this retired editor's
        // draft over the composer now being used in Code or the notch.
        if !self.visible{return}
        self.sync_to_main(cx);
        self.visible = false;
        // One-shot dictation belongs to this editor. Conversational voice
        // belongs to the shared chat and continues through surface dismissal.
        self.orbit.update(cx, |app, cx| {
            app.stop_dictation(cx);
            if app.overlay_window == Some(window.window_handle().window_id()) {
                app.overlay_window = None;
                app.dismiss_panel(window, cx);
                app.dismiss_agent_picker();
                app.dismiss_action_menu();
            }
        });
        window.blur();
        native_panel::hide(window);
        unified::quick_visibility(false);
    }
    pub(super) fn hide_if_current(&mut self,issued_at:Option<f64>,window:&mut Window,cx:&mut Context<Self>){
        if issued_at.is_none_or(|issued|issued>=self.shown_at){self.hide(window,cx)}
    }
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_to_main(cx);
        self.orbit.update(cx, |app, cx| {
            app.send(window, cx);
        });
        self.composer.read(cx).focus_handle.focus(window);
        self.sync_from_main(cx);
    }
    fn escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let overlay = {
            let app = self.orbit.read(cx);
            app.modal.is_some() || app.agent_picker.is_some() || app.action_menu.is_some()
        };
        if overlay {
            self.orbit.update(cx, |app, cx| {
                app.dismiss_panel(window, cx);
                app.dismiss_agent_picker();
                app.dismiss_action_menu();
                cx.notify();
            });
            self.composer.read(cx).focus_handle.focus(window);
        } else {
            self.hide(window, cx);
        }
    }
}
impl Render for QuickChat {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let revision = {
            let app = self.orbit.read(cx);
            (
                app.active.clone(),
                app.rows.len(),
                app.active
                    .as_ref()
                    .and_then(|id| app.runs.get(id))
                    .map(|run| run.revision),
                app.flow_revision,
            )
        };
        if self.transcript_revision.as_ref() != Some(&revision) {
            let session_changed = self
                .transcript_revision
                .as_ref()
                .is_none_or(|previous| previous.0 != revision.0);
            let anchor=self.transcript_scroll.logical_scroll_top();
            let at_bottom = self.transcript_follow;
            self.transcript_scroll.reset(revision.1);
            self.transcript_revision = Some(revision);
            if session_changed || at_bottom {
                self.transcript_scroll.scroll_to(ListOffset{item_ix:self.orbit.read(cx).rows.len(),offset_in_item:px(0.)});
            }else{
                self.transcript_scroll.scroll_to(anchor);
            }
        }
        if !self.orbit.read(cx).work_animations.is_empty()||!self.orbit.read(cx).interaction_animations.is_empty(){let anchor=self.transcript_scroll.logical_scroll_top();self.transcript_scroll.reset(self.orbit.read(cx).rows.len());self.transcript_scroll.scroll_to(anchor);}
        // Legacy workspace helpers focus their own composer after selection.
        // Redirect only that handle; search/model/permission fields keep focus.
        let main_focus = self.orbit.read(cx).composer.read(cx).focus_handle.clone();
        if main_focus.is_focused(window) {
            self.composer.read(cx).focus_handle.focus(window);
        }
        native::apply_appearance(window, &self.orbit.read(cx).effective_appearance());
        native::apply_theme(window, &self.orbit.read(cx).resolved_theme);
        let surface = self.orbit.update(cx, |app, cx| {
            app.quick_chat_surface(&self.composer, &self.transcript_scroll, window, cx)
        });
        let view = div()
            .key_context("QuickChat")
            .size_full()
            .rounded(px(20.))
            .overflow_hidden()
            .font_family(theme::ui_font())
            .text_size(theme::ui_px(13.))
            .text_color(rgb(TEXT))
            .capture_any_mouse_down(cx.listener(|this, _, _, cx| {
                this.orbit.update(cx, |app, _| {
                    app.panel_at_pointer_down = app.modal;
                    app.remember_picker_press();
                });
            }))
            .on_action(cx.listener(|this, _: &Escape, window, cx| this.escape(window, cx)))
            .on_action(cx.listener(|this, _: &CloseWindow, window, cx| this.hide(window, cx)))
            .on_action(cx.listener(|this, _: &Send, window, cx| this.submit(window, cx)))
            .on_action(cx.listener(|this, action: &keyboard::Execute, window, cx| {
                if action.id=="composer"{this.composer.read(cx).focus_handle.focus(window);}else{this.sync_to_main(cx);this.orbit.update(cx,|app,cx|app.execute_shortcut(&action.id,window,cx));this.sync_from_main(cx);}
            }))
            .on_action(cx.listener(|this, _: &NewConversation, window, cx| {
                this.orbit
                    .update(cx, |app, cx| app.new_conversation(window, cx));
                this.sync_from_main(cx);
                this.composer.read(cx).focus_handle.focus(window);
            }))
            .on_action(cx.listener(|this, _: &ToggleSearch, window, cx| {
                this.orbit
                    .update(cx, |app, cx| app.show_modal(Modal::Search, window, cx));
            }))
            .on_action(cx.listener(|this, _: &ToggleSettings, window, cx| {
                this.orbit
                    .update(cx, |app, cx| app.show_modal(Modal::Settings, window, cx));
            }))
            .child(surface);
        if self.visible && self.orbit.read(cx).motion && !native::reduced_motion() {
            view.with_animation(
                ("quick-chat-enter", self.show_generation),
                Animation::new(Duration::from_millis(360)),
                |view, t| {
                    view.opacity((t / 0.55).min(1.))
                        .relative()
                        .top(px((1. - presentation::spring(t)) * 10.))
                },
            )
            .into_any_element()
        } else {
            view.into_any_element()
        }
    }
}

#[derive(Clone, Debug)]
pub enum RecorderEvent {
    Recorded(Shortcut),
    Cancelled,
}
pub struct ShortcutRecorder {
    pub focus_handle: FocusHandle,
    shortcut: Shortcut,
    recording: bool,
    error: Option<String>,
    blur: Option<Subscription>,
    activation: Option<Subscription>,
}
impl gpui::EventEmitter<RecorderEvent> for ShortcutRecorder {}
impl ShortcutRecorder {
    pub fn new(shortcut: Shortcut, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            shortcut: shortcut.normalized(),
            recording: false,
            error: None,
            blur: None,
            activation: None,
        }
    }
    pub fn begin(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.recording = true;
        self.error = None;
        self.blur = Some(
            cx.on_blur(&self.focus_handle.clone(), window, |this, _, cx| {
                this.cancel(cx);
            }),
        );
        self.activation = Some(cx.observe_window_activation(window, |this, window, cx| {
            if !window.is_window_active() {
                this.cancel(cx);
            }
        }));
        self.focus_handle.focus(window);
        cx.notify();
    }
    pub fn set_shortcut(&mut self, shortcut: Shortcut, cx: &mut Context<Self>) {
        self.shortcut = shortcut.normalized();
        self.recording = false;
        self.error = None;
        cx.notify();
    }
    pub fn set_error(&mut self, message: &str, cx: &mut Context<Self>) {
        self.recording = false;
        self.error = Some(message.chars().take(180).collect());
        cx.notify();
    }
    pub fn is_recording(&self) -> bool {
        self.recording
    }
    pub(super) fn is_capturing(&self, window: &Window) -> bool {
        self.is_recording() && window.is_window_active() && self.focus_handle.is_focused(window)
    }
    fn cancel(&mut self, cx: &mut Context<Self>) {
        if self.recording {
            self.recording = false;
            self.error = None;
            cx.emit(RecorderEvent::Cancelled);
            cx.notify();
        }
    }
    fn record_current(&mut self, cx: &mut Context<Self>) {
        self.recording = false;
        self.error = None;
        cx.emit(RecorderEvent::Recorded(self.shortcut.clone()));
        cx.notify();
    }
    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.recording {
            return;
        }
        cx.stop_propagation();
        window.prevent_default();
        if event.is_held {
            return;
        }
        if event.keystroke.key == "escape" {
            self.cancel(cx);
            return;
        }
        match Shortcut::from_keystroke(&event.keystroke) {
            Ok(mut shortcut) => {
                shortcut.capture_physical_key();
                self.recording = false;
                cx.emit(RecorderEvent::Recorded(shortcut));
            }
            Err(error) => self.error = Some(error.to_string()),
        }
        cx.notify();
    }
}
impl Focusable for ShortcutRecorder {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
impl Render for ShortcutRecorder {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("quick-chat-shortcut-recorder")
            .debug_selector(|| "quick-chat-shortcut-recorder".into())
            .key_context("ShortcutRecorder")
            .track_focus(&self.focus_handle)
            .w_full()
            .flex()
            .flex_col()
            .gap_1()
            .capture_key_down(cx.listener(Self::key_down))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .text_size(theme::ui_px(12.))
                            .child("Quick Chat shortcut"),
                    )
                    .child(
                        div()
                            .id("record-quick-chat-shortcut")
                            .debug_selector(|| "record-quick-chat-shortcut".into())
                            .min_w(px(94.))
                            .px_2()
                            .h(px(28.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .border_1()
                            .border_color(rgba(if self.recording {
                                0x459bf780
                            } else {
                                0xebebeb22
                            }))
                            .bg(rgba(GLASS_SURFACE))
                            .cursor_pointer()
                            .hover(|s| s.bg(rgba(GLASS_HOVER)))
                            .text_size(theme::ui_px(11.))
                            .child(if self.recording {
                                "Press shortcut…".to_owned()
                            } else {
                                self.shortcut.label()
                            })
                            .on_click(cx.listener(|this, _, window, cx| this.begin(window, cx))),
                    ),
            )
            .when_some(self.error.as_ref(), |element, error| {
                element.child(
                    div()
                        .text_size(theme::ui_px(11.))
                        .line_height(theme::ui_line_height(16.))
                        .text_color(rgb(0xe58c8c))
                        .child(error.clone()),
                )
            })
    }
}

#[cfg(all(target_os = "macos", not(test)))]
mod native_panel {
    use gpui::Window;
    use objc2::{
        msg_send,
        runtime::{AnyObject, Bool},
    };
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    unsafe fn native(window: &Window) -> Option<*mut AnyObject> {
        let RawWindowHandle::AppKit(handle) = HasWindowHandle::window_handle(window).ok()?.as_raw()
        else {
            return None;
        };
        let view = handle.ns_view.as_ptr().cast::<AnyObject>();
        let window: *mut AnyObject = unsafe { msg_send![view, window] };
        (!window.is_null()).then_some(window)
    }
    pub(super) fn configure(window: &Window) {
        unsafe {
            let Some(native) = native(window) else {
                return;
            };
            let _: () = msg_send![native, setLevel: 3isize]; // NSFloatingWindowLevel
            let _: () = msg_send![native, setCollectionBehavior: (1usize << 0) | (1usize << 8)]; // all Spaces + fullscreen auxiliary
            let _: () = msg_send![native, setMovableByWindowBackground: Bool::YES];
            let _: () = msg_send![native, setReleasedWhenClosed: Bool::NO];
            let content: *mut AnyObject = msg_send![native, contentView];
            if !content.is_null() {
                let _: () = msg_send![content, setWantsLayer: Bool::YES];
                let layer: *mut AnyObject = msg_send![content, layer];
                if !layer.is_null() {
                    let _: () = msg_send![layer, setCornerRadius: 20f64];
                    let _: () = msg_send![layer, setMasksToBounds: Bool::YES];
                    // kCACornerCurveContinuous (available on supported macOS).
                    let value = objc2_foundation::NSString::from_str("continuous");
                    let _: () = msg_send![layer, setCornerCurve: &*value];
                }
            }
        }
    }
    pub(super) fn visible(window: &Window) -> bool {
        unsafe {
            native(window).is_some_and(|native| {
                let visible: Bool = msg_send![native, isVisible];
                visible.as_bool()
            })
        }
    }
    pub(super) fn show(window: &Window) {
        unsafe {
            if let Some(native) = native(window) {
                if crate::unified::quick_surface(native.cast(),true){return;}
                let _: () = msg_send![native, makeKeyAndOrderFront: std::ptr::null::<AnyObject>()];
            }
        }
    }
    pub(super) fn hide(window: &Window) {
        unsafe {
            if let Some(native) = native(window) {
                if crate::unified::quick_surface(native.cast(),false){return;}
                let _: () = msg_send![native, orderOut: std::ptr::null::<AnyObject>()];
            }
        }
    }
}
#[cfg(any(not(target_os = "macos"), test))]
mod native_panel {
    use gpui::Window;
    pub(super) fn configure(_: &Window) {}
    pub(super) fn visible(_: &Window) -> bool {
        true
    }
    pub(super) fn show(window: &Window) {
        window.activate_window();
    }
    pub(super) fn hide(_: &Window) {}
}
