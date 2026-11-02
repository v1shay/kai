//! Personal chats retain normal agent rendering and host-owned composer state.
use super::*;
use ninjas_ui::{Host, call};
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Control {
    Models,
    Effort,
    Providers,
    Permissions,
    Workspace,
    Slash,
}
pub(super) struct Artifact {
    pub path: PathBuf,
    pub image: Option<Arc<RenderImage>>,
    pub native_preview: Option<String>,
}
pub(super) const COMMANDS: &[(&str, &str)] = &[
    ("model", "Choose a host model"),
    ("agent", "Choose a provider installed on the host"),
    ("thinking", "Adjust this model’s thinking intensity"),
    ("orchestrate", "Toggle orchestration"),
    ("permissions", "Change workspace access and approvals"),
    ("workspace", "Choose the host workspace"),
    ("new", "Start a new chat with this Ninja"),
    ("stop", "Stop this chat’s current task"),
    ("context", "Show reported context and token usage"),
    ("help", "Show Ninja chat commands"),
];
pub(super) fn live(t: &Value) -> bool {
    !t["finished"].is_number()
        && matches!(t["state"].as_str(), Some("running" | "approval" | "queued"))
}
pub(super) fn block_key(task: &str, index: usize) -> usize {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    task.hash(&mut h);
    index.hash(&mut h);
    // Separate from ordinary transcript row indices and stable across surfaces.
    (h.finish() as usize) | (1usize << (usize::BITS - 2))
}
pub(super) fn usage_label(value: &Value) -> Option<String> {
    let u: usage::Usage = serde_json::from_value(value.clone()).ok()?;
    if let Some((used, limit)) = u.context_used.zip(u.context_limit).filter(|(_, l)| *l > 0) {
        return Some(format!(
            "Context · {:.0}% used",
            used as f64 / limit as f64 * 100.
        ));
    }
    u.input
        .zip(u.output)
        .map(|(i, o)| format!("{i} in · {o} out"))
}
impl Orbit {
    pub(super) fn ninja_blocks(&self,user:bool,text:&str,task:&str)->Arc<Vec<MessageBlock>>{
        let p=self.ninjas.panel.as_ref().unwrap();
        let key=format!("{task}:{user}");
        if let Some((source,blocks))=p.blocks.borrow().get(&key){if source==text{return blocks.clone()}}
        let mut cache = p.blocks.borrow_mut();
            if cache.get(&key).is_none_or(|(source, _)| source != text) {
                if cache.len() >= 64 {
                    if let Some(old)=cache.keys().find(|k|*k!=&key).cloned(){cache.remove(&old);}
                }
                cache.insert(
                    key.clone(),
                    (
                        text.to_owned(),
                        Arc::new(if user{presentation::message_blocks("user",text)}else{presentation::live_page_blocks(text)}),
                    ),
                );
            }
            cache[&key].1.clone()

    }
    pub(super) fn ninja_bubble(
        &self,
        user: bool,
        text: String,
        task: &str,
        window: &Window,
        cx: &Context<Self>,
    ) -> Div {
        let p = self.ninjas.panel.as_ref().unwrap();
        let blocks=self.ninja_blocks(user,&text,task);
        let mut body = div().w_full().min_w(px(0.)).flex().flex_col().gap_2();
        let mut grouped_until=0;
        for (i, block) in blocks.iter().enumerate() {
            if i<grouped_until{continue}
            let activity=|b:&MessageBlock|matches!(b,MessageBlock::Activity{..}|MessageBlock::Interaction(chat_interactions::ChatInteraction::Status{..}|chat_interactions::ChatInteraction::Delegation{..}));
            if !user&&activity(block){
                grouped_until=i+blocks[i..].iter().take_while(|b|activity(b)).count();
                body=body.child(self.ninja_work_group(task,blocks.clone(),i..grouped_until,cx));continue
            }
            let ix = block_key(&format!("{task}:{user}"), i);
            let decoded =
                if !user && self.motion && self.unified_text_morph && !native::reduced_motion() {
                    let source = match block {
                        MessageBlock::Paragraph(s)
                        | MessageBlock::Heading(s)
                        | MessageBlock::Quote(s)
                        | MessageBlock::RichParagraph { text: s, .. }
                        | MessageBlock::ListItem { text: s, .. } => Some(s),
                        _ => None,
                    };
                    source.map(|s| {
                        let (frame, active) = p.decode.borrow_mut().frame(ix, s, Instant::now());
                        if active {
                            request_motion_frame(window, cx);
                        }
                        frame
                    })
                } else {
                    None
                };
            let content = div().min_w(px(0.)).w_full();
            body = body.child(self.render_message_block(
                content,
                ix,
                block,
                decoded.as_deref(),
                p.profile.agent,
                cx,
            ));
        }
        if !text.is_empty() {
            body = body.child(
                div().mt_1().flex().when(user, |s| s.justify_end()).child(
                    copy_control::CopyControl::source(format!("ninja-copy-{task}-{user}"),move |_|if user{text.clone()}else{ninjas::readable_brief(&text)})
                        .scope(self.ninja_surface_owner().unwrap_or_default())
                        .motion(self.motion),
                ),
            );
        }
        div()
            .w_full()
            .flex()
            .gap_2()
            .items_start()
            .when(user, |s| s.justify_end())
            .when(!user, |s| {
                s.child(ninja_art::art(&p.profile.style, 24., false))
            })
            .child(
                div()
                    .debug_selector({let task=task.to_owned();move||format!("ninja-bubble-{task}-{user}")})
                    .flex().flex_col()
                    .when(user,|s|s.w(relative(0.82)).flex_none())
                    .when(!user,|s|s.flex_1())
                    .min_w(px(0.))
                    .px_3()
                    .py_2()
                    .rounded_lg()
                    .bg(rgba(if user { 0x9380ff32 } else { 0xffffff0b }))
                    .text_size(theme::ui_px(13.))
                    .child(body),
            )
    }
    fn ninja_work_group(&self,task:&str,blocks:Arc<Vec<MessageBlock>>,range:std::ops::Range<usize>,cx:&Context<Self>)->Div{
        let p=self.ninjas.panel.as_ref().unwrap();let ix=block_key(&format!("{task}:work"),range.start);let expanded=self.expanded_tools.contains(&ix);let reveal=self.interaction_reveal(ix);
        let tasks=p.history_tasks.borrow();let current=tasks.iter().find(|t|t["id"]==task);let running=current.is_some_and(live);
        let elapsed=current.and_then(|t|t["started"].as_i64().zip(t["finished"].as_i64())).map(|(a,b)|chat_timing::duration(b.saturating_sub(a)as u64*1000));
        let label=if running{"Working".to_owned()}else if let Some(elapsed)=elapsed{format!("Worked for {elapsed}")}else{"Worked".into()};
        let motion=self.motion&&!native::reduced_motion();
        let mut group=div().w_full().min_w(px(0.)).flex().flex_col().child(div().id(("ninja-work",ix)).debug_selector(move||format!("ninja-work-{ix}")).w_full().h(px(28.)).flex_none().flex().items_center().gap_2().text_size(theme::ui_px(11.)).text_color(rgb(MUTED)).cursor_pointer()
            .child(icon("chevron-right",12.,MUTED).with_transformation(Transformation::rotate(radians(reveal*std::f32::consts::FRAC_PI_2))))
            .child(ninja_art::status(&p.profile.style,if running{"thinking"}else{"success"},false)).child(label).child(format!("· {} steps",range.len()))
            .on_click(cx.listener(move|app,_,_,cx|{cx.stop_propagation();let from=app.interaction_reveal(ix);let open=if app.expanded_tools.remove(&ix){false}else{app.expanded_tools.insert(ix);true};if motion{app.interaction_animations.insert(ix,(Instant::now(),open,from));}cx.notify();})));
        if expanded||reveal>0.{
            let mut details=div().id(("ninja-work-details",ix)).w_full().min_w(px(0.)).max_h(px(640.)).overflow_y_scroll().flex().flex_col().gap_1();
            for i in range{details=details.child(self.render_message_block(div().w_full().min_w(px(0.)),block_key(&format!("{task}:false"),i),&blocks[i],None,p.profile.agent,cx));}
            group=group.child(div().w_full().max_h(px(640.*reveal)).overflow_hidden().opacity(reveal).child(details));
        }
        group
    }
    pub(super) fn ninja_toggle_control(&mut self, control: Control, cx: &mut Context<Self>) {
        let Some(p) = &mut self.ninjas.panel else {
            return;
        };
        p.control = if p.control == Some(control)
            || p.dismissed_control.take().is_some_and(|(old, at)| {
                old == control && at.elapsed() < Duration::from_millis(500)
            }) {
            None
        } else {
            Some(control)
        };
        p.control_cursor = 0;
        p.control_at = Instant::now();
        if matches!(
            control,
            Control::Models | Control::Providers | Control::Effort
        ) && p.models.is_empty()
            && p.models_loading.is_none()
        {
            self.ninja_load_models(cx);
        }
        cx.notify();
    }
    pub(super) fn ninja_command(
        &mut self,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let control = match name {
            "model" => Some(Control::Models),
            "agent" => Some(Control::Providers),
            "thinking" => Some(Control::Effort),
            "permissions" => Some(Control::Permissions),
            "workspace" => Some(Control::Workspace),
            "help" => Some(Control::Slash),
            _ => None,
        };
        if let Some(control) = control {
            self.ninja_toggle_control(control, cx);
            return Ok(());
        }
        match name {
            "orchestrate" => {
                if let Some(p) = &mut self.ninjas.panel {
                    p.profile.orchestrator = !p.profile.orchestrator;
                    p.control_at = Instant::now();
                }
                self.ninja_persist_controls(cx);
            }
            "new" => {
                self.remember_ninja_draft(cx);
                if let Some(p) = &mut self.ninjas.panel {
                    p.chat = None;
                    p.task = None;
                    p.messages.clear();
                    p.fields[3].update(cx, |i, cx| i.set_text("", cx));
                }
            }
            "stop" => {
                let p = self.ninjas.panel.as_ref().ok_or("No Ninja selected")?;
                let t = self
                    .ninjas
                    .snapshots
                    .get(&p.host)
                    .and_then(|s| s["tasks"].as_array())
                    .and_then(|ts| {
                        ts.iter().find(|t| {
                            t["profile"] == p.profile.id
                                && t["chat"].as_str() == p.chat.as_deref()
                                && live(t)
                        })
                    })
                    .and_then(|t| t["id"].as_str())
                    .map(str::to_owned);
                if let Some(id) = t {
                    self.ninja_operation(json!({"action":"stop","id":id}), cx);
                }
            }
            "context" => {
                let p = self.ninjas.panel.as_ref().ok_or("No Ninja selected")?;
                self.notice = Some(
                    self.ninjas
                        .snapshots
                        .get(&p.host)
                        .and_then(|v| v["tasks"].as_array())
                        .and_then(|ts| {
                            ts.iter().find(|t| {
                                t["profile"] == p.profile.id
                                    && t["chat"].as_str() == p.chat.as_deref()
                            })
                        })
                        .and_then(|t| usage_label(&t["usage"]))
                        .unwrap_or_else(|| {
                            "This provider has not reported token usage for this chat yet".into()
                        }),
                );
            }
            _ => return Err("Unknown Ninja command".into()),
        }
        let _ = window;
        cx.notify();
        Ok(())
    }
    pub(super) fn ninja_persist_controls(&mut self, cx: &mut Context<Self>) {
        if let Some(p) = &mut self.ninjas.panel {
            if p.profile.revision > 0 {
                p.controls_dirty = true;
                if !p.busy {
                    p.controls_dirty = false;
                    let request = json!({"action":"save_profile","profile":p.profile});
                    self.ninja_operation(request, cx);
                }
            }
        }
    }
    pub(super) fn ninja_input_event(
        &mut self,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(p) = &mut self.ninjas.panel else {
            return;
        };
        match event {
            InputEvent::Changed | InputEvent::CursorChanged => {
                let slash = p.fields[3].read(cx).command_query().is_some();
                if slash {
                    p.control = Some(Control::Slash);
                } else if p.control == Some(Control::Slash) {
                    p.control = None;
                }
                p.fields[3].update(cx, |i, _| i.set_command_navigation(slash));
                p.control_cursor = 0;
            }
            InputEvent::CommandPrevious => p.control_cursor = p.control_cursor.saturating_sub(1),
            InputEvent::CommandNext => p.control_cursor += 1,
            InputEvent::CommandAccept | InputEvent::CommandComplete => {
                let query = p.fields[3]
                    .read(cx)
                    .command_query()
                    .unwrap_or("")
                    .to_lowercase();
                let matches = COMMANDS
                    .iter()
                    .filter(|(name, _)| name.contains(&query))
                    .collect::<Vec<_>>();
                if let Some((name, _)) =
                    matches.get(p.control_cursor.min(matches.len().saturating_sub(1)))
                {
                    let name = *name;
                    p.fields[3].update(cx, |i, cx| {
                        if let Some(span) = i.command_span() {
                            let mut text = i.text().to_string();
                            text.replace_range(span, "");
                            i.set_text(&text, cx);
                        }
                    });
                    p.control = None;
                    let _ = self.ninja_command(name, window, cx);
                }
            }
            InputEvent::Submit => {
                let text = p.fields[3].read(cx).text();
                if let Some(name) =
                    commands::name(text.trim()).filter(|n| COMMANDS.iter().any(|(v, _)| v == n))
                {
                    let name = name.to_owned();
                    p.fields[3].update(cx, |i, cx| i.set_text("", cx));
                    let _ = self.ninja_command(&name, window, cx);
                } else {
                    self.ninja_submit_task(cx);
                }
            }
            InputEvent::FilesPasted(paths) => self.ninja_reference_paths(paths.clone(), cx),
            InputEvent::ImagePasted(bytes) => self.ninja_reference_image(bytes.clone(), cx),
            InputEvent::ClipboardError(e) => self.notice = Some(e.clone()),
            _ => {}
        }
        cx.notify();
    }
    pub(super) fn ninja_control_view(&self, window: &Window, cx: &Context<Self>) -> AnyElement {
        let Some(p) = &self.ninjas.panel else {
            return div().into_any_element();
        };
        let Some(control) = p.control else {
            return div().into_any_element();
        };
        let mut list = div().flex().flex_col().gap_1();
        match control {
            Control::Slash => {
                let query = p.fields[3]
                    .read(cx)
                    .command_query()
                    .unwrap_or("")
                    .to_lowercase();
                for (i, (name, description)) in COMMANDS
                    .iter()
                    .filter(|(n, _)| n.contains(&query))
                    .enumerate()
                {
                    let name = *name;
                    list = list.child(
                        div()
                            .id(SharedString::from(format!("ninja-command-{name}")))
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .bg(rgba(if i == p.control_cursor { 0xffffff18 } else { 0 }))
                            .text_size(theme::ui_px(12.))
                            .cursor_pointer()
                            .child(format!("/{name}   {description}"))
                            .on_click(cx.listener(move |app, _, w, cx| {
                                if let Some(p) = &mut app.ninjas.panel {
                                    p.control = None;
                                    p.fields[3].update(cx, |i, cx| {
                                        if let Some(span) = i.command_span() {
                                            let mut text = i.text().to_string();
                                            text.replace_range(span, "");
                                            i.set_text(&text, cx);
                                        }
                                    });
                                }
                                let _ = app.ninja_command(name, w, cx);
                            })),
                    );
                }
            }
            Control::Models | Control::Effort => {
                let mut models = div().flex_1().min_w(px(0.)).flex().flex_col().gap_1();
                if control == Control::Models {
                    models = models.child(p.fields[5].clone());
                    for m in p
                        .models
                        .iter()
                        .filter(|m| {
                            p.fields[5].read(cx).text().is_empty()
                                || m["id"]
                                    .as_str()
                                    .unwrap_or("")
                                    .to_lowercase()
                                    .contains(&p.fields[5].read(cx).text().to_lowercase())
                        })
                        .take(120)
                    {
                        let id = m["id"].as_str().unwrap_or("").to_owned();
                        let label = m["label"].as_str().unwrap_or(&id).to_owned();
                        models = models.child(ninjas_ui::marked_chip(
                            SharedString::from(format!("ninja-pick-model-{id}")),
                            composer_design::display_label(&label),
                            icon(
                                composer_design::model_glyph(&id)
                                    .unwrap_or(agent_icon(p.profile.agent)),
                                16.,
                                TEXT,
                            ),
                            p.profile.model.as_ref() == Some(&id),
                            cx,
                            move |app, _, cx| {
                                if let Some(p) = &mut app.ninjas.panel {
                                    p.profile.model = Some(id.clone());
                                    p.profile.effort = None;
                                    if let Some(n) = &mut p.profile.ninja {
                                        n.model = id.clone();
                                        n.thinking = None;
                                    }
                                    p.control = Some(Control::Effort);
                                    p.control_at = Instant::now();
                                }
                                app.ninja_persist_controls(cx);
                            },
                        ));
                    }
                }
                let levels = p
                    .models
                    .iter()
                    .find(|m| m["id"].as_str() == p.profile.model.as_deref())
                    .and_then(|m| m["efforts"].as_array())
                    .cloned()
                    .unwrap_or_default();
                let mut thinking = div().w(px(150.)).flex_none().flex().flex_col().gap_1();
                for (i, e) in levels.iter().filter_map(Value::as_str).enumerate() {
                    let value = e.to_owned();
                    let color = composer_design::effort_color(
                        e,
                        i as f32 / levels.len().saturating_sub(1).max(1) as f32,
                    );
                    let on = p.profile.effort.as_deref() == Some(e);
                    thinking = thinking.child(
                        div()
                            .id(SharedString::from(format!("ninja-pick-effort-{e}")))
                            .debug_selector({
                                let e = e.to_owned();
                                move || format!("ninja-pick-effort-{e}")
                            })
                            .h(px(32.))
                            .relative()
                            .overflow_hidden()
                            .rounded_md()
                            .bg(rgba(if on { 0xffffff18 } else { 0 }))
                            .cursor_pointer()
                            .child(composer_design::shimmer_tiles(
                                Some(p.control_at.elapsed().as_secs_f32()),
                                color,
                                self.motion && !native::reduced_motion(),
                            ))
                            .child(
                                div()
                                    .relative()
                                    .px_3()
                                    .h_full()
                                    .flex()
                                    .items_center()
                                    .text_size(theme::ui_px(12.))
                                    .text_color(rgb(color))
                                    .child(composer_design::title(e)),
                            )
                            .on_click(cx.listener(move |app, _, _, cx| {
                                if let Some(p) = &mut app.ninjas.panel {
                                    p.profile.effort = Some(value.clone());
                                    if let Some(n) = &mut p.profile.ninja {
                                        n.thinking = Some(value.clone());
                                    }
                                    p.effort_preview = Some((value.clone(), Instant::now()));
                                    p.control = None;
                                    p.control_at = Instant::now();
                                }
                                app.ninja_persist_controls(cx);
                            })),
                    );
                }
                if p.models_loading.is_some() {
                    models = models.child("Reading host models…");
                }
                list = list.child(
                    div()
                        .flex()
                        .gap_3()
                        .when(control == Control::Models, |s| s.child(models))
                        .child(thinking),
                );
            }
            Control::Providers => {
                for a in self
                    .ninjas
                    .snapshots
                    .get(&p.host)
                    .into_iter()
                    .flat_map(|v| v["providers"].as_array().into_iter().flatten())
                    .filter(|v| v["installed"] == true)
                    .filter_map(|v| serde_json::from_value::<AgentKind>(v["agent"].clone()).ok())
                {
                    list = list.child(ninjas_ui::marked_chip(
                        SharedString::from(format!("ninja-pick-agent-{}", a.command())),
                        a.name(),
                        icon(agent_icon(a), 16., TEXT),
                        p.profile.agent == a,
                        cx,
                        move |app, w, cx| {
                            let owner = app.ninja_surface_owner();
                            let _ = app.ninja_surface_command(
                                &json!({"action":"agent","agent":a.command(),"ownerToken":owner}),
                                w,
                                cx,
                            );
                            if let Some(p) = &mut app.ninjas.panel {
                                p.control = Some(Control::Models);
                            }
                            app.ninja_load_models(cx);
                        },
                    ));
                }
            }
            Control::Workspace => {
                list = list
                    .child(
                        div()
                            .text_size(theme::ui_px(11.))
                            .child("Workspace on this Ninja’s host"),
                    )
                    .child(p.fields[2].clone())
                    .child(ninjas_ui::chip(
                        "ninja-workspace-apply",
                        "Apply workspace",
                        true,
                        cx,
                        |app, _, cx| {
                            if let Some(p) = &mut app.ninjas.panel {
                                p.profile.project = PathBuf::from(p.fields[2].read(cx).text());
                                p.control = None;
                            }
                            app.ninja_persist_controls(cx);
                        },
                    ));
            }
            Control::Permissions => {
                for (label, access) in [
                    ("Read only", "read-only"),
                    ("Workspace", "workspace"),
                    ("Full access", "full"),
                ] {
                    list=list.child(ninjas_ui::marked_chip(SharedString::from(format!("ninja-permission-{access}")),label,icon(if access=="full"{"lock-keyhole-open"}else if access=="workspace"{"shield"}else{"lock-keyhole"},16.,if access=="full"{0xf38c82}else{0xf2b66e}),false,cx,move|app,w,cx|{let owner=app.ninja_surface_owner();let _=app.ninja_surface_command(&json!({"action":"workspace_permissions","access":access,"ownerToken":owner}),w,cx);}));
                }
                for (label, approval) in [("Ask me", "ask"), ("Never ask", "never")] {
                    list=list.child(ninjas_ui::chip(SharedString::from(format!("ninja-policy-{approval}")),label,(p.profile.approvals==runtime::CodexApprovals::Never)==(approval=="never"),cx,move|app,w,cx|{let owner=app.ninja_surface_owner();let _=app.ninja_surface_command(&json!({"action":"workspace_permissions","approvals":approval,"ownerToken":owner}),w,cx);}));
                }
            }
        }
        let elapsed = p.control_at.elapsed().as_secs_f32();
        if self.motion && !native::reduced_motion() && window.is_window_active() && elapsed < 5. {
            request_motion_frame(window, cx);
        }
        div()
            .id("ninja-chat-controls")
            .debug_selector(|| "ninja-chat-controls".into())
            .absolute()
            .bottom(px(150.))
            .left(px(12.))
            .w(px(560.).min(window.viewport_size().width - px(48.)))
            .max_h(px(340.))
            .overflow_y_scroll()
            .occlude()
            .p_3()
            .rounded_xl()
            .border_1()
            .border_color(rgba(0xffffff24))
            .bg(rgba(appearance::MENU_SURFACE))
            .shadow_lg()
            .opacity(if !self.motion {
                1.
            } else {
                (elapsed / 0.18).clamp(0., 1.)
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(|_, _, cx| cx.stop_propagation())
            .capture_key_down(cx.listener(Self::ninja_control_key))
            .on_mouse_down_out(cx.listener(|app, _, _, cx| {
                if let Some(p) = &mut app.ninjas.panel {
                    p.dismissed_control = p.control.map(|c| (c, Instant::now()));
                    p.control = None;
                }
                cx.notify();
            }))
            .child(list)
            .into_any_element()
    }
    fn ninja_control_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.keystroke.modifiers.platform
            || event.keystroke.modifiers.alt
            || event.keystroke.modifiers.control
        {
            return;
        }
        let Some(p) = &mut self.ninjas.panel else {
            return;
        };
        let Some(control) = p.control else { return };
        if control == Control::Slash {
            return;
        }
        let key = event.keystroke.key.as_str();
        if !matches!(key, "up" | "down" | "left" | "right" | "enter" | "escape") {
            return;
        }
        cx.stop_propagation();
        window.prevent_default();
        if key == "escape" {
            p.control = None;
            p.fields[3].read(cx).focus_handle.focus(window);
            cx.notify();
            return;
        }
        if key == "right" && control == Control::Models {
            p.control = Some(Control::Effort);
            p.control_cursor = 0;
            cx.notify();
            return;
        }
        if key == "left" && control == Control::Effort {
            p.control = Some(Control::Models);
            p.control_cursor = 0;
            cx.notify();
            return;
        }
        let query = p.fields[5].read(cx).text().to_lowercase();
        let choices = match control {
            Control::Models => p
                .models
                .iter()
                .filter(|m| {
                    m["id"]
                        .as_str()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&query)
                })
                .filter_map(|m| m["id"].as_str().map(str::to_owned))
                .collect::<Vec<_>>(),
            Control::Effort => p
                .models
                .iter()
                .find(|m| m["id"].as_str() == p.profile.model.as_deref())
                .into_iter()
                .flat_map(|m| m["efforts"].as_array().into_iter().flatten())
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect(),
            _ => vec![],
        };
        if key == "up" {
            p.control_cursor = p.control_cursor.saturating_sub(1);
        } else if key == "down" {
            p.control_cursor = (p.control_cursor + 1).min(choices.len().saturating_sub(1));
        } else if key == "enter" {
            if let Some(value) = choices.get(p.control_cursor) {
                if control == Control::Models {
                    p.profile.model = Some(value.clone());
                    p.profile.effort = None;
                    if let Some(n) = &mut p.profile.ninja {
                        n.model = value.clone();
                        n.thinking = None;
                    }
                    p.control = Some(Control::Effort);
                } else {
                    p.profile.effort = Some(value.clone());
                    if let Some(n) = &mut p.profile.ninja {
                        n.thinking = Some(value.clone());
                    }
                    p.effort_preview = Some((value.clone(), Instant::now()));
                    p.control = None;
                }
                p.control_cursor = 0;
                p.control_at = Instant::now();
                self.ninja_persist_controls(cx);
            }
        }
        cx.notify();
    }
    pub(super) fn ninja_animated_control(
        &self,
        id: &'static str,
        label: String,
        glyph: &'static str,
        color: u32,
        on: bool,
        control: Option<Control>,
        window: &Window,
        cx: &Context<Self>,
    ) -> AnyElement {
        let p = self.ninjas.panel.as_ref().unwrap();
        let motion = self.motion && !native::reduced_motion() && window.is_window_active();
        let chip = div()
            .id(id)
            .debug_selector(move || id.into())
            .h(px(28.))
            .px_2()
            .flex_none()
            .relative()
            .overflow_hidden()
            .rounded_md()
            .flex()
            .items_center()
            .gap_1()
            .text_size(theme::ui_px(11.))
            .text_color(rgb(color))
            .cursor_pointer()
            .when(!self.theme.borderless_composer, |s| {
                s.bg(rgba(if on { (color << 8) | 0x22 } else { 0xffffff0b }))
            })
            .child(icon(glyph, 14., color))
            .when(!label.is_empty(), |s| s.child(label))
            .on_click(cx.listener(move |app, w: &ClickEvent, window, cx| {
                cx.stop_propagation();
                let _ = w;
                if let Some(control) = control {
                    app.ninja_toggle_control(control, cx);
                } else {
                    let _ = app.ninja_command("orchestrate", window, cx);
                }
            }));
        if on && motion {
            chip.with_animation(
                SharedString::from(format!("{id}-{:?}", p.control_at)),
                Animation::new(Duration::from_secs(5)),
                move |chip, t| {
                    chip.child(composer_design::shimmer_tiles(Some(t * 5.), color, true))
                },
            )
            .into_any_element()
        } else {
            chip.into_any_element()
        }
    }
}

