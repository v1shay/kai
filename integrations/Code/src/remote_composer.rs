//! Composer state is scoped to the selected host and never changes local defaults.
use super::*;
use crate::voice_bridge::{Activity as VoiceActivity, Media, MediaEvent};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum ComposerPanel {
    Projects,
    Model,
    Thinking,
    Permissions,
    #[default]
    More,
}

#[derive(Default, Deserialize)]
struct Catalog {
    default_model: Option<String>,
    default_effort: Option<String>,
    #[serde(default)]
    models: Vec<Model>,
    #[serde(default)]
    fast_models: HashMap<String, String>,
}

#[derive(Deserialize)]
struct Model {
    id: String,
    name: String,
    #[serde(default)]
    efforts: Vec<String>,
}

#[derive(Default, Deserialize)]
struct Options {
    #[serde(default)]
    providers: Vec<String>,
    #[serde(default)]
    edits: bool,
    #[serde(default)]
    full_access: bool,
    #[serde(default)]
    codex_access: runtime::CodexAccess,
    #[serde(default)]
    realtime: bool,
    #[serde(default)]
    orchestrator: bool,
    desktop_unavailable: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    voice: Option<String>,
    #[serde(default)]
    fast: bool,
}

pub(super) struct Controls {
    panel: ComposerPanel,
    options: Option<Options>,
    catalog: Option<Catalog>,
    loading: bool,
    model: Option<String>,
    effort: Option<String>,
    pub(super) edits: Option<bool>,
    pub(super) access: Option<runtime::CodexAccess>,
    pub(super) draft_agent: AgentKind,
    fast: Option<bool>,
    pub(super) orchestrator: bool,
    pub(super) voice: Option<ClientCall>,
    voice_error: Option<String>,
    pub(super) voice_choice: Option<String>,
    pub(super) attachments: Vec<(String, String)>,
    pub(super) attachment_pending: bool,
}

impl Default for Controls {
    fn default() -> Self {
        Self {
            panel: ComposerPanel::More,
            options: None,
            catalog: None,
            loading: false,
            model: None,
            effort: None,
            edits: None,
            access: None,
            draft_agent: AgentKind::Codex,
            fast: None,
            orchestrator: false,
            voice: None,
            voice_error: None,
            voice_choice: None,
            attachments: Vec::new(),
            attachment_pending: false,
        }
    }
}

impl Controls {
    pub(super) fn reset_selection(&mut self) {
        *self = Self::default();
    }
    fn model(&self) -> Option<&str> {
        self.model
            .as_deref()
            .or_else(|| self.options.as_ref()?.model.as_deref())
            .or_else(|| self.catalog.as_ref()?.default_model.as_deref())
    }
    fn effort(&self) -> Option<&str> {
        self.effort.as_deref()
            .or_else(|| self.options.as_ref()?.effort.as_deref())
            .or_else(|| self.catalog.as_ref()?.default_effort.as_deref())
    }

    fn choice(&self) -> Option<&Model> {
        let model = self.model()?;
        self.catalog
            .as_ref()?
            .models
            .iter()
            .find(|item| item.id == model)
    }
    pub(super) fn apply_turn(&self, command: &mut Value) {
        // Send only user overrides. Host defaults stay authoritative.
        if let Some(model) = &self.model {
            command["model"] = json!(model);
        }
        if let Some(effort) = &self.effort {
            command["effort"] = json!(effort);
        }
        if let Some(edits) = self.edits {
            command["allow_edits"] = json!(edits);
        }
        if let Some(access) = self.access {
            command["codex_access"] = json!(access);
        }
        if let Some(fast) = self.fast {
            command["fast"] = json!(fast);
        }
        command["orchestrator"] = json!(self.orchestrator);
    }
}

pub(super) struct ClientCall {
    id: String,
    media: Media,
    started: bool,
    connected: bool,
    active: bool,
    answered: bool,
    muted: bool,
    activity: VoiceActivity,
    offer_sent: bool,
    preview: String,
    initial_prompt: Option<String>,
}

impl ClientCall {
    fn activate(&mut self) -> anyhow::Result<()> {
        if self.started && self.connected && !self.active {
            self.media.activate()?;
            if let Some(audio) = self.initial_prompt.take() {
                self.media.initial_prompt(&audio)?;
            }
            self.active = true;
        }
        Ok(())
    }
    fn caption(&self) -> &'static str {
        if !self.active {
            return "Connecting voice…";
        }
        if self.muted {
            return "Microphone muted";
        }
        match self.activity {
            VoiceActivity::Listening => "Listening",
            VoiceActivity::Thinking => "Thinking",
            VoiceActivity::Speaking => "Speaking",
        }
    }
}

type Reply = async_channel::Receiver<Result<Value, String>>;

pub(super) fn payload(response: Value) -> Result<Value, String> {
    let result = response
        .get("result")
        .filter(|value| value.is_object())
        .unwrap_or(&response);
    if let Some(error) = result["error"]
        .as_str()
        .or_else(|| response["error"].as_str())
    {
        return Err(error.into());
    }
    if matches!(
        response["state"].as_str(),
        Some("pending" | "uncertain" | "rejected")
    ) {
        return Err("Kai could not confirm the host’s response".into());
    }
    Ok(result.clone())
}

fn command(state: &State, args: Value, voice: bool) -> Option<Reply> {
    let peer = state
        .selected
        .as_ref()
        .filter(|peer| peer.allowed && peer.online && peer.control)?;
    let helper = state.helper.clone()?;
    let worker = if voice {
        state.voice_worker.as_ref()?
    } else {
        state.composer_worker.as_ref()?
    };
    let mut req = request("command", peer);
    req["command"] = args;
    Some(worker.submit(false, move || call::<Value>(&helper, req).and_then(payload)))
}

