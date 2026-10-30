//! Small, anchored native controls. Probes are demand-driven and cached, never polled.
use super::*;
use agent_controls::AuthState;

pub(super) struct Picker {
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
        self.agent_picker=None;self.panel_popover=None;self.action_menu=None;self.modal=None;
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
            self.dismiss_agent_picker();cx.notify();return;
        }
        self.agent_picker=None;
        self.toggle_agent_picker(position,window,cx);
        if let Some(picker)=self.agent_picker.as_mut(){
            picker.effort_only=effort;
            if effort{picker.anchor=position-point(px(20.),px(230.));}
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
            self.dismiss_agent_picker();
            cx.notify();
            return;
        }
        self.modal = None;
        self.panel_popover = None;
        self.action_menu = None;
        let height = window.viewport_size().height;
        let search=cx.new(|cx|{let mut input=TextInput::new(window,cx);input.set_placeholder("Search models…",cx);input});
        let subscription=cx.subscribe(&search,|_,_,event,cx|{if matches!(event,InputEvent::Changed){cx.notify();}});
        let effort_preview=self.unified_effort().map(str::to_owned).or_else(||self.effective_model_settings().1).map(|value|(value,Instant::now()));
        self.agent_picker = Some(Picker {
            search,_search_subscription:subscription,
            anchor: point(
                position.x,
                (position.y - px(340.)).max(px(8.)).min(height - px(350.)),
            ),
            started: Instant::now(),
            closing: false,
            closing_from: 1.,
            auth_pulse: None,effort_only:false,effort_hover:effort_preview,effort_motion:None,max_height:f32::from(height)-16.,
        });
        self.refresh_controls(false, cx);
        cx.notify();
    }
    pub(super) fn refresh_controls(&mut self, force: bool, cx: &mut Context<Self>) {
        let mut jobs: Vec<_> = self
            .agents
            .iter()
            .filter(|a| a.available)
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
    }
    pub(super) fn choose_model(&mut self, model: Option<String>, cx: &mut Context<Self>) {
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
        let selected = self.model == id;
        let glyph=composer_design::model_glyph(id.as_deref().unwrap_or(&label));
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
            .when_some(glyph,|s,glyph|s.child(icon(glyph,15.,TEXT)))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .text_size(crate::theme::ui_px(12.))
                    .text_ellipsis()
                    .child(composer_design::display_label(&label)),
            )
            .when(selected, |s| s.child(icon("check", 12., ACCENT)))
            .on_click(cx.listener(move |this, _, _, cx| this.choose_model(id.clone(), cx)))
    }
    pub(super) fn agent_picker_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(picker) = &self.agent_picker else {
            return div().into_any_element();
        };
        let reveal = picker.reveal(self.motion);
        if picker.effort_only{return anchored().position(picker.anchor).snap_to_window_with_margin(px(8.)).child(div().id("thinking-popover").w(px(264.)).p_2().rounded_lg().bg(rgba(appearance::MENU_SURFACE)).border_1().border_color(rgba(0xffffff24)).shadow_lg().opacity(reveal.clamp(0.,1.)).occlude().on_mouse_down_out(cx.listener(|this,event:&MouseDownEvent,_,cx|{if this.agent_picker.as_ref().is_some_and(|p|!p.closing&&p.effort_only){this.dismissed_on_press(control_clicks::Control::Model(true),event.position);this.dismiss_agent_picker();cx.notify();}})).when_some(self.effort_model_choice(),|s,m|s.child(self.reasoning_slider(m,cx)))).into_any_element()}

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
        let query=picker.search.read(cx).text().to_lowercase();
        if !query.trim().is_empty(){models.retain(|(id,label)|id.is_none() || query.split_whitespace().all(|word|label.to_lowercase().contains(word) || id.as_deref().unwrap_or("").to_lowercase().contains(word)));}
        anchored()
            .position(picker.anchor + point(px(0.), px(8. * (1. - reveal))))
            .snap_to_window_with_margin(px(8.))
            .child(
                div()
                    .id("agent-picker")
                    .debug_selector(|| "agent-picker".into())
                    .w(px(280.)).max_h(px(picker.max_height)).overflow_y_scroll()
                    .p(px(6.))
                    .rounded_lg()
                    .bg(rgba(appearance::MENU_SURFACE))
                    .border_1()
                    .border_color(rgba(0xebebeb1c))
                    .opacity(reveal.clamp(0., 1.))
                    .occlude()
                    .on_mouse_down_out(cx.listener(|this, event:&MouseDownEvent, _, cx| {
                        if this.agent_picker.as_ref().is_some_and(|p|!p.closing&&!p.effort_only){this.dismissed_on_press(control_clicks::Control::Model(false),event.position);
                        this.dismiss_agent_picker();cx.notify();}
                    }))
                    .children(self.agents.iter().filter(|a| a.available).map(|agent| {
                        let kind = agent.kind;
                        let auth = self
                            .controls
                            .get(kind.command())
                            .map(|(_, c)| c.auth)
                            .unwrap_or(AuthState::Unknown);
                        let pending = self.controls_pending.contains(kind.command());
                        let (glyph, color) = match auth {
                            AuthState::VerifiedSignedIn => ("lock-keyhole", 0x8fc89a),
                            AuthState::SignedOut => ("lock-keyhole-open", 0xe58c8c),
                            _ => ("lock-keyhole", MUTED),
                        };
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
                            .when(self.agent == kind, |s| s.bg(rgba(GLASS_HOVER)))
                            .hover(|s| s.bg(rgba(GLASS_HOVER)))
                            .child(
                                div()
                                    .debug_selector(move || {
                                        format!("picker-logo-{}", kind.command())
                                    })
                                    .size(px(16.))
                                    .flex_none()
                                    .child(icon(agent_icon(kind), 16., agent_color(kind))),
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
                                this.choose_agent(kind, window, cx)
                            }))
                    }))
                    .child(div().mt_1().mb_1().h(px(1.)).bg(rgba(0xebebeb14)))
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .flex()
                            .items_center()
                            .justify_between()
                            .text_size(crate::theme::ui_px(10.))
                            .text_color(rgb(MUTED))
                            .child(if self.active.as_ref().is_some_and(|id|self.runs.contains_key(id)){"Model for next turn"}else{"Model"})
                            .child(
                                div()
                                    .id("refresh-controls")
                                    .cursor_pointer()
                                    .child(icon("refresh", 12., MUTED))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.refresh_controls(true, cx);
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(div().px_2().pb_1().text_size(crate::theme::ui_px(10.)).text_color(rgb(MUTED)).child(format!("{} · {}",self.agent.name(),self.effective_model_label())))
                    .when(self.active.as_ref().is_some_and(|id|self.runs.contains_key(id)) && self.model.is_some(),|s|s.child(div().px_2().pb_1().text_size(crate::theme::ui_px(10.)).text_color(rgb(ACCENT)).child(format!("Next turn · {}{}",self.model.as_deref().unwrap_or(""),self.unified_effort().map(|e|format!(" · {e}")).unwrap_or_default()))))
                    .when_some(self.effort_model_choice().filter(|m|!m.reasoning_efforts.is_empty()),|s,model|s.child(self.reasoning_slider(model,cx)))
                    .child(div().debug_selector(||"model-search".into()).px_2().pb_1().child(picker.search.clone()))
                    .child({
                        let count = models.len();
                        uniform_list(
                            "model-options",
                            count,
                            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                                range
                                    .map(|ix| {
                                        let (id, label) = models[ix].clone();
                                        this.model_choice_row(ix, id, label, cx).into_any_element()
                                    })
                                    .collect::<Vec<_>>()
                            }),
                        )
                        .h(px((count as f32 * 30.).min(180.)))
                        .w_full()
                    })
                    .when(
                        controls.is_some_and(|c| {
                            matches!(c.auth, AuthState::Unknown | AuthState::Configured)
                        }),
                        |s| {
                            s.child(
                                div()
                                    .id("open-agent-cli")
                                    .debug_selector(|| "open-agent-cli".into())
                                    .h(px(28.))
                                    .px_2()
                                    .rounded_md()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .text_size(crate::theme::ui_px(11.))
                                    .text_color(rgb(MUTED))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(rgba(GLASS_HOVER)))
                                    .child(icon("terminal", 12., MUTED))
                                    .child("Open CLI")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.account_action(this.agent, true, cx)
                                    })),
                            )
                        },
                    ),
            )
            .into_any_element()
    }
}