pub(super) fn encode_path(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
fn artifact_key(host: &str, profile: &str, target: &str) -> String {
    format!("{host}:{profile}:{target}")
}
fn decode64(text: &str) -> anyhow::Result<Vec<u8>> {
    anyhow::ensure!(
        text.len() <= 360 * 1024 && text.len() % 4 == 0,
        "Invalid artifact chunk"
    );
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    for group in text.as_bytes().chunks_exact(4) {
        let mut n = 0u32;
        for &b in group {
            let v = match b {
                b'A'..=b'Z' => b - b'A',
                b'a'..=b'z' => b - b'a' + 26,
                b'0'..=b'9' => b - b'0' + 52,
                b'+' => 62,
                b'/' => 63,
                b'=' => 0,
                _ => anyhow::bail!("Invalid artifact encoding"),
            };
            n = n << 6 | v as u32;
        }
        out.push((n >> 16) as u8);
        if group[2] != b'=' {
            out.push((n >> 8) as u8);
        }
        if group[3] != b'=' {
            out.push(n as u8);
        }
    }
    Ok(out)
}
fn image_target(target: &str) -> bool {
    target.split(['?', '#']).next().is_some_and(|p| {
        ["png", "jpg", "jpeg", "gif", "webp", "bmp", "tiff"].contains(
            &p.rsplit('.')
                .next()
                .unwrap_or("")
                .to_ascii_lowercase()
                .as_str(),
        )
    })
}
fn fetch_artifact(
    host: &Host,
    profile: &str,
    target: &str,
    root: &std::path::Path,
) -> anyhow::Result<Artifact> {
    use anyhow::{Context as _, ensure};
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let mut data = Vec::new();
    let mut direct = false;
    let source_path = if let Some(id) = target.strip_prefix("kai://artifact/") {
        let v = call(
            host,
            &json!({"action":"artifact_document","id":profile,"artifact":id}),
        )?;
        let content = v["content"].as_str().context("Missing artifact content")?;
        ensure!(content.len() <= 512 * 1024, "Artifact is too large");
        data.extend_from_slice(content.as_bytes());
        direct = true;
        let title = v["title"]
            .as_str()
            .unwrap_or("Document")
            .chars()
            .filter(|c| c.is_alphanumeric() || " _-".contains(*c))
            .take(80)
            .collect::<String>();
        let extension = match v["kind"].as_str() {
            Some("html") => "html",
            Some("markdown" | "mermaid") => "md",
            _ => "txt",
        };
        PathBuf::from(format!(
            "{}{extension}",
            if title.is_empty() {
                "Document.".into()
            } else {
                format!("{title}.")
            }
        ))
    } else if target.starts_with("https://") && image_target(target) {
        // Public image URLs render as images too, with bounded HTTPS-only reads.
        let result = std::process::Command::new("/usr/bin/curl")
            .args([
                "--silent",
                "--show-error",
                "--fail",
                "--location",
                "--proto",
                "=https",
                "--proto-redir",
                "=https",
                "--connect-timeout",
                "3",
                "--max-time",
                "20",
                "--max-filesize",
                "16777216",
                "--url",
                target,
            ])
            .output()?;
        ensure!(result.status.success(), "Image download failed");
        ensure!(
            result.stdout.len() as u64 <= attachments::MAX_FILE_BYTES,
            "Image is too large"
        );
        data = result.stdout;
        direct = true;
        PathBuf::from(
            target
                .split(['?', '#'])
                .next()
                .unwrap_or(target)
                .rsplit('/')
                .next()
                .unwrap_or("image.png"),
        )
    } else {
        match citations::classify(target).map_err(anyhow::Error::msg)? {
            citations::Citation::File(f) => f.path,
            _ => anyhow::bail!("Choose a generated file"),
        }
    };
    let mut offset = 0;
    let mut total = None;
    while !direct {
        let v = call(
            host,
            &json!({"action":"artifact","id":profile,"path":source_path,"offset":offset}),
        )?;
        let size = v["total_bytes"].as_u64().context("Missing file size")?;
        ensure!(
            size <= attachments::MAX_FILE_BYTES,
            "Preview files must be 16 MB or smaller"
        );
        ensure!(
            total.is_none_or(|n| n == size) && v["offset"] == offset,
            "Artifact changed while downloading"
        );
        total = Some(size);
        let part = decode64(v["base64"].as_str().context("Missing file data")?)?;
        ensure!(
            part.len() <= 256 * 1024 && (size == 0 || !part.is_empty()),
            "Invalid artifact chunk"
        );
        offset += part.len() as u64;
        ensure!(offset <= size, "Invalid file size");
        data.extend(part);
        if v["next_offset"].is_null() {
            ensure!(offset == size, "Incomplete artifact");
            break;
        }
        ensure!(v["next_offset"] == offset, "Invalid next chunk");
    }
    std::fs::create_dir_all(root)?;
    std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700))?;
    let name = source_path.file_name().context("Missing file name")?;
    let directory = root.join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir(&directory)?;
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
    let path = directory.join(name);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?;
    file.write_all(&data)?;
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let image = if image_target(target) {
        Some(attachments::preview(&path, &cancel)?)
    } else {
        None
    };
    let native_preview = if image.is_some() {
        let pixels = background::attachment_thumbnail(&data, 280)?;
        let mut bytes = Vec::new();
        use image::ImageEncoder;
        image::codecs::png::PngEncoder::new(&mut bytes).write_image(
            pixels.as_raw(),
            pixels.width(),
            pixels.height(),
            image::ExtendedColorType::Rgba8,
        )?;
        Some(artifacts::base64_bytes(&bytes))
    } else {
        None
    };
    Ok(Artifact {
        path,
        image,
        native_preview,
    })
}
impl Orbit {
    pub(super) fn ninja_prefetch_images(&mut self, cx: &mut Context<Self>) {
        if cfg!(test) {
            return;
        }
        let Some(p) = &self.ninjas.panel else { return };
        let targets = p
            .messages
            .iter()
            .chain(
                self.ninjas
                    .snapshots
                    .get(&p.host)
                    .into_iter()
                    .flat_map(|s| s["tasks"].as_array().into_iter().flatten()),
            )
            .filter(|t| t["chat"].as_str() == p.chat.as_deref())
            .flat_map(|t| {
                presentation::message_blocks("assistant", t["brief"].as_str().unwrap_or(""))
            })
            .filter_map(|b| match b {
                MessageBlock::Attachment { target, .. } if image_target(&target) => Some(target),
                _ => None,
            })
            .collect::<HashSet<_>>();
        for target in targets.into_iter().take(8) {
            self.ninja_request_artifact(target, false, cx);
        }
    }
    pub(super) fn ninja_request_artifact(
        &mut self,
        target: String,
        open: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(p) = &self.ninjas.panel else { return };
        let Some(host) = self.ninjas.hosts.iter().find(|h| h.id == p.host).cloned() else {
            return;
        };
        let key = artifact_key(&p.host, &p.profile.id, &target);
        if let Some(result) = self.ninjas.artifacts.get(&key) {
            if open {
                match result {
                    Ok(a) => unified::emit(&json!({"type":"quickLook","paths":[a.path]})),
                    Err(e) => self.notice = Some(e.clone()),
                }
            }
            return;
        }
        if self.ninjas.artifact_pending.len() >= 3
            || !self.ninjas.artifact_pending.insert(key.clone())
        {
            return;
        }
        let profile = p.profile.id.clone();
        let root = self.attachment_dir.join("ninja-previews");
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    fetch_artifact(&host, &profile, &target, &root).map_err(|e| e.to_string())
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                app.ninjas.artifact_pending.remove(&key);
                if open {
                    match &result {
                        Ok(a) => unified::emit(&json!({"type":"quickLook","paths":[a.path]})),
                        Err(e) => app.notice = Some(e.clone()),
                    }
                }
                if app.ninjas.artifacts.len() >= 8 {
                    if let Some(old) = app.ninjas.artifacts.keys().next().cloned() {
                        if let Some(Ok(a)) = app.ninjas.artifacts.remove(&old) {
                            cx.background_executor()
                                .spawn(async move {
                                    if let Some(parent) = a.path.parent() {
                                        let _ = std::fs::remove_dir_all(parent);
                                    }
                                })
                                .detach();
                        }
                    }
                }
                app.ninjas.artifacts.insert(key, result);
                cx.notify();
            });
        })
        .detach();
    }
    pub(super) fn ninja_open_file(&mut self, target: &str, cx: &mut Context<Self>) -> bool {
        if self.ninja_surface_owner().is_none() {
            return false;
        }
        if target.starts_with("kai://artifact/") {
            self.ninja_request_artifact(target.into(), true, cx);
            return true;
        }
        let Ok(citations::Citation::File(citation)) = citations::classify(target) else {
            return false;
        };
        if citation.path.starts_with(&self.attachment_dir) {
            return false;
        }
        self.ninja_request_artifact(target.into(), true, cx);
        true
    }
    pub(super) fn ninja_artifact_card(
        &self,
        ix: usize,
        name: &str,
        target: &str,
        cx: &Context<Self>,
    ) -> AnyElement {
        let p = self.ninjas.panel.as_ref().unwrap();
        let key = artifact_key(&p.host, &p.profile.id, target);
        let image = self
            .ninjas
            .artifacts
            .get(&key)
            .and_then(|v| v.as_ref().ok())
            .and_then(|a| a.image.clone());
        let target = target.to_owned();
        div()
            .id(("ninja-artifact", ix))
            .debug_selector(move || format!("ninja-artifact-{ix}"))
            .w_full()
            .rounded_lg()
            .overflow_hidden()
            .border_1()
            .border_color(rgba(0xffffff18))
            .bg(rgba(self.appearance.tint(GLASS_SURFACE)))
            .flex()
            .flex_col()
            .cursor_pointer()
            .when_some(image, |s, image| {
                s.child(
                    img(image)
                        .w_full()
                        .max_h(px(300.))
                        .object_fit(ObjectFit::Contain),
                )
            })
            .child(
                div()
                    .px_3()
                    .py_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(icon(
                        if image_target(&target) {
                            "file-image"
                        } else {
                            "file"
                        },
                        16.,
                        TEXT,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .text_size(theme::ui_px(12.))
                            .child(name.to_owned()),
                    )
                    .child(icon("external", 12., MUTED)),
            )
            .on_click(cx.listener(move |app, _, _, cx| {
                cx.stop_propagation();
                app.ninja_request_artifact(target.clone(), true, cx);
            }))
            .into_any_element()
    }
    pub(super) fn ninja_native_url(&self, target: &str) -> String {
        if target.starts_with("kai://artifact/") {
            return format!("kai-ninja-file://open?path={}", encode_path(target));
        }
        if matches!(
            citations::classify(target),
            Ok(citations::Citation::File(_))
        ) {
            let local = self.ninja_native_target(target);
            if local == target {
                format!("kai-ninja-file://open?path={}", encode_path(target))
            } else {
                format!("file://{}", encode_path(&local).replace("%2F", "/"))
            }
        } else {
            target.into()
        }
    }
    pub(super) fn ninja_native_links(&self, block: &MessageBlock) -> Value {
        match block{
            MessageBlock::RichParagraph{text,highlights}|MessageBlock::ListItem{text,highlights,..}=>json!(highlights.iter().filter_map(|(range,style)|{
                if let InlineStyle::Link(target)=style{if range.end<=text.len()&&text.is_char_boundary(range.start)&&text.is_char_boundary(range.end){return Some(json!({"location":text[..range.start].encode_utf16().count(),"length":text[range.clone()].encode_utf16().count(),"target":self.ninja_native_url(target)}));}}None
            }).collect::<Vec<_>>()),
            _=>Value::Null,
        }
    }
    pub(super) fn ninja_native_preview(&self, target: &str) -> Option<&str> {
        let p = self.ninjas.panel.as_ref()?;
        self.ninjas
            .artifacts
            .get(&artifact_key(&p.host, &p.profile.id, target))?
            .as_ref()
            .ok()?
            .native_preview
            .as_deref()
    }
    pub(super) fn ninja_native_target(&self, target: &str) -> String {
        let Some(p) = &self.ninjas.panel else {
            return target.into();
        };
        self.ninjas
            .artifacts
            .get(&artifact_key(&p.host, &p.profile.id, target))
            .and_then(|a| a.as_ref().ok())
            .map_or_else(|| target.into(), |a| a.path.to_string_lossy().into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    #[test]
    fn terminal_tasks_never_advertise_activity_and_usage_is_provider_reported() {
        for state in ["completed", "failed", "stopped", "interrupted"] {
            assert!(!live(&json!({"state":state})));
        }
        assert!(!live(&json!({"state":"running","finished":10})));
        assert!(live(&json!({"state":"running","finished":null})));
        assert_eq!(
            usage_label(&json!({"context_used":64,"context_limit":128})).as_deref(),
            Some("Context · 50% used")
        );
        assert_eq!(usage_label(&json!({})), None);
        assert_eq!(decode64("aGVsbG8=").unwrap(), b"hello");
        assert!(decode64("!!!!").is_err());
    }
    #[gpui::test]
    fn long_ninja_turns_keep_readable_width_and_virtualize_offscreen_work(cx:&mut TestAppContext){
        let(view,cx)=crate::ui_tests::fixture(cx);
        cx.update(|w,cx|view.update(cx,|app,cx|{
            app.motion=false;app.sidebar_split=false;app.sidebar_layout_transition=None;app.open_ninjas(Some("local".into()),None,w,cx);let p=app.ninjas.panel.as_mut().unwrap();p.profile.revision=1;p.settings=false;p.chat=Some("rendering".into());
            let tool=chat_interactions::serialize_interaction(&chat_interactions::ChatInteraction::Delegation{title:"Terminal output".into(),details:"line one\nline two\n".repeat(300),status:chat_interactions::InteractionStatus::Completed,agent:None});
            p.messages=(0..24).map(|i|json!({"id":format!("turn-{i}"),"profile":p.profile.id,"chat":"rendering","created":i,"prompt":"Please update the terminal and verify the finished result. ".repeat(8),"brief":format!("{tool}\n\n## Finished\n\n**Verified** the result with a readable response."),"state":"completed","finished":i+1})).collect();
        }));
        crate::ui_tests::draw_frame(&view,cx);crate::ui_tests::draw_frame(&view,cx);
        let user=cx.debug_bounds("ninja-bubble-turn-23-true").unwrap();let reply=cx.debug_bounds("ninja-bubble-turn-23-false").unwrap();
        assert!(user.size.width>px(400.)&&user.size.height<px(350.),"User content collapsed into a vertical strip: {user:?}");assert!(reply.size.width>px(400.),"Assistant content lost its flex width: {reply:?}");
        let work=block_key("turn-23:work",0);let button=Box::leak(format!("ninja-work-{work}").into_boxed_str());crate::ui_tests::click_selector(cx,button);crate::ui_tests::draw_frame(&view,cx);
        let ix=block_key("turn-23:false",0);let selector=Box::leak(format!("interaction-title-{ix}").into_boxed_str());assert!(cx.debug_bounds(selector).unwrap().size.width>px(150.),"Terminal title was clipped to Ter");
        view.update(cx,|app,cx|{let p=app.ninjas.panel.as_ref().unwrap();assert!(p.blocks.borrow().len()<16,"Offscreen turns were still being built for each frame");let before=app.ninja_blocks(false,p.messages[23]["brief"].as_str().unwrap(),"turn-23");let snapshot=app.unified_snapshot(cx);let after=app.ninja_blocks(false,p.messages[23]["brief"].as_str().unwrap(),"turn-23");assert!(Arc::ptr_eq(&before,&after),"Native snapshots must reuse the same parsed document");assert!(!snapshot["history"].to_string().contains("orbit_interaction"));});
    }
    #[gpui::test]
    fn ninja_rich_reply_uses_normal_code_and_tool_cards_without_touching_code_history(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        cx.update(|w,cx|view.update(cx,|app,cx|{
            app.motion=false;app.open_ninjas(Some("local".into()),None,w,cx);let p=app.ninjas.panel.as_mut().unwrap();p.settings=false;p.profile.revision=1;p.chat=Some("chat".into());
            let tool=chat_interactions::serialize_interaction(&chat_interactions::ChatInteraction::Status{title:"Terminal output".into(),details:"cargo check passed".into(),status:chat_interactions::InteractionStatus::Completed});
            p.messages=vec![json!({"id":"rich","profile":p.profile.id,"chat":"chat","created":1,"prompt":"Show result","brief":format!("## Result\n\n{tool}\n\n```rust\nlet answer = 42;\n```\n\n![Screenshot](/tmp/result.png)"),"state":"completed","finished":2})];
            app.rows=vec![Row{role:Some("assistant".into()),block:MessageBlock::Paragraph("Original Code chat".into())}];
        }));
        crate::ui_tests::draw_frame(&view, cx);
        view.update(cx, |app, cx| {
            let p = app.ninjas.panel.as_ref().unwrap();
            let blocks = &p.blocks.borrow()["rich:false"].1;
            assert!(
                blocks
                    .iter()
                    .any(|b| matches!(b, MessageBlock::Code { .. }))
            );
            assert!(
                blocks
                    .iter()
                    .any(|b| matches!(b, MessageBlock::Attachment { .. }))
            );
            assert!(
                blocks
                    .iter()
                    .any(|b| matches!(b, MessageBlock::Interaction(_)))
            );
            assert_eq!(app.rows.len(), 1);
            let snapshot = app.unified_snapshot(cx);
            assert!(
                snapshot["history"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|m| m["lines"][0]["style"] == "code")
            );
            assert!(
                !snapshot["history"]
                    .to_string()
                    .contains("orbit_interaction")
            );
            assert!(snapshot["activeTurnId"].is_null());
            assert!(!snapshot["history"].to_string().contains("cargo check passed"),"Collapsed native work leaked raw tool output");

        });
    }
    #[gpui::test]
    fn ninja_composer_controls_and_slash_commands_keep_the_host_owner_and_draft(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        cx.update(|w, cx| {
            view.update(cx, |app, cx| {
                app.motion = false;
                app.open_ninjas(Some("local".into()), None, w, cx);
                let p = app.ninjas.panel.as_mut().unwrap();
                p.settings = false;
                p.profile.revision = 1;
                p.busy = true;
                p.profile.model = Some("host-model".into());
                p.profile.effort = Some("low".into());
                p.models =
                    vec![json!({"id":"host-model","label":"Host Model","efforts":["low","high"]})];
                p.fields[3].update(cx, |i, cx| i.set_text("Keep this Ninja draft", cx));
                let owner = app.ninja_surface_owner();
                app.ninja_toggle_control(Control::Models, cx);
                assert!(!app.ninjas.panel.as_ref().unwrap().settings);
                app.ninja_toggle_control(Control::Models, cx);
                assert!(app.ninjas.panel.as_ref().unwrap().control.is_none());
                app.ninja_toggle_control(Control::Effort, cx);
                assert_eq!(app.ninja_surface_owner(), owner);
                assert_eq!(
                    app.ninjas.panel.as_ref().unwrap().fields[3].read(cx).text(),
                    "Keep this Ninja draft"
                );
            })
        });
        crate::ui_tests::draw_frame(&view, cx);
        let high = cx.debug_bounds("ninja-pick-effort-high");
        // The actual click uses the same animated effort card as the picker.
        let high = high.expect("Direct reasoning picker must be visible in the chat");
        cx.simulate_click(high.center(), Modifiers::none());
        view.update(cx, |app, _| {
            assert_eq!(
                app.ninjas.panel.as_ref().unwrap().profile.effort.as_deref(),
                Some("high")
            )
        });
        cx.update(|w,cx|view.update(cx,|app,cx|{
            let p=app.ninjas.panel.as_mut().unwrap();p.fields[3].update(cx,|i,cx|i.set_text("/model",cx));app.ninja_input_event(&InputEvent::Submit,w,cx);assert_eq!(app.ninjas.panel.as_ref().unwrap().control,Some(Control::Models));assert!(app.ninjas.panel.as_ref().unwrap().fields[3].read(cx).text().is_empty());
            app.composer.update(cx,|i,cx|i.set_text("Unrelated Code draft",cx));let owner=app.ninja_surface_owner();let result=app.ninja_surface_command(&json!({"action":"composer_command","name":"thinking","draft":"/thinking","caret":9,"ownerToken":owner}),w,cx);assert!(result.unwrap());assert_eq!(app.ninjas.panel.as_ref().unwrap().control,Some(Control::Effort));assert_eq!(app.composer.read(cx).text(),"Unrelated Code draft");assert!(app.unified_snapshot(cx)["slashCommands"].as_array().unwrap().iter().any(|v|v["name"]=="model"));
        }));
    }
    #[test]
    fn remote_artifacts_are_chunked_workspace_scoped_and_previewed_off_thread() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let root = std::env::temp_dir().join(format!("kai-rich-artifact-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let project = root.join("project");
        std::fs::create_dir(&project).unwrap();
        let path = project.join("result image.png");
        image::RgbaImage::from_pixel(64, 32, image::Rgba([40, 90, 220, 255]))
            .save(&path)
            .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server_project = project.clone();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0; 4096];
            let split;
            loop {
                let n = stream.read(&mut buffer).unwrap();
                bytes.extend_from_slice(&buffer[..n]);
                if let Some(i) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    split = i + 4;
                    break;
                }
            }
            let headers = String::from_utf8_lossy(&bytes[..split]);
            let length: usize = headers
                .lines()
                .find_map(|s| {
                    s.to_lowercase()
                        .strip_prefix("content-length: ")
                        .and_then(|v| v.trim().parse().ok())
                })
                .unwrap();
            while bytes.len() < split + length {
                let n = stream.read(&mut buffer).unwrap();
                bytes.extend_from_slice(&buffer[..n]);
            }
            let request: Value = serde_json::from_slice(&bytes[split..split + length]).unwrap();
            assert_eq!(request["action"], "artifact");
            let value = artifacts::scoped_file(
                &server_project,
                &PathBuf::from(request["path"].as_str().unwrap()),
                0,
            )
            .unwrap();
            let body = serde_json::to_vec(&value).unwrap();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(&body).unwrap();
        });
        let host = Host {
            id: "fixture".into(),
            name: "Remote host".into(),
            endpoint: Some(format!("http://{address}")),
        };
        let result = fetch_artifact(
            &host,
            "profile",
            path.to_str().unwrap(),
            &root.join("cache"),
        )
        .unwrap();
        assert!(result.image.is_some());
        assert!(result.native_preview.is_some());
        assert_eq!(
            std::fs::read(&path).unwrap(),
            std::fs::read(result.path).unwrap()
        );
        server.join().unwrap();
        assert!(artifacts::scoped_file(&project, &root.join("outside.txt"), 0).is_err());
        std::os::unix::fs::symlink("/etc/hosts", project.join("escape")).unwrap();
        assert!(artifacts::scoped_file(&project, &project.join("escape"), 0).is_err());
        let _ = std::fs::remove_dir_all(root);
    }
}