async fn receive(reply: Reply) -> Result<Value, String> {
    reply
        .recv()
        .await
        .unwrap_or_else(|_| Err("Another host control is pending".into()))
}

fn row(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    selected: bool,
) -> Stateful<Div> {
    let id = id.into();
    let selector = id.clone();
    project_tree::navigation_row(id, 10.)
        .debug_selector(move || selector.to_string())
        .text_size(theme::ui_px(12.))
        .when(selected, |row| row.bg(rgba(GLASS_HOVER)))
        .child(label.into())
}

impl Orbit {
    pub(in crate) fn remote_split_providers(&self) -> Vec<AgentKind> {
        self.remote_workspace.controls.options.as_ref().map(|options| {
            options.providers.iter().map(|name| agent(name)).collect()
        }).unwrap_or_else(|| vec![self.remote_split_provider()])
    }

    pub(super) fn refresh_remote_composer(&mut self, cx: &mut Context<Self>) {
        let state = &self.remote_workspace;
        if !state
            .selected
            .as_ref()
            .is_some_and(|peer| peer.supports("workspace.composer"))
        {
            return;
        }
        let mut args = json!({"action":"composer_options"});
        if let Some(chat) = &state.chat {
            args["chat_id"] = json!(chat.id);
        }
        let Some(job) = command(state, args, false) else {
            return;
        };
        let epoch = state.epoch;
        cx.spawn(async move |this, cx| {
            if let Ok(value) = receive(job).await {
                let _ = this.update(cx, |app, cx| app.with_remote_epoch(epoch, |app| {
                    let state = &mut app.remote_workspace;
                    state.controls.options = serde_json::from_value(value).ok();
                    if let Some(options) = &state.controls.options {
                        state.controls.orchestrator = options.orchestrator;
                    }
                    if state.chat.is_none()
                        && let Some(options) = &state.controls.options
                        && let Some(provider) = options.providers.first()
                        && !options
                            .providers
                            .iter()
                            .any(|key| key == state.controls.draft_agent.command())
                    {
                        state.controls.draft_agent = agent(provider);
                    }
                    app.load_remote_models(cx);
                    cx.notify();
                }));
            }
        })
        .detach();
    }

    fn load_remote_models(&mut self, cx: &mut Context<Self>) {
        let state = &mut self.remote_workspace;
        let project = state
            .chat
            .as_ref()
            .map(|chat| chat.project.as_str())
            .filter(|path| !path.is_empty())
            .or(state.project.as_deref());
        let Some(project) = project else {
            return;
        };
        if state.controls.loading {
            return;
        }
        let provider = state
            .chat
            .as_ref()
            .map(|chat| agent(&chat.agent))
            .unwrap_or(state.controls.draft_agent);
        let mut request = json!({"action":"list_models","agent":provider.command(),"project":project});
        if provider == AgentKind::Ninja && let Some(chat) = &state.chat {
            request["chat_id"] = json!(chat.id);
        }
        let Some(job) = command(
            state,
            request,
            false,
        ) else {
            return;
        };
        let epoch = state.epoch;
        state.controls.loading = true;
        cx.spawn(async move |this, cx| {
            let value = receive(job).await;
            let _ = this.update(cx, |app, cx| app.with_remote_epoch(epoch, |app| {
                let state = &mut app.remote_workspace;
                state.controls.loading = false;
                let current = state
                    .chat
                    .as_ref()
                    .map(|chat| agent(&chat.agent))
                    .unwrap_or(state.controls.draft_agent);
                if current != provider {
                    app.load_remote_models(cx);
                    return;
                }
                state.controls.catalog = value
                    .ok()
                    .and_then(|data| serde_json::from_value(data).ok());
                cx.notify();
            }));
        })
        .detach();
    }

    pub(super) fn remote_native_composer(&self, value: &mut Value) {
        let state = &self.remote_workspace;
        let controls = &state.controls;
        let current = self.remote_slash_agent().unwrap_or(AgentKind::Codex);
        let model = controls.model();
        let effort = controls.effort();
        value["selectedAgent"] = json!(current.command());
        value["selectedModel"] = json!(model);
        value["effectiveModel"] = json!(model);
        value["providerDefaultModel"] = json!(controls.catalog.as_ref().and_then(|catalog| catalog.default_model.as_deref()));
        value["reasoningEffort"] = json!(effort);
        value["effectiveReasoningEffort"] = json!(effort);
        value["modelsLoading"] = json!(controls.loading);
        value["models"] = json!(controls.catalog.iter().flat_map(|catalog| &catalog.models).map(|item|
            json!({"model":item.id,"displayName":item.name,"supportedReasoningEfforts":
                item.efforts.iter().map(|effort| json!({"reasoningEffort":effort})).collect::<Vec<_>>()})
        ).collect::<Vec<_>>());
        let options = controls.options.as_ref();
        let providers = if state.chat.is_some() { vec![current.command()] } else {
            options.into_iter().flat_map(|options| options.providers.iter().map(String::as_str)).collect()
        };
        value["agents"] = json!(providers.into_iter().map(|id| {
            json!({"id":id,"name":agent(id).name(),"available":true})
        }).collect::<Vec<_>>());
        value["workspaceVoiceChoice"] = json!(self.remote_voice_choice());
        value["workspaceVoiceError"] = json!(controls.voice_error);
        value["workspaceVoiceActive"] = json!(controls.voice.is_some());
        value["workspaceVoiceScene"] = json!(controls.voice.as_ref().map(|call| {
            if !call.active || call.muted { return "thinking"; }
            match call.activity {
                VoiceActivity::Listening => "listening",
                VoiceActivity::Thinking => "thinking",
                VoiceActivity::Speaking => "speaking",
            }
        }));
        value["fastMode"] = json!(self.remote_fast());
        value["fastSupported"] = json!(self.remote_can_fast());
        let edits = controls.edits.or_else(|| options.map(|options| options.edits)).unwrap_or(false);
        let access = controls.access.or_else(|| options.map(|options| options.codex_access)).unwrap_or_default();
        value["workspaceAccess"] = json!(if !edits { "read-only" } else if access == runtime::CodexAccess::Full { "full" } else { "workspace" });
    }

