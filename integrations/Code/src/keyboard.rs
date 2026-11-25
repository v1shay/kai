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
    ("cmd-t", "new_tab", "New tab"),
    ("cmd-n", "new", "New chat"),
    ("cmd-e", "conversations", "Conversation sidebar"),
    ("cmd-f", "files", "Files"),
    ("cmd-d", "split_beside", "Split pane right"),
    ("cmd-shift-d", "split_below", "Split pane below"),
    ("cmd-k", "search", "Command palette"),
    ("cmd-b", "sidebar", "Project sidebar"),
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

    ("cmd-w", "close_tab", "Close tab"),
    ("ctrl-l", "composer", "Focus message"),
    ("ctrl-shift-l", "conversation", "Focus conversation"),
    ("ctrl-m", "model", "Choose agent and model"),
    ("ctrl-shift-m", "thinking", "Thinking level"),
    ("ctrl-p", "projects", "Switch project"),
    ("ctrl-shift-p", "fold_project", "Fold project"),
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
    ("cmd-shift-k", "notch", "Return to notch"),
];
pub(super) fn split_command(command: &str) -> bool {
    matches!(command, "split_add" | "split_beside" | "split_below" | "maximize_pane" | "split_left" | "split_right" | "split_up" | "split_down")
}

pub fn bind(cx: &mut App, overrides: &std::collections::BTreeMap<String, String>) {
    cx.bind_keys(COMMANDS.iter().map(|(key, id, _)| {
        let key = overrides.get(*key).map_or(*key, String::as_str);
        KeyBinding::new(
            key,
            Execute { id: (*id).into() },
            Some(if key.starts_with("ctrl-") && !key.starts_with("ctrl-shift-") && control_clicks::Control::from_shortcut(id).is_none() {
                "Orbit && !CodeTerminal"
            } else { "Orbit" }),
        )
    }));
}
pub fn catalog(overrides: &std::collections::BTreeMap<String, String>) -> serde_json::Value {
    serde_json::json!(
        COMMANDS
            .iter()
            .map(|(key, id, label)| serde_json::json!({"key":overrides.get(*key).map_or(*key, String::as_str),"id":id,"label":label,"toggle":control_clicks::Control::from_shortcut(id).is_some()}))
            .collect::<Vec<_>>()
    )
}
pub(super) const CLI_DRAFT: &str = "workspace-draft-cli";

