//! Ninja shares the native picker and composer; discovery happens only on demand.
use super::*;
use crate::workspace_state::WorkspaceState;
use std::collections::{BTreeMap, HashMap, HashSet};
#[derive(Default)]
pub(super) struct State {
    pub draft: ninja::Selection,
    pub chats: BTreeMap<String, ninja::Selection>,
    pub bound: BTreeMap<String, ninja::Selection>,
    pub machines: Vec<ninja::Machine>,
    pub inventories: HashMap<String, Vec<ninja::Model>>,
    pub registry: Vec<ninja::Model>,
    pub native_query: String,
    pub download_cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    pub download_status: Option<String>,
    pub download_fraction: Option<(f32, f32)>,
    pub variants: HashMap<String, Vec<ninja::Model>>,
    pub pending: HashSet<String>,
    pub error: Option<String>,
    pub progress: Option<String>,
    route_request: Option<uuid::Uuid>,
    pub catalog_loaded: bool,
}
impl State {
    pub fn restore(state: Option<&WorkspaceState>) -> Self {
        Self {
            draft: state.map(|s| s.ninja.clone()).unwrap_or_default(),
            chats: state.map(|s| s.ninja_chats.clone()).unwrap_or_default(),
            bound: state.map(|s| s.ninja_bound.clone()).unwrap_or_default(),
            machines: vec![ninja::local_machine()],
            ..Default::default()
        }
    }
}
pub(super) fn key(s: &ninja::Selection) -> String {
    format!("{}:{:?}", s.endpoint, s.engine)
}
impl Orbit {
    pub(super) fn ninja_route_for(&self, chat: Option<&str>) -> ninja::Selection {
        let mut s = chat.and_then(|id| self.ninja.chats.get(id))
            .unwrap_or(&self.ninja.draft).clone();
        s.use_local_kai_engine();
        s
    }