    pub(crate) fn remote_voice_levels(&self) -> Option<&[std::sync::atomic::AtomicU32; 4]> {
        self.remote_workspace.controls.voice.as_ref().map(|call| call.media.levels.as_ref())
    }

    pub(super) fn set_remote_native_model(&mut self, request: &Value, cx: &mut Context<Self>) -> Result<(), String> {
        let controls = &mut self.remote_workspace.controls;
        let model = request["model"].as_str().filter(|model| !model.is_empty());
        let effort = request["effort"].as_str().filter(|effort| !effort.is_empty());
        let effective = model.or_else(|| controls.options.as_ref()?.model.as_deref())
            .or_else(|| controls.catalog.as_ref()?.default_model.as_deref());
        let choice = controls.catalog.as_ref().and_then(|catalog|
            catalog.models.iter().find(|entry| Some(entry.id.as_str()) == effective));
        if model.is_some() && choice.is_none() {
            return Err("Choose a model from the host Mac’s catalog.".into());
        }
        if effort.is_some_and(|effort| choice.is_none_or(|choice| !choice.efforts.iter().any(|item| item == effort))) {
            return Err("That thinking level is unavailable for the host model.".into());
        }
        controls.model = model.map(str::to_owned);
        controls.effort = effort.map(str::to_owned);
        let supports_fast = self.remote_can_fast();
        if !supports_fast { self.remote_workspace.controls.fast = None; }
        cx.notify();
        Ok(())
    }

    pub(super) fn set_remote_native_agent(&mut self, id: &str, cx: &mut Context<Self>) -> Result<(), String> {
        let state = &mut self.remote_workspace;
        if let Some(chat) = &state.chat {
            return if chat.agent == id { Ok(()) } else { Err("Choose the agent when starting a new host chat.".into()) };
        }
        let controls = &mut state.controls;
        if !controls.options.as_ref().is_some_and(|options| options.providers.iter().any(|provider| provider == id)) {
            return Err("The agent is unavailable on the host Mac.".into());
        }
        controls.draft_agent = agent(id);
        controls.catalog = None;
        controls.model = None;
        controls.effort = None;
        controls.fast = None;
        self.load_remote_models(cx);
        cx.notify();
        Ok(())
    }

