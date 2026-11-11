//! One command catalog for bindings, native-notch routing and discoverable help.
use super::*;
use serde_json::json;
#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(namespace = kai, no_json)]
pub struct Execute {
    pub id: String,
}
pub(super) const MAX_CHAT_TABS:usize=9;
pub(super) fn tab_index(id:&str)->Option<usize>{id.strip_prefix("tab_")?.parse::<usize>().ok().filter(|n|(1..=MAX_CHAT_TABS).contains(n)).map(|n|n-1)}
pub const COMMANDS: &[(&str, &str, &str)] = &[
    ("ctrl-t", "new", "New chat"),
    ("cmd-t", "new", "New chat"),
    ("cmd-n", "new", "New chat"),
    ("cmd-e", "conversations", "Conversation sidebar"),
    ("cmd-f", "files", "Files"),
    ("cmd-shift-e", "files", "Files"),
    ("cmd-d", "split_beside", "Split pane right"),
    ("cmd-shift-d", "split_below", "Split pane below"),
    ("cmd-k", "search", "Search chats"),
    ("cmd-b", "sidebar", "Project sidebar"),
    ("cmd-s", "search", "Commands and files"),
    ("cmd-,", "settings", "Settings"),
    ("cmd-1", "tab_1", "Select chat tab 1"),
    ("cmd-2", "tab_2", "Select chat tab 2"),
    ("cmd-3", "tab_3", "Select chat tab 3"),
    ("cmd-4", "tab_4", "Select chat tab 4"),
    ("cmd-5", "tab_5", "Select chat tab 5"),
    ("cmd-6", "tab_6", "Select chat tab 6"),
    ("cmd-7", "tab_7", "Select chat tab 7"),
    ("cmd-8", "tab_8", "Select chat tab 8"),
    ("cmd-9", "tab_9", "Select chat tab 9"),

    ("ctrl-w", "close_tab", "Close tab"),
    ("cmd-w", "close_tab", "Close tab"),
    ("ctrl-l", "composer", "Focus message"),
    ("ctrl-shift-l", "conversation", "Focus conversation"),
    ("ctrl-m", "model", "Choose agent and model"),
    ("ctrl-shift-m", "thinking", "Thinking level"),
    ("ctrl-p", "projects", "Switch project"),
    ("ctrl-shift-p", "fold_project", "Fold project"),
    ("ctrl-k", "search", "Search chats"),
    ("ctrl-b", "sidebar", "Project sidebar"),
    ("ctrl-shift-b", "conversations", "Conversation sidebar"),
    ("ctrl-e", "files", "Files"),
    ("ctrl-g", "changes", "Source control"),
    ("ctrl-shift-g", "github", "GitHub repositories and pull requests"),
    ("cmd-shift-j", "terminalize", "Switch chat and CLI"),
    ("ctrl-j", "terminal", "Toggle terminal"),
    (
        "ctrl-shift-j",
        "dock_terminal",
        "Move terminal to next edge",
    ),
    ("ctrl-u", "usage", "Context and usage"),
    ("ctrl-o", "orchestrator", "Toggle orchestration"),
    ("ctrl-r", "dictate", "Dictate"),
    ("ctrl-shift-r", "voice", "Real-time voice"),
    ("ctrl-shift-a", "children", "Subagents"),
    ("ctrl-f", "fold_work", "Fold latest work"),
    ("ctrl-n", "next_tab", "Next tab"),
    ("ctrl-shift-n", "previous_tab", "Previous tab"),
    ("ctrl-shift-t", "next_project", "Next project"),
    ("ctrl-shift-y", "previous_project", "Previous project"),
    ("ctrl-shift-x", "stop", "Stop agent"),
    ("ctrl-shift-h", "keyboard", "Keyboard shortcuts"),
    ("cmd-shift-o", "open_project", "Open folder"),
    ("cmd-shift-n", "new", "New chat"),
    ("cmd-shift-k", "notch", "Return to notch"),
];
pub(super) fn split_command(command: &str) -> bool {
    matches!(command, "split_add" | "split_beside" | "split_below" | "split_left" | "split_right" | "split_up" | "split_down")
}

pub fn bind(cx: &mut App) {
    cx.bind_keys(COMMANDS.iter().map(|(key, id, _)| {
        KeyBinding::new(
            key,
            Execute { id: (*id).into() },
            Some(if key.starts_with("ctrl-") && !key.starts_with("ctrl-shift-") { "Orbit && !CodeTerminal" } else { "Orbit" }),
        )
    }));
}
pub fn catalog() -> serde_json::Value {
    serde_json::json!(
        COMMANDS
            .iter()
            .map(|(key, id, label)| serde_json::json!({"key":key,"id":id,"label":label}))
            .collect::<Vec<_>>()
    )
}
pub(super) const CLI_DRAFT: &str = "workspace-draft-cli";