    pub(super) fn ninja_selection(&self) -> ninja::Selection {
        let mut s = self.ninja_route_for(self.active.as_deref());
        if self.agent == AgentKind::Ninja {
            if let Some(model) = &self.model {
                s.model = model.clone()
            }
        }
        s
    }
    fn ninja_save(&mut self, s: ninja::Selection) {
        self.ninja.draft = s.clone();
        if let Some(id) = &self.active {
            self.ninja.chats.insert(id.clone(), s);
        }
    }
    pub(super) fn ninja_models(&self) -> Vec<ninja::Model> {
        let selection = self.ninja_selection();
        let mut out = self
            .ninja
            .inventories
            .get(&key(&selection))
            .cloned()
            .unwrap_or_default();
        let mut seen = out.iter().map(|m| m.id.clone()).collect::<HashSet<_>>();
        for model in self
            .ninja
            .variants
            .values()
            .flatten()
            .chain(self.ninja.registry.iter())
        {
            if seen.insert(model.id.clone()) {
                out.push(model.clone());
            }
        }
        ninja::sort(&mut out);
        out
    }
    pub(super) fn ninja_choices(&self) -> Vec<agent_controls::ModelChoice> {
        self.ninja_models()
            .into_iter()
            .map(|m| agent_controls::ModelChoice {
                id: m.id.clone(),
                label: format!(
                    "{} · {}{}",
                    m.label,
                    m.lab,
                    if m.installed {
                        " · Installed"
                    } else if m.id.contains(':') {
                        " · Download"
                    } else {
                        " · Choose size"
                    }
                ),
                reasoning_efforts: m.thinking,
            })
            .collect()
    }
    pub(super) fn ninja_snapshot_choices(&self) -> Vec<agent_controls::ModelChoice> {
        let query = self.ninja.native_query.to_lowercase();
        let mut choices = self
            .ninja_choices()
            .into_iter()
            .filter(|m| {
                query
                    .split_whitespace()
                    .all(|w| m.label.to_lowercase().contains(w))
            })
            .take(128)
            .collect::<Vec<_>>();
        let selected = self.ninja_selection().model;
        if !selected.is_empty() && !choices.iter().any(|m| m.id == selected) {
            if let Some(model) = self.ninja_choices().into_iter().find(|m| m.id == selected) {
                choices.insert(0, model);
            }
        }
        choices
    }
    pub(super) fn ninja_refresh(&mut self, force: bool, cx: &mut Context<Self>) {
        if self.ninja.registry.is_empty() {
            self.ninja.registry = ninja::bundled_registry();
        }
        let paired = self.paired_inference_machines();
        self.ninja.machines.retain(|machine| !machine.id.starts_with("kai:"));
        self.ninja.machines.extend(paired);
        let selection = self.ninja_selection();
        let inventory_key = key(&selection);
        if self.ninja.pending.contains(&inventory_key)
            || (!force && self.ninja.inventories.contains_key(&inventory_key))
        {
            return;
        }
        self.ninja.pending.insert(inventory_key.clone());
        cx.spawn(async move |this, cx| {
            let selected = selection.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    let machines = ninja::machines();
                    let models = ninja::installed(&selected);
                    (machines, models)
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                app.ninja.pending.remove(&inventory_key);
                let current = app.agent == AgentKind::Ninja
                    && key(&app.ninja_selection()) == inventory_key;
                match result.0 {
                    Ok(mut machines) => {
                        let paired = app.paired_inference_machines();
                        machines.retain(|machine| machine.local
                            || !paired.iter().any(|peer| peer.name == machine.name));
                        machines.extend(paired);
                        app.ninja.machines = machines;
                    }
                    Err(e) if current => app.ninja.error = Some(e.to_string()),
                    Err(_) => {},
                };
                match result.1 {
                    Ok(models) => {
                        app.ninja.inventories.insert(inventory_key, models);
                        if current && app.ninja.progress.is_none() { app.ninja.error = None; }
                    }
                    Err(e) if current => app.ninja.error = Some(e.to_string()),
                    Err(_) => {},
                };
                app.ninja_inspect(cx);
                cx.notify();
            });
        })
        .detach();
        if !self.ninja.catalog_loaded && !self.ninja.pending.contains("registry") {
            self.ninja.pending.insert("registry".into());
            cx.spawn(async move |this, cx| {
                let result = cx
                    .background_executor()
                    .spawn(async move { ninja::registry() })
                    .await;
                let _ = this.update(cx, |app, cx| {
                    app.ninja.pending.remove("registry");
                    match result {
                        Ok(models) => {
                            app.ninja.registry = models;
                            app.ninja.catalog_loaded = true;
                        }
                        Err(e) => app.ninja.error = Some(e.to_string()),
                    }
                    cx.notify();
                });
            })
            .detach();
        }
    }
    pub(super) fn ninja_inspect(&mut self, cx: &mut Context<Self>) {
        let selection = self.ninja_selection();
        if selection.model.is_empty() {
            return;
        }
        let fingerprint = selection.fingerprint();
        let pending = format!("inspect:{fingerprint}");
        if !self.ninja.pending.insert(pending.clone()) {
            return;
        }
        cx.spawn(async move |this, cx| {
            let selected = selection.clone();
            let result = cx
                .background_executor()
                .spawn(async move { ninja::coding_route(&selected) })
                .await;
            let _ = this.update(cx, |app, cx| {
                app.ninja.pending.remove(&pending);
                match result {
                    Ok((route, model)) => {
                        if app.agent == AgentKind::Ninja
                            && app.ninja_selection().fingerprint() == selection.fingerprint()
                        {
                            const OLLAMA_FALLBACK: &str = "This model uses Ollama on the selected Mac";
                            app.ninja.error = None;
                            if route.engine != selection.engine {
                                app.ninja.progress = Some(OLLAMA_FALLBACK.into());
                            } else if app.ninja.progress.as_deref() == Some(OLLAMA_FALLBACK) {
                                app.ninja.progress = None;
                            }
                            if selection.thinking.is_none() && let Some(default) = &model.thinking_default {
                                let mut selected = selection.clone();
                                selected.thinking = Some(default.clone());
                                app.ninja_save(selected);
                                app.queue_workspace_save(cx);
                            }
                        }
                        let inventory = app.ninja.inventories.entry(key(&selection)).or_default();
                        if let Some(existing) = inventory.iter_mut().find(|m| m.id == model.id) {
                            *existing = model
                        } else {
                            inventory.push(model)
                        }
                    }
                    Err(e) if app.agent == AgentKind::Ninja
                        && app.ninja_selection().fingerprint() == selection.fingerprint() => {
                        app.ninja.error = Some(e.to_string())
                    },
                    Err(_) => {},
                }
                cx.notify();
            });
        })
        .detach();
    }
    pub(super) fn ninja_set(
        &mut self,
        field: &str,
        value: &str,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.agent != AgentKind::Ninja {
            return Err("Select Ninja first".into());
        }
        if field == "cancel" {
            if let Some(cancel) = &self.ninja.download_cancel {
                cancel.store(true, std::sync::atomic::Ordering::Release);
            }
            return Ok(());
        }
        if field == "search" {
            if value.len() > 256 {
                return Err("Search is too long".into());
            }
            self.ninja.native_query = value.into();
            cx.notify();
            return Ok(());
        }
        let mut selection = self.ninja_selection();
        match field {
            "machine" => {
                let machine = self
                    .ninja
                    .machines
                    .iter()
                    .find(|m| m.id == value)
                    .cloned()
                    .ok_or("Unknown Tailscale device")?;
                let engine = selection.engine;
                let request = uuid::Uuid::new_v4();
                self.ninja.route_request = Some(request);
                self.ninja.progress = Some(format!("Connecting to {}…", machine.name));
                self.ninja.error = None;
                cx.notify();
                let source = self.active.clone();
                cx.spawn(async move |this, cx| {
                    let device = machine.clone();
                    let result = cx
                        .background_executor()
                        .spawn(async move { ninja::detect(&device, engine) })
                        .await;
                    let _ = this.update(cx, |app, cx| {
                        if app.ninja.route_request != Some(request) { return; }
                        app.ninja.route_request = None;
                        app.ninja.progress = None;
                        match result {
                            Ok((endpoint, node)) => {
                                selection.machine = machine.id;
                                selection.endpoint = endpoint;
                                selection.node = node;
                                selection.model = String::new();
                                selection.thinking = None;
                                let mut current = source.as_ref().and_then(|id| app.ninja.chats.get(id)).unwrap_or(&app.ninja.draft).clone();
                                current.machine = selection.machine;
                                current.endpoint = selection.endpoint;
                                current.node = selection.node;
                                current.model.clear();
                                current.thinking = None;
                                if let Some(id) = source {
                                    app.ninja.chats.insert(id.clone(), current);
                                    if app.active.as_ref() == Some(&id)
                                        && app.agent == AgentKind::Ninja
                                    {
                                        app.model = None;
                                    }
                                } else {
                                    app.ninja.draft = current;
                                    if app.active.is_none() && app.agent == AgentKind::Ninja {
                                        app.model = None;
                                    }
                                }
                                app.ninja_refresh(true, cx);
                                app.queue_workspace_save(cx);
                            }
                            Err(e) => app.ninja.error = Some(e.to_string()),
                        }
                        cx.notify();
                    });
                })
                .detach();
                return Ok(());
            }
            "engine" => {
                self.ninja.route_request = None;
                self.ninja.progress = None;
                selection.engine = match value {
                    "ollama" => ninja::Engine::Ollama,
                    "llama_cpp" => ninja::Engine::LlamaCpp,
                    _ => return Err("Unknown inference engine".into()),
                };
                if selection.machine == "local" && let Some(endpoint) = ninja::local_kai_engine(selection.engine) {
                    selection.endpoint = endpoint;
                    selection.node = true;
                } else if !selection.node || selection.machine.starts_with("kai:") {
                    let Some(m) = self
                        .ninja
                        .machines
                        .iter()
                        .find(|m| m.id == selection.machine)
                    else {
                        return Err("Refresh devices first".into());
                    };
                    selection.endpoint = ninja::paired_engine_endpoint(m, selection.engine).unwrap_or_else(|| format!(
                        "http://{}:{}",
                        if m.local { "127.0.0.1" } else { m.id.as_str() },
                        if selection.engine == ninja::Engine::Ollama {
                            11434
                        } else {
                            8080
                        }
                    ));
                }
                selection.thinking = None;
            }
            "harness" => {
                selection.harness = match value {
                    "codex" => ninja::Harness::Codex,
                    "claude" => ninja::Harness::Claude,
                    _ => return Err("Unknown coding harness".into()),
                };
                if ninja::executable(selection.harness, &self.controls_home).is_none() {
                    return Err(format!(
                        "Install {} on this Mac first",
                        selection.harness.label()
                    ));
                }
            }
            "model" => {
                if value.len() > 256 || value.chars().any(char::is_control) {
                    return Err("Invalid model".into());
                }
                selection.model = value.into();
                selection.thinking = None;
                self.model = Some(value.into());
                self.agent_models.insert("ninja", self.model.clone());
                if let Some(session) = self
                    .active
                    .as_ref()
                    .and_then(|id| {
                        self.sessions
                            .iter()
                            .find(|s| s.id == *id && s.agent == AgentKind::Ninja)
                    })
                    .cloned()
                {
                    self.session_models
                        .insert(session.id.clone(), self.model.clone());
                    if !self.draft_sessions.contains(&session.id) {
                        self.engine.set_model(session, self.model.clone());
                    }
                }
            }
            "thinking" => {
                let model = self
                    .ninja_models()
                    .into_iter()
                    .find(|m| m.id == selection.model)
                    .ok_or("Inspect this model first")?;
                if !model.thinking.iter().any(|v| v == value) {
                    return Err("This model does not advertise that thinking level".into());
                }
                selection.thinking = Some(value.into());
            }
            _ => return Err("Unknown Ninja option".into()),
        }
        self.ninja_save(selection);
        self.queue_workspace_save(cx);
        self.ninja.error = None;
        if field == "engine" {
            self.ninja_refresh(true, cx)
        } else if field == "model" {
            self.ninja_inspect(cx)
        }
        cx.notify();
        Ok(())
    }
    pub(super) fn ninja_choose_model(&mut self, id: String, cx: &mut Context<Self>) {
        if self
            .ninja_models()
            .iter()
            .any(|m| m.id == id && m.installed)
        {
            if let Err(e) = self.ninja_set("model", &id, cx) {
                self.notice = Some(e)
            }
            return;
        }
        if !id.contains(':') {
            self.ninja.error = None;
            if !self.ninja.pending.insert(format!("tags:{id}")) {
                return;
            }
            if let Some(picker) = &self.agent_picker {
                picker
                    .search
                    .update(cx, |input, cx| input.set_text(&format!("{id}:"), cx));
            }
            let family = id.clone();
            cx.spawn(async move |this, cx| {
                let result = cx
                    .background_executor()
                    .spawn(async move { ninja::tags(&family) })
                    .await;
                let _ = this.update(cx, |app, cx| {
                    app.ninja.pending.remove(&format!("tags:{id}"));
                    match result {
                        Ok(tags) => {
                            app.ninja.variants.insert(
                                id.clone(),
                                tags.into_iter()
                                    .map(|tag| ninja::Model {
                                        id: tag.clone(),
                                        label: tag,
                                        lab: ninja::lab(&id, ""),
                                        installed: false,
                                        size: 0,
                                        format: "gguf".into(),
                                        tools: false,
                                        vision: false,
                                        thinking: vec![],
                                        thinking_default: None,
                                        context: 0,
                                    })
                                    .collect(),
                            );
                        }
                        Err(e) => app.ninja.error = Some(e.to_string()),
                    }
                    cx.notify();
                });
            })
            .detach();
            return;
        }
        if self.ninja.download_cancel.is_some() {
            return;
        }
        let selection = self.ninja_selection();
        let source = self.active.clone();
        let target = key(&selection);
        let inventory_key = target.clone();
        self.ninja.error = None;
        self.ninja.download_status = Some(format!("Downloading {id}…"));
        self.ninja.download_fraction = None;
        let (tx, rx) = async_channel::bounded(1);
        let download = id.clone();
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.ninja.download_cancel = Some(cancel.clone());
        cx.background_executor().spawn(async move {
            let result = ninja::pull(&selection, &download, cancel, |status, fraction| {
                let _ = tx.try_send(Ok((status, fraction)));
            }).map(|()| ninja::installed(&selection).ok()).map_err(|error| error.to_string());
            let _ = tx.send(Err(result)).await;
        }).detach();
        cx.spawn(async move |this, cx| {
            while let Ok(update) = rx.recv().await {
                let finished = update.is_err();
                let _ = this.update(cx, |app, cx| {
                    match update {
                        Ok((status, fraction)) => {
                            app.ninja.download_status = Some(status);
                            app.ninja.download_fraction = fraction.map(|next| {
                                let previous = app.ninja.download_fraction.map_or(0., |(_, end)| end);
                                (previous, next)
                            });
                        }
                        Err(result) => {
                            app.ninja.download_cancel = None;
                            app.ninja.download_fraction = None;
                            match result {
                                Ok(inventory) => {
                                    app.ninja.download_status = None;
                                    if let Some(models) = inventory {
                                        app.ninja.inventories.insert(inventory_key.clone(), models);
                                    }
                                    unified::workspace_toast(&format!("{id} is ready"), false);
                                    let current = app.ninja_selection();
                                    if source == app.active && app.agent == AgentKind::Ninja && key(&current) == target {
                                        let _ = app.ninja_set("model", &id, cx);
                                    }
                                }
                                Err(error) => {
                                    let detail = format!("{id} · {error}");
                                    app.ninja.download_status = Some(detail.clone());
                                    unified::workspace_toast(&detail, true);
                                }
                            }
                        }
                    }
                    cx.notify();
                });
                if finished {
                    break;
                }
            }
        }).detach();
        cx.notify();
    }

    pub(super) fn ninja_download_row(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let status = self.ninja.download_status.as_ref()?.clone();
        let motion = self.motion && !native::reduced_motion();
        let fill = self.ninja.download_fraction.map(|(from, to)| {
            let bar = div().h_full().rounded_full().bg(rgb(ACCENT));
            if motion {
                bar.with_animation(
                    ("ninja-download", to.to_bits() as usize),
                    Animation::new(workspace_motion::pane_duration()),
                    move |bar, progress| bar.w(relative(from + (to - from) * workspace_motion::ease_out(progress))),
                ).into_any_element()
            } else {
                bar.w(relative(to)).into_any_element()
            }
        });
        Some(div().flex().flex_col().gap_1().min_w(px(0.)).rounded_md().px_2().py_1()
            .bg(rgba(GLASS_HOVER))
            .child(div().flex().items_center().justify_between().gap_2()
                .child(div().flex_1().min_w(px(0.)).truncate().text_size(theme::ui_px(11.)).child(status))
                .when_some(self.ninja.download_cancel.as_ref(), |row, _| row.child(div().id("ninja-download-cancel").cursor_pointer().child(icon("close", 12., MUTED))
                    .on_click(cx.listener(|app, _, _, cx| { let _ = app.ninja_set("cancel", "", cx); })))))
            .when_some(fill, |row, fill| row.child(div().w_full().h(px(3.)).rounded_full()
                .overflow_hidden().bg(rgba(0xffffff20)).child(fill)))
            .into_any_element())
    }

    pub(super) fn ninja_options(&self, cx: &mut Context<Self>) -> AnyElement {
        let selection = self.ninja_selection();
        let mut items = div()
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .border_l_1()
            .border_color(rgba(0xffffff18));
        let row = |id: String,
                   label: String,
                   selected: bool,
                   field: &'static str,
                   value: String,
                   glyph: &'static str,
                   cx: &mut Context<Self>| {
            div()
                .id(SharedString::from(id))
                .px_2()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .text_size(theme::ui_px(11.))
                .text_color(rgb(if selected { 0x67dab0 } else { TEXT }))
                .when(selected, |s| s.bg(rgba(0x67dab018)))
                .hover(|s| s.bg(rgba(GLASS_HOVER)))
                .flex().items_center().gap_2().child(provider_icon(glyph,16.,TEXT))
                .child(label)
                .on_click(cx.listener(move |app, _, _, cx| {
                    if let Err(e) = app.ninja_set(field, &value, cx) {
                        app.ninja.error = Some(e)
                    }
                    cx.notify();
                }))
        };
        items = items.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(icon("tailscale", 16., TEXT))
                .child("Inference machine"),
        );
        for machine in &self.ninja.machines {
            items = items.child(row(
                format!("ninja-machine-{}", machine.id),
                format!(
                    "{}{}",
                    machine.name,
                    if machine.online { "" } else { " · Offline" }
                ),
                machine.id == selection.machine,
                "machine",
                machine.id.clone(),
                machine.glyph(),
                cx,
            ));
        }
        for (field, values) in [
            (
                "engine",
                vec![
                    (
                        "ollama",
                        "Ollama",
                        selection.engine == ninja::Engine::Ollama,
                    ),
                    (
                        "llama_cpp",
                        "llama.cpp",
                        selection.engine == ninja::Engine::LlamaCpp,
                    ),
                ],
            ),
            (
                "harness",
                vec![
                    ("codex", "Codex", selection.harness == ninja::Harness::Codex),
                    (
                        "claude",
                        "Claude Code",
                        selection.harness == ninja::Harness::Claude,
                    ),
                ],
            ),
        ] {
            let mut group = div().flex().gap_1();
            for (value, label, selected) in values {
                group = group.child(row(
                    format!("ninja-{field}-{value}"),
                    label.into(),
                    selected,
                    field,
                    value.into(),
                    match value {"ollama"=>"lab-ollama","llama_cpp"=>"engine-llamacpp","codex"=>"codex",_=>"claude"},
                    cx,
                ));
            }
            items = items.child(group);
        }
        items = items.child(
            div()
                .text_size(theme::ui_px(10.))
                .text_color(rgb(MUTED))
                .child("Harness and tools run on this Mac"),
        );
        if let Some(status) = self.ninja.progress.as_ref().or(self.ninja.error.as_ref()) {
            items = items.child(
                div()
                    .text_size(theme::ui_px(10.))
                    .text_color(rgb(MUTED))
                    .child(status.clone()),
            );
        }
        items.into_any_element()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    use std::prelude::v1::test;
    #[gpui::test]
    fn ninja_routes_and_thinking_are_chat_scoped_and_native_packets_are_bounded(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update(cx, |app, cx| {
            let a = ninja::Selection {
                model: "qwen3:8b".into(),
                thinking: Some("true".into()),
                ..Default::default()
            };
            let b = ninja::Selection {
                model: "deepseek-r1:8b".into(),
                harness: ninja::Harness::Claude,
                ..Default::default()
            };
            app.agent = AgentKind::Ninja;
            app.model = None;
            app.ninja.chats.insert("one".into(), a.clone());
            app.ninja.chats.insert("two".into(), b.clone());
            app.active = Some("one".into());
            assert_eq!(app.ninja_selection(), a);
            app.active = Some("two".into());
            assert_eq!(app.ninja_selection(), b);
            let (saved_model, effort) = app.orchestration_chat_selection(
                &serde_json::json!({}), AgentKind::Ninja, &app.project, None, Some("one"),
            ).unwrap();
            assert_eq!(saved_model.as_deref(), Some("qwen3:8b"));
            assert_eq!(effort, None);
            let (override_model, override_effort) = app.orchestration_chat_selection(
                &serde_json::json!({"model":"qwen3:4b","effort":"false"}), AgentKind::Ninja,
                &app.project, None, Some("one"),
            ).unwrap();
            assert_eq!(override_model.as_deref(), Some("qwen3:4b"));
            assert_eq!(override_effort.as_deref(), Some("false"));
            assert_eq!(app.ninja_route_for(Some("one")), a);
            app.ninja.registry = ninja::bundled_registry();
            assert!(app.ninja.registry.len() > 200);
            assert!(app.ninja_snapshot_choices().len() <= 129);
            app.ninja.native_query = "qwen".into();
            assert!(
                app.ninja_snapshot_choices()
                    .iter()
                    .any(|m| m.id.starts_with("qwen"))
            );
            assert_eq!(app.ninja.chats["one"], a);
            let saved = app.workspace_snapshot(cx);
            let restored = State::restore(Some(&saved));
            assert_eq!(restored.chats["one"], a);
            assert_eq!(restored.chats["two"], b);
        });
    }
    #[gpui::test]
    fn changing_ninja_thinking_never_relabels_another_chat_or_refreshes_the_network(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update(cx, |app, cx| {
            let s = ninja::Selection {
                model: "qwen3:8b".into(),
                ..Default::default()
            };
            app.agent = AgentKind::Ninja;
            app.active = Some("one".into());
            app.model = Some(s.model.clone());
            app.ninja.chats.insert("one".into(), s.clone());
            app.ninja.chats.insert("two".into(), s.clone());
            app.ninja.inventories.insert(
                key(&s),
                vec![ninja::Model {
                    id: s.model.clone(),
                    label: s.model.clone(),
                    lab: "Qwen".into(),
                    installed: true,
                    size: 0,
                    format: "gguf".into(),
                    tools: true,
                    vision: false,
                    thinking: vec!["false".into(), "true".into()],
                    thinking_default: None,
                    context: 32768,
                }],
            );
            app.ninja_set("thinking", "true", cx).unwrap();
            assert_eq!(app.ninja_selection().thinking.as_deref(), Some("true"));
            assert_eq!(app.ninja.chats["two"].thinking, None);
            assert!(app.ninja.pending.is_empty());
            assert!(app.ninja_set("thinking", "ultra", cx).is_err());
        });
    }
}