    pub(crate) fn remote_prompt_input(&self) -> Option<Entity<TextInput>> {
        if self.remote_cli_selected() { return None; }
        self.remote_workspace.composer.clone()
    }
    pub(crate) fn remote_composer_model(&self) -> Option<String> {
        self.remote_workspace.controls.model().map(str::to_owned)
    }
    pub(crate) fn remote_composer_thinking(&self) -> (Option<agent_controls::ModelChoice>, String) {
        let controls = &self.remote_workspace.controls;
        let choice = controls.choice().map(|item| agent_controls::ModelChoice {
            id: item.id.clone(),
            label: item.name.clone(),
            reasoning_efforts: item.efforts.clone(),
        });
        let effort = controls.effort().unwrap_or_default().to_owned();
        (choice, effort)
    }
    pub(crate) fn remote_can_fast(&self) -> bool {
        let controls = &self.remote_workspace.controls;
        controls.model().is_some_and(|model| {
            controls
                .catalog
                .as_ref()
                .is_some_and(|catalog| catalog.fast_models.contains_key(model))
        })
    }
    pub(crate) fn remote_fast(&self) -> bool {
        let controls = &self.remote_workspace.controls;
        controls.fast.unwrap_or_else(|| controls.options.as_ref().is_some_and(|options| options.fast))
            && self.remote_can_fast()
    }
    pub(crate) fn remote_toggle_fast(&mut self, cx: &mut Context<Self>) {
        if self.remote_can_fast() {
            self.remote_workspace.controls.fast = Some(!self.remote_fast());
            self.fast_transition = Some(Instant::now());
            cx.notify();
        }
    }
    pub(crate) fn remote_orchestrating(&self) -> bool {
        self.remote_workspace.controls.orchestrator
    }
    pub(crate) fn remote_toggle_orchestrate(&mut self, cx: &mut Context<Self>) {
        if self.remote_composer_supported() {
            self.remote_workspace.controls.orchestrator =
                !self.remote_workspace.controls.orchestrator;
            cx.notify();
        }
    }
    fn remote_composer_supported(&self) -> bool {
        self.remote_workspace
            .selected
            .as_ref()
            .is_some_and(|peer| peer.supports("workspace.composer"))
    }
    pub(crate) fn remote_composer_edits(&self) -> bool {
        let controls = &self.remote_workspace.controls;
        controls
            .options
            .as_ref()
            .is_some_and(|options| options.edits)
            && controls.edits.unwrap_or(true)
    }
    pub(crate) fn open_remote_composer_panel(
        &mut self,
        panel: ComposerPanel,
        anchor: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.modal == Some(Modal::ComposerControls)
            && self.remote_workspace.controls.panel != panel
        {
            self.dismiss_panel(window, cx);
        }
        self.remote_workspace.controls.panel = panel;
        if self.remote_workspace.controls.catalog.is_none()
            && matches!(panel, ComposerPanel::Model | ComposerPanel::Thinking)
            && self.remote_composer_supported()
        {
            self.load_remote_models(cx);
        }
        self.toggle_panel_from_click(Modal::ComposerControls, anchor, true, window, cx);
    }
    pub(crate) fn remote_composer_panel(&self, cx: &Context<Self>) -> AnyElement {
        let state = &self.remote_workspace;
        let controls = &state.controls;
        let mut body = div().p_2().flex().flex_col().gap_1();
        if controls.panel == ComposerPanel::Projects {
            let Some(peer) = &state.selected else {
                return body.into_any_element();
            };
            if peer.online && peer.allowed && peer.control && state.project.is_some() {
                body = body.child(row("remote-create-project-folder", "Create project…", false)
                    .on_click(cx.listener(|app, _, window, cx| app.new_project_folder(window, cx))));
            }
            if let Some(directory) = state
                .tree_branches
                .get(&peer.key())
                .and_then(|branch| branch.directory.as_ref())
            {
                body = body.child(
                    div()
                        .id("remote-composer-projects")
                        .max_h(px(320.))
                        .overflow_y_scroll()
                        .children(directory.projects.iter().map(|project| {
                            let device = peer.clone();
                            let path = project.path.clone();
                            row(
                                SharedString::from(format!("composer-project-{path}")),
                                project.name.clone(),
                                state.project.as_ref() == Some(&path),
                            )
                            .on_click(cx.listener(
                                move |app, _, window, cx| {
                                    app.dismiss_panel(window, cx);
                                    app.open_remote_chat(
                                        device.clone(),
                                        Some(path.clone()),
                                        None,
                                        window,
                                        cx,
                                    );
                                },
                            ))
                        })),
                );
            }
            return body.into_any_element();
        }
        if !self.remote_composer_supported() {
            return body
                .child(
                    div()
                        .p_2()
                        .child("Update Kai on the host to use these composer controls."),
                )
                .into_any_element();
        }
        match controls.panel {
            ComposerPanel::Projects => {}
            ComposerPanel::Model => {
                if state.chat.is_none()
                    && let Some(options) = &controls.options
                {
                    body = body.child(div().flex().flex_wrap().gap_1().children(
                        options.providers.iter().map(|name| {
                            let kind = agent(name);
                            row(
                                SharedString::from(format!("remote-provider-{name}")),
                                kind.name().to_owned(),
                                kind == controls.draft_agent,
                            )
                            .on_click(cx.listener(
                                move |app, _, _, cx| {
                                    let controls = &mut app.remote_workspace.controls;
                                    controls.draft_agent = kind;
                                    controls.catalog = None;
                                    controls.model = None;
                                    controls.effort = None;
                                    controls.fast = None;
                                    app.load_remote_models(cx);
                                    cx.notify();
                                },
                            ))
                        }),
                    ));
                }
                body = body.child(
                    row(
                        "remote-model-default",
                        "Host default",
                        controls.model.is_none(),
                    )
                    .on_click(cx.listener(|app, _, window, cx| {
                        let controls = &mut app.remote_workspace.controls;
                        controls.model = None;
                        controls.effort = None;
                        controls.fast = None;
                        app.dismiss_panel(window, cx);
                        cx.notify();
                    })),
                );
                if let Some(catalog) = &controls.catalog {
                    body = body.child(
                        div()
                            .id("remote-model-list")
                            .max_h(px(290.))
                            .overflow_y_scroll()
                            .children(catalog.models.iter().map(|model| {
                                let id = model.id.clone();
                                row(
                                    SharedString::from(format!("remote-model-{id}")),
                                    model.name.clone(),
                                    controls.model() == Some(id.as_str()),
                                )
                                .on_click(cx.listener(
                                    move |app, _, window, cx| {
                                        let controls = &mut app.remote_workspace.controls;
                                        controls.model = Some(id.clone());
                                        controls.effort = None;
                                        controls.fast = None;
                                        app.dismiss_panel(window, cx);
                                        cx.notify();
                                    },
                                ))
                            })),
                    );
                } else {
                    body = body.child(div().p_2().child(if controls.loading {
                        "Loading host models…"
                    } else {
                        "No model catalog available"
                    }));
                }
            }
            ComposerPanel::Thinking => {
                if let Some(model) = controls.choice() {
                    body = body.children(model.efforts.iter().map(|effort| {
                        let effort = effort.clone();
                        row(
                            SharedString::from(format!("remote-effort-{effort}")),
                            composer_design::title(&effort),
                            controls.effort.as_ref() == Some(&effort),
                        )
                        .on_click(cx.listener(
                            move |app, _, window, cx| {
                                app.remote_workspace.controls.effort = Some(effort.clone());
                                app.dismiss_panel(window, cx);
                                cx.notify();
                            },
                        ))
                    }));
                }
            }
            ComposerPanel::Permissions => {
                use runtime::CodexAccess::{Full, Workspace};
                let mode = controls.access.or_else(|| controls.options.as_ref().map(|options| options.codex_access))
                    .unwrap_or_default();
                let edits = self.remote_composer_edits();
                let available = controls.options.as_ref();
                for (id, label, access, enabled) in [
                    ("remote-permission-plan", "Read only / plan", None, true),
                    ("remote-permission-edit", "Workspace edits", Some(Workspace), available.is_some_and(|options| options.edits)),
                    ("remote-permission-full", "Full access", Some(Full), available.is_some_and(|options| options.full_access)),
                ] {
                    if enabled {
                        body = body.child(row(id, label, access == edits.then_some(mode))
                            .on_click(cx.listener(move |app, _, window, cx| {
                                app.remote_workspace.controls.edits = Some(access.is_some());
                                app.remote_workspace.controls.access = Some(access.unwrap_or(Workspace));
                                app.dismiss_panel(window, cx);
                                cx.notify();
                            })));
                    }
                }
                if let Some(reason) = available.and_then(|options| options.desktop_unavailable.as_ref()) {
                    body = body.child(div().max_w(px(235.)).px_2().py_1()
                        .text_color(rgb(MUTED)).text_size(theme::ui_px(11.)).child(reason.clone()));
                }
            }
            ComposerPanel::More => {
                body = body.children(
                    [
                        (ComposerPanel::Model, "Choose model"),
                        (ComposerPanel::Thinking, "Thinking level"),
                        (ComposerPanel::Permissions, "Workspace access"),
                        (ComposerPanel::Projects, "Choose project"),
                    ]
                    .into_iter()
                    .map(|(panel, label)| {
                        row(label, label, false).on_click(cx.listener(move |app, _, _, cx| {
                            app.remote_workspace.controls.panel = panel;
                            if panel == ComposerPanel::Model
                                && app.remote_workspace.controls.catalog.is_none()
                            {
                                app.load_remote_models(cx);
                            }
                            cx.notify();
                        }))
                    }),
                );
                let kind = state
                    .chat
                    .as_ref()
                    .map(|chat| agent(&chat.agent))
                    .unwrap_or(controls.draft_agent);
                if runtime::supports_native_delegation(kind) {
                    body = body
                        .child(
                            row(
                                "remote-more-orchestrate",
                                if controls.orchestrator {
                                    "Orchestrator on"
                                } else {
                                    "Orchestrate"
                                },
                                controls.orchestrator,
                            )
                            .on_click(
                                cx.listener(|app, _, _, cx| app.remote_toggle_orchestrate(cx)),
                            ),
                        )
                        .child(
                            row("remote-more-delegate", "Delegate task", false).on_click(
                                cx.listener(|app, _, window, cx| {
                                    app.remote_insert_delegate(window, cx)
                                }),
                            ),
                        );
                }
                if self.remote_can_fast() {
                    body = body.child(
                        row("remote-more-fast", "Fast mode", self.remote_fast())
                            .on_click(cx.listener(|app, _, _, cx| app.remote_toggle_fast(cx))),
                    );
                }
                body = body.child(row("remote-more-attach", "Attach files", false).on_click(
                    cx.listener(|app, _, window, cx| {
                        app.dismiss_panel(window, cx);
                        app.choose_remote_attachments(cx);
                    }),
                ));
            }
        }
        body.into_any_element()
    }

