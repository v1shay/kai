//! Native child workspace. Service events coalesce; transcript pages and parsing
//! stay on workers. Pending decisions reuse the owning native request UI.
use super::*;
use runtime::broker::{self, ChildAction, ChildSnapshot};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Clone)]
pub(super) struct RequestOwner {
    pub project: PathBuf,
    pub provider: AgentKind,
    pub child: String,
    pub live: Arc<AtomicBool>,
}
pub(super) struct Panel {
    pub project: PathBuf,
    pub started: Instant,
    pub children: Vec<ChildSnapshot>,
    services: Vec<broker::ServiceSnapshot>,
    diagnostics: bool,
    selected: Option<(AgentKind, String)>,
    rows: Vec<(String, bool)>,
    // Worker-cached width: rendering measures one bounded row, never rescans a page.
    row_width: f32,
    offset: usize,
    next: usize,
    total: usize,
    revision: u64,
    loading: bool,
    follow: bool,
    request: uuid::Uuid,
    error: Option<String>,
    input: Entity<TextInput>,
    _subscription: Subscription,
    scroll: UniformListScrollHandle,
}
impl Drop for Panel {
    fn drop(&mut self) {
        broker::watch_transcripts(false);
    }
}
impl Panel {
    fn reconcile_selection(&mut self, cx: &mut App) {
        if self.selected.as_ref().is_some_and(|(provider, id)| {
            self.children
                .iter()
                .any(|child| child.provider == *provider && child.id == *id)
        }) {
            return;
        }
        let next = self
            .children
            .first()
            .map(|child| (child.provider, child.id.clone()));
        if next == self.selected {
            return;
        }
        // Invalidate any page already in flight before selecting another owner.
        self.request = uuid::Uuid::new_v4();
        self.selected = next;
        self.rows.clear();
        self.row_width = 24.;
        self.offset = 0;
        self.next = 0;
        self.total = 0;
        self.revision = 0;
        self.loading = false;
        self.follow = true;
        self.error = None;
        self.scroll
            .0
            .borrow()
            .base_handle
            .set_offset(point(px(0.), px(0.)));
        self.input.update(cx, |input, cx| input.set_text("", cx));
    }
    pub fn animating(&self) -> bool {
        self.started.elapsed() < Duration::from_millis(320)
    }
}
// Rows remain bounded to 2 KiB by the page worker. Reserve generous per-scalar
// width for proportional text, wide glyphs and tab stops without text shaping on
// the UI thread. This only scans the bounded page when new data arrives.
fn transcript_width(rows: &[(String, bool)]) -> f32 {
    let columns = rows
        .iter()
        .map(|(text, _)| {
            text.chars().fold(0usize, |column, ch| {
                if ch == '\t' {
                    (column / 8 + 1) * 8
                } else {
                    column + 1
                }
            })
        })
        .max()
        .unwrap_or(0);
    24. + columns as f32 * 14.
}
impl Orbit {
    pub(super) fn toggle_children(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.conversation_form.compact(){unified::reveal();if self.broker_panel.is_some(){return}}
        if self.reopen_closing_pane(pane_layout::Pane::Children,cx){return}
        if self.broker_panel.is_some() {self.close_pane(pane_layout::Pane::Children,cx);return}
        if self.broker_panel.take().is_some() {
            cx.notify();
            return;
        }
        self.prepare_inspector_switch(pane_layout::Pane::Children);
        self.files_visible = false;
        if let Some(browser) = &self.file_browser {
            browser.update(cx, |browser, cx| browser.set_visible(false, cx));
        }
        self.diff_view = None;
        self.diff_observer = None;
        self.diff_started = None;
        broker::watch_transcripts(true);
        let input = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_placeholder("Steer this subagent…", cx);
            input
        });
        let subscription = cx.subscribe_in(&input, window, |this, _, event, _, cx| {
            if matches!(event, InputEvent::Submit) {
                this.steer_child(cx);
            }
            cx.notify();
        });
        self.broker_panel = Some(Panel {
            project: self
                .execution_root()
                .unwrap_or_else(|| self.project.clone()),
            started: Instant::now(),
            children: vec![],
            services: vec![],
            diagnostics: false,
            selected: None,
            rows: vec![],
            row_width: 24.,
            offset: 0,
            next: 0,
            total: 0,
            revision: 0,
            loading: false,
            follow: true,
            request: uuid::Uuid::new_v4(),
            error: None,
            input,
            _subscription: subscription,
            scroll: UniformListScrollHandle::new(),
        });
        self.refresh_broker(cx);
        cx.notify();
    }
    pub(super) fn refresh_broker(&mut self, cx: &mut Context<Self>) {
        if self.broker_refreshing {
            self.broker_dirty = true;
            return;
        }
        self.broker_refreshing = true;
        let project = self
            .broker_panel
            .as_ref()
            .map(|p| p.project.clone())
            .unwrap_or_else(|| {
                self.execution_root()
                    .unwrap_or_else(|| self.project.clone())
            });
        self.broker_project = project.clone();
        let job = cx.background_executor().spawn(async move {
            let services = broker::services();
            let roots: HashSet<_> = services
                .iter()
                .map(|s| s.project.clone())
                .chain(std::iter::once(project.clone()))
                .collect();
            let mut children: Vec<_> = roots
                .iter()
                .flat_map(|root| broker::children(root))
                .collect();
            children.sort_by(|a, b| (&a.parent_id, &a.id).cmp(&(&b.parent_id, &b.id)));
            let errors: Vec<_> = children
                .iter()
                .flat_map(|child| {
                    child.requests.iter().filter_map(|r| {
                        broker::take_child_request_error(
                            &child.project,
                            child.provider,
                            &child.id,
                            &r.id,
                        )
                        .map(|e| (r.id.clone(), e))
                    })
                })
                .collect();
            (project, children, errors, services)
        });
        cx.spawn(async move |this, cx| {
            let (project, children, errors, services) = job.await;
            let _ = this.update(cx, |this, cx| {
                this.broker_refreshing = false;
                let stale_project = this
                    .broker_panel
                    .as_ref()
                    .map(|p| p.project.clone())
                    .unwrap_or_else(|| {
                        this.execution_root()
                            .unwrap_or_else(|| this.project.clone())
                    })
                    != project;
                for pending in this.native_pending.values().filter(|p| p.broker.is_some()) {
                    pending
                        .broker
                        .as_ref()
                        .unwrap()
                        .live
                        .store(false, Ordering::Release);
                }
                this.remember_service_usage(&services);
                this.broker_children.clear();
                for child in children.iter().filter(|c|c.status==chat_interactions::InteractionStatus::Completed){
                    let event=(crate::unified::child_id(child),child.turn_id.clone().unwrap_or_else(||child.id.clone()));
                    if !this.unified_successes.contains(&event){if this.unified_successes.len()>=128{this.unified_successes.pop_front();}this.unified_successes.push_back(event);}
                }
                // Keep the affordance after completion, scoped to the owning chat.
                for child in &children {
                    if let Some(owner) = this.child_owner(child, &children).map(|s| s.id.clone()) {
                        this.subagent_chats.insert(owner);
                    }
                }
                this.unified_children=children.clone();
                for child in children.iter().filter(|c| c.connected) {
                    let owner = this.child_owner(child, &children).map(|s| s.id.clone());
                    let Some(owner) = owner else {
                        continue;
                    };
                    if matches!(
                        child.status,
                        crate::chat_interactions::InteractionStatus::Started
                            | crate::chat_interactions::InteractionStatus::Working
                    ) || !child.requests.is_empty()
                    {
                        *this.broker_children.entry(owner.clone()).or_default() += 1;
                    }
                    for request in &child.requests {
                        if let Some(pending) = this.native_pending.get_mut(&request.id) {
                            if let Some(b) = &pending.broker {
                                b.live.store(true, Ordering::Release);
                            }
                        } else {
                            let mut request = request.clone();
                            request.title = format!(
                                "{} · subagent {} · {}",
                                agent_name(child.provider),
                                short(&child.id, 10),
                                request.title
                            );
                            let binding = RequestOwner {
                                project: child.project.clone(),
                                provider: child.provider,
                                child: child.id.clone(),
                                live: Arc::new(AtomicBool::new(true)),
                            };
                            this.native_pending.insert(
                                request.id.clone(),
                                native_request_ui::LiveRequest::for_child(&owner, request, binding),
                            );
                        }
                    }
                }
                for (token, error) in errors {
                    if let Some(p) = this.native_pending.get_mut(&token) {
                        p.retry();
                        this.notice = Some(error);
                    }
                }
                this.native_pending.retain(|_, p| p.current(&this.runs));
                let mut load = None;
                let owned:HashSet<_>=children.iter().filter(|child|this.child_owner(child,&children).is_some_and(|owner|this.active.as_ref()==Some(&owner.id))).map(crate::unified::child_id).collect();
                if let Some(panel) = this.broker_panel.as_mut().filter(|p| p.project == project) {
                    panel.children = children
                        .into_iter()
                        .filter(|c| c.project == project || owned.contains(&crate::unified::child_id(c)))
                        .collect();
                    panel.services = services
                        .into_iter()
                        .filter(|s| s.project == project)
                        .collect();
                    panel.reconcile_selection(cx);
                    if let Some(child) = panel
                        .children
                        .iter()
                        .find(|c| Some(&(c.provider, c.id.clone())) == panel.selected.as_ref())
                    {
                        let follow = panel.follow;
                        if !panel.loading
                            && (panel.rows.is_empty()
                                || follow && panel.revision < child.transcript_revision)
                        {
                            load = Some(child.transcript_bytes.saturating_sub(64 * 1024));
                        }
                    }
                }
                if let Some(offset) = load {
                    this.load_child_page(offset, cx);
                }
                let dirty = std::mem::take(&mut this.broker_dirty);
                if stale_project || dirty {
                    this.refresh_broker(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }
    // Called on coalesced service events, not while painting. Idle child quota
    // updates merge with turn usage and are persisted only when they change it.
    fn remember_service_usage(&mut self, services: &[broker::ServiceSnapshot]) {
        for service in services {
            if let Some(report) = &service.usage
                && let Some(owner) = self
                    .sessions
                    .iter()
                    .find(|session| {
                        session.agent == service.provider
                            && session.external_id.as_deref() == Some(service.parent_id.as_str())
                            && session
                                .execution_path
                                .as_ref()
                                .unwrap_or(&session.project_path)
                                == &service.project
                    })
                    .map(|session| session.id.clone())
            {
                let mut merged = self.usage.get(&owner).cloned().unwrap_or_default();
                merged.merge(report.clone());
                if self.usage.get(&owner) != Some(&merged) {
                    self.engine.save_usage(owner.clone(), merged.clone());
                    self.remember_usage(owner, merged);
                }
            }
        }
    }
    // Providers report the immediate parent for nested delegation. Walk only
    // observed ownership links, bounded even if a malformed graph has a cycle.
    pub(super) fn child_owner<'a>(
        &'a self,
        child: &ChildSnapshot,
        children: &[ChildSnapshot],
    ) -> Option<&'a SessionSummary> {
        let mut parent = child.parent_id.as_str();
        for _ in 0..=children.len() {
            if let Some(owner) = self.sessions.iter().filter(|s| {
                s.agent == child.provider
                    && s.external_id.as_deref() == Some(parent)
            }).max_by_key(|s|(self.runs.contains_key(&s.id),self.active.as_ref()==Some(&s.id))) {
                return Some(owner);
            }
            parent = children
                .iter()
                .find(|ancestor| {
                    ancestor.provider == child.provider
                        && ancestor.id == parent
                })?
                .parent_id
                .as_str();
        }
        None
    }
    fn open_child_decisions(
        &mut self,
        provider: AgentKind,
        child_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((owner, request)) = self.broker_panel.as_ref().and_then(|panel| {
            panel
                .children
                .iter()
                .find(|child| child.provider == provider && child.id == child_id)
                .and_then(|child| {
                    let request = child.requests.first()?;
                    let owner = self.child_owner(child, &panel.children)?;
                    Some((owner.clone(), request.id.clone()))
                })
        }) else {
            return;
        };
        if self.active.as_ref() != Some(&owner.id) {
            self.open_session(owner, window, cx);
        }
        self.native_request_focus = Some(request);
        cx.notify();
    }
    pub(super) fn select_child(&mut self, provider: AgentKind, id: String, cx: &mut Context<Self>) {
        self.reopen_closing_pane(pane_layout::Pane::Children,cx);
        let mut offset = 0;
        if let Some(panel) = &mut self.broker_panel {
            offset = panel
                .children
                .iter()
                .find(|c| c.id == id && c.provider == provider)
                .map_or(0, |c| c.transcript_bytes.saturating_sub(64 * 1024));
            panel.follow = true;
            panel.selected = Some((provider, id));
            panel.rows.clear();
            panel.row_width = 24.;
            panel.revision = 0;
            panel
                .scroll
                .0
                .borrow()
                .base_handle
                .set_offset(point(px(0.), px(0.)));
            panel.input.update(cx, |input, cx| input.set_text("", cx));
        }
        self.load_child_page(offset, cx);
    }
    fn load_child_page(&mut self, offset: usize, cx: &mut Context<Self>) {
        let Some(panel) = &mut self.broker_panel else {
            return;
        };
        let Some((provider, id)) = panel.selected.clone() else {
            return;
        };
        let Some(project)=panel.children.iter().find(|c|c.provider==provider&&c.id==id).map(|c|c.project.clone())else{return};
        let request = uuid::Uuid::new_v4();
        panel.request = request;
        panel.loading = true;
        panel.error = None;
        let job = cx.background_executor().spawn(async move {
            broker::child_transcript_page(&project, provider, &id, offset)
                .map(|page| {
                    let mut rows = Vec::new();
                    for block in presentation::blocks(&page.text) {
                        let (text, code) = match block {
                            MessageBlock::Code { text, .. } => (text, true),
                            MessageBlock::Paragraph(t)
                            | MessageBlock::RichParagraph { text: t, .. }
                            | MessageBlock::Heading(t)
                            | MessageBlock::Quote(t)
                            | MessageBlock::ListItem { text: t, .. } => (t, false),
                            MessageBlock::Activity { label, details } => {
                                (format!("{label}\n{details}"), true)
                            }
                            MessageBlock::TableRow { cells, .. } => (cells.join("  ·  "), false),
                            MessageBlock::Attachment { name, .. } => (name, false),
                            MessageBlock::Voice { text, .. } => (text, false),
                            MessageBlock::Interaction(card) => (
                                match card {
                                    chat_interactions::ChatInteraction::Status {
                                        title,
                                        details,
                                        ..
                                    }
                                    | chat_interactions::ChatInteraction::Delegation {
                                        title,
                                        details,
                                        ..
                                    } => format!("{title}\n{details}"),
                                    _ => "Agent interaction · use the native decision controls"
                                        .into(),
                                },
                                false,
                            ),
                        };
                        for line in text.split('\n') {
                            let mut tail = line;
                            while tail.len() > 2048 {
                                let mut end = 2048;
                                while !tail.is_char_boundary(end) {
                                    end -= 1;
                                }
                                rows.push((tail[..end].to_owned(), code));
                                tail = &tail[end..];
                            }
                            rows.push((tail.to_owned(), code));
                        }
                    }
                    let width = transcript_width(&rows);
                    (
                        rows,
                        width,
                        page.offset,
                        page.next_offset,
                        page.total_bytes,
                        page.revision,
                    )
                })
                .map_err(|e| e.to_string())
        });
        cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                let mut reload = None;
                if let Some(panel) = this.broker_panel.as_mut().filter(|p| p.request == request) {
                    panel.loading = false;
                    match result {
                        Ok((rows, width, start, next, total, revision)) => {
                            panel.rows = rows;
                            panel.row_width = width;
                            panel.offset = start;
                            panel.next = next;
                            panel.total = total;
                            panel.revision = revision;
                            if panel.follow
                                && let Some(child) = panel.children.iter().find(|child| {
                                    Some(&(child.provider, child.id.clone()))
                                        == panel.selected.as_ref()
                                })
                                && child.transcript_revision > revision
                            {
                                reload = Some(child.transcript_bytes.saturating_sub(64 * 1024));
                            }
                            if panel.follow {
                                panel.scroll.scroll_to_item(
                                    panel.rows.len().saturating_sub(1),
                                    ScrollStrategy::Bottom,
                                );
                            } else {
                                panel.scroll.scroll_to_item(0, ScrollStrategy::Top);
                            }
                        }
                        Err(error) => panel.error = Some(error),
                    }
                    cx.notify();
                }
                if let Some(offset) = reload {
                    this.load_child_page(offset, cx);
                }
            });
        })
        .detach();
        cx.notify();
    }
    fn control_selected_child(&mut self, action: ChildAction, cx: &mut Context<Self>) -> bool {
        let Some(panel) = &mut self.broker_panel else {
            return false;
        };
        let Some((provider, id)) = &panel.selected else {
            return false;
        };
        let Some(project)=panel.children.iter().find(|c|c.provider==*provider&&c.id==*id).map(|c|c.project.clone())else{return false};
        match broker::control_child(&project, *provider, id, action) {
            Ok(()) => true,
            Err(e) => {
                panel.error = Some(e.to_string());
                cx.notify();
                false
            }
        }
    }
    fn steer_child(&mut self, cx: &mut Context<Self>) {
        let Some(panel) = &self.broker_panel else {
            return;
        };
        let text = panel.input.read(cx).text();
        if self.control_selected_child(ChildAction::Steer(text), cx)
            && let Some(panel) = &self.broker_panel
        {
            panel.input.update(cx, |input, cx| input.set_text("", cx));
        }
    }
    pub(super) fn children_panel(&self, cx: &Context<Self>) -> AnyElement {
        let Some(panel) = &self.broker_panel else {
            return div().into_any_element();
        };
        div()
            .w_full()
            .h_full()
            .min_h(px(0.))
            .flex_none()
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(rgba(0xebebeb12))
            .bg(rgba(appearance::MENU_SURFACE))
            .opacity(if self.motion {
                presentation::spring((panel.started.elapsed().as_secs_f32() / 0.32).min(1.))
                    .clamp(0., 1.)
            } else {
                1.
            })
            .child(
                div()
                    .h(px(40.))
                    .flex_none()
                    .px_3()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .id("child-panel-title")
                            .debug_selector(|| "child-panel-title".into())
                            .flex_1()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(format!(
                                "Subagents · {}",
                                panel
                                    .project
                                    .file_name()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                            )),
                    )
                    .child(
                        div()
                            .id("close-children")
                            .debug_selector(|| "close-children".into())
                            .flex_none()
                            .p_1()
                            .cursor_pointer()
                            .child(icon("close", 14., MUTED))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.close_pane(pane_layout::Pane::Children,cx);
                            })),
                    ),
            )
            .child(
                div()
                    .px_3()
                    .py_1()
                    .text_size(crate::theme::ui_px(10.))
                    .text_color(rgb(MUTED))
                    .child(
                        div()
                            .id("child-service-diagnostics")
                            .cursor_pointer()
                            .child("Runtime details")
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(panel) = &mut this.broker_panel {
                                    panel.diagnostics = !panel.diagnostics;
                                }
                                cx.notify();
                            })),
                    ),
            )
            .when(panel.diagnostics, |s| {
                s.children(panel.services.iter().map(|service| {
                    div()
                        .px_3()
                        .py_1()
                        .text_size(crate::theme::ui_px(10.))
                        .text_color(rgb(MUTED))
                        .child(format!(
                            "{} · {} · PID {}",
                            agent_name(service.provider),
                            if !service.connected {
                                "Disconnected"
                            } else if service.active {
                                "Working"
                            } else {
                                "Idle"
                            },
                            service.process_id
                        ))
                }))
            })
            .child(div().id("child-tabs").debug_selector(||"child-tabs".into()).flex_none().h(px(74.)).overflow_x_scroll().flex().gap_2().px_3().py_2().children(panel.children.iter().enumerate().map(|(ix,child)|{
                let provider=child.provider;let id=child.id.clone();let selected=panel.selected.as_ref()==Some(&(provider,id.clone()));let key=crate::unified::child_id(child);
                let colors=self.task_pet_visuals.get(&key).map(|v|v.colors.as_slice()).unwrap_or(&[0x94b8db,0x6c819c]);
                let tint=colors[0];let (_,_,status)=crate::interaction_ui::status_appearance(child.status);
                div().id(("child",ix)).debug_selector(move||format!("child-tab-{ix}")).w(px(168.)).flex_none().rounded_lg().border_1().border_color(rgba((tint<<8)|if selected{150}else{32})).bg(linear_gradient(135.,linear_color_stop(rgba((tint<<8)|if selected{42}else{12}),0.),linear_color_stop(rgba(0xffffff04),1.))).px_2().py_1().flex().items_center().gap_2().cursor_pointer().hover(|s|s.bg(rgba(GLASS_HOVER)))
                .child(self.task_pet(&key,30.,selected&&child.connected&&matches!(child.status,chat_interactions::InteractionStatus::Started|chat_interactions::InteractionStatus::Working)).unwrap_or_else(||provider_icon(agent_icon(provider),18.,agent_color(provider)).into_any_element()))
                .child(div().flex_1().min_w(px(0.)).flex().flex_col().gap_1().child(div().text_ellipsis().text_size(theme::ui_px(12.)).font_weight(FontWeight::MEDIUM).child(child.name.clone().unwrap_or_else(||format!("{} {}",provider.name(),short(&id,8))))).child(div().text_size(theme::ui_px(10.)).text_color(rgb(MUTED)).child(status)))
                .on_click(cx.listener(move|app,_,window,cx|{app.select_child(provider,id.clone(),cx);app.open_child_decisions(provider,&id,window,cx);})).into_any_element()
            })))
            .when_some(panel.selected.as_ref().and_then(|key|panel.children.iter().find(|c|(c.provider,&c.id)==(key.0,&key.1))),|s,child|{
                s.child(div().px_3().py_2().flex_none().flex().flex_col().gap_1().text_size(theme::ui_px(11.))
                .when_some(child.task.clone(),|s,task|s.child(div().max_h(px(64.)).overflow_hidden().child(task)))
                .when_some(child.scope.as_ref(),|s,path|s.child(div().text_color(rgb(MUTED)).child(format!("Delegated scope · {path}"))))
                .child(div().text_color(rgb(MUTED)).child(format!("{}{} · delegated by {}",child.model.as_deref().unwrap_or_else(||child.provider.name()),child.effort.as_ref().map(|e|format!(" · {e}")).unwrap_or_default(),short(&child.parent_id,18))))
                .when_some(child.workspace.as_ref(),|s,path|s.child(div().text_color(rgb(MUTED)).text_ellipsis().child(path.display().to_string()))))
            })
            .when(panel.children.is_empty(), |s| {
                s.child(
                    div()
                        .p_3()
                        .text_size(crate::theme::ui_px(12.))
                        .text_color(rgb(MUTED))
                        .child(
                            "Native subagents appear here when an owned provider launches them.",
                        ),
                )
            })
            .when_some(panel.error.as_ref(), |s, error| {
                s.child(
                    div()
                        .p_3()
                        .text_size(crate::theme::ui_px(12.))
                        .child(error.clone()),
                )
            })
            .child(
                uniform_list(
                    "child-transcript",
                    panel.rows.len(),
                    cx.processor(|this, range: std::ops::Range<usize>, _, _| {
                        let Some(panel) = &this.broker_panel else {
                            return vec![];
                        };
                        range
                            .map(|ix| {
                                let (text, code) = &panel.rows[ix];
                                div()
                                    .id(("child-transcript-row", ix))
                                    .debug_selector(move || format!("child-transcript-row-{ix}"))
                                    .h(px(22.))
                                    .w(px(panel.row_width))
                                    .px_3()
                                    .whitespace_nowrap()
                                    .text_size(crate::theme::ui_px(12.))
                                    .when(*code, |s| {
                                        s.font_family(crate::theme::mono_font())
                                            .bg(rgba(GLASS_SURFACE))
                                    })
                                    .child(text.clone())
                            })
                            .collect()
                    }),
                )
                .with_horizontal_sizing_behavior(gpui::ListHorizontalSizingBehavior::Unconstrained)
                .track_scroll(panel.scroll.clone())
                .on_scroll_wheel(cx.listener(|this, _, _, _| {
                    if let Some(panel) = &mut this.broker_panel {
                        panel.follow = false;
                    }
                }))
                .flex_1()
                .w_full()
                .min_w(px(0.))
                .min_h(px(0.)),
            )
            .child(
                div()
                    .p_2()
                    .flex()
                    .gap_3()
                    .text_size(crate::theme::ui_px(11.))
                    .when(panel.offset > 0, |s| {
                        s.child(
                            div()
                                .id("child-previous")
                                .cursor_pointer()
                                .child("Earlier")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(panel) = &mut this.broker_panel {
                                        panel.follow = false;
                                    }
                                    let offset = this
                                        .broker_panel
                                        .as_ref()
                                        .map_or(0, |p| p.offset.saturating_sub(64 * 1024));
                                    this.load_child_page(offset, cx);
                                })),
                        )
                    })
                    .when(panel.next < panel.total, |s| {
                        s.child(
                            div()
                                .id("child-next")
                                .cursor_pointer()
                                .child("Next")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(panel) = &mut this.broker_panel {
                                        panel.follow = false;
                                    }
                                    let offset = this.broker_panel.as_ref().map_or(0, |p| p.next);
                                    this.load_child_page(offset, cx);
                                })),
                        )
                    })
                    .child(
                        div()
                            .id("child-latest")
                            .cursor_pointer()
                            .child(if panel.loading {
                                "Loading…"
                            } else {
                                "Latest"
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(panel) = &mut this.broker_panel {
                                    panel.follow = true;
                                }
                                let offset = this
                                    .broker_panel
                                    .as_ref()
                                    .and_then(|p| {
                                        p.children.iter().find(|c| {
                                            Some(&(c.provider, c.id.clone())) == p.selected.as_ref()
                                        })
                                    })
                                    .map_or(0, |c| c.transcript_bytes.saturating_sub(64 * 1024));
                                this.load_child_page(offset, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .px_3()
                    .py_2()
                    .border_t_1()
                    .border_color(rgba(0xebebeb12))
                    .when(
                        panel
                            .selected
                            .as_ref()
                            .is_some_and(|(provider, _)| *provider == AgentKind::Codex),
                        |s| s.child(panel.input.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .text_size(crate::theme::ui_px(11.))
                            .when(
                                panel
                                    .selected
                                    .as_ref()
                                    .is_some_and(|(provider, _)| *provider == AgentKind::Codex),
                                |s| {
                                    s.child(
                                        div()
                                            .id("steer-child")
                                            .cursor_pointer()
                                            .child("Send instruction")
                                            .on_click(
                                                cx.listener(|this, _, _, cx| this.steer_child(cx)),
                                            ),
                                    )
                                },
                            )
                            .child(
                                div()
                                    .id("stop-child")
                                    .cursor_pointer()
                                    .child("Stop")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.control_selected_child(ChildAction::Stop, cx);
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }
}

impl Orbit {
    // Quitting the app must reap every service, including roots no longer in
    // the sidebar. Window release remains scoped to this window's own roots.
    pub(super) fn shutdown_app_brokers(&self) -> async_channel::Receiver<()> {
        #[cfg(not(test))]
        {
            broker::shutdown()
        }
        #[cfg(test)]
        {
            self.shutdown_brokers()
        }
    }
    pub(super) fn shutdown_brokers(&self) -> async_channel::Receiver<()> {
        let roots: HashSet<_> = self
            .sessions
            .iter()
            .map(|s| {
                s.execution_path
                    .clone()
                    .unwrap_or_else(|| s.project_path.clone())
            })
            .chain(std::iter::once(self.project.clone()))
            .collect();
        let receivers: Vec<_> = broker::services()
            .into_iter()
            .filter(|s| roots.contains(&s.project) && s.connected)
            .map(|s| broker::shutdown_service(&s.project, s.provider, &s.parent_id))
            .collect();
        let (sender, receiver) = async_channel::bounded(1);
        if !receivers.is_empty() {
            std::thread::spawn(move || {
                for receiver in receivers {
                    let _ = receiver.recv_blocking();
                }
                drop(sender);
            });
        }
        receiver
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    #[gpui::test]
    fn idle_service_quotas_update_the_owning_chat_without_erasing_turn_usage(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update(cx, |app, _| {
            app.sessions[0].external_id = Some("quota-parent".into());
            app.usage.insert("fixture-session".into(), usage::Usage { input: Some(700), ..Default::default() });
            let limits = usage::Usage::codex_limits(&serde_json::json!({"rateLimits":{"primary":{"usedPercent":9,"resetsAt":1800000000},"secondary":{"usedPercent":42,"resetsAt":1800600000}}}), false).unwrap();
            let service = broker::ServiceSnapshot { provider: AgentKind::Codex, project: app.project.clone(),
                parent_id: "quota-parent".into(), process_id: 0, active: false, connected: true, usage: Some(limits) };
            app.remember_service_usage(std::slice::from_ref(&service));
            let actual = &app.usage["fixture-session"];
            assert_eq!(actual.input, Some(700));
            assert_eq!(actual.limit_windows.len(), 2);
            assert_eq!(actual.limit_windows[0].used_percent, 9.);
            let saved = actual.clone();
            app.remember_service_usage(std::slice::from_ref(&service));
            assert_eq!(app.usage["fixture-session"], saved);
            let mut foreign = service;
            foreign.project = app.project.join("other-worktree");
            foreign.usage.as_mut().unwrap().limit_windows[0].used_percent = 99.;
            app.remember_service_usage(&[foreign]);
            assert_eq!(app.usage["fixture-session"], saved);
        });
    }

    #[gpui::test]
    fn nested_child_decisions_resolve_the_real_chat_without_crossing_workspace_or_cycles(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update_in(cx, |app, window, cx| {
            let root = app.project.join("managed-worktree");
            app.sessions[0].external_id = Some("native-root".into());
            app.sessions[0].execution_path = Some(root.clone());
            let child = ChildSnapshot {
                name:None,scope:None,task:None,model:None,effort:None,workspace:None,
                provider: AgentKind::Codex,
                project: root,
                parent_id: "native-root".into(),
                id: "child".into(),
                turn_id: Some("turn".into()),
                status: chat_interactions::InteractionStatus::Working,
                preview: String::new(),
                requests: vec![],
                transcript_bytes: 0,
                transcript_revision: 0,
                connected: true,
            };
            let mut nested = child.clone();
            nested.id = "grandchild".into();
            nested.parent_id = "child".into();
            nested.requests.push(runtime::NativeRequest {
                id: "opaque-grandchild-request".into(),
                method: "item/commandExecution/requestApproval".into(),
                title: "Approve".into(),
                detail: "printf nested".into(),
                kind: runtime::NativeRequestKind::Approval { choices: vec![] },
            });
            let children = vec![child.clone(), nested.clone()];
            assert_eq!(
                app.child_owner(&nested, &children).unwrap().id,
                "fixture-session"
            );
            let mut foreign = nested.clone();
            foreign.project = app.project.clone();
            assert_eq!(app.child_owner(&foreign,&children).unwrap().id,"fixture-session","The native parent remains the owner across child worktrees");
            foreign.parent_id="unrelated-native-thread".into();
            assert!(app.child_owner(&foreign, &children).is_none());
            let mut cyclic = child.clone();
            cyclic.parent_id = nested.id.clone();
            assert!(
                app.child_owner(&nested, &[cyclic, nested.clone()])
                    .is_none()
            );
            app.toggle_children(window, cx);
            app.broker_panel.as_mut().unwrap().children = children;
            app.active = None;
            app.open_child_decisions(AgentKind::Codex, "grandchild", window, cx);
            assert_eq!(app.active.as_deref(), Some("fixture-session"));
            assert_eq!(
                app.native_request_focus.as_deref(),
                Some("opaque-grandchild-request")
            );
        });
    }

    #[test]
    fn transcript_width_accounts_for_every_row_and_tabs() {
        assert_eq!(transcript_width(&[]), 24.);
        assert_eq!(transcript_width(&[("a\tb".into(), true)]), 150.);
        assert_eq!(
            transcript_width(&[("short".into(), false), ("界".repeat(200), true)]),
            2824.
        );
    }

    #[gpui::test]
    fn pruned_selected_child_invalidates_pending_transcript_and_steering_draft(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update_in(cx, |app, window, cx| {
            app.toggle_children(window, cx);
            let panel = app.broker_panel.as_mut().unwrap();
            panel.selected = Some((AgentKind::Codex, "old-child".into()));
            panel.rows = vec![("Previous child's output".into(), false)];
            panel.loading = true;
            panel.total = 500;
            panel.offset = 100;
            panel.next = 200;
            panel.error = Some("Old error".into());
            panel
                .input
                .update(cx, |input, cx| input.set_text("Steer old child", cx));
            panel.children = vec![ChildSnapshot {
                name:None,scope:None,task:None,model:None,effort:None,workspace:None,
                provider: AgentKind::Gemini,
                project: panel.project.clone(),
                parent_id: "parent".into(),
                id: "new-child".into(),
                turn_id: None,
                status: chat_interactions::InteractionStatus::Working,
                preview: String::new(),
                requests: vec![],
                transcript_bytes: 123,
                transcript_revision: 0,
                connected: true,
            }];
            let old_request = panel.request;
            panel.reconcile_selection(cx);
            assert_ne!(panel.request, old_request);
            assert_eq!(
                panel.selected,
                Some((AgentKind::Gemini, "new-child".into()))
            );
            assert!(panel.rows.is_empty());
            assert_eq!((panel.offset, panel.next, panel.total), (0, 0, 0));
            assert!(!panel.loading);
            assert!(panel.error.is_none());
            assert!(panel.input.read(cx).text().is_empty());
            let valid_request = panel.request;
            panel.reconcile_selection(cx);
            assert_eq!(
                panel.request, valid_request,
                "same live owner preserves its page request"
            );
            panel.children.clear();
            panel.reconcile_selection(cx);
            assert!(panel.selected.is_none());
            assert_ne!(panel.request, valid_request);
        });
    }

    #[gpui::test]
    fn child_transcript_long_lines_scroll_without_expanding_or_wrapping(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update_in(cx, |app, window, cx| {
            app.toggle_children(window, cx);
            let panel = app.broker_panel.as_mut().unwrap();
            panel.project = panel
                .project
                .with_file_name("long-project-name-".repeat(12));
            panel.rows = (0..500)
                .map(|ix| {
                    (
                        if ix == 0 {
                            format!("{} END_OF_OUTPUT", "long-output-".repeat(80))
                        } else {
                            format!("line {ix}")
                        },
                        ix % 2 == 0,
                    )
                })
                .collect();
            panel.row_width = transcript_width(&panel.rows);
            cx.notify();
        });
        crate::ui_tests::draw_frame(&view, cx);
        let title = cx.debug_bounds("child-panel-title").unwrap();
        let close = cx.debug_bounds("close-children").unwrap();
        assert!(title.right() <= close.left());
        let before = cx.debug_bounds("child-transcript-row-0").unwrap();
        let second = cx.debug_bounds("child-transcript-row-1").unwrap();
        assert_eq!(before.size.height, px(22.));
        assert_eq!(second.top() - before.top(), px(22.));
        assert!(
            cx.debug_bounds("child-transcript-row-499").is_none(),
            "offscreen rows must stay virtualized"
        );
        let (viewport, max) = cx.update(|_, cx| {
            let panel = view.read(cx).broker_panel.as_ref().unwrap();
            let state = panel.scroll.0.borrow();
            (state.base_handle.bounds(), state.base_handle.max_offset())
        });
        assert!(viewport.size.width < px(600.));
        assert!(viewport.size.height < px(790.));
        assert!(max.width > px(1000.));
        view.update(cx, |app, cx| {
            let panel = app.broker_panel.as_ref().unwrap();
            panel
                .scroll
                .0
                .borrow()
                .base_handle
                .set_offset(point(-max.width, px(0.)));
            cx.notify();
        });
        crate::ui_tests::draw_frame(&view, cx);
        let after = cx.debug_bounds("child-transcript-row-0").unwrap();
        assert!(after.left() < before.left() - px(1000.));
        assert!(
            after.right() <= viewport.right() + px(1.),
            "the end of a long row must be reachable"
        );
        assert_eq!(after.size.height, px(22.));
        assert!(cx.debug_bounds("child-transcript-row-499").is_none());
    }

    #[gpui::test]
    fn child_inspector_excludes_other_inspectors_and_closes_cleanly(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update(cx, |app, cx| app.open_changes(cx));
        view.update_in(cx, |app, window, cx| {
            assert!(app.diff_view.is_some());
            app.toggle_children(window, cx);
            assert!(app.broker_panel.is_some());
            assert!(app.diff_view.is_none());
            assert!(!app.files_visible);
        });
        crate::ui_tests::draw_frame(&view, cx);
        view.update_in(cx, |app, window, cx| {
            app.toggle_files(window, cx);
            assert!(app.broker_panel.is_none());
            assert!(app.files_visible);
            app.toggle_children(window, cx);
            assert!(app.broker_panel.is_some());
            assert!(!app.files_visible);
            app.toggle_children(window, cx);
            assert!(app.broker_panel.is_none());
        });
    }
}