pub(super) struct ChatCli {
    pub launch: Option<(std::path::PathBuf, Vec<String>, Vec<(String, String)>)>,
    pub agent: AgentKind,
    pub retained: Option<Box<dyn std::marker::Send>>,
    pub preparing: bool,
    pub launch_id: uuid::Uuid,
    pub fallback: Option<(Entity<terminal_ui::TerminalPanel>, Subscription)>,
    pub selected: bool,
    pub metal_visible: bool,
    pub resumes_chat: bool,
    pub cursor_history: Option<cursor_cli_history::Sync>,
    pub send_when_closed: Option<String>,
}

impl Orbit {
    pub(super) fn chat_cli_owner(&self) -> Option<std::borrow::Cow<'_, str>> {
        Some(self.ninja_surface_owner().map_or_else(
            || std::borrow::Cow::Borrowed(self.active.as_deref().unwrap_or(CLI_DRAFT)),
            std::borrow::Cow::Owned,
        ))
    }

    pub(super) fn active_chat_cli(&self) -> Option<&ChatCli> {
        self.chat_clis.get(self.chat_cli_owner()?.as_ref())
    }

    pub(super) fn chat_cli_canvas(&self, owner: &str) -> impl IntoElement {
        let owner = owner.to_owned();
        canvas(|bounds, _, _| bounds, move |_, bounds, _, _| unified::chat_cli_slot(&owner, bounds))
            .absolute().inset_0()
    }

    pub(super) fn render_chat_cli(&self, owner: &str, cx: &Context<Self>) -> Option<AnyElement> {
        let cli = self.chat_clis.get(owner).filter(|cli| cli.selected)?;
        if let Some((panel, _)) = &cli.fallback {
            let owner = owner.to_owned();
            Some(self.conversation_pane().key_context("ChatCli")
                .child(div().h(px(32.)).px_4().flex().items_center().justify_between()
                    .child(format!("{} CLI · Ghostty VT", cli.agent.name()))
                    .child(div().id("chat-cli-back").cursor_pointer().child("Back to Chat")
                        .on_click(cx.listener(move |app, _, window, cx| {
                            if app.chat_cli_owner().as_deref() == Some(owner.as_str()) || app.focus_split_chat(&owner, window, cx) {
                                app.terminalize_chat(window, cx);
                            }
                        }))))
                .child(div().flex_1().min_h(px(0.)).child(panel.clone())).into_any_element())
        } else if cli.metal_visible {
            Some(self.conversation_pane().child(self.chat_cli_canvas(owner)).into_any_element())
        } else {
            None
        }
    }

    pub(super) fn close_chat_cli(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(mut cli) = self.chat_clis.remove(id) {
            if let Some(history) = cli.cursor_history.take().filter(|_| id != CLI_DRAFT) {
                self.engine.import_cursor_cli(id.to_owned(), history);
            }
            if let Some((panel, _)) = cli.fallback.take() {
                panel.update(cx, |panel, _| panel.stop());
            }
            if let Some(retained) = cli.retained.take() {
                cx.background_executor().spawn(async move { drop(retained); }).detach();
            }
            unified::emit(&json!({"type":"chatTerminalClose","id":id,"launchId":cli.launch_id}));
        }
    }

    pub(super) fn finish_chat_cli_for_send(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.chat_cli_owner().map(|owner| owner.into_owned()) else { return; };
        let draft = self.composer.read(cx).text();
        if draft.trim().is_empty() && self.attachments.is_empty() { return; }
        let Some(cli) = self.chat_clis.get_mut(&id) else { return; };
        if cli.send_when_closed.is_some() { return; }
        if cli.preparing || !cli.metal_visible && cli.fallback.is_none() {
            self.notice = Some("The CLI is still opening. Your draft is saved.".into());
            cx.notify();
            return;
        }
        cli.send_when_closed = Some(draft);
        if let Some((panel, _)) = &cli.fallback {
            panel.update(cx, |panel, cx| panel.close(cx));
        } else {
            unified::emit(&json!({"type":"chatTerminalClose","id":id,"launchId":cli.launch_id,"acknowledge":true}));
        }
        cx.notify();
    }

    pub(super) fn terminalize_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.active.is_none() && self.ninja_surface_owner().is_none()
            && self.chat_clis.get(CLI_DRAFT).is_some_and(|cli| cli.cursor_history.is_some())
        {
            self.preserve_blank_tab(cx);
            self.sync_chat_terminal_owner(cx);
        }
        let Some(id) = self.chat_cli_owner().map(|owner| owner.into_owned()) else { return; };
        let personal = self.ninjas.panel.as_ref().filter(|panel| panel.profile.revision > 0);
        if let Some(panel) = personal {
            let host = self.ninjas.hosts.iter().find(|host| host.id == panel.host);
            if !host.is_some_and(|host| host.endpoint.is_none()) {
                self.notice = Some("This Ninja runs on another host. Its current connection provides task controls; open its workspace on that host to use the CLI.".into());
                cx.notify();
                return;
            }
            if !self.chat_clis.contains_key(&id) && (panel.history_loading || panel.busy || panel.messages.iter().any(ninja_chat::live)) {
                self.notice = Some("Wait for this Ninja’s active task and history load to finish before opening its CLI.".into());
                cx.notify();
                return;
            }
        }
        let agent = personal.map_or(self.agent, |panel| panel.profile.agent);
        self.notice = None;
        if self.chat_clis.get(&id).is_some_and(|cli| cli.agent != agent) {
            self.close_chat_cli(&id, cx);
        }
        if let Some(cli) = self.chat_clis.get_mut(&id) {
            if cli.send_when_closed.is_some() { return; }
            if cli.preparing {
                self.close_chat_cli(&id, cx);
                cx.notify();
                return;
            }
            cli.selected = !cli.selected;
            if let Some((panel, _)) = &cli.fallback {
                panel.update(cx, |panel, _| panel.set_visible(cli.selected));
                if cli.selected { panel.read(cx).focus_handle(cx).focus(window); }
            } else {
                unified::emit(&json!({"type":"chatTerminalToggle","id":id,"launchId":cli.launch_id,"selected":cli.selected}));
            }
            if !cli.selected {
                let resumes = cli.resumes_chat;
                if let Some(history) = cli.cursor_history.clone() {
                    self.engine.import_cursor_cli(id.clone(), history);
                }
                if resumes && let Some(session) = self.sessions.iter().find(|session| session.id == id).cloned() {
                    self.open_session(session, window, cx);
                }
                self.composer.read(cx).focus_handle(cx).focus(window);
            }
            cx.notify();
            return;
        }
        if self.quit_pending || (self.ninja_surface_owner().is_none() && self.starting_current()) || self.runs.contains_key(&id)
            || self.settling.contains_key(&id) || self.finalizing.contains(&id)
            || self.voice_call.as_ref().is_some_and(|call| call.owner == id)
            || self.activity.pending_rank(&id).is_some()
        {
            self.notice = Some("Wait for the current turn to finish before switching to the CLI.".into());
            cx.notify();
            return;
        }
        let personal = self.ninjas.panel.as_ref().filter(|panel| panel.profile.revision > 0);
        let session = self.active.as_ref().and_then(|id| self.sessions.iter().find(|session| &session.id == id));
        let ninja_selection = if let Some(panel) = personal {
            (agent == AgentKind::Ninja).then(|| panel.profile.ninja.clone()).flatten()
        } else {
            (agent == AgentKind::Ninja).then(|| self.ninja_selection())
        };
        let same_provider = session.is_some_and(|session| session.agent == agent)
            && ninja_selection.as_ref().is_none_or(|next| self.active.as_ref()
                .and_then(|id| self.ninja.bound.get(id)).is_none_or(|bound| bound.harness == next.harness));
        let external = if let Some(panel) = personal {
            panel.messages.iter().rev().filter(|task| task["finished"].is_number()).find_map(|task| {
                let profile: ninjas::Profile = serde_json::from_value(task["profile"].clone()).ok()?;
                let compatible = profile.agent == agent && profile.project == panel.profile.project
                    && profile.ninja.as_ref().map(|route| route.harness) == panel.profile.ninja.as_ref().map(|route| route.harness);
                compatible.then(|| task["native_id"].as_str().map(str::to_owned)).flatten()
            })
        } else {
            session.filter(|_| same_provider).and_then(|session| session.external_id.clone())
        };
        let directory = personal.map_or_else(|| self.execution_root().unwrap_or_else(|| self.project.clone()), |panel| panel.profile.project.clone());
        let create_workspace = personal.is_none() && directory.parent() == Some(self.unprojected_root().as_path())
            && directory.file_name().and_then(|name| name.to_str()).is_some_and(|name| uuid::Uuid::parse_str(name).is_ok());
        let executable = self.agents.iter().find(|provider| provider.kind == agent)
            .and_then(|provider| provider.executable.clone());
        let home = self.controls_home.clone();
        let model = personal.map_or_else(|| self.model.clone(), |panel| panel.profile.model.clone());
        let title = personal.map_or_else(|| session.map_or_else(|| format!("New {} session", agent.name()), |session| session.title.clone()), |panel| panel.profile.name.clone());
        let session_can_bind = session.is_none_or(|session| session.agent == agent);
        let cursor_lookup = (agent == AgentKind::Cursor && personal.is_none() && session_can_bind)
            .then(|| session.map(|session| self.engine.cursor_cli_id(session.id.clone())))
            .flatten();
        let personal = personal.is_some();
        let launch_id = uuid::Uuid::new_v4();
        if id == CLI_DRAFT {
            self.cli_draft_project = Some((self.project.clone(), self.new_execution_path.clone(), agent));
        }
        self.chat_clis.insert(id, ChatCli {
            launch: None, agent, retained: None, preparing: true, launch_id,
            fallback: None, selected: true, metal_visible: false, resumes_chat: external.is_some(), cursor_history: None, send_when_closed: None,
        });
        if personal { self.remember_ninja_draft(cx); }
        else { self.remember_draft(cx); }
        cx.notify();
        cx.spawn(async move |this, cx| {
            let cursor_id = match cursor_lookup {
                Some(lookup) => lookup.recv().await.unwrap_or_else(|_| Err("Could not read the saved Cursor CLI session.".into())),
                None => Ok(None),
            };
            let prepared = cx.background_executor().spawn(async move {
                if create_workspace {
                    std::fs::create_dir_all(&directory).map_err(|error| format!("Could not create the chat workspace: {error}"))?;
                }
                let executable = if let Some(selection) = &ninja_selection {
                    ninja::executable(selection.harness, &home)
                } else {
                    executable.or_else(|| discovery::discover_agent(agent, &home).executable)
                }.ok_or_else(|| format!("{} CLI is unavailable.", agent.name()))?;
                let history = if agent == AgentKind::Cursor && !personal && session_can_bind {
                    let id = match cursor_id? {
                        Some(id) => id,
                        None => {
                            let mut command = std::process::Command::new(&executable);
                            command.current_dir(&directory).arg("create-chat");
                            let bytes = ninja::http::run(command, None, Duration::from_secs(8))
                                .map_err(|error| format!("Could not create the Cursor chat: {error}"))?;
                            String::from_utf8_lossy(&bytes).trim().to_owned()
                        }
                    };
                    Some(cursor_cli_history::Sync::start(&home, &directory, id)
                        .map_err(|error| format!("Could not open Cursor chat history: {error}"))?)
                } else { None };
                let resume = history.as_ref().map(|history| history.id.as_str())
                    .or_else(|| (agent != AgentKind::Cursor || personal).then_some(external.as_deref()).flatten());
                provider_cli::prepare(agent, &executable, &directory, resume, model.as_deref(), ninja_selection.as_ref())
                    .map(|prepared| (directory, prepared, history))
            }).await;
            // Keep the prepared lease outside the update closure. If the entity
            // disappeared, dropping the unused closure must not join a gateway
            // process on the UI thread.
            let mut pending = Some(prepared);
            let _ = this.update(cx, |app, cx| {
                // A blank tab can become a saved local tab during preparation.
                // Resolve by launch instance instead of the original owner key.
                let owner = app.chat_clis.iter().find(|(_, cli)| cli.launch_id == launch_id).map(|(id, _)| id.clone());
                let Some(owner) = owner else { return; };
                let Some(prepared) = pending.take() else { return; };
                match prepared {
                    Ok((directory, prepared, history)) => {
                        let provider_cli::PreparedLaunch { arguments, environment, retained } = prepared;
                        if let Some(history) = &history && owner != CLI_DRAFT {
                            app.engine.bind_cursor_cli(owner.clone(), history.id.clone());
                        }
                        let cli = app.chat_clis.get_mut(&owner).unwrap();
                        cli.resumes_chat |= history.is_some();
                        cli.cursor_history = history;
                        cli.preparing = false;
                        cli.retained = retained;
                        cli.launch = Some((directory.clone(), arguments.clone(), environment.clone()));
                        unified::emit(&json!({"type":"chatTerminalOpen","id":owner,"launchId":launch_id,
                            "arguments":arguments,"environment":environment,"directory":directory,
                            "title":title,"agent":agent.name()}));
                    }
                    Err(error) => {
                        app.chat_clis.remove(&owner);
                        if app.chat_cli_owner().as_deref() == Some(owner.as_str()) { app.notice = Some(error); }
                    }
                }
                cx.notify();
            });
            if let Some(prepared) = pending {
                cx.background_executor().spawn(async move { drop(prepared); }).detach();
            }
        }).detach();
    }

    pub(super) fn open_cli_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((project, execution, agent)) = self.cli_draft_project.clone() else { return; };
        self.reset_conversation(cx);
        self.project = project;
        self.new_execution_path = execution;
        self.agent = agent;
        self.model = self.agent_models.get(agent.command()).cloned().flatten();
        self.attachments = self.attachment_drafts.get(CLI_DRAFT).cloned().unwrap_or_default();
        let draft = self.drafts.get(CLI_DRAFT).cloned().unwrap_or_default();
        self.composer.update(cx, |input, cx| input.set_text(&draft, cx));
        if let Some((panel, _)) = self.active_chat_cli().filter(|cli| cli.selected).and_then(|cli| cli.fallback.as_ref()) {
            panel.read(cx).focus_handle(cx).focus(window);
        } else {
            self.composer.read(cx).focus_handle.focus(window);
        }
        self.rebuild_workspace_sessions();
        self.sync_file_root(window, cx);
        cx.notify();
    }

    pub(super) fn close_cli_draft(&mut self, cx: &mut Context<Self>) {
        self.close_chat_cli(CLI_DRAFT, cx);
        self.cli_draft_project = None;
        self.drafts.remove(CLI_DRAFT);
        self.attachment_drafts.remove(CLI_DRAFT);
    }

    pub(super) fn open_tab_sessions(&self)->Vec<&SessionSummary>{self.open_tabs.iter().filter_map(|id|self.sessions.iter().find(|s|&s.id==id && !self.hidden_sessions.contains(id))).collect()}
    pub(super) fn keyboard_controls(&self) -> AnyElement {
        div()
            .id("keyboard-help")
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Keyboard shortcuts"),
            )
            .children(COMMANDS.iter().map(|(key, _, label)| {
                div()
                    .flex()
                    .justify_between()
                    .py_1()
                    .gap_4()
                    .text_size(theme::ui_px(12.))
                    .child(*label)
                    .child(
                        div().text_color(rgb(MUTED)).child(
                            key.replace("ctrl", "⌃")
                                .replace("cmd", "⌘")
                                .replace("shift", "⇧")
                                .replace('-', " ")
                                .to_uppercase(),
                        ),
                    )
            }))
            .into_any_element()
    }
    pub(super) fn execute_shortcut(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(index)=tab_index(id){
            if let Some(session)=self.open_tab_sessions().into_iter().nth(index).cloned(){
                if self.active.as_ref()!=Some(&session.id){self.open_session(session,window,cx);}
            }
            return;
        }
        match id {
            "split_beside" | "split_below" => self.workspace_split_key(id, window, cx),
            "new" => self.new_conversation(window, cx),
            "close_tab" => {
                if let Some(owner) = self.ninja_surface_owner() {
                    self.close_chat_cli(&owner, cx);
                    self.remember_ninja_draft(cx);
                    self.ninjas.desktop = None;
                    self.ninjas.panel = None;
                    cx.notify();
                    return;
                }
                if let Some(id) = self.active.clone() {
                    self.close_tab(&id, window, cx)
                } else if self.cli_draft_project.is_some() || self.chat_clis.contains_key(CLI_DRAFT) {
                    self.close_blank_tab(window, cx);
                }
            }
            "composer" => self.composer.read(cx).focus_handle.focus(window),
            "conversation" => self.focus.focus(window),
            "model" | "thinking" => {
                let same = self
                    .agent_picker
                    .as_ref()
                    .is_some_and(|p| !p.closing && p.effort_only == (id == "thinking"));
                self.picker_at_pointer_down = if same { Some(id == "thinking") } else { None };
                let p = point(
                    window.viewport_size().width - px(300.),
                    window.viewport_size().height - px(80.),
                );
                self.toggle_picker_from_click(id == "thinking", p, window, cx);
            }
            "projects" => self.show_modal(Modal::Projects, window, cx),
            "search" => self.show_modal(Modal::Search, window, cx),
            "sidebar" => self.toggle_sidebar(),
            "conversations" => self.toggle_conversations(cx),
            "files" => self.toggle_files(window, cx),
            "changes" => self.toggle_diffs(cx),
            "terminal" => self.toggle_terminal(window, cx),
            "terminalize" => self.terminalize_chat(window, cx),
            "dock_terminal" => {
                let sides = terminal_dock::Side::ALL;
                let i = sides
                    .iter()
                    .position(|s| *s == self.terminal_dock.side)
                    .unwrap_or(0);
                self.dock_terminal(sides[(i + 1) % sides.len()], cx)
            }
            "usage" => self.show_modal(Modal::Status, window, cx),
            "orchestrator" => self.toggle_orchestrator(window, cx),
            "dictate" => self.toggle_dictation(window, cx),
            "voice" => self.toggle_voice(window, cx),
            "children" => self.toggle_children(window, cx),
            "stop" => self.interrupt_native_session(cx),
            "fold_project" => {
                let path = self.project.clone();
                self.toggle_project_branch(path, cx)
            }
            "fold_work" => {
                if let Some(i) = self.flow_index.work.iter().rev().find_map(|span|span.as_ref().map(|range|range.start)) {
                    self.toggle_work(i, cx)
                }
            }
            "next_tab" | "previous_tab" => {
                let n = self.open_tabs.len();
                if n > 0 {
                    let i = self
                        .active
                        .as_ref()
                        .and_then(|id| self.open_tabs.iter().position(|s| s == id))
                        .unwrap_or(0);
                    let step = if id == "next_tab" { 1 } else { n - 1 };
                    if let Some(session) = self
                        .sessions
                        .iter()
                        .find(|s| s.id == self.open_tabs[(i + step) % n])
                        .cloned()
                    {
                        self.open_session(session, window, cx)
                    }
                }
            }
            "next_project" | "previous_project" => {
                let n = self.projects.len();
                if n > 0 {
                    let i = self
                        .projects
                        .iter()
                        .position(|p| p.path == self.project)
                        .unwrap_or(0);
                    let step = if id == "next_project" { 1 } else { n - 1 };
                    self.select_project(self.projects[(i + step) % n].path.clone(), window, cx)
                }
            }
            "settings" => self.show_modal(Modal::Settings, window, cx),
            "keyboard" => {
                self.show_modal(Modal::Settings, window, cx);
                if let Some(input) = &self.appearance_search {
                    input.update(cx, |i, cx| i.set_text("Keyboard", cx));
                }
            }
            "github"=>self.open_github(github_ui::Tab::Repository,window,cx),
            "open_project" => self.open_folder(window, cx),
            "notch" => unified::collapse(),
            _ => return,
        }
        cx.notify();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    fn prepare_cli_fixture(app: &mut Orbit) {
        app.active = None;
        app.agent = AgentKind::Claude;
        app.model = None;
        app.new_execution_path = None;
        app.starting = false;
        let claude = app.agents.iter_mut().find(|agent| agent.kind == AgentKind::Claude).unwrap();
        claude.executable = Some(PathBuf::from("/bin/sh"));
        claude.available = true;
    }

    #[gpui::test]
    fn cli_preparation_cancellation_keeps_the_unsent_draft(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        cx.update(|window, cx| view.update(cx, |app, cx| {
            prepare_cli_fixture(app);
            app.composer.update(cx, |input, cx| input.set_text("Keep unsent text", cx));
            app.terminalize_chat(window, cx);
            assert!(app.chat_clis[CLI_DRAFT].preparing);
            app.terminalize_chat(window, cx);
            assert!(!app.chat_clis.contains_key(CLI_DRAFT));
            assert_eq!(app.composer.read(cx).text(), "Keep unsent text");
        }));
        cx.run_until_parked();
        view.update(cx, |app, cx| {
            assert!(!app.chat_clis.contains_key(CLI_DRAFT));
            assert_eq!(app.composer.read(cx).text(), "Keep unsent text");
        });
    }

    #[gpui::test]
    fn cli_preparation_follows_a_promoted_draft_owner(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        let (owner, token) = cx.update(|window, cx| view.update(cx, |app, cx| {
            prepare_cli_fixture(app);
            app.composer.update(cx, |input, cx| input.set_text("Keep promoted text", cx));
            app.terminalize_chat(window, cx);
            let token = app.chat_clis[CLI_DRAFT].launch_id;
            app.preserve_blank_tab(cx);
            let owner = app.active.clone().unwrap();
            assert_eq!(app.chat_clis[&owner].launch_id, token);
            assert!(!app.chat_clis.contains_key(CLI_DRAFT));
            (owner, token)
        }));
        cx.run_until_parked();
        view.update(cx, |app, cx| {
            let cli = &app.chat_clis[&owner];
            assert_eq!(cli.launch_id, token);
            assert!(!cli.preparing);
            assert!(cli.selected);
            assert_eq!(cli.agent, AgentKind::Claude);
            let (_, args, _) = cli.launch.as_ref().expect("Preparation completed for promoted owner");
            assert_eq!(args[3], "/bin/sh");
            assert!(!args.iter().any(|arg| arg == "resume" || arg == "--resume"));
            assert_eq!(app.composer.read(cx).text(), "Keep promoted text");
            assert_eq!(app.drafts[&owner], "Keep promoted text");
        });
    }

    #[gpui::test]
    fn changing_the_selected_provider_replaces_only_that_tabs_cli(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        let previous = uuid::Uuid::new_v4();
        cx.update(|window, cx| view.update(cx, |app, cx| {
            prepare_cli_fixture(app);
            app.composer.update(cx, |input, cx| input.set_text("Provider switch draft", cx));
            app.chat_clis.insert(CLI_DRAFT.into(), ChatCli {
                launch: None, agent: AgentKind::Codex, retained: None, preparing: false,
                launch_id: previous, fallback: None, selected: true, metal_visible: false, resumes_chat: false, cursor_history: None, send_when_closed: None,
            });
            app.terminalize_chat(window, cx);
            let cli = &app.chat_clis[CLI_DRAFT];
            assert_ne!(cli.launch_id, previous);
            assert_eq!(cli.agent, AgentKind::Claude);
            assert_eq!(app.composer.read(cx).text(), "Provider switch draft");
        }));
        cx.run_until_parked();
        view.update(cx, |app, cx| {
            let cli = &app.chat_clis[CLI_DRAFT];
            assert_eq!(cli.agent, AgentKind::Claude);
            assert!(!cli.preparing);
            assert!(cli.selected);
            assert_eq!(app.composer.read(cx).text(), "Provider switch draft");
        });
    }

    #[gpui::test]
    fn shortcut_from_peer_cli_targets_its_tab_and_leaves_the_other_cli_selected(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        cx.update(|window, cx| view.update(cx, |app, cx| {
            let first = app.sessions[0].clone();
            let mut second = first.clone();
            second.id = "other-split-chat".into();
            app.sessions.push(second.clone());
            app.open_session(first.clone(), window, cx);
            app.split_chat(second.clone(), terminal_dock::Side::Right, window, cx);
            assert_eq!(app.split_peer(), Some(second.id.as_str()));
            let cli = || ChatCli {
                launch: None, agent: AgentKind::Codex, retained: None, preparing: false,
                launch_id: uuid::Uuid::new_v4(), fallback: None, selected: true, metal_visible: true,
                resumes_chat: false, cursor_history: None, send_when_closed: None,
            };
            app.chat_clis.insert(first.id.clone(), cli());
            app.chat_clis.insert(second.id.clone(), cli());
            let token = app.chat_clis[&second.id].launch_id;
            app.unified_command(&json!({"action":"chat_terminal_command","id":second.id,
                "launchId":token,"command":"terminalize"}), window, cx).unwrap();
            assert_eq!(app.active.as_deref(), Some(second.id.as_str()));
            assert!(!app.chat_clis[&second.id].selected);
            assert!(app.chat_clis[&first.id].selected);
            app.unified_command(&json!({"action":"chat_terminal_command","id":second.id,
                "launchId":token,"command":"close_tab"}), window, cx).unwrap();
            assert!(!app.chat_clis.contains_key(&second.id));
            assert!(app.chat_clis[&first.id].selected);
        }));
    }

    #[gpui::test]
    fn keyboard_navigation_preserves_chat_drafts_and_focuses_the_existing_composer(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                app.active = Some("fixture-session".into());
                app.composer
                    .update(cx, |i, cx| i.set_text("Keep my original draft", cx));
                app.execute_shortcut("new", window, cx);
                assert!(app.active.is_none());
                assert_eq!(
                    app.drafts.get("fixture-session").map(String::as_str),
                    Some("Keep my original draft")
                );
                app.execute_shortcut("composer", window, cx);
                assert!(app.composer.read(cx).focus_handle.is_focused(window));
                app.execute_shortcut("keyboard", window, cx);
                assert_eq!(
                    app.appearance_search.as_ref().unwrap().read(cx).text(),
                    "Keyboard"
                );
            })
        });
    }
    #[gpui::test]
    fn command_w_closes_tabs_and_keeps_the_workspace_and_empty_draft(cx:&mut TestAppContext){
        let(view,cx)=crate::ui_tests::fixture(cx);
        view.update(cx,|app,cx|{
            let mut second=app.sessions[0].clone();second.id="second-tab".into();app.sessions.push(second);
            app.open_tabs=vec!["fixture-session".into(),"second-tab".into()];app.active=Some("second-tab".into());
            app.composer.update(cx,|input,cx|input.set_text("Preserved closed-tab draft",cx));cx.notify();
        });
        crate::ui_tests::draw_frame(&view,cx);
        cx.simulate_keystrokes("cmd-w");
        view.update(cx,|app,_|{assert_eq!(app.open_tabs,["fixture-session"]);assert_eq!(app.active.as_deref(),Some("fixture-session"));assert_eq!(app.drafts["second-tab"],"Preserved closed-tab draft");assert_eq!(app.sessions.len(),2);});
        cx.simulate_keystrokes("cmd-w");
        view.update(cx,|app,cx|{assert!(app.open_tabs.is_empty());assert!(app.active.is_none());app.composer.update(cx,|input,cx|input.set_text("Keep empty-tab draft",cx));});
        cx.simulate_keystrokes("cmd-w cmd-w");
        view.update(cx,|app,cx|{assert!(app.active.is_none());assert_eq!(app.composer.read(cx).text(),"Keep empty-tab draft");});
        assert_eq!(cx.update(|_,cx|cx.windows().len()),1,"Cmd-W removed the Code window");
    }
    #[gpui::test]
    fn numbered_chat_shortcuts_follow_visible_tab_order_and_preserve_drafts(cx:&mut TestAppContext){
        let(view,cx)=crate::ui_tests::fixture(cx);
        view.update(cx,|app,cx|{
            let base=app.sessions[0].clone();app.sessions=(0..9).map(|n|{let mut s=base.clone();s.id=format!("tab-{n}");s.agent=if n%2==0{AgentKind::Codex}else{AgentKind::Gemini};s}).collect();
            app.open_tabs=vec!["missing".into()];app.open_tabs.extend((0..9).rev().map(|n|format!("tab-{n}")));app.active=Some("tab-8".into());app.composer.update(cx,|i,cx|i.set_text("Keep tab eight draft",cx));cx.notify();
        });
        crate::ui_tests::draw_frame(&view,cx);
        cx.simulate_keystrokes("cmd-9");
        view.update(cx,|app,cx|{assert_eq!(app.active.as_deref(),Some("tab-0"));assert_eq!(app.drafts["tab-8"],"Keep tab eight draft");app.composer.update(cx,|i,cx|i.set_text("Keep tab zero draft",cx));});
        cx.simulate_keystrokes("cmd-1");
        view.update(cx,|app,cx|{assert_eq!(app.active.as_deref(),Some("tab-8"));assert_eq!(app.composer.read(cx).text(),"Keep tab eight draft");assert_eq!(app.drafts["tab-0"],"Keep tab zero draft");});
        let generation=view.update(cx,|app,_|app.generation);
        cx.simulate_keystrokes("cmd-1");view.update(cx,|app,_|assert_eq!(app.generation,generation,"Selecting current tab restarted it"));
        for n in 1..=9{cx.simulate_keystrokes(&format!("cmd-{n}"));view.update(cx,|app,_|assert_eq!(app.active.as_deref(),Some(format!("tab-{}",9-n).as_str())));}
        view.update(cx,|app,cx|{app.open_tabs=vec!["tab-0".into()];app.composer.update(cx,|i,cx|i.set_text("No missing-slot navigation",cx));cx.notify();});
        cx.simulate_keystrokes("cmd-9");view.update(cx,|app,cx|{assert_eq!(app.active.as_deref(),Some("tab-0"));assert_eq!(app.composer.read(cx).text(),"No missing-slot navigation");});
    }
    #[gpui::test]
    fn numbered_native_noops_keep_captured_context(cx:&mut TestAppContext){
        let(view,cx)=crate::ui_tests::fixture(cx);
        view.update_in(cx,|app,w,cx|{
            app.active=Some("fixture-session".into());app.open_tabs=vec!["fixture-session".into()];app.unified_context=Some("Keep captured app context".into());
            for id in ["tab_1","tab_9"]{
                let request=serde_json::json!({"action":"ui_command","command":id,"ownerToken":app.unified_token(),"draftBase":app.composer.read(cx).text()});app.unified_command(&request,w,cx).unwrap();assert_eq!(app.unified_context.as_deref(),Some("Keep captured app context"));
            }
        });
    }
    #[test]
    fn command_w_and_catalog_use_unique_simple_keys() {
        let mut keys = std::collections::HashSet::new();
        for (key, id, label) in COMMANDS {
            assert!(keys.insert(key));
            assert!(!id.is_empty() && !label.is_empty());
            let parts: Vec<_> = key.split('-').collect();
            assert!(matches!(
                parts.as_slice(),
                ["ctrl", _] | ["ctrl", "shift", _] | ["cmd", _] | ["cmd", "shift", _]
            ));
        }
        assert_eq!(catalog().as_array().unwrap().len(), COMMANDS.len());
    }
}