    pub(crate) fn remote_has_attachments(&self) -> bool {
        !self.remote_workspace.controls.attachments.is_empty()
    }

    pub(crate) fn remote_running(&self) -> bool {
        let state = &self.remote_workspace;
        state.selected.as_ref().is_some_and(|peer| peer.online && peer.allowed)
            && (self.remote_voice_state().is_some() || state.selected_chat_running())
    }

    pub(crate) fn remote_run_control(
        &mut self,
        action: &'static str,
        prompt: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.remote_voice_state().is_some() && action == "stop" {
            self.stop_remote_voice(cx);
            return;
        }
        let state = &mut self.remote_workspace;
        let (Some(peer), Some(chat), Some(helper)) = (
            state
                .selected
                .clone()
                .filter(|peer| peer.allowed && peer.online && peer.control),
            state.chat.clone(),
            state.helper.clone(),
        ) else {
            return;
        };
        let original = (action == "steer")
            .then(|| {
                state
                    .composer
                    .clone()
                    .map(|input| (input.clone(), input.read(cx).text()))
            })
            .flatten();
        let epoch = state.epoch;
        let expected_run = state.host_run_id.clone();
        let stop = if action == "stop" {
            let Some(key) = state.draft_key() else { return };
            if state.stopping.get(&key).is_some_and(|pending| !pending.is_closed()) { return; }
            let (done, pending) = async_channel::bounded(1);
            state.stopping.insert(key.clone(), pending.clone());
            Some((key, done, pending))
        } else {
            None
        };
        let mut query = request("run-controls", &peer);
        query["chat"] = json!(chat.id);
        let job = cx.background_executor().spawn(async move {
            let run = if let Some(run) = expected_run {
                run
            } else {
                let controls = call::<Value>(&helper, query)?;
                controls["tasks"].as_array().into_iter().flatten().find(|task| task["chat_id"] == chat.id)
                    .and_then(|task| task["run_id"].as_str())
                    .ok_or("The host chat no longer has an active run")?.to_owned()
            };
            let mut req = request("command", &peer);
            req["command"] = json!({"action":action,"chat_id":chat.id,"run_id":run,"request_id":uuid::Uuid::new_v4()});
            if let Some(prompt) = prompt { req["command"]["prompt"] = json!(prompt); }
            call::<Value>(&helper, req).and_then(payload)
        });
        cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |app, cx| {
                if let Some((key, _, pending)) = &stop
                    && app.remote_workspace.stopping.get(key).is_some_and(|current| current.same_channel(pending))
                {
                    app.remote_workspace.stopping.remove(key);
                }
                if app.remote_workspace.epoch == epoch {
                    if let Err(error) = result {
                        app.remote_workspace.issue = Some(error);
                    } else {
                        if let Some((input, text)) = original
                            && input.read(cx).text() == text
                        {
                            input.update(cx, |input, cx| input.set_text("", cx));
                        }
                        app.fetch_remote_activity(cx);
                    }
                    cx.notify();
                }
            });
            drop(stop);
        })
        .detach();
    }

    pub(crate) fn remote_insert_delegate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.remote_prompt_input() else {
            return;
        };
        let text = input.read(cx).text();
        if commands::name(text.trim()) != Some("delegate") {
            input.update(cx, |input, cx| {
                input.set_text(&format!("/delegate {text}"), cx)
            });
        }
        self.dismiss_panel(window, cx);
        input.read(cx).focus_handle.focus(window);
    }

    pub(crate) fn choose_remote_attachments(&mut self, cx: &mut Context<Self>) {
        let state = &self.remote_workspace;
        if state.controls.attachment_pending || state.has_pending_send() {
            return;
        }
        let available = state.selected.as_ref().is_some_and(|peer| {
            peer.allowed
                && peer.control
                && peer.online
                && state.project.as_deref().is_some_and(|project| peer.has_project_files(project))
        });
        if !available {
            self.remote_project_access(false, cx);
            return;
        }
        let epoch = state.epoch;
        let selection = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach files to the host chat".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = selection.await {
                let _ = this.update(cx, |app, cx| {
                    if app.remote_workspace.epoch == epoch {
                        app.upload_remote_attachments(paths, cx);
                    }
                });
            }
        })
        .detach();
    }

    pub(crate) fn upload_remote_attachments(
        &mut self,
        paths: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        let state = &mut self.remote_workspace;
        if state.controls.attachment_pending || state.has_pending_send() {
            return;
        }
        let (Some(peer), Some(helper), Some(project)) = (
            state.selected.clone(),
            state.helper.clone(),
            state.project.clone(),
        ) else {
            return;
        };
        if !peer.allowed || !peer.online || !peer.control || !peer.has_project_files(&project) {
            self.remote_project_access(false, cx);
            return;
        }
        let Some(worker) = state.composer_worker.as_ref() else {
            return;
        };
        let slots = attachments::MAX_ATTACHMENTS.saturating_sub(state.controls.attachments.len());
        if slots == 0 || paths.is_empty() {
            return;
        }
        let source = RemoteFiles {
            helper,
            peer,
            root: PathBuf::from(project),
        };
        let epoch = state.epoch;
        let job = worker.submit(false, move || {
            let mut attachments = Vec::new();
            let mut error = None;
            for path in paths.into_iter().take(slots) {
                let Some(name) = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .filter(|name| !name.chars().any(char::is_control))
                else {
                    continue;
                };
                let target = PathBuf::from(".kai-attachments")
                    .join(format!("{}-{name}", uuid::Uuid::new_v4()));
                match source.call::<Value>("composer-upload", &target, json!({"path":path})) {
                    Ok(_) => attachments.push((
                        name.to_owned(),
                        source.root.join(&target).to_string_lossy().into_owned(),
                    )),
                    Err(issue) => {
                        error = Some(issue);
                        break;
                    }
                }
            }
            (attachments, error)
        });
        state.controls.attachment_pending = true;
        cx.spawn(async move |this, cx| {
            let result = job.recv().await;
            let _ = this.update(cx, |app, cx| {
                if app.remote_workspace.epoch != epoch {
                    return;
                }
                let controls = &mut app.remote_workspace.controls;
                controls.attachment_pending = false;
                match result {
                    Ok((uploaded, error)) => {
                        controls.attachments.extend(uploaded);
                        if error.is_some() {
                            app.remote_workspace.issue = error;
                        }
                    }
                    Err(_) => app.remote_workspace.issue = Some("Another host control is pending".into()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn remote_attachment_strip(&self, cx: &Context<Self>) -> AnyElement {
        let controls = &self.remote_workspace.controls;
        div()
            .flex()
            .flex_wrap()
            .gap_1()
            .px_3()
            .when(controls.attachment_pending, |body| {
                body.child("Uploading to host…")
            })
            .children(controls.attachments.iter().map(|(display, path)| {
                let path = path.clone();
                div()
                    .id(SharedString::from(path.clone()))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(rgba(GLASS_HOVER))
                    .flex()
                    .gap_2()
                    .child(icon("file", 14., MUTED))
                    .child(display.clone())
                    .child(
                        div()
                            .id("remove")
                            .cursor_pointer()
                            .child(icon("close", 12., MUTED))
                            .on_click(cx.listener(move |app, _, _, cx| {
                                if !app.remote_workspace.has_pending_send() {
                                    app.remote_workspace
                                        .controls
                                        .attachments
                                        .retain(|(_, stored)| stored != &path);
                                    cx.notify();
                                }
                            })),
                    )
            }))
            .into_any_element()
    }

    pub(crate) fn remote_can_realtime(&self) -> bool {
        let state = &self.remote_workspace;
        state
            .chat
            .as_ref()
            .is_some_and(|chat| agent(&chat.agent) == AgentKind::Codex)
            && state.selected.as_ref().is_some_and(|peer| {
                peer.supports("workspace.realtime") && peer.online && peer.allowed && peer.control
            })
            && state
                .controls
                .options
                .as_ref()
                .is_some_and(|options| options.realtime)
    }
    pub(crate) fn remote_voice_state(&self) -> Option<(&'static str, bool)> {
        self.remote_workspace
            .controls
            .voice
            .as_ref()
            .map(|call| (call.caption(), call.muted))
    }
    pub(crate) fn remote_voice_choice(&self) -> &str {
        self.remote_workspace
            .controls
            .voice_choice
            .as_deref()
            .or_else(|| self.remote_workspace.controls.options.as_ref()?.voice.as_deref())
            .unwrap_or("default")
    }
    pub(crate) fn select_remote_voice(&mut self, voice: &str, cx: &mut Context<Self>) {
        self.remote_workspace.controls.voice_choice = Some(voice.into());
        cx.notify();
    }
    pub(crate) fn start_remote_voice(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(error) = self.start_remote_voice_prompt(None, window, cx) {
            self.remote_workspace.issue = Some(error);
            cx.notify();
        }
    }

    pub(super) fn start_remote_voice_prompt(&mut self, initial_prompt: Option<String>, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        if !self.remote_can_realtime() {
            return Err("Open a saved Codex chat on an online Mac with agent control to use RealTime.".into());
        }
        if self.remote_workspace.controls.voice.is_some() {
            return Err("A remote RealTime call is already running.".into());
        }
        self.remote_workspace.issue = None;
        self.remote_workspace.controls.voice_error = None;
        self.stop_voice(cx);
        self.stop_dictation(cx);
        let id = uuid::Uuid::new_v4().to_string();
        let media = Media::start(window, &id).map_err(|error| error.to_string())?;
        let wake = media.wake.clone();
        let timeout_id = id.clone();
        self.remote_workspace.controls.voice = Some(ClientCall {
            id: id.clone(),
            media,
            started: false,
            connected: false,
            active: false,
            answered: false,
            muted: false,
            activity: VoiceActivity::Thinking,
            offer_sent: false,
            preview: String::new(),
            initial_prompt,
        });
        cx.spawn(async move |this, cx| {
            while wake.recv().await.is_ok() {
                if !matches!(
                    this.update(cx, |app, cx| app.drain_remote_voice(&id, cx)),
                    Ok(true)
                ) {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_secs(60)).await;
            let _ = this.update(cx, |app, cx| {
                if app
                    .remote_workspace
                    .controls
                    .voice
                    .as_ref()
                    .is_some_and(|call| call.id == timeout_id && !call.active)
                {
                    app.fail_remote_voice(
                        "RealTime did not connect. Check microphone access and try again.".into(),
                        cx,
                    );
                }
            });
        })
        .detach();
        cx.notify();
        Ok(())
    }
    fn drain_remote_voice(&mut self, id: &str, cx: &mut Context<Self>) -> bool {
        while let Some(call) = self
            .remote_workspace
            .controls
            .voice
            .as_mut()
            .filter(|call| call.id == id)
        {
            let Ok(event) = call.media.try_recv() else {
                return true;
            };
            match event {
                MediaEvent::Offer { sdp, .. } if !call.offer_sent => {
                    call.offer_sent = true;
                    self.signal_remote_voice_start(id, sdp, cx);
                }
                MediaEvent::Connected { .. } => {
                    call.connected = true;
                    if let Err(error) = call.activate() {
                        self.fail_remote_voice(error.to_string(), cx);
                        return false;
                    }
                }
                MediaEvent::Activity { activity, .. } => {
                    call.activity = activity;
                    cx.notify();
                }
                MediaEvent::Error { message, .. } => {
                    self.fail_remote_voice(message, cx);
                    return false;
                }
                _ => {}
            }
        }
        false
    }
    fn voice_job(&self, action: &str, id: &str, args: Value) -> Option<Reply> {
        let state = &self.remote_workspace;
        let chat = state.chat.as_ref()?;
        let mut payload = json!({"action":action,"chat_id":chat.id,"voice_session":id,"request_id":uuid::Uuid::new_v4()});
        if let Some(map) = args.as_object() {
            for (key, value) in map {
                payload[key] = value.clone();
            }
        }
        command(state, payload, true)
    }
    fn signal_remote_voice_start(&mut self, id: &str, sdp: String, cx: &mut Context<Self>) {
        let mut args = json!({"sdp":sdp,"voice":self.remote_voice_choice()});
        self.remote_workspace.controls.apply_turn(&mut args);
        let Some(job) = self.voice_job("voice_start", id, args) else {
            self.fail_remote_voice("Could not connect to RealTime on the host".into(), cx);
            return;
        };
        let id = id.to_owned();
        cx.spawn(async move |this, cx| {
            if let Err(error) = receive(job).await {
                let _ = this.update(cx, |app, cx| {
                    if app
                        .remote_workspace
                        .controls
                        .voice
                        .as_ref()
                        .is_some_and(|call| call.id == id)
                    {
                        app.fail_remote_voice(error, cx);
                    }
                });
                return;
            }
            let start = Instant::now();
            loop {
                let job = this
                    .update(cx, |app, _| {
                        if app
                            .remote_workspace
                            .controls
                            .voice
                            .as_ref()
                            .is_none_or(|call| call.id != id)
                        {
                            return None;
                        }
                        app.voice_job("voice_status", &id, Value::Null)
                    })
                    .ok()
                    .flatten();
                let Some(job) = job else {
                    let _ = this.update(cx, |app, cx| {
                        if app
                            .remote_workspace
                            .controls
                            .voice
                            .as_ref()
                            .is_some_and(|call| call.id == id)
                        {
                            app.fail_remote_voice("The host is unavailable".into(), cx);
                        }
                    });
                    break;
                };
                let result = receive(job).await;
                if !matches!(
                    this.update(cx, |app, cx| app
                        .remote_voice_response(&id, result, start, cx)),
                    Ok(true)
                ) {
                    break;
                }
                Timer::after(Duration::from_millis(850)).await;
            }
        })
        .detach();
    }
    fn remote_voice_response(
        &mut self,
        id: &str,
        result: Result<Value, String>,
        start: Instant,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(call) = self
            .remote_workspace
            .controls
            .voice
            .as_mut()
            .filter(|call| call.id == id)
        else {
            return false;
        };
        let value = match result {
            Ok(value) => value,
            Err(error) => {
                self.fail_remote_voice(error, cx);
                return false;
            }
        };
        if value["closed"] == true || !call.active && start.elapsed() >= Duration::from_secs(60) {
            if let Some(error) = value["error"].as_str() {
                self.fail_remote_voice(error.into(), cx);
            } else if value["closed"] != true {
                self.fail_remote_voice("RealTime did not connect. Try again.".into(), cx);
            } else {
                self.stop_remote_voice(cx);
            }
            self.fetch_remote_workspace(cx);
            return false;
        }
        let applied = (|| -> anyhow::Result<()> {
            if !call.answered
                && let Some(sdp) = value["answer"].as_str()
            {
                call.media.answer(sdp)?;
                call.answered = true;
            }
            call.started = value["started"] == true;
            call.activate()
        })();
        if let Err(error) = applied {
            self.fail_remote_voice(error.to_string(), cx);
            return false;
        }
        // Let live output use the ordinary host transcript parser and virtual list.
        if let Some(text) = value["preview"].as_str()
            && text != call.preview
        {
            call.preview = text.into();
            let status = call.caption().to_owned();
            let state = &mut self.remote_workspace;
            if let Some(chat) = &state.chat {
                let rows = canonical_remote_rows(
                    chat,
                    vec![runtime::TranscriptMessage {
                        role: "assistant".into(),
                        text: text.into(),
                        code_language: None,
                    }],
                );
                let flow = chat_flow::Index::new(&rows);
                state.receive_live(LiveTranscript {
                    rows: Arc::new(rows),
                    flow: Arc::new(flow),
                    status,
                });
            }
        }
        cx.notify();
        true
    }
    pub(crate) fn toggle_remote_mute(&mut self, cx: &mut Context<Self>) {
        if let Some(call) = self.remote_workspace.controls.voice.as_mut() {
            if let Err(error) = call.media.mute(!call.muted) {
                self.remote_workspace.issue = Some(error.to_string());
            } else {
                call.muted = !call.muted;
            }
            cx.notify();
        }
    }
    pub(crate) fn interrupt_remote_voice(&mut self, cx: &mut Context<Self>) {
        if let Some(call) = self.remote_workspace.controls.voice.as_ref() {
            let _ = call.media.interrupt();
            let _ = self.voice_job("voice_interrupt", &call.id, Value::Null);
            cx.notify();
        }
    }
    pub(crate) fn stop_remote_voice(&mut self, cx: &mut Context<Self>) {
        if self.remote_workspace.controls.voice.is_none() {
            return;
        }
        self.remote_workspace.clear_preview();
        let Some(call) = self.remote_workspace.controls.voice.take() else {
            return;
        };
        if call.offer_sent {
            let _ = self.voice_job("voice_end", &call.id, Value::Null);
        }
        self.remote_workspace.finish_poll_until = Some(Instant::now() + Duration::from_secs(12));
        cx.notify();
    }
    fn fail_remote_voice(&mut self, error: String, cx: &mut Context<Self>) {
        self.stop_remote_voice(cx);
        self.remote_workspace.controls.voice_error = Some(error.clone());
        self.remote_workspace.issue = Some(error);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;

    #[gpui::test]
    fn remote_toolbar_uses_host_options_and_keeps_the_local_composer_unchanged(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update(cx, |app, cx| {
            let peer: Device = serde_json::from_value(json!({
                "node":"remote-node","device":"remote-device","name":"Studio","allowed":true,"online":true,
                "compatible":true,"control":true,"capabilities":["workspace.composer","workspace.realtime"]
            })).unwrap();
            let chat: RemoteChat = serde_json::from_value(json!({
                "id":"remote-chat","title":"Host chat","agent":"codex","project":"/remote/work"
            })).unwrap();
            app.remote_workspace.started = true;
            app.open_remote_selection(peer, Some(chat.project.clone()), Some(chat), cx);
            let controls = &mut app.remote_workspace.controls;
            controls.options = serde_json::from_value(json!({"providers":["codex"],"edits":true,"realtime":true,"model":"host-a","effort":"medium","voice":"breeze"})).ok();
            controls.catalog = serde_json::from_value(json!({
                "default_model":"host-a","models":[{"id":"host-a","name":"Host model A","efforts":["medium"]},
                    {"id":"host-b","name":"Host model B","efforts":["low","high"]}],"fast_models":{"host-b":"priority"}
            })).ok();
            app.composer.update(cx, |input, cx| input.set_text("Local draft", cx));
            let snapshot = app.unified_snapshot(cx);
            assert_eq!(snapshot["effectiveModel"], "host-a");
            assert_eq!(snapshot["effectiveReasoningEffort"], "medium");
            assert_eq!(snapshot["workspaceVoiceChoice"], "breeze");
        });
        crate::ui_tests::draw_frame(&view, cx);
        crate::ui_tests::draw_frame(&view, cx);
        assert!(cx.debug_bounds("conversation-voice").is_some());
        assert!(cx.debug_bounds("dictate").is_some());
        crate::ui_tests::click_selector(cx, "choose-agent");
        crate::ui_tests::draw_frame(&view, cx);
        crate::ui_tests::click_selector(cx, "remote-model-host-b");
        view.update(cx, |app, cx| {
            let controls = &mut app.remote_workspace.controls;
            controls.effort = Some("high".into());
            controls.edits = Some(false);
            controls.fast = Some(true);
            let mut payload = json!({});
            controls.apply_turn(&mut payload);
            assert_eq!(payload, json!({"model":"host-b","effort":"high","allow_edits":false,"fast":true,"orchestrator":false}));
            assert_eq!(app.composer.read(cx).text(), "Local draft");
            assert_ne!(app.model.as_deref(), Some("host-b"));
        });
    }
}
