//! Native entry points and one searchable command/workflow catalog.
use super::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::Digest;
use std::{path::Path, sync::{Arc, OnceLock}};
static URLS: OnceLock<(
    async_channel::Sender<Vec<String>>,
    async_channel::Receiver<Vec<String>>,
)> = OnceLock::new();
fn url_queue() -> &'static (
    async_channel::Sender<Vec<String>>,
    async_channel::Receiver<Vec<String>>,
) {
    URLS.get_or_init(|| async_channel::bounded(16))
}
pub(super) fn open_urls(urls: Vec<String>) {
    let _ = url_queue().0.try_send(urls.into_iter().take(64).collect());
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Workflow {
    pub name: String,
    pub project: PathBuf,
    pub agent: AgentKind,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub prompt: String,
}
impl Workflow {
    pub fn valid(&self) -> bool {
        preferences::valid_name(&self.name)
            && self.project.is_absolute()
            && !self.prompt.trim().is_empty()
            && self.prompt.len() <= 4096
            && self.model.as_deref().is_none_or(runtime::valid_model_id)
            && self
                .effort
                .as_ref()
                .is_none_or(|e| e.len() <= 64 && !e.chars().any(char::is_control))
    }
}
#[derive(Clone)]
pub(super) enum Target {
    Command(String),
    Panel(Modal),
    Setting(String),
    NativeSetting(String),
    File(PathBuf, PathBuf),
    NewTerminal,
    NewProjectChat,
    Project(PathBuf),
    Workflow(String),
    Result(String),
    Artifact(String),
}
#[derive(Default)]
pub(super) struct PaletteFiles {
    root: PathBuf,
    items: Vec<(String, String, PathBuf)>,
    indexed_at: Option<Instant>,
    pending: bool,
}

impl Orbit {
    pub(super) fn load_artifact_index(&mut self, cx: &mut Context<Self>) {
        let root = self.workflow_dir.join("artifacts");
        let project = self.project.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let result =
                        artifacts::command(&root, &json!({"action":"list_artifacts"}), &project);
                    (project, result)
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                if app.project == result.0 {
                    app.artifact_index = result
                        .1
                        .ok()
                        .and_then(|v| v["artifacts"].as_array().cloned())
                        .unwrap_or_default();
                    cx.notify();
                }
            });
        })
        .detach();
    }
    pub(super) fn workflow_slash(
        &mut self,
        args: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let result = if let Some(rest) = args.strip_prefix("save ") {
            if let Some((name, prompt)) = rest.split_once('\n') {
                self.workflow_command(&json!({"action":"save_workflow","name":name.trim(),"prompt":prompt.trim(),"project":self.project,"agent":self.agent.command(),"model":self.model,"effort":self.request_effort()}),&json!({"scope":"control"}),window,cx).map(|_|())
            } else {
                Err("Use /workflow save Name, then a new line with the workflow prompt".into())
            }
        } else if let Some(name) = args.strip_prefix("run ") {
            self.run_named_workflow(name.trim(), window, cx)
        } else {
            self.show_modal(Modal::Search, window, cx);
            Ok(())
        };
        if let Err(error) = result {
            self.composer.update(cx, |input, cx| {
                input.set_text(&format!("/workflow {args}"), cx)
            });
            self.notice = Some(error)
        }
        cx.notify();
    }
    pub(super) fn fork_conversation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(source) = self
            .active
            .as_ref()
            .and_then(|id| self.sessions.iter().find(|s| &s.id == id))
            .cloned()
        else {
            return;
        };
        let rx = self
            .engine
            .workspace_query(json!({"action":"read_chat","chat_id":source.id}));
        let generation = self.generation;
        let model = self.session_models.get(&source.id).cloned().flatten();
        let executable = self
            .agents
            .iter()
            .find(|a| a.kind == AgentKind::Codex)
            .and_then(|a| a.executable.clone());
        cx.spawn_in(window, async move |this, cx| {
            let value = rx
                .recv()
                .await
                .unwrap_or(json!({"error":"History worker closed"}));
            let native = if source.agent == AgentKind::Codex {
                if let Some(id) = source.external_id.clone() {
                    let project = source
                        .execution_path
                        .clone()
                        .unwrap_or_else(|| source.project_path.clone());
                    cx.background_executor()
                        .spawn(async move {
                            let executable =
                                executable.ok_or_else(|| "Codex is not installed".to_owned())?;
                            agent_controls::fork_native_thread(&executable, &project, &id)
                                .map(Some)
                                .map_err(|e| e.to_string())
                        })
                        .await
                } else {
                    Ok(None)
                }
            } else {
                Ok(None)
            };
            let _ = this.update_in(cx, |app, window, cx| {
                let native = match native {
                    Ok(id) => id,
                    Err(error) => {
                        app.notice = Some(error);
                        cx.notify();
                        return;
                    }
                };
                let packet =
                    match serde_json::from_value::<handoff::Packet>(value["context"].clone()) {
                        Ok(p) => Arc::new(p),
                        Err(_) => {
                            app.notice = Some(
                                value["error"]
                                    .as_str()
                                    .unwrap_or("Unable to read source history")
                                    .into(),
                            );
                            cx.notify();
                            return;
                        }
                    };
                let mut fork = source.clone();
                fork.id = uuid::Uuid::new_v4().to_string();
                fork.title = format!("{} · branch", short(&source.title, 100));
                fork.updated_at = chrono::Utc::now().timestamp();
                fork.transcript_path = None;
                fork.external_id = native;
                app.engine.bind_handoff(fork.clone(), packet.clone());
                app.engine.set_model(fork.clone(), model.clone());
                app.remember_handoff(fork.id.clone(), packet);
                app.sessions.push(fork.clone());
                if app.generation == generation && app.active.as_deref() == Some(&source.id) {
                    app.open_session(fork, window, cx)
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn attach_daily_entry_points(&mut self, cx: &mut Context<Self>) {
        static ATTACHED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if cfg!(test) || ATTACHED.swap(true, std::sync::atomic::Ordering::AcqRel) {
            return;
        }
        let rx = url_queue().1.clone();
        let window = self.orchestrator_window;
        cx.spawn(async move |_, cx| {
            while let Ok(urls) = rx.recv().await {
                if window
                    .update(cx, |app, window, cx| {
                        app.receive_external_urls(urls, window, cx)
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }
    pub(super) fn prepare_palette_files(&mut self, cx: &mut Context<Self>) {
        let root = self.execution_root().unwrap_or_else(|| self.project.clone());
        if root == Path::new("/") || root == self.unprojected_root() {
            self.palette_files = PaletteFiles::default();
            return;
        }
        if self.palette_files.root == root && (self.palette_files.pending
            || self.palette_files.indexed_at.is_some_and(|time| time.elapsed() < Duration::from_secs(30)))
        {
            return;
        }
        self.palette_files = PaletteFiles { root: root.clone(), pending: true, ..Default::default() };
        cx.spawn(async move |this, cx| {
            let source = root.clone();
            let items = cx.background_executor().spawn(async move {
                walkdir::WalkDir::new(&source).max_depth(12).follow_links(false)
                    .into_iter().filter_entry(|entry| {
                        entry.depth() == 0 || (!entry.file_name().to_string_lossy().starts_with('.')
                            && !matches!(entry.file_name().to_str(), Some("node_modules" | "target" | "build" | "dist" | "vendor")))
                    })
                    .take(8_192).filter_map(Result::ok)
                    .filter(|entry| entry.file_type().is_file())
                    .filter_map(|entry| {
                        let path = entry.into_path();
                        let title = path.strip_prefix(&source).ok()?.to_string_lossy().into_owned();
                        Some((title.to_lowercase(), title, path))
                    }).collect::<Vec<_>>()
            }).await;
            let _ = this.update(cx, |app, cx| {
                if app.palette_files.root == root {
                    app.palette_files = PaletteFiles { root, items, indexed_at: Some(Instant::now()), pending: false };
                    cx.notify();
                }
            });
        }).detach();
    }

    pub(super) fn update_palette_search(&mut self, cx: &mut Context<Self>) {
        self.search_generation += 1;
        self.palette_scroll.scroll_to_item(0,ScrollStrategy::Top);
        if self.modal != Some(Modal::Search) { return; }
        let _span = performance::Span::new("palette_chat_lookup");
        let query = self.search.read(cx).text();
        let query = query.trim().to_lowercase();
        if query.starts_with('>') {
            self.visible_sessions.clear();
            cx.notify();
            return;
        }
        self.visible_sessions = self.sessions.iter().take(4_096)
            .filter(|session| !self.hidden_sessions.contains(&session.id) && !self.draft_sessions.contains(&session.id))
            .filter(|session| query.is_empty() || session.title.to_lowercase().contains(&query)
                || self.lifecycle.chat_names.get(&session.id).is_some_and(|name| name.to_lowercase().contains(&query)))
            .take(60).cloned().collect();
        cx.notify();
        if query.is_empty() { return; }
        let generation = self.search_generation;
        cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_millis(100)).await;
            let _ = this.update(cx, |app, _| {
                if app.search_generation == generation && app.modal == Some(Modal::Search) {
                    app.engine.search(query, generation);
                }
            });
        }).detach();
    }

    pub(super) fn palette_entries(&self, cx: &App) -> Vec<(String, String, Target)> {
        let _span = performance::Span::new("palette_retrieval");
        let query = self.search.read(cx).text();
        let command_mode = query.trim_start().starts_with('>');
        let q = query.trim_start().trim_start_matches('>').trim().to_lowercase();
        if q.is_empty() && !command_mode { return Vec::new(); }
        let mut entries = Vec::with_capacity(64);
        let matches = |key: &str| q.split_whitespace().all(|word| key.contains(word));
        for &(label,section,aliases) in PALETTE_SETTINGS {
            let key=format!("settings {} {aliases}",label.to_lowercase());
            if matches(&key){entries.push((label.into(),"Setting".into(),Target::Setting(section.into())));}
        }
        for preset in theme::Preset::ALL {
            let label=format!("Theme: {}",preset.label());
            if matches(&format!("settings palette preset {}",label.to_lowercase())){
                entries.push((label,"Setting · Palette".into(),Target::Setting("palette".into())));
            }
        }
        for slot in theme::ColorSlot::ALL {
            let label=format!("{} color",slot.label());
            if matches(&format!("settings colors colour {}",label.to_lowercase())){
                entries.push((label,"Setting · Colors".into(),Target::Setting("colors".into())));
            }
        }
        entries.extend(self.palette_native_settings.iter().filter(|(_,key,_)|matches(key))
            .map(|(label,_,id)|(label.clone(),"Notch setting or action".into(),Target::NativeSetting(id.clone()))));
        let mut seen = HashSet::new();
        for &(key, id, label) in keyboard::COMMANDS {
            if (q.is_empty() || label.to_lowercase().contains(&q) || id.contains(&q)) && seen.insert(id) {
                entries.push((label.into(), self.shortcut_overrides.get(key).cloned().unwrap_or_else(|| key.into()), Target::Command(id.into())));
            }
        }
        for (label, target) in [
            ("New terminal", Target::NewTerminal),
            ("New chat in this project", Target::NewProjectChat),
            ("Wallpaper and background", Target::Panel(Modal::Background)),
            ("Permissions and approvals", Target::Panel(Modal::Permissions)),
            ("Composer settings", Target::Panel(Modal::ComposerControls)),
            ("Agent activity", Target::Panel(Modal::Activity)),
            ("Voice settings", Target::Panel(Modal::Realtime)),
            ("Transfer context", Target::Panel(Modal::Handoff)),
        ] {
            if label.to_lowercase().contains(&q) { entries.push((label.into(), "Action".into(), target)); }
        }
        for &(label,aliases,panel) in PALETTE_SETTING_PAGES {
            if matches(&format!("settings {} {aliases}",label.to_lowercase())){
                entries.push((label.into(),"Setting".into(),Target::Panel(panel)));
            }
        }
        if !command_mode {
            entries.extend(self.projects.iter()
                .filter_map(|project| {
                    let label = self.project_label(project);
                    label.to_lowercase().contains(&q).then(|| (label, "Project".into(), Target::Project(project.path.clone())))
                }).take(8));
            entries.extend(self.workflows.iter().filter(|workflow| workflow.name.to_lowercase().contains(&q)).take(4)
                .map(|workflow| (workflow.name.clone(), format!("Workflow · {}", workflow.agent.name()), Target::Workflow(workflow.name.clone()))));
            entries.extend(self.artifact_index.iter().filter_map(|artifact| {
                let (id, title) = (artifact["id"].as_str()?, artifact["title"].as_str()?);
                title.to_lowercase().contains(&q).then(|| (title.into(), "Saved artifact".into(), Target::Artifact(id.into())))
            }).take(4));
            entries.extend(self.recent_results.iter().rev().take(8).filter_map(|id| {
                let chat = self.sessions.iter().find(|session| &session.id == id)?;
                chat.title.to_lowercase().contains(&q).then(|| (chat.title.clone(), "Recent result".into(), Target::Result(id.clone())))
            }).take(3));
            if q.chars().count() > 1 {
                entries.extend(self.palette_files.items.iter().filter(|(key, _, _)| key.contains(&q)).take(10)
                    .map(|(_, name, path)| (name.clone(), "File".into(), Target::File(self.palette_files.root.clone(), path.clone()))));
            }
        }
        entries
    }
    pub(super) fn palette_view(&self, entries: Vec<(String, String, Target)>, cx: &Context<Self>) -> AnyElement {
        let count=entries.len();
        uniform_list("palette-settings-and-actions",count,cx.processor(move |app,range:std::ops::Range<usize>,_,cx|{
            range.filter_map(|i|{
                let (label,detail,target)=entries.get(i)?.clone();
                Some(div().id(("palette-command",i)).h(px(48.)).px_2().py_1().rounded_md()
                    .cursor_pointer().hover(|s|s.bg(rgba(GLASS_HOVER)))
                    .when(i==app.palette_selection,|s|s.bg(rgba(GLASS_HOVER)))
                    .child(div().text_size(theme::ui_px(12.)).truncate().child(label))
                    .child(div().text_size(theme::ui_px(10.)).text_color(rgb(MUTED)).child(detail))
                    .on_click(cx.listener(move |app,_,window,cx|app.palette_choose(target.clone(),window,cx))))
            }).collect()
        })).track_scroll(self.palette_scroll.clone()).h(px(48.*count.min(8) as f32)).w_full().into_any_element()
    }
    pub(super) fn palette_move(&mut self, delta: isize, cx: &mut Context<Self>) {
        let count = self.palette_entries(cx).len() + self.visible_sessions.len();
        if count > 0 {
            self.palette_selection =
                (self.palette_selection as isize + delta).rem_euclid(count as isize) as usize;
        }
        self.palette_scroll.scroll_to_item(self.palette_selection.min(self.palette_entries(cx).len().saturating_sub(1)),ScrollStrategy::Center);
        cx.notify();
    }
    pub(super) fn palette_accept(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let entries = self.palette_entries(cx);
        if let Some((_, _, target)) = entries.get(self.palette_selection) {
            self.palette_choose(target.clone(), window, cx)
        } else if let Some(chat) = self
            .visible_sessions
            .get(self.palette_selection.saturating_sub(entries.len()))
            .cloned()
        {
            self.dismiss_panel(window, cx);
            self.open_session(chat, window, cx);
        }
    }
    fn palette_choose(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        self.dismiss_panel(window, cx);
        match target {
            Target::Command(id) => self.execute_shortcut(&id, window, cx),
            Target::Panel(panel) => self.show_modal(panel, window, cx),
            Target::Setting(query) => {
                self.show_modal(Modal::Settings,window,cx);
                if let Some(input)=&self.appearance_search {input.update(cx,|input,cx|input.set_text(&query,cx));}
            }
            Target::NativeSetting(id) => unified::emit(&json!({"type":"nativeMenuAction","id":id})),
            Target::NewTerminal => self.new_terminal(window, cx),
            Target::NewProjectChat => self.new_conversation(window, cx),
            Target::File(root, path) => {
                if self.execution_root().unwrap_or_else(|| self.project.clone()) != root { return; }
                if !self.files_visible { self.toggle_files(window, cx); }
                if let Some(browser) = &self.file_browser {
                    browser.update(cx, |browser, cx| browser.open_citation(citations::FileCitation {
                        path, line: None, end_line: None,
                    }, cx));
                }
            }
            Target::Project(path) => self.select_project(path, window, cx),
            Target::Workflow(name) => {
                if let Err(error) = self.run_named_workflow(&name, window, cx) {
                    self.notice = Some(error)
                }
            }
            Target::Artifact(id) => self.open_saved_artifact(&id, window, cx),
            Target::Result(id) => {
                if let Some(chat) = self.sessions.iter().find(|s| s.id == id).cloned() {
                    self.open_session(chat, window, cx)
                }
            }
        }
    }
    pub(super) fn workflow_command(
        &mut self,
        args: &Value,
        grant: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        let allowed=|project:&PathBuf|grant["projects"].as_array().is_none_or(|roots|roots.is_empty()||roots.iter().any(|p|p.as_str()==project.to_str()));
        match args["action"].as_str().unwrap_or("") {
            "list_workflows" => Ok(
                json!({"workflows":self.workflows.iter().filter(|w|grant["projects"].as_array().is_none_or(|p|p.is_empty()||p.iter().any(|p|p.as_str()==w.project.to_str()))).collect::<Vec<_>>()}),
            ),
            "save_workflow" => {
                if grant["scope"] != "control" {
                    return Err("Read-only client".into());
                }
                let workflow = Workflow {
                    name: args["name"].as_str().ok_or("Name this workflow")?.into(),
                    project: PathBuf::from(args["project"].as_str().ok_or("Select a project")?),
                    agent: self
                        .agents
                        .iter()
                        .find(|a| {
                            a.available
                                && a.kind.command() == args["agent"].as_str().unwrap_or("codex")
                        })
                        .map(|a| a.kind)
                        .ok_or("Choose an installed provider")?,
                    model: args["model"].as_str().map(str::to_owned),
                    effort: args["effort"].as_str().map(str::to_owned),
                    prompt: args["prompt"].as_str().ok_or("Supply a prompt")?.into(),
                };
                if !workflow.valid() || !self.projects.iter().any(|p| p.path == workflow.project) {
                    return Err("Invalid workflow or unknown project".into());
                }
                if !allowed(&workflow.project) || self.workflows.iter().any(|old|old.name==workflow.name&&!allowed(&old.project)){
                    return Err("Workflow is outside this grant; a name from another project cannot be replaced".into());
                }
                if self.workflows.len() >= 32
                    && !self.workflows.iter().any(|w| w.name == workflow.name)
                {
                    return Err("Workflow capacity reached".into());
                }
                self.workflows.retain(|w| w.name != workflow.name);
                self.workflows.push(workflow);
                self.save_workspace_now(cx);
                Ok(json!({"saved":true}))
            }
            "run_workflow" => {
                if grant["scope"] != "control" {
                    return Err("Read-only client".into());
                }
                let name = args["name"].as_str().ok_or("Choose a workflow")?;
                if self.workflows.iter().find(|workflow|workflow.name==name).is_some_and(|workflow|!allowed(&workflow.project)){
                    return Err("Workflow is outside this grant".into());
                }
                self.run_named_workflow(name, window, cx)?;
                Ok(json!({"accepted":true,"completion":"Observe task status and read its result"}))
            }
            _ => Err("Unknown workflow action".into()),
        }
    }
    pub(super) fn run_named_workflow(
        &mut self,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let workflow = self
            .workflows
            .iter()
            .find(|w| w.name == name)
            .cloned()
            .ok_or("Unknown named workflow")?;
        if !workflow.valid() {
            return Err("Saved workflow is invalid".into());
        }
        let key = (workflow.agent, workflow.project.clone());
        let needs_catalog = (workflow.model.is_some() || workflow.effort.is_some())
            && self
                .orchestration_models
                .get(&key)
                .is_none_or(|(at, _)| at.elapsed() > Duration::from_secs(300));
        if needs_catalog {
            if self.orchestration_probes.len() >= 4
                || !self.orchestration_probes.insert(key.clone())
            {
                return Err("This workflow's provider is already connecting".into());
            }
            let executable = self
                .agents
                .iter()
                .find(|a| a.kind == workflow.agent && a.available)
                .and_then(|a| a.executable.clone())
                .ok_or("Workflow provider is not installed")?;
            let home = self.controls_home.clone();
            let original = workflow.clone();
            cx.spawn_in(window, async move |this, cx| {
                let info = cx
                    .background_executor()
                    .spawn(async move {
                        agent_controls::inspect_model_project(
                            workflow.agent,
                            &executable,
                            &home,
                            &workflow.project,
                            workflow.model.as_deref(),
                        )
                    })
                    .await;
                let _ = this.update_in(cx, |app, window, cx| {
                    app.orchestration_probes.remove(&key);
                    app.orchestration_models.insert(key, (Instant::now(), info));
                    if app.workflows.iter().any(|w| w == &original) {
                        if let Err(error) = app.run_named_workflow(&original.name, window, cx) {
                            app.notice = Some(error)
                        }
                    }
                    cx.notify();
                });
            })
            .detach();
            return Ok(());
        }
        let hash: [u8; 32] = sha2::Sha256::digest(
            format!("workflow:{}:{}", workflow.project.display(), workflow.name).as_bytes(),
        )
        .into();
        let owner = uuid::Uuid::from_bytes(hash[..16].try_into().unwrap()).to_string();
        let args = json!({"action":"create_chat","project":workflow.project,"agent":workflow.agent.command(),"name":workflow.name,"prompt":workflow.prompt,"model":workflow.model,"effort":workflow.effort});
        let mut args = args;
        for key in ["model", "effort"] {
            if args[key].is_null() {
                args.as_object_mut().unwrap().remove(key);
            }
        }
        // Validate exact selected capabilities before claiming acceptance.
        self.orchestration_selection(&args, workflow.agent, &workflow.project, None)?;
        self.remote.policies.borrow_mut().insert(
            owner.clone(),
            orchestrator::Policy {
                project: workflow.project,
                model: None,
                allow_edits: self.allow_edits,
                codex_access: self.codex_access,
                codex_approvals: self.codex_approvals,
            },
        );
        self.workspace_tool(&owner, "workflow", "code_workspace", args, window, cx);
        Ok(())
    }
    fn receive_external_urls(
        &mut self,
        urls: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut files = vec![];
        for url in urls.into_iter().take(64) {
            if let Some(name) = url.strip_prefix("kai://workflow/") {
                let name = decode(name);
                if let Err(error) = self.run_named_workflow(&name, window, cx) {
                    self.notice = Some(error)
                }
                continue;
            }
            if let Some(id) = url.strip_prefix("kai://artifact/") {
                self.open_saved_artifact(id, window, cx);
                continue;
            }
            if let Some(path) = url.strip_prefix("file://") {
                files.push(PathBuf::from(decode(path)));
            }
        }
        if files.is_empty() {
            return;
        }
        let projects: Vec<_> = self.projects.iter().map(|p| p.path.clone()).collect();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { route_files(&files, &projects) })
                .await;
            let _ = this.update_in(cx, |app, window, cx| match result {
                Ok((project, files)) => {
                    // Preserve each existing draft; selecting a project uses normal navigation.
                    if !app.projects.iter().any(|p| p.path == project) {
                        let p = Project {
                            name: project
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .into(),
                            path: project.clone(),
                        };
                        app.preferences.add(p.clone());
                        app.custom_projects.push(p);
                        app.rebuild_projects();
                    }
                    app.select_project(project, window, cx);
                    if let Some(path) = files.first() {
                        let _ = app.open_citation(&path.to_string_lossy(), window, cx);
                    }
                    app.unified_notch_sources = files.clone();
                    app.notice = Some(format!(
                        "Opened {} selected file{} in its project",
                        files.len(),
                        if files.len() == 1 { "" } else { "s" }
                    ));
                    if unified::active() {
                        unified::reveal()
                    }
                    cx.notify();
                }
                Err(error) => {
                    app.notice = Some(error);
                    cx.notify();
                }
            });
        })
        .detach();
    }
    pub(super) fn open_saved_artifact(
        &mut self,
        id: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let directory = self.workflow_dir.join("artifacts");
        let project = self.project.clone();
        let id = id.to_owned();
        cx.spawn(async move |this, cx| {
            let value = cx
                .background_executor()
                .spawn(async move {
                    artifacts::command(
                        &directory,
                        &json!({"action":"read_artifact","id":id}),
                        &project,
                    )
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                match value {
                    Ok(value) => unified::emit(&json!({"type":"artifactPreview","artifact":value})),
                    Err(error) => app.notice = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }
    pub(super) fn workspace_artifact_reply(&self, owner: &str, token: &str, value: Value) {
        self.workspace_reply(owner, token, value);
    }
}
pub(super) fn decode(text: &str) -> String {
    let mut b = vec![];
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (
                (bytes[i + 1] as char).to_digit(16),
                (bytes[i + 2] as char).to_digit(16),
            ) {
                b.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        b.push(bytes[i]);
        i += 1
    }
    String::from_utf8_lossy(&b).into()
}
pub(super) fn route_files(
    files: &[PathBuf],
    projects: &[PathBuf],
) -> Result<(PathBuf, Vec<PathBuf>), String> {
    let files: Vec<_> = files
        .iter()
        .map(|p| p.canonicalize().map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;
    let first = files.first().ok_or("No files selected")?;
    let folder = if first.is_dir() {
        first.clone()
    } else {
        first.parent().ok_or("No parent folder")?.into()
    };
    let project = projects
        .iter()
        .filter_map(|p| p.canonicalize().ok())
        .filter(|p| folder.starts_with(p))
        .max_by_key(|p| p.components().count())
        .unwrap_or_else(|| {
            folder
                .ancestors()
                .find(|p| p.join(".git").exists())
                .map(PathBuf::from)
                .unwrap_or(folder)
        });
    if files.iter().any(|p| !p.starts_with(&project)) {
        return Err("Select files from one project at a time".into());
    }
    Ok((project, files.into_iter().filter(|p| p.is_file()).collect()))
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    #[test]
    fn finder_uses_deepest_project_and_preserves_percent_names() {
        let root = std::env::temp_dir().join(format!("kai-finder-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("nested")).unwrap();
        let file = root.join("nested/100% done.txt");
        std::fs::write(&file, "x").unwrap();
        let (p, files) =
            route_files(&[file.clone()], &[root.clone(), root.join("nested")]).unwrap();
        assert_eq!(p, root.join("nested").canonicalize().unwrap());
        assert_eq!(files.len(), 1);
        assert_eq!(decode("100%25%20done.txt"), "100% done.txt");
        let _ = std::fs::remove_dir_all(root);
    }
}

#[cfg(test)]
mod ui_tests {
    use super::*;
    use std::prelude::v1::test;
    #[gpui::test]
    fn remote_workflow_names_cannot_replace_a_workflow_in_another_project(cx:&mut TestAppContext){
        let(view,cx)=crate::ui_tests::fixture(cx);
        view.update_in(cx,|app,window,cx|{
            app.agents.iter_mut().find(|agent|agent.kind==AgentKind::Codex).unwrap().available=true;
            let root=app.projects[0].path.clone();let outside=root.with_file_name("other-project");
            app.workflows.push(Workflow{name:"Check build".into(),project:outside.clone(),agent:AgentKind::Codex,model:None,effort:None,prompt:"Original workflow".into()});
            let grant=json!({"scope":"control","projects":[root]});
            let args=json!({"action":"save_workflow","name":"Check build","project":root,"agent":"codex","prompt":"Replacement"});
            assert!(app.workflow_command(&args,&grant,window,cx).unwrap_err().contains("another project"));
            assert_eq!(app.workflows[0].project,outside);assert_eq!(app.workflows[0].prompt,"Original workflow");
        });
    }
    #[gpui::test]
    fn palette_keyboard_accepts_the_selected_command_and_keeps_the_draft(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update_in(cx,|app,window,cx|{
   app.composer.update(cx,|input,cx|input.set_text("Keep this draft",cx));app.show_modal(Modal::Search,window,cx);app.search.update(cx,|input,cx|input.set_text("Focus message",cx));
   assert!(matches!(app.palette_entries(cx).first(),Some((_,_,Target::Command(id)))if id=="composer"));
   app.palette_accept(window,cx);assert!(app.modal.is_none());assert_eq!(app.composer.read(cx).text(),"Keep this draft");
   app.search.update(cx,|input,cx|input.set_text("Fixture conversation",cx));app.visible_sessions=app.sessions.clone();app.palette_selection=0;
   app.palette_move(-1,cx);let count=app.palette_entries(cx).len()+app.visible_sessions.len();assert_eq!(app.palette_selection,count-1);
  });
    }
}

const PALETTE_SETTINGS: &[(&str,&str,&str)] = &[
    ("Appearance mode","appearance","system dark light theme"),
    ("Theme palette","palette","preset harbor moss ember orchid tidepool dune rosewood glacier arcade ink"),
    ("Interface font","Typography","font family system installed typeface"),
    ("Code font","Typography","monospace terminal editor typeface"),
    ("Interface font size","Typography","text size typography"),
    ("Code font size","Typography","monospace size typography"),
    ("Transparent window","glass","transparency opaque"),
    ("Sidebar glass","glass","transparency"),
    ("Main pane glass","glass","transparency"),
    ("Blur radius","glass","frost gaussian blur amount"),
    ("Window opacity","glass","transparent opacity glass"),
    ("Native glass material","material","liquid automatic classic blur"),
    ("Sidebar pull request widget","GitHub","github source control"),
    ("Horizontal agent picker","picker","provider model columns layout"),
    ("Adaptive text colors","adaptive","contrast automatic light dark"),
    ("Local screen sampling permission","adaptive","contrast screen access"),
    ("Compact diff mode","compact","response changes edited files footer"),
    ("Borderless prompt controls","composer","invisible frames outlines buttons"),
    ("Wallpaper through side panes","wallpaper surfaces","sidebar files transparency"),
    ("Wallpaper through top and bottom bars","wallpaper surfaces","chrome transparency"),
    ("High contrast","contrast","accessibility standard"),
    ("Diff colors","diff","colorblind blue orange red green"),
    ("Import theme","theme","load appearance"),
    ("Export theme","theme","save appearance"),
    ("Reset appearance","appearance","defaults theme"),
    ("Open at startup","startup","launch compact extended notch workspace"),
    ("Pane and tab motion","motion","animation quick standard relaxed speed"),
    ("Opening animation only on first open","opening","once entrance replay"),
    ("Automatic chat titles","titles","names naming"),
    ("Terminal engine","terminal","ghostty metal vt alacritty emulator shell"),
    ("Quick Chat shortcut","Quick Chat","global hotkey keyboard"),
    ("Keyboard shortcuts","keyboard","bindings commands remap"),
    ("GitHub & source control","GitHub","repository branches credentials"),
    ("Remote access & ChatGPT","remote","host tailscale ssh mobile connection transport"),
];

const PALETTE_SETTING_PAGES: &[(&str,&str,Modal)] = &[
    ("Choose wallpaper","background image gif import",Modal::Background),
    ("Default Bonsai wallpaper","background reset image",Modal::Background),
    ("Wallpaper effect","background none dither ascii halftone scanlines haze",Modal::Background),
    ("Wallpaper fade direction","background classic up down full",Modal::Background),
    ("Wallpaper coverage","background surface size",Modal::Background),
    ("Wallpaper opacity in empty chats","background transparency",Modal::Background),
    ("Wallpaper opacity in sessions","background transparency",Modal::Background),
    ("Wallpaper project or global scope","background apply all workspaces",Modal::Background),
    ("Remove wallpaper","background delete clear",Modal::Background),
    ("Workspace access","permissions read only plan workspace full access unrestricted",Modal::Permissions),
    ("Approval policy","permissions ask me auto review never ask",Modal::Permissions),
    ("Provider permission mode","permissions codex claude cursor gemini antigravity",Modal::Permissions),
    ("Parallel workspace mode","permissions worktrees projects isolated",Modal::Permissions),
    ("Realtime voice","audio model live talking microphone",Modal::Realtime),
    ("Realtime mute microphone","audio input mute",Modal::Realtime),
    ("Realtime interrupt","voice stop speech",Modal::Realtime),
    ("Realtime voice selection","audio voice speaker",Modal::Realtime),
    ("Composer model","provider ai model picker",Modal::ComposerControls),
    ("Thinking intensity","reasoning effort low medium high extra",Modal::ComposerControls),
    ("Fast mode","service tier speed priority",Modal::ComposerControls),
    ("Orchestrator mode","orchestration subagents delegation",Modal::ComposerControls),
    ("Composer context","files folders references attachments",Modal::ComposerControls),
    ("Continue with another provider","handoff transfer context",Modal::Handoff),
    ("Usage and context window","quota tokens limit status account",Modal::Status),
];