pub(super) struct ChatCli {
    pub shell: bool,
    pub persistent:bool,
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

pub(super) fn keycaps(label:&str)->Div{
    let mut keys=Vec::new();let mut main=String::new();
    for ch in label.chars(){if "⌘⌃⌥⇧".contains(ch){keys.push(ch.to_string());}else{main.push(ch);}}
    if !main.trim().is_empty(){keys.push(main.trim().to_owned());}
    div().flex().items_center().gap_1().children(keys.into_iter().map(|key|
        div().min_w(px(25.)).h(px(25.)).px_2().flex().items_center().justify_center()
            .rounded_md().border_1().border_b_2().border_color(rgba(0xffffff30))
            .bg(linear_gradient(180.,linear_color_stop(rgba(0xffffff16),0.),linear_color_stop(rgba(0xffffff07),1.)))
            .text_size(theme::ui_px(11.)).text_color(rgb(TEXT)).child(key)))
}
fn shortcut_label(key: &str) -> String {
    key.replace("ctrl-", "⌃").replace("cmd-", "⌘").replace("alt-", "⌥").replace("shift-", "⇧").to_uppercase()
}

fn shortcut_allowed(key: &str) -> bool {
    let Ok(stroke) = Keystroke::parse(key) else { return false; };
    let mods = stroke.modifiers;
    !mods.alt && !mods.function && mods.platform != mods.control
        && stroke.key.len() == 1 && stroke.key.is_ascii()
        && !matches!(key, "cmd-q" | "cmd-m" | "cmd-a" | "cmd-c" | "cmd-v" | "cmd-x" | "cmd-z" | "cmd-shift-z")
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

    pub(super) fn render_chat_cli(&self, owner: &str, window: &Window, cx: &Context<Self>) -> Option<AnyElement> {
        let cli = self.chat_clis.get(owner).filter(|cli| cli.selected)?;
        let session = self.sessions.iter().find(|session| session.id == owner);
        let peer = session.filter(|session| self.active.as_ref() != Some(&session.id));
        let pet_slot = unified::active()
            && peer.is_none()
            && window.window_handle().window_id() == self.primary_window;
        let title = session.map_or("New session".into(), |session| self.chat_label(session));
        let target = owner.to_owned();
        let shortcut = self.shortcut_overrides.get("cmd-shift-j").map_or("cmd-shift-j", String::as_str);
        let tooltip = format!("Switch to GUI  {}", shortcut_label(shortcut));
        let button = context_ui::header_control("chat-cli-back", false)
            .h(px(22.)).min_w(px(24.)).px(px(3.))
            .child(div().text_size(theme::ui_px(10.)).child("GUI"))
            .tooltip(move |_, cx| cx.new(|_| tui_badges::Label(tooltip.clone())).into())
            .on_click(cx.listener(move |app, _, window, cx| {
                if app.chat_cli_owner().as_deref() == Some(target.as_str()) || app.focus_split_chat(&target, window, cx) {
                    app.terminalize_chat(window, cx);
                }
            }));
        let pane = self.conversation_pane().key_context("ChatCli")
            .child(div().h(px(44.)).flex_none().px_3().flex().items_center().gap_1()
                .child(div().w(px(48.)).h(px(34.)).flex_none().flex().items_center().justify_center()
                    .when(pet_slot, |slot| slot.child(canvas(
                        |bounds, _, _| unified::pet_slot(Some(bounds)), |_, _, _, _| {},
                    ).size_full()))
                    .when(!pet_slot, |slot| slot.child(
                        provider_icon(agent_icon(cli.agent), 16., agent_color(cli.agent)),
                    )))
                .child(div().id("cli-chat-header-title").debug_selector(|| "cli-chat-header-title".into())
                    .flex_1().min_w(px(0.)).rounded_md().px_2().py_1().flex().items_center().gap_2()
                    .cursor_pointer().hover(|s| s.bg(rgba(GLASS_HOVER)))
                    .child(div().flex_1().min_w(px(0.)).truncate().text_size(theme::ui_px(12.)).child(title))
                    .child(icon("chevron-down", 10., MUTED))
                    .on_click(cx.listener({ let owner = session.map(|s| s.id.clone());
                        move |app, event: &ClickEvent, window, cx| { cx.stop_propagation(); app.toggle_chat_switcher(owner.clone(), event.position(), window, cx); }
                    })))
                .child(button)
                .when(session.is_some(), |header| {
                    let id = owner.to_owned();
                    header.child(context_ui::header_control("close-cli-pane", false).h(px(22.)).min_w(px(22.)).px(px(3.))
                        .child(icon("close", 12., MUTED))
                        .on_click(cx.listener(move |app, _, window, cx| app.close_tab(&id, window, cx))))
                })
                .child(self.workspace_action_buttons(peer, cx)));
        if let Some((panel, _)) = &cli.fallback {
            Some(pane.child(div().flex_1().min_h(px(0.)).child(panel.clone())).into_any_element())
        } else if cli.metal_visible {
            Some(pane.child(div().flex_1().min_h(px(0.)).relative().child(self.chat_cli_canvas(owner))).into_any_element())
        } else {
            Some(pane.child(div().flex_1().min_h(px(0.))).into_any_element())
        }
    }

    pub(super) fn import_cursor_history(&self, id: &str, history: cursor_cli_history::Sync) {
        let draft = self.draft_sessions.contains(id).then(|| {
            self.sessions.iter().find(|session| session.id == id)
                .map(|session| (session.clone(), self.session_models.get(id).cloned().flatten()))
        }).flatten();
        self.engine.import_cursor_cli(id.to_owned(), history, draft);
    }

    pub(super) fn close_chat_cli(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(mut cli) = self.chat_clis.remove(id) {
            if cli.persistent{let profile=self.attachment_dir.parent().unwrap_or(&self.attachment_dir).to_owned();let launch=cli.launch_id;cx.background_executor().spawn(async move{if let Some(server)=terminal::SessionServer::find(&profile){let _=server.end(launch);}}).detach();}
            if let Some(history) = cli.cursor_history.take().filter(|_| id != CLI_DRAFT) {
                self.import_cursor_history(id, history);
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
        if cli.preparing || !cli.persistent && !cli.metal_visible && cli.fallback.is_none() {
            self.notice = Some("The CLI is still opening. Your draft is saved.".into());
            cx.notify();
            return;
        }
        cli.send_when_closed = Some(draft);
        if cli.persistent {
            // Closing a native surface only detaches tmux. Explicit GUI Send
            // must hand ownership back before starting a second provider writer.
            let launch = cli.launch_id;
            let profile = self.attachment_dir.parent().unwrap_or(&self.attachment_dir).to_owned();
            let native = self.sessions.iter().find(|s| s.id == id && s.agent == AgentKind::Codex).and_then(|s|s.external_id.clone());
            cx.spawn(async move |this, cx| {
                let result = cx.background_executor().spawn(async move {
                    let server = terminal::SessionServer::find(&profile).ok_or_else(|| anyhow::anyhow!("Persistent CLI host is unavailable"))?;
                    server.end(launch)?;
                    if let Some(native) = native {
                        let home = std::env::var_os("CODEX_HOME").map(PathBuf::from).or_else(||dirs::home_dir().map(|h|h.join(".codex")));
                        if let Some(home) = home { wait_codex_cli_release(&home, &native)?; }
                    }
                    Ok::<_,anyhow::Error>(())
                }).await;
                let _ = this.update(cx, |app, cx| {
                    if !app.chat_clis.get(&id).is_some_and(|c|c.launch_id == launch) { return; }
                    if let Err(error) = result {
                        if let Some(cli) = app.chat_clis.get_mut(&id) { cli.send_when_closed = None; }
                        app.notice = Some(format!("Could not hand this CLI back to chat: {error}. Your draft is saved."));
                        cx.notify();
                        return;
                    }
                    // Acknowledge via the normal owner event, even for a hidden
                    // or missing renderer. That path sends the preserved draft.
                    if let Some(cli) = app.chat_clis.get_mut(&id) {
                        if let Some((panel,_)) = &cli.fallback { panel.update(cx, |panel,_|panel.stop()); }
                    }
                    unified::emit(&json!({"type":"chatTerminalClose","id":id,"launchId":launch,"acknowledge":true}));
                    app.pending_cli_handoffs.push((id, launch));
                    cx.notify();
                });
            }).detach();
            return;
        }
        if let Some((panel, _)) = &cli.fallback {
            panel.update(cx, |panel, cx| panel.close(cx));
        } else {
            unified::emit(&json!({"type":"chatTerminalClose","id":id,"launchId":cli.launch_id,"acknowledge":true}));
        }
        cx.notify();
    }

    pub(super) fn terminalize_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.terminalize_chat_as(false, window, cx);
    }

    pub(super) fn terminalize_chat_as(&mut self, shell: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.remote_workspace.active() {
            if shell { self.open_remote_terminal(true, window, cx); }
            else { self.toggle_remote_cli(window, cx); }
            return;
        }
        self.startup_owner_pristine = false;
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
                    self.import_cursor_history(&id, history);
                }
                if resumes && let Some(session) = self.sessions.iter().find(|session| session.id == id).cloned() {
                    self.open_session(session, window, cx);
                }
                self.composer.read(cx).focus_handle(cx).focus(window);
            }
            self.queue_workspace_save(cx);
            cx.notify();
            return;
        }
        if self.quit_pending || (self.ninja_surface_owner().is_none() && self.starting_current()) || self.runs.contains_key(&id)
            || self.settling.contains_key(&id) || self.finalizing.contains(&id)
            || self.voice_call.as_ref().is_some_and(|call| call.owner == id)
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
        let cursor_lookup = (!shell && agent == AgentKind::Cursor && personal.is_none() && session_can_bind)
            .then(|| session.map(|session| self.engine.cursor_cli_id(session.id.clone())))
            .flatten();
        let personal = personal.is_some();
        let launch_id = uuid::Uuid::new_v4();
        let persistent_profile=self.attachment_dir.parent().unwrap_or(&self.attachment_dir).to_owned();
        if id == CLI_DRAFT {
            self.cli_draft_project = Some((self.project.clone(), self.new_execution_path.clone(), agent));
        }
        self.chat_clis.insert(id, ChatCli {
            shell, persistent:false, launch: None, agent, retained: None, preparing: true, launch_id,
            fallback: None, selected: true, metal_visible: false, resumes_chat: !shell && external.is_some(), cursor_history: None, send_when_closed: None,
        });
        cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_secs(18)).await;
            let _ = this.update(cx, |app, cx| {
                let pending = app.chat_clis.iter().find(|(_, cli)| cli.launch_id == launch_id)
                    .filter(|(_, cli)| cli.selected && !cli.metal_visible &&
                        cli.fallback.as_ref().is_none_or(|(panel, _)| panel.read(cx).launching()))
                    .map(|(owner, _)| owner.clone());
                if let Some(owner) = pending {
                    unified::pane_feedback_failed(&owner);
                    unified::workspace_toast("The CLI did not start. Your conversation is still open.", true);
                    app.close_chat_cli(&owner, cx);
                    cx.notify();
                }
            });
        }).detach();
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
                if shell {
                    let program = std::env::var("SHELL").ok().filter(|path| path.starts_with('/')).unwrap_or_else(|| "/bin/zsh".into());
                    let command = format!("exec '{}' -l", program.replace('\'', "'\"'\"'"));
                    let prepared = provider_cli::PreparedLaunch {
                        arguments: vec!["-lc".into(), command], environment: Vec::new(), retained: None,
                    };
                    let prepared=if !cfg!(test){persistent_cli_launch(prepared,&persistent_profile,launch_id,&directory)?}else{prepared};
                    return Ok((directory, prepared, None));
                }
                let executable = if let Some(selection) = &ninja_selection {
                    ninja::executable(selection.harness, &home)
                } else {
                    executable.or_else(|| discovery::discover_agent(agent, &home).executable)
                }.ok_or_else(|| format!("{} CLI is unavailable.", agent.name()))?;
                let mut history = if agent == AgentKind::Cursor && !personal && session_can_bind {
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
                if let Some(history)=&mut history{if !cfg!(test){history.persist_at(persistent_profile.join("cursor-sync").join(format!("{launch_id}.json"))).map_err(|e|format!("Could not preserve Cursor progress: {e}"))?;}}
                let resume = history.as_ref().map(|history| history.id.as_str())
                    .or_else(|| (agent != AgentKind::Cursor || personal).then_some(external.as_deref()).flatten());
                provider_cli::prepare(agent, &executable, &directory, resume, model.as_deref(), ninja_selection.as_ref())
                    .and_then(|prepared|{let prepared=if !cfg!(test){persistent_cli_launch(prepared,&persistent_profile,launch_id,&directory)?}else{prepared};Ok((directory, prepared, history))})
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
                        if let Some(history) = &history && owner != CLI_DRAFT && !app.draft_sessions.contains(&owner) {
                            app.engine.bind_cursor_cli(owner.clone(), history.id.clone());
                        }
                        let cli = app.chat_clis.get_mut(&owner).unwrap();
                        cli.resumes_chat |= history.is_some();
                        cli.cursor_history = history;
                        cli.preparing = false;
                        cli.persistent=arguments.get(1).is_some_and(|s|s.contains("attach-session -t ="));
                        cli.retained=retained.map(|lease|Box::new(lease) as Box<dyn std::marker::Send>);
                        cli.launch = Some((directory.clone(), arguments.clone(), environment.clone()));
                        unified::emit(&json!({"type":"chatTerminalOpen","id":owner,"launchId":launch_id,
                            "arguments":arguments,"environment":environment,"directory":directory,
                            "title":title,"agent":if shell { "Shell" } else { agent.name() }}));
                    }
                    Err(error) => {
                        app.chat_clis.remove(&owner);
                        unified::pane_feedback_failed(&owner);
                        unified::workspace_toast(&error, true);
                    }
                }
                app.queue_workspace_save(cx);
                cx.notify();
            });
            if let Some(prepared) = pending {
                cx.background_executor().spawn(async move { drop(prepared); }).detach();
            }
        }).detach();
    }

    pub(super) fn open_cli_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((project, execution, agent)) = self.cli_draft_project.clone() else { return; };
        self.startup_owner_pristine = false;
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

    pub(super) fn open_tab_sessions(&self) -> Vec<&SessionSummary> {
        let mut grouped = false;
        self.open_tabs.iter().filter_map(|id| {
            let target = if let Some(split) = self.split_tab_for(id) {
                if std::mem::replace(&mut grouped, true) { return None; }
                split.sessions.iter().find(|member| **member == split.focused).unwrap_or(&split.sessions[0])
            } else { id };
            self.sessions.iter().find(|session| &session.id == target && !self.hidden_sessions.contains(target))
        }).collect()
    }

    pub(super) fn restore_shortcuts(&mut self, saved: std::collections::BTreeMap<String, String>, cx: &mut Context<Self>) {
        let mut taken = HashSet::new();
        let mut saved=saved;
        // Preserve customized legacy aliases while giving each action one slot.
        for (alias,canonical) in [("ctrl-t","cmd-n"),("cmd-shift-n","cmd-n"),("cmd-shift-e","cmd-f"),("ctrl-e","cmd-f"),("cmd-s","cmd-k"),("ctrl-k","cmd-k"),("ctrl-b","cmd-b"),("ctrl-shift-b","cmd-e"),("ctrl-w","cmd-w")] {
            if let Some(chord)=saved.remove(alias){saved.entry(canonical.into()).or_insert(chord);}
        }
        self.shortcut_overrides = saved.into_iter().filter(|(old, key)| {
            COMMANDS.iter().any(|(default, _, _)| default == old)
                && shortcut_allowed(key)
                && taken.insert(key.clone())
        }).collect();
        loop {
            let conflicts: Vec<_> = self.shortcut_overrides.iter().filter(|(old, key)| {
                COMMANDS.iter().any(|(default, _, _)| *default == key.as_str() && *default != old.as_str()
                    && !self.shortcut_overrides.contains_key(*default))
            }).map(|(old, _)| old.clone()).collect();
            if conflicts.is_empty() { break; }
            for key in conflicts { self.shortcut_overrides.remove(&key); }
        }
        if !self.shortcut_overrides.is_empty() { self.apply_shortcuts(cx); }
    }

    fn apply_shortcuts(&self, cx: &mut Context<Self>) {
        cx.clear_key_bindings();
        bind_workspace_keys(cx, &self.shortcut_overrides);
        unified::emit(&json!({"type":"keyboardCatalog","commands":catalog(&self.shortcut_overrides)}));
    }

    fn capture_shortcut(&mut self, key: &str, pressed: &Keystroke, cx: &mut Context<Self>) {
        if pressed.key == "escape" {
            self.shortcut_capture = None;
            cx.notify();
            return;
        }
        let chord = pressed.unparse();
        let conflict = COMMANDS.iter().any(|(other, _, _)| {
            *other != key && (self.shortcut_overrides.get(*other).map_or(*other, String::as_str) == chord)
        });
        if !shortcut_allowed(&chord) || conflict {
            self.notice = Some(if conflict { "That shortcut is already in use." } else {
                "Use Command or Control with a letter, number, or punctuation key."
            }.into());
            cx.notify();
            return;
        }
        if chord == key { self.shortcut_overrides.remove(key); }
        else { self.shortcut_overrides.insert(key.into(), chord); }
        self.shortcut_capture = None;
        self.apply_shortcuts(cx);
        self.queue_workspace_save(cx);
        cx.notify();
    }

    pub(super) fn keyboard_controls(&self, cx: &Context<Self>) -> AnyElement {
        div().id("keyboard-help").flex().flex_col().gap_1()
            .child(div().font_weight(FontWeight::SEMIBOLD).child("Keyboard shortcuts"))
            .children(COMMANDS.iter().map(|(key, _, label)| {
                let current = self.shortcut_overrides.get(*key).map_or(*key, String::as_str);
                let capture = self.shortcut_capture.as_ref().filter(|(active, _, _)| active == key);
                let key = *key;
                div().id(SharedString::from(format!("shortcut-{key}"))).flex().justify_between().py_1().gap_4()
                    .text_size(theme::ui_px(12.)).cursor_pointer()
                    .hover(|row| row.bg(rgba(GLASS_HOVER)))
                    .child(*label)
                    .child(if capture.is_some(){div().text_color(rgb(MUTED)).child("Press a shortcut…").into_any_element()}
                        else{keycaps(&shortcut_label(current)).into_any_element()})
                    .on_click(cx.listener(move |app, _, window, cx| {
                        let focus = cx.focus_handle();
                        focus.focus(window);
                        let entity = cx.weak_entity();
                        let capture_focus = focus.clone();
                        let subscription = cx.intercept_keystrokes(move |event, window, cx| {
                            if !capture_focus.is_focused(window) { return; }
                            cx.stop_propagation();
                            let _ = entity.update(cx, |app, cx| app.capture_shortcut(key, &event.keystroke, cx));
                        });
                        app.shortcut_capture = Some((key.into(), focus, subscription));
                        cx.notify();
                    }))
                    .when_some(capture, |row, (_, focus, _)| row.track_focus(focus))

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
            if let Some(panel)=self.active_terminal_panel().filter(|panel|self.active_chat_cli().is_none_or(|cli|!cli.selected) && panel.read(cx).focus_handle(cx).is_focused(window)){
                let source=panel.entity_id();self.terminal_keyboard_command(source,id,window,cx);return;
            }
            if let Some(session)=self.open_tab_sessions().into_iter().nth(index).cloned(){
                if self.remote_workspace.active() || self.active.as_ref()!=Some(&session.id){self.open_session(session,window,cx);}
            }
            return;
        }
        if let Some(control) = control_clicks::Control::from_shortcut(id) {
            self.activate_control(control, window, cx);
            return;
        }
        match id {
            "new" | "new_tab" => self.new_conversation(window, cx),
            "close_tab" => {
                if self.remote_workspace.active() && window.window_handle().window_id() == self.primary_window {
                    self.close_remote_workspace(window, cx);
                    return;
                }
                if self.dismiss_split_picker(window, cx) { return; }
                if self.terminal_launcher.is_some() {
                    self.finish_terminal_launcher(None, window, cx);
                    return;
                }
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
                } else if !self.open_tabs.is_empty() || self.cli_draft_project.is_some() || self.chat_clis.contains_key(CLI_DRAFT) {
                    self.close_blank_tab(window, cx);
                }
            }
            "composer" => self.composer.read(cx).focus_handle.focus(window),
            "conversation" => self.focus.focus(window),
            "dock_terminal" => {
                let sides = terminal_dock::Side::ALL;
                let i = sides
                    .iter()
                    .position(|s| *s == self.effective_terminal_dock().side)
                    .unwrap_or(0);
                self.dock_terminal(sides[(i + 1) % sides.len()], cx)
            }
            "stop" => self.interrupt_native_session(cx),
            "next_tab" | "previous_tab" => {
                let tabs = self.open_tab_sessions();
                if !tabs.is_empty() {
                    let n = tabs.len();
                    let i = tabs.iter().position(|session| self.active.as_ref() == Some(&session.id)).unwrap_or(0);
                    let step = if id == "next_tab" { 1 } else { n - 1 };
                    self.open_session(tabs[(i + step) % n].clone(), window, cx);
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
    fn terminal_shortcut_toggles_while_its_pty_has_focus_and_preserves_the_shell(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        for key in ["ctrl-j", "ctrl-i"] {
            if key != "ctrl-j" {
                view.update(cx, |app, cx| app.capture_shortcut("ctrl-j", &Keystroke::parse(key).unwrap(), cx));
            }
            crate::ui_tests::draw_frame(&view, cx);
            cx.simulate_keystrokes(key);
            crate::ui_tests::draw_frame(&view, cx);
            let terminal = cx.update(|window, cx| view.update(cx, |app, cx| {
                assert!(app.terminal_visible);
                let terminal = app.active_terminal_panel().unwrap();
                assert!(terminal.read(cx).focus_handle(cx).is_focused(window));
                terminal.entity_id()
            }));
            cx.simulate_keystrokes(key);
            crate::ui_tests::draw_frame(&view, cx);
            view.update(cx, |app, _| {
                assert!(!app.terminal_visible);
                assert_eq!(app.active_terminal_panel().unwrap().entity_id(), terminal);
            });
        }
    }

    #[gpui::test]
    fn remapping_split_releases_the_old_key_and_dispatches_the_new_one(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update(cx, |app, cx| app.capture_shortcut("cmd-d", &Keystroke::parse("cmd-shift-u").unwrap(), cx));
        cx.simulate_keystrokes("cmd-d");
        view.update(cx, |app, _| assert!(app.split_peer().is_none()));
        cx.simulate_keystrokes("cmd-shift-u");
        view.update(cx, |app, _| assert!(app.split_peer().is_some()));
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
                shell: false, persistent:false, launch: None, agent: AgentKind::Codex, retained: None, preparing: false,
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
                shell: false, persistent:false, launch: None, agent: AgentKind::Codex, retained: None, preparing: false,
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
        assert_eq!(catalog(&Default::default()).as_array().unwrap().len(), COMMANDS.len());
    }
}

fn persistent_cli_launch(prepared:provider_cli::PreparedLaunch,profile:&std::path::Path,id:uuid::Uuid,root:&std::path::Path)->Result<provider_cli::PreparedLaunch,String>{
    let server=terminal::SessionServer::find(profile).ok_or("tmux is required for persistent CLI sessions")?;
    let session=terminal::Resume{id,project:root.to_owned(),backend:terminal::Backend::Metal};
    let command=server.cli_attach(&session,&prepared.arguments,&prepared.environment).map_err(|e|e.to_string())?;
    if let Some(lease)=&prepared.retained{if let Err(error)=lease.persist_for_cli(&server,id){let _=server.end(id);return Err(error.to_string());}}
    Ok(provider_cli::PreparedLaunch{arguments:vec!["-lc".into(),command],environment:Vec::new(),retained:prepared.retained})
}

fn wait_codex_cli_release(home: &std::path::Path, native: &str) -> anyhow::Result<()> {
    use std::os::fd::AsRawFd;
    anyhow::ensure!(uuid::Uuid::parse_str(native).is_ok(), "Invalid native chat identity");
    let path = home.join("thread-writer-locks").join(format!("{native}.lock"));
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN); }
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::EWOULDBLOCK) { return Err(error.into()); }
        anyhow::ensure!(Instant::now() < until, "The provider has not released this chat yet");
        std::thread::sleep(Duration::from_millis(20));
    }
}
