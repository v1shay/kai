//! Small, anchored native controls. Probes are demand-driven and cached, never polled.
use super::*;
use agent_controls::AuthState;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Scope { All, Providers, Models }
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Column { Providers, Models, Effort }
pub(super) struct Picker {
    pub scope: Scope,
    pub column: Column,
    pub provider_cursor: usize,
    pub model_cursor: usize,
    pub effort_cursor: usize,
    return_focus: Option<FocusHandle>,
    model_scroll: UniformListScrollHandle,
    max_width: f32,
    pub anchor: Point<Pixels>,
    pub started: Instant,
    pub closing: bool,
    pub search:Entity<TextInput>,
    _search_subscription:Subscription,
    closing_from: f32,
    pub effort_motion:Option<(Instant,f32,f32)>,
    pub effort_only:bool,
    pub effort_hover:Option<(String,Instant)>,
    max_height:f32,
    auth_pulse: Option<(&'static str, Instant)>,
}
impl Picker {
    fn reveal(&self, motion: bool) -> f32 {
        if !motion {
            return if self.closing { 0. } else { 1. };
        }
        let t =
            (self.started.elapsed().as_secs_f32() / if self.closing { 0.12 } else { 0.32 }).min(1.);
        if self.closing {
            self.closing_from * (1. - t * t)
        } else {
            presentation::spring(t)
        }
    }
    pub fn animating(&self) -> bool {
        (!self.closing && !native::reduced_motion() && self.effort_hover.as_ref().is_some_and(|(_,at)|at.elapsed()<Duration::from_secs(5))) || self.effort_motion.is_some_and(|(at,_,_)|at.elapsed()<Duration::from_millis(340)) || self.started.elapsed() < Duration::from_millis(if self.closing { 120 } else { 320 })
            || self
                .auth_pulse
                .is_some_and(|(_, t)| t.elapsed() < Duration::from_millis(360))
    }
}
impl Orbit {
    pub(super) fn execution_root(&self) -> Option<PathBuf> {
        self.active
            .as_ref()
            .and_then(|id| self.sessions.iter().find(|s| &s.id == id))
            .map(|s| {
                s.execution_path
                    .clone()
                    .unwrap_or_else(|| s.project_path.clone())
            })
            .or_else(|| self.new_execution_path.clone())
    }
    pub(super) fn open_citation(&mut self, url: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.ninja_open_file(url,cx){return}
        if let Some(id)=url.strip_prefix("kai://artifact/"){self.open_saved_artifact(id,window,cx);return}
        match citations::classify(url) {
            Ok(citations::Citation::File(citation)) => {
                self.files_visible = true;
                self.sync_file_root(window, cx);
                if let Some(browser) = &self.file_browser {
                    browser.update(cx, |browser, cx| browser.open_citation(citation, cx));
                }
                cx.notify();
            }
            Ok(citations::Citation::Web(url)) => {
                let task = cx
                    .background_executor()
                    .spawn(async move { native::open_web_url(&url) });
                cx.spawn(async move |this, cx| {
                    if let Err(error) = task.await {
                        let _ = this.update(cx, |this, cx| {
                            this.notice = Some(error);
                            cx.notify();
                        });
                    }
                })
                .detach();
            }
            Err(error) => {
                self.notice = Some(error);
                cx.notify();
            }
        }
    }
    pub(super) fn dismiss_agent_picker(&mut self) {
        if let Some(picker) = &mut self.agent_picker
            && !picker.closing
        {
            picker.closing_from = picker.reveal(self.motion).clamp(0., 1.);
            picker.closing = true;
            picker.started = Instant::now();
        }
    }
    pub(super) fn dismiss_controls_for_window(&mut self,id:gpui::WindowId) {
        if self.overlay_window!=Some(id){return}
        // Deactivation must not focus/resign native views inside AppKit's callback.
        self.agent_picker=None;self.panel_popover=None;self.action_menu=None;self.modal=None;self.github.panel=None;
        self.picker_at_pointer_down=None;self.panel_at_pointer_down=None;
    }
    pub(super) fn remember_picker_press(&mut self) {
        self.picker_at_pointer_down=self.agent_picker.as_ref().filter(|p|!p.closing).map(|p|p.effort_only);
    }
    pub(super) fn toggle_picker_from_click(&mut self, effort:bool, position:Point<Pixels>,window:&mut Window,cx:&mut Context<Self>) {
        // Outside-dismiss runs on pointer down, before the trigger's click.
        // Use the surface that owned this press, not its closing animation.
        self.picker_at_pointer_down=None;
        if self.consumed_control_click(control_clicks::Control::Model(effort),window) || self.agent_picker.as_ref().is_some_and(|p|!p.closing && p.effort_only==effort) {
            self.close_picker_with_focus(window,cx);return;
        }
        self.agent_picker=None;
        self.toggle_agent_picker(position,window,cx);
        if let Some(picker)=self.agent_picker.as_mut(){
            picker.effort_only=effort;
            if effort{picker.anchor=position-point(px(20.),px(230.));if let Some(focus)=picker.return_focus.as_ref(){focus.focus(window);}}
        }
    }
    pub(super) fn toggle_agent_picker(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.overlay_window = Some(window.window_handle().window_id());
        if self.agent_picker.as_ref().is_some_and(|p| !p.closing) {
            self.close_picker_with_focus(window,cx);
            return;
        }
        self.modal = None;
        self.panel_popover = None;
        self.action_menu = None;
        let height = window.viewport_size().height;
        let search=cx.new(|cx|{let mut input=TextInput::new(window,cx);input.set_placeholder("Search models…",cx);input});
        let subscription=cx.subscribe(&search,|this,_,event,cx|{if matches!(event,InputEvent::Changed){if let Some(p)=this.agent_picker.as_mut(){p.model_cursor=0;p.model_scroll.scroll_to_item(0,ScrollStrategy::Top);}cx.notify();}});
        let effort_preview=self.unified_effort().map(str::to_owned).or_else(||self.effective_model_settings().1).map(|value|(value,Instant::now()));
        self.agent_picker = Some(Picker {
            scope:Scope::All,column:Column::Models,
            provider_cursor:self.agents.iter().filter(|a|a.available).position(|a|a.kind==self.agent).unwrap_or(0),
            model_cursor:0,effort_cursor:0,return_focus:window.focused(cx),model_scroll:UniformListScrollHandle::default(),max_width:(f32::from(window.viewport_size().width)-16.).max(180.),
            search:search.clone(),_search_subscription:subscription,
            anchor: point(
                position.x,
                (position.y - px(340.)).max(px(8.)).min(height - px(350.)),
            ),
            started: Instant::now(),
            closing: false,
            closing_from: 1.,
            auth_pulse: None,effort_only:false,effort_hover:effort_preview,effort_motion:None,max_height:f32::from(height)-16.,
        });
        self.reset_picker_cursors(cx);
        search.read(cx).focus_handle.focus(window);
        self.refresh_controls(false, cx);
        cx.notify();
    }
    pub(super) fn open_scoped_picker(&mut self, scope:Scope, position:Point<Pixels>,window:&mut Window,cx:&mut Context<Self>){
        self.agent_picker=None;
        self.toggle_agent_picker(position,window,cx);
        if let Some(p)=self.agent_picker.as_mut(){p.scope=scope;p.column=if scope==Scope::Providers{Column::Providers}else{Column::Models};}
        cx.notify();
    }
    pub(super) fn close_picker_with_focus(&mut self,window:&mut Window,cx:&mut Context<Self>){
        let focus=self.agent_picker.as_ref().and_then(|p|p.return_focus.clone());
        self.dismiss_agent_picker();
        if let Some(focus)=focus{focus.focus(window);}
        cx.notify();
    }
    fn reset_picker_cursors(&mut self,cx:&Context<Self>){
        let models=self.picker_models(cx);
        let model_cursor=models.iter().position(|(id,_)|id==&self.model).unwrap_or(0);
        let selected=self.unified_effort().map(str::to_owned).or_else(||self.effective_model_settings().1);
        let provider_cursor=self.agents.iter().filter(|a|a.available).position(|a|a.kind==self.agent).unwrap_or(0);
        let effort_cursor=self.effort_model_choice().and_then(|m|m.reasoning_efforts.iter().position(|e|Some(e)==selected.as_ref())).unwrap_or(0);
        if let Some(p)=self.agent_picker.as_mut(){p.provider_cursor=provider_cursor;p.model_cursor=model_cursor;p.effort_cursor=effort_cursor;p.model_scroll.scroll_to_item(model_cursor,ScrollStrategy::Center);}
    }
    fn picker_choose_agent(&mut self,kind:AgentKind,window:&mut Window,cx:&mut Context<Self>){
        let mut picker=self.agent_picker.take();
        self.choose_agent(kind,window,cx);
        if let Some(p)=picker.as_mut(){p.column=Column::Models;p.search.update(cx,|input,cx|input.set_text("",cx));}
        self.agent_picker=picker;
        self.reset_picker_cursors(cx);
        if self.agent_picker.as_ref().is_some_and(|p|p.scope==Scope::Providers){self.close_picker_with_focus(window,cx);}
        cx.notify();
    }
    fn picker_choose_model(&mut self,model:Option<String>,window:&mut Window,cx:&mut Context<Self>){
        if self.loading||self.starting_current()||self.handoff_pending.is_some(){return}
        if self.agent==AgentKind::Ninja{self.choose_model(model,cx);self.reset_picker_cursors(cx);cx.notify();return}
        let picker=self.agent_picker.take();
        self.choose_model(model,cx);
        self.agent_picker=picker;
        self.reset_picker_cursors(cx);
        if self.effort_model_choice().is_some_and(|m|!m.reasoning_efforts.is_empty()){
            if let Some(p)=self.agent_picker.as_mut(){p.column=Column::Effort;}
        }else{self.close_picker_with_focus(window,cx);}
        cx.notify();
    }
    fn picker_key_down(&mut self,event:&KeyDownEvent,window:&mut Window,cx:&mut Context<Self>){
        if !self.agent_picker.as_ref().is_some_and(|p|!p.closing&&!p.effort_only){return}
        let mods=&event.keystroke.modifiers;
        if mods.control||mods.platform||mods.alt||mods.shift{return}
        let key=event.keystroke.key.as_str();
        if !matches!(key,"up"|"down"|"left"|"right"|"enter"|"escape"){return}
        self.picker_navigate(key,window,cx);
    }
    fn picker_navigate(&mut self,key:&str,window:&mut Window,cx:&mut Context<Self>){
        let Some(p)=self.agent_picker.as_ref().filter(|p|!p.closing&&!p.effort_only)else{cx.propagate();return};
        let column=p.column;let scope=p.scope;
        let providers:Vec<_>=self.agents.iter().filter(|a|a.available).map(|a|a.kind).collect();
        let models=self.picker_models(cx);
        let levels=self.effort_model_choice().map(|m|m.reasoning_efforts).unwrap_or_default();
        cx.stop_propagation();window.prevent_default();
        match key{
            "escape"=>self.close_picker_with_focus(window,cx),
            "up"|"down"=>{
                let p=self.agent_picker.as_mut().unwrap();
                let (cursor,count)=match column{Column::Providers=>(&mut p.provider_cursor,providers.len()),Column::Models=>(&mut p.model_cursor,models.len()),Column::Effort=>(&mut p.effort_cursor,levels.len())};
                *cursor=if key=="up"{cursor.saturating_sub(1).min(count.saturating_sub(1))}else{(*cursor+1).min(count.saturating_sub(1))};
                if column==Column::Models{p.model_scroll.scroll_to_item(*cursor,ScrollStrategy::Center);}
            },
            "left"=>{if let Some(p)=self.agent_picker.as_mut(){p.column=match column{Column::Effort=>Column::Models,Column::Models if scope==Scope::All=>Column::Providers,_=>column};}},
            "right"|"enter"=>match column{
                Column::Providers=>{if let Some(kind)=providers.get(self.agent_picker.as_ref().unwrap().provider_cursor){self.picker_choose_agent(*kind,window,cx);}},
                Column::Models=>{if let Some((id,_))=models.get(self.agent_picker.as_ref().unwrap().model_cursor){self.picker_choose_model(id.clone(),window,cx);}},
                Column::Effort if key=="enter"=>{if let Some(level)=levels.get(self.agent_picker.as_ref().unwrap().effort_cursor){self.select_reasoning_effort(level,cx);self.close_picker_with_focus(window,cx);}},
                _=>{}
            },
            _=>{}
        }
        cx.notify();
    }
    pub(super) fn refresh_controls(&mut self, force: bool, cx: &mut Context<Self>) {
        if self.agent==AgentKind::Ninja{self.ninja_refresh(force,cx);}
        let mut jobs: Vec<_> = self
            .agents
            .iter()
            .filter(|a| a.available && a.kind!=AgentKind::Ninja)
            .filter(|a| self.modal != Some(Modal::Status) || a.kind == self.agent)
            .filter_map(|a| {
                let key = a.kind.command();
                if self.controls_pending.contains(key)
                    || (!force
                        && self
                            .controls
                            .get(key)
                            .is_some_and(|(t, info)| t.elapsed() < Duration::from_secs(60) && info.defaults.project==self.model_project()))
                {
                    return None;
                }
                a.executable.clone().map(|path| (a.kind, path))
            })
            .collect();
        jobs.sort_by_key(|(kind, _)| *kind != self.agent);
        for (kind, _) in &jobs {
            self.controls_pending.insert(kind.command());
        }
        let home = self.controls_home.clone();let project=self.model_project();
        cx.spawn(async move |this, cx| {
            // One probe at a time: opening controls never launches a burst of CLIs.
            for (kind, path) in jobs {
                let proceed = this.update(cx, |this, _| {
                    let open = this.agent_picker.as_ref().is_some_and(|p| !p.closing)
                        || this.modal == Some(Modal::Status);
                    if !open {
                        this.controls_pending.remove(kind.command());
                    }
                    open
                });
                match proceed {
                    Ok(true) => {}
                    Ok(false) => continue,
                    Err(_) => break,
                }
                let home = home.clone();let project=project.clone();
                let task = cx
                    .background_executor()
                    .spawn(async move { agent_controls::inspect_project(kind, &path, &home,&project) });
                let info = task.await;
                if this
                    .update(cx, |this, cx| {
                        this.controls_pending.remove(kind.command());
                        let mut info = info;
                        if let Some((_, previous)) = this.controls.get_mut(kind.command()) {
                            info.catalog = previous.catalog.take();
                        }
                        this.controls.insert(kind.command(), (Instant::now(), info));
                        if this.agent_picker.is_some() || this.modal == Some(Modal::Status) {
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }
    fn choose_agent(&mut self, kind: AgentKind, window: &mut Window, cx: &mut Context<Self>) {
        self.request_handoff(kind, window, cx);
        if kind==AgentKind::Ninja{self.ninja_refresh(false,cx);}
    }
    pub(super) fn choose_model(&mut self, model: Option<String>, cx: &mut Context<Self>) {
        if self.agent==AgentKind::Ninja{if let Some(id)=model{self.ninja_choose_model(id,cx);}return}
        if self.loading || self.starting_current() || self.handoff_pending.is_some() {
            return;
        }
        self.model = model.clone();
        if self
            .active
            .as_ref()
            .is_some_and(|id| self.runs.contains_key(id))
        {
            self.notice = Some("The selected model will take over on the next turn, keeping this native session's context.".into());
        }
        self.agent_models
            .insert(self.agent.command(), model.clone());
        if let Some(session) = self
            .active
            .as_ref()
            .and_then(|id| {
                self.sessions
                    .iter()
                    .find(|s| &s.id == id && s.agent == self.agent)
            })
            .cloned()
        {
            self.session_models
                .insert(session.id.clone(), model.clone());
            self.engine.set_model(session, model);
        }
        self.dismiss_agent_picker();
        self.refresh_unified_models(cx);
        cx.notify();
    }
    fn account_action(&mut self, kind: AgentKind, open_cli: bool, cx: &mut Context<Self>) {
        if kind==AgentKind::Ninja{self.ninja_refresh(true,cx);return}
        if self.controls_pending.contains(kind.command()) {
            return;
        }
        if let Some(picker) = &mut self.agent_picker {
            picker.auth_pulse = Some((kind.command(), Instant::now()));
        }
        let auth = self
            .controls
            .get(kind.command())
            .map(|(_, c)| c.auth)
            .unwrap_or(AuthState::Unknown);
        if auth == AuthState::VerifiedSignedIn && !open_cli {
            self.controls.remove(kind.command());
            self.refresh_controls(false, cx);
            cx.notify();
            return;
        }
        let Some(path) = self
            .agents
            .iter()
            .find(|a| a.kind == kind)
            .and_then(|a| a.executable.clone())
        else {
            return;
        };
        let cache = self.controls_cache.clone();
        self.controls_pending.insert(kind.command());
        let task = cx.background_executor().spawn(async move {
            match agent_controls::login_script(kind, &path, &cache, auth) {
                Ok(Some(script)) => native::open_login_script(&script),
                Ok(None) => Err("Open this CLI in Terminal to sign in.".into()),
                Err(error) => Err(error.to_string()),
            }
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.controls_pending.remove(kind.command());
                this.controls.remove(kind.command());
                this.notice = Some(result.err().unwrap_or_else(|| {
                    "Finish signing in in Terminal, then refresh the lock.".into()
                }));
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn model_choice_row(
        &self,
        ix: usize,
        id: Option<String>,
        label: String,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let selected = if self.agent==AgentKind::Ninja{id.as_deref()==Some(self.ninja_selection().model.as_str())}else{self.model == id};
        let cursor=self.agent_picker.as_ref().is_some_and(|p|p.column==Column::Models&&p.model_cursor==ix);
        let glyph=composer_design::model_glyph(id.as_deref().unwrap_or(&label));
        let installed=self.agent==AgentKind::Ninja&&self.ninja_models().iter().any(|m|Some(&m.id)==id.as_ref()&&m.installed);
        let glyph_color=if installed{ninja::lab_color(id.as_deref().unwrap_or(""))}else{TEXT};
        let label=if self.agent==AgentKind::Ninja{label.replace(" · Installed","")}else{label};
        div()
            .id(("model", ix))
            .debug_selector(move || format!("model-choice-{ix}"))
            .h(px(30.))
            .px_2()
            .rounded_md()
            .flex()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .hover(|s| s.bg(rgba(GLASS_HOVER)))
            .when(cursor,|s|s.bg(rgba(GLASS_HOVER)))
            .when_some(glyph,|s,glyph|s.child(icon(glyph,15.,glyph_color)))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .text_size(crate::theme::ui_px(12.))
                    .text_ellipsis()
                    .child(composer_design::display_label(&label)),
            )
            .when(selected, |s| s.child(icon("check", 12., ACCENT)))
            .on_click(cx.listener(move |this, _, window, cx| this.picker_choose_model(id.clone(), window,cx)))
    }
    pub(super) fn agent_picker_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(picker) = &self.agent_picker else {
            return div().into_any_element();
        };
        let reveal = picker.reveal(self.motion);
        if picker.effort_only{return anchored().position(picker.anchor).snap_to_window_with_margin(px(8.)).child(div().id("thinking-popover").w(px(264.)).p_2().rounded_lg().bg(rgba(appearance::MENU_SURFACE)).border_1().border_color(rgba(0xffffff24)).shadow_lg().opacity(reveal.clamp(0.,1.)).occlude().on_mouse_down_out(cx.listener(|this,event:&MouseDownEvent,_,cx|{if this.agent_picker.as_ref().is_some_and(|p|!p.closing&&p.effort_only){this.dismissed_on_press(control_clicks::Control::Model(true),event.position);this.dismiss_agent_picker();cx.notify();}})).when_some(self.effort_model_choice(),|s,m|s.child(self.reasoning_slider(m,cx)))).into_any_element()}

        let models=self.picker_models(cx);
        let providers=picker.scope!=Scope::Models;
        let model_column=picker.scope!=Scope::Providers;
        let effort=self.effort_model_choice().filter(|m|!m.reasoning_efforts.is_empty());
        let natural_width=if providers&&model_column{if effort.is_some(){620.}else{444.}}else if model_column{if effort.is_some(){444.}else{280.}}else{220.};
        let ninja_panel=self.agent==AgentKind::Ninja && model_column;
        let natural_width=natural_width+if ninja_panel{228.}else{0.};
        let horizontal=(self.theme.horizontal_agent_picker||ninja_panel) && picker.max_width>=natural_width;
        let width=if horizontal{natural_width}else{280.0_f32}.min(picker.max_width);
        let heading=|label:&'static str,focused:bool|div().px_2().py_1().text_size(theme::ui_px(10.)).text_color(rgb(if focused{ACCENT}else{MUTED})).child(label);
        let provider_column=div().flex().flex_col().min_w(px(0.)).when(horizontal,|s|s.w(px(if model_column{166.}else{width-12.})).flex_none()).when(!horizontal,|s|s.w_full())
            .child(heading("Provider",picker.column==Column::Providers))
                    .children(self.agents.iter().filter(|a| a.available).enumerate().map(|(ix,agent)| {
                        let kind = agent.kind;
                        let auth = self
                            .controls
                            .get(kind.command())
                            .map(|(_, c)| c.auth)
                            .unwrap_or(AuthState::Unknown);
                        let pending = self.controls_pending.contains(kind.command());
                        let (glyph, color) = if kind==AgentKind::Ninja{("tailscale",0x67dab0)}else{match auth {
                            AuthState::VerifiedSignedIn => ("lock-keyhole", 0x8fc89a),
                            AuthState::SignedOut => ("lock-keyhole-open", 0xe58c8c),
                            _ => ("lock-keyhole", MUTED),
                        }};
                        let bounce = if self.motion {
                            picker
                                .auth_pulse
                                .filter(|(k, _)| *k == kind.command())
                                .map(|(_, t)| {
                                    let p = (t.elapsed().as_secs_f32() / 0.36).min(1.);
                                    2. * (1. - presentation::spring(p))
                                })
                                .unwrap_or(0.)
                        } else {
                            0.
                        };
                        div()
                            .id(SharedString::from(format!(
                                "picker-agent-{}",
                                kind.command()
                            )))
                            .debug_selector(move || format!("agent-choice-{}", agent_icon(kind)))
                            .h(px(36.))
                            .px_2()
                            .flex()
                            .items_center()
                            .gap_2()
                            .rounded_md()
                            .cursor_pointer()
                            .when(self.agent == kind || (picker.column==Column::Providers&&picker.provider_cursor==ix), |s| s.bg(rgba(GLASS_HOVER)))
                            .hover(|s| s.bg(rgba(GLASS_HOVER)))
                            .child(
                                div()
                                    .debug_selector(move || {
                                        format!("picker-logo-{}", kind.command())
                                    })
                                    .size(px(16.))
                                    .flex_none()
                                    .child(provider_icon(agent_icon(kind), 16., agent_color(kind))),
                            )
                            .child(
                                div()
                                    .debug_selector(move || {
                                        format!("picker-label-{}", kind.command())
                                    })
                                    .flex_1()
                                    .min_w(px(0.))
                                    .text_size(crate::theme::ui_px(12.))
                                    .text_ellipsis()
                                    .child(agent.name.clone()),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!("account-{}", kind.command())))
                                    .debug_selector(move || {
                                        format!("agent-account-{}", kind.command())
                                    })
                                    .size(px(26.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_md()
                                    .hover(|s| s.bg(rgba(GLASS_HOVER)))
                                    .child(
                                        div()
                                            .relative()
                                            .top(px(bounce))
                                            .opacity(if pending { 0.5 } else { 1. })
                                            .child(icon(glyph, 14., color)),
                                    )
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.account_action(kind, false, cx);
                                    })),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.picker_choose_agent(kind, window, cx)
                            }))
                    }))
;
        let model_view=div().flex().flex_col().min_w(px(0.)).when(horizontal,|s|s.w(px(256.)).flex_none()).when(!horizontal,|s|s.w_full())
            .child(div().px_2().flex().items_center().justify_between().child(heading("Model",picker.column==Column::Models))
                .child(div().id("refresh-controls").cursor_pointer().child(icon("refresh",12.,MUTED)).on_click(cx.listener(|this,_,_,cx|{this.refresh_controls(true,cx);cx.notify();}))))
            .child(div().px_2().pb_1().text_size(theme::ui_px(10.)).text_color(rgb(MUTED)).child(self.effective_model_label()))
            .child(div().debug_selector(||"model-search".into()).px_2().pb_1().child(picker.search.clone()))
            .child({let count=models.len();uniform_list("model-options",count,cx.processor(move|this,range:std::ops::Range<usize>,_,cx|range.map(|ix|{let(id,label)=models[ix].clone();this.model_choice_row(ix,id,label,cx).into_any_element()}).collect::<Vec<_>>())).track_scroll(picker.model_scroll.clone()).h(px((count as f32*30.).min(if horizontal{240.}else{180.}))).w_full()})
            .when(self.controls.get(self.agent.command()).is_some_and(|(_,c)|matches!(c.auth,AuthState::Unknown|AuthState::Configured)),|s|s.child(div().id("open-agent-cli").debug_selector(||"open-agent-cli".into()).h(px(28.)).px_2().flex().items_center().gap_2().text_size(theme::ui_px(11.)).text_color(rgb(MUTED)).cursor_pointer().hover(|s|s.bg(rgba(GLASS_HOVER))).child(icon("terminal",12.,MUTED)).child("Open CLI").on_click(cx.listener(|this,_,_,cx|this.account_action(this.agent,true,cx)))));
        anchored().position(picker.anchor+point(px(0.),px(8.*(1.-reveal)))).snap_to_window_with_margin(px(8.)).child(
            div().id("agent-picker").debug_selector(||"agent-picker".into()).w(px(width)).max_h(px(picker.max_height)).overflow_y_scroll().p(px(6.)).rounded_lg().bg(rgba(appearance::MENU_SURFACE)).border_1().border_color(rgba(0xebebeb1c)).opacity(reveal.clamp(0.,1.)).occlude()
            .when(picker.scope==Scope::Providers,|s|s.track_focus(&picker.search.read(cx).focus_handle))
            .capture_key_down(cx.listener(Self::picker_key_down))
            .capture_action(cx.listener(|this,_:&input::Up,window,cx|this.picker_navigate("up",window,cx)))
            .capture_action(cx.listener(|this,_:&input::Down,window,cx|this.picker_navigate("down",window,cx)))
            .capture_action(cx.listener(|this,_:&input::Left,window,cx|this.picker_navigate("left",window,cx)))
            .capture_action(cx.listener(|this,_:&input::Right,window,cx|this.picker_navigate("right",window,cx)))
            .capture_action(cx.listener(|this,_:&input::Submit,window,cx|this.picker_navigate("enter",window,cx)))
            .on_mouse_down_out(cx.listener(|this,event:&MouseDownEvent,_,cx|{if this.agent_picker.as_ref().is_some_and(|p|!p.closing&&!p.effort_only){this.dismissed_on_press(control_clicks::Control::Model(false),event.position);this.dismiss_agent_picker();cx.notify();}}))
            .child(div().flex().gap_1().when(!horizontal,|s|s.flex_col()).when(providers,|s|s.child(provider_column)).when(model_column,|s|s.child(model_view).when_some(effort,|s,m|s.child(div().when(horizontal,|s|s.w(px(170.)).flex_none()).when(!horizontal,|s|s.w_full()).child(self.reasoning_slider(m,cx)))).when(ninja_panel,|s|s.child(div().w(px(if horizontal{222.}else{width-12.})).flex_none().child(self.ninja_options(cx))))))
        ).into_any_element()
    }
    fn picker_models(&self,cx:&Context<Self>)->Vec<(Option<String>,String)>{
        if self.agent==AgentKind::Ninja{let query=self.agent_picker.as_ref().map(|p|p.search.read(cx).text().to_lowercase()).unwrap_or_default();return self.ninja_choices().into_iter().filter(|m|query.split_whitespace().all(|w|m.label.to_lowercase().contains(w))).map(|m|(Some(m.id),m.label)).collect()}
        let controls = self
            .controls
            .get(self.agent.command())
            .map(|(_, info)| info);
        let automatic=if self.active.is_some(){format!("Continue · {}",self.effective_model_settings().0.unwrap_or_else(||self.agent.name().into()))}else{format!("Use {}",self.provider_default_model().unwrap_or_else(||self.agent.name().into()))};
        let mut models = vec![(None,automatic)];
        if let Some(catalog) = self
            .native_models
            .get(&self.agent)
            .or_else(|| controls.map(|info| &info.models))
        {
            models.extend(
                catalog
                    .iter()
                    .map(|m| (Some(m.id.clone()), m.label.clone())),
            );
        }
        if let Some(id) = &self.model
            && !models.iter().any(|(m, _)| m.as_ref() == Some(id))
        {
            models.push((Some(id.clone()), id.clone()));
        }
        let query=self.agent_picker.as_ref().map(|p|p.search.read(cx).text().to_lowercase()).unwrap_or_default();
        if !query.trim().is_empty(){models.retain(|(id,label)|id.is_none() || query.split_whitespace().all(|word|label.to_lowercase().contains(word) || id.as_deref().unwrap_or("").to_lowercase().contains(word)));}
        models
    }
}
