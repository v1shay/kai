//! Restore one active view, keep inactive tabs cold, and coalesce UI snapshots.
use crate::appearance::GlassPopup as _;
use super::*;
use std::hash::{Hash, Hasher};
use workspace_state::{TranscriptAnchor, WindowState, WorkspaceState};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
pub(super) enum TerminalOwner {
    Chat(String),
    Draft(PathBuf),
    Remote(String, String, String),
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(super) struct SavedTerminalDeck {
    pub owner: TerminalOwner,
    pub sessions: Vec<terminal::Resume>,
    #[serde(default)]
    pub layout: Option<terminal_panes::Node<uuid::Uuid>>,
    #[serde(default)]
    pub active: Option<uuid::Uuid>,
}

pub(super) struct TerminalDeck {
    pub sessions: Vec<terminal_ui::Session>,
    pub active: Option<EntityId>,
    pub layout: terminal_panes::Layout,
    pub visible: bool,
}

pub(super) fn capture_window(window: &Window) -> WindowState {
    let bounds = window.window_bounds();
    let rect = bounds.get_bounds();
    WindowState {
        x: rect.origin.x.into(),
        y: rect.origin.y.into(),
        width: rect.size.width.into(),
        height: rect.size.height.into(),
        maximized: matches!(bounds, WindowBounds::Maximized(_)),
        fullscreen: matches!(bounds, WindowBounds::Fullscreen(_)),
    }
}

/// Display changes must not strand the saved window outside the desktop.
pub(super) fn restored_bounds(state: Option<&WindowState>, cx: &App) -> WindowBounds {
    let Some(state) = state else {
        return WindowBounds::centered(size(px(1380.), px(880.)), cx);
    };
    let displays: Vec<_> = cx
        .displays()
        .iter()
        .map(|display| display.bounds())
        .collect();
    let rect = fit_rect(state, &displays);
    if state.fullscreen {
        WindowBounds::Fullscreen(rect)
    } else if state.maximized {
        WindowBounds::Maximized(rect)
    } else {
        WindowBounds::Windowed(rect)
    }
}
fn fit_rect(state: &WindowState, displays: &[Bounds<Pixels>]) -> Bounds<Pixels> {
    let saved = Bounds::new(
        point(px(state.x), px(state.y)),
        size(px(state.width), px(state.height)),
    );
    let Some(display) = displays.iter().max_by(|a, b| {
        let overlap = |display: &Bounds<Pixels>| {
            let w =
                f32::from(saved.right().min(display.right()) - saved.left().max(display.left()))
                    .max(0.);
            let h =
                f32::from(saved.bottom().min(display.bottom()) - saved.top().max(display.top()))
                    .max(0.);
            w * h
        };
        overlap(a).total_cmp(&overlap(b))
    }) else {
        return saved;
    };
    let width = state.width.clamp(
        940f32.min(display.size.width.into()),
        display.size.width.into(),
    );
    let height = state.height.clamp(
        620f32.min(display.size.height.into()),
        display.size.height.into(),
    );
    Bounds::new(
        point(
            px(state
                .x
                .clamp(display.origin.x.into(), f32::from(display.right()) - width)),
            px(state.y.clamp(
                display.origin.y.into(),
                f32::from(display.bottom()) - height,
            )),
        ),
        size(px(width), px(height)),
    )
}

pub(super) struct TerminalLauncher {
    focus: FocusHandle,
    selection: usize,
    previously_visible: bool,
    agent: AgentKind,
    executable: Option<PathBuf>,
    directory_name: String,
}

impl Orbit {
    pub(super) fn sync_chat_terminal_owner(&mut self, cx: &mut Context<Self>) {
        let ninja = self.ninja_surface_owner();
        let cli_owner = self.chat_cli_owner();
        let launcher_open = self.overlay_owns_input();
        if launcher_open { unified::chat_cli_owner(None, std::iter::empty::<&str>()); }
        else { unified::chat_cli_owner(cli_owner.as_deref(), self.split_companions()); }
        for (id, cli) in &self.chat_clis {
            if let Some((panel, _)) = &cli.fallback {
                let visible = !launcher_open && cli.selected && (cli_owner.as_deref() == Some(id.as_str()) || self.split_companions().any(|peer| peer == id));
                if panel.read(cx).is_visible() != visible {
                    panel.update(cx, |panel, _| panel.set_visible(visible));
                }
            }
        }
        let chat = ninja.as_deref().or(self.active.as_deref());
        let owner = self.remote_terminal_owner().unwrap_or_else(|| {
            chat.map_or_else(|| TerminalOwner::Draft(self.project.clone()), |id| TerminalOwner::Chat(id.to_owned()))
        });
        if self.terminal_owner.as_ref() != Some(&owner) {
            if let Some(launcher) = self.terminal_launcher.take() {
                self.terminal_visible = launcher.previously_visible;
            }
            self.switch_terminal_owner(owner, cx);
        }
        self.sync_split_terminal_visibility(cx);
        self.sync_remote_cli_visibility(cx);
    }

    pub(super) fn adopt_draft_terminals(&mut self, chat: &str) {
        if self.active.is_none() && matches!(self.terminal_owner, Some(TerminalOwner::Draft(_))) {
            let next = TerminalOwner::Chat(chat.to_owned());
            for deck in &mut self.saved_terminal_decks {
                if Some(&deck.owner) == self.terminal_owner.as_ref() { deck.owner = next.clone(); }
            }
            self.terminal_owner = Some(next);
        }
    }

    fn switch_terminal_owner(&mut self, owner: TerminalOwner, cx: &mut Context<Self>) {
        if self.terminal_owner.as_ref() == Some(&owner) { return; }
        self.terminal_panes.clear_hover();
        self.terminal_dock_hover = None;
        let mut next = self.parked_terminals.remove(&owner).unwrap_or_else(|| TerminalDeck {
            sessions: Vec::new(), active: None, layout: Default::default(), visible: false,
        });
        let was_visible_peer = self.terminal_workspace_visible
            && next.visible && self.split_terminal_owners.contains(&owner);
        self.cancel_terminal_pane_motion(was_visible_peer);
        let old = self.terminal_owner.replace(owner);
        std::mem::swap(&mut self.terminals, &mut next.sessions);
        std::mem::swap(&mut self.active_terminal, &mut next.active);
        std::mem::swap(&mut self.terminal_panes, &mut next.layout);
        std::mem::swap(&mut self.terminal_visible, &mut next.visible);
        if let Some(old) = old.filter(|_| !next.sessions.is_empty()) {
            // A focused split chat becomes a visible peer. Keep its PTY and
            // native surface attached across the deck swap.
            let remains_peer = self.split_visible_terminal_owners().contains(&old);
            if !remains_peer {
                for session in &next.sessions {
                    session.panel.update(cx, |panel, _| panel.set_visible(false));
                }
            }
            self.parked_terminals.insert(old, next);
        }
        self.sync_terminal_visibility(cx);
    }

    pub(super) fn sync_split_terminal_visibility(&mut self, cx: &mut Context<Self>) {
        let peers: Vec<_> = if self.terminal_workspace_visible && self.split_picker.is_none() && self.terminal_launcher.is_none() && !self.overlay_owns_input() {
            self.split_visible_terminal_owners()
        } else { Vec::new() };
        let previous = std::mem::replace(&mut self.split_terminal_owners, peers.clone());
        for owner in previous.into_iter().filter(|owner| !peers.contains(owner)) {
            if let Some(deck) = self.parked_terminals.get(&owner) {
                for session in &deck.sessions { session.panel.update(cx, |panel, _| panel.set_visible(false)); }
            }
        }
        for owner in peers {
            if let Some(deck) = self.parked_terminals.get(&owner) {
                for session in &deck.sessions {
                    let panel = session.panel.read(cx);
                    let visible = deck.visible && deck.layout.visible(session.panel.entity_id());
                    if panel.is_visible() != visible { session.panel.update(cx, |panel, _| panel.set_visible(visible)); }
                }
            }
        }
    }

    pub(super) fn all_terminals(&self) -> impl Iterator<Item = &terminal_ui::Session> {
        self.terminals.iter().chain(self.parked_terminals.values().flat_map(|deck| &deck.sessions))
    }

    pub(super) fn active_terminal_panel(&self) -> Option<&Entity<terminal_ui::TerminalPanel>> {
        self.terminals.iter().find(|session| Some(session.panel.entity_id()) == self.active_terminal).map(|session| &session.panel)
    }
    pub(super) fn sync_terminal_visibility(&self, cx: &mut Context<Self>) {
        for session in &self.terminals {
            let panel = session.panel.read(cx);
            let visible = self.terminal_workspace_visible && self.terminal_visible && self.terminal_launcher.is_none() && self.split_picker.is_none() && !self.overlay_owns_input()
                && self.terminal_panes.visible(session.panel.entity_id());
            if panel.is_visible() != visible { session.panel.update(cx, |panel, _| panel.set_visible(visible)); }
        }
    }
    pub(super) fn set_terminal_workspace_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.terminal_workspace_visible != visible {
            self.terminal_workspace_visible = visible;
            self.sync_terminal_visibility(cx);
            self.sync_split_terminal_visibility(cx);
            self.sync_remote_cli_visibility(cx);
        }
    }
    pub(super) fn select_terminal(&mut self, id: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.terminals.iter().find(|session| session.panel.entity_id() == id) else { return; };
        let save = tab.panel.read(cx).resume().is_some()
            && (self.active_terminal != Some(id) || !self.terminal_panes.visible(id));
        self.active_terminal = Some(id);
        self.terminal_panes.select(id);
        if save { self.queue_workspace_save(cx); }
        self.terminal_visible = true;
        self.reopen_closing_pane(pane_layout::Pane::Terminal, cx);
        self.sync_terminal_visibility(cx);
        if let Some(panel) = self.active_terminal_panel() { panel.read(cx).focus_handle(cx).focus(window); }
        cx.notify();
    }
    pub(super) fn open_terminal_launcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.startup_owner_pristine = false;
        self.sync_chat_terminal_owner(cx);
        if self.quit_pending { return; }
        if self.terminal_launcher.is_some() {
            self.finish_terminal_launcher(None, window, cx);
            return;
        }
        let directory = self.execution_root().unwrap_or_else(|| self.project.clone());
        let directory_name = directory.file_name().map_or_else(|| directory.display().to_string(), |name| name.to_string_lossy().into_owned());
        let executable = self.agents.iter().find(|agent| agent.kind == self.agent)
            .and_then(|agent| agent.executable.clone());
        let focus = cx.focus_handle();
        focus.focus(window);
        self.terminal_launcher = Some(TerminalLauncher {
            focus,
            selection: 0,
            previously_visible: self.terminal_visible,
            agent: self.agent,
            executable,
            directory_name,
        });
        self.terminal_visible = true;
        self.reopen_closing_pane(pane_layout::Pane::Terminal, cx);
        self.sync_chat_terminal_owner(cx);
        self.sync_terminal_visibility(cx);
        cx.notify();
    }

    pub(super) fn confirm_terminal_launcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(launcher) = &self.terminal_launcher {
            self.finish_terminal_launcher(Some(launcher.selection), window, cx);
        }
    }

    pub(super) fn finish_terminal_launcher(&mut self, option: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(launcher) = self.terminal_launcher.take() else { return; };
        self.terminal_visible = launcher.previously_visible;
        self.sync_chat_terminal_owner(cx);
        match option {
            Some(0) => self.new_terminal(window, cx),
            Some(1) if launcher.executable.is_some() => {
                self.launch_terminal(launcher.executable.map(|path| (launcher.agent, path)), window, cx);
            }
            _ => {
                self.sync_terminal_visibility(cx);
                if let Some(panel) = self.active_terminal_panel().filter(|_| self.terminal_visible) {
                    panel.read(cx).focus_handle(cx).focus(window);
                } else {
                    self.composer.read(cx).focus_handle.focus(window);
                }
            }
        }
        cx.notify();
    }

    pub(super) fn render_terminal_launcher(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let launcher = self.terminal_launcher.as_ref()?;
        let agent = launcher.agent;
        let available = launcher.executable.is_some();
        let choice = |index: usize, glyph: AnyElement, title: String, enabled: bool| {
            div().id(("terminal-launch-option", index)).h(px(34.)).px_2().rounded_md()
                .flex().items_center().gap_2()
                .opacity(if enabled { 1. } else { 0.45 })
                .when(index == launcher.selection, |row| row.bg(rgba(GLASS_HOVER)))
                .when(enabled, |row| row.cursor_pointer().hover(|row| row.bg(rgba(GLASS_HOVER)))
                    .on_click(cx.listener(move |app, _, window, cx| app.finish_terminal_launcher(Some(index), window, cx))))
                .child(glyph)
                .child(div().flex_1().min_w(px(0.)).truncate().child(title))
                .child(div().text_color(rgb(MUTED)).child(format!("{}", index + 1)))
        };
        Some(div().id("terminal-launcher").track_focus(&launcher.focus).size_full()
            .flex().items_center().justify_center()
            .on_key_down(cx.listener(|app, event: &KeyDownEvent, window, cx| {
                let Some(launcher) = app.terminal_launcher.as_mut() else { return; };
                let stroke = &event.keystroke;
                if stroke.modifiers.platform || stroke.modifiers.control || stroke.modifiers.alt || stroke.modifiers.shift { return; }
                match stroke.key.as_str() {
                    "up" | "down" if launcher.executable.is_some() => {
                        launcher.selection = 1 - launcher.selection;
                        cx.notify();
                    }
                    "enter" => {
                        let option = launcher.selection;
                        app.finish_terminal_launcher(Some(option), window, cx);
                    }
                    "1" => app.finish_terminal_launcher(Some(0), window, cx),
                    "2" if launcher.executable.is_some() => app.finish_terminal_launcher(Some(1), window, cx),
                    "escape" | "tab" => app.finish_terminal_launcher(None, window, cx),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .child(div().w(px(284.)).max_w(relative(0.94)).p_1().rounded_lg().border_1()
                .border_color(rgba(0xebebeb1c)).glass_popup().occlude()
                .text_size(theme::ui_px(12.))
                .child(choice(0, icon("terminal", 15., TEXT).into_any_element(), "New terminal".into(), true))
                .child(choice(1, provider_icon(agent_icon(agent), 15., agent_color(agent)).into_any_element(),
                    format!("{} in {}", agent.name(), launcher.directory_name), available)))
            .into_any_element())
    }

    fn focus_terminal_deck(&mut self, source: EntityId, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let contains = |sessions: &[terminal_ui::Session]| sessions.iter().any(|session| session.panel.entity_id() == source);
        if contains(&self.terminals) { return true; }
        let owner = self.parked_terminals.iter().find_map(|(owner, deck)|
            contains(&deck.sessions).then_some(owner.clone()));
        let focused = match owner {
            Some(TerminalOwner::Chat(id)) => self.focus_split_chat(&id, window, cx),
            Some(owner @ TerminalOwner::Remote(..)) => self.focus_remote_terminal_pane(&owner, cx),
            _ => false,
        };
        if !focused { return false; }
        self.sync_chat_terminal_owner(cx);
        contains(&self.terminals)
    }

    pub(super) fn terminal_keyboard_command(&mut self,source:EntityId,command:&str,window:&mut Window,cx:&mut Context<Self>)->bool{
        let numbered=keyboard::tab_index(command);
        let direction=matches!(command,"split_left"|"split_right"|"split_up"|"split_down");
        if numbered.is_none() && !direction{return false}
        if !self.focus_terminal_deck(source, window, cx) { return false; }
        if let Some(index)=numbered{
            if let Some(id)=self.terminal_panes.tabs_for(source).get(index).copied(){self.select_terminal(id,window,cx);}
            return true
        }
        self.select_terminal(source,window,cx);
        if !self.terminal_panes.move_direction(source,command){
            let side=match command{"split_left"=>terminal_dock::Side::Left,"split_right"=>terminal_dock::Side::Right,"split_up"=>terminal_dock::Side::Top,_=>terminal_dock::Side::Bottom};
            self.dock_terminal(side,cx);
        }
        self.sync_terminal_visibility(cx);self.queue_workspace_save(cx);cx.notify();true
    }
    pub(super) fn new_terminal_tab_from(&mut self, source: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        if !self.focus_terminal_deck(source, window, cx) { return; }
        self.select_terminal(source, window, cx);
        self.new_terminal(window, cx);
    }

    pub(super) fn new_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.launch_terminal(None, window, cx);
    }

    fn launch_terminal(&mut self, agent: Option<(AgentKind, PathBuf)>, window: &mut Window, cx: &mut Context<Self>) {
        self.startup_owner_pristine = false;
        if self.quit_pending { return; }
        self.sync_chat_terminal_owner(cx);
        if self.remote_workspace.active() {
            self.open_remote_terminal(true, window, cx);
            return;
        }
        if self.checkpoint_busy > 0 {
            self.notice = Some("Wait for file checkpoint work before opening a terminal.".into());
            cx.notify();
            return;
        }
        let saved_count: usize = self.saved_terminal_decks.iter().map(|deck| deck.sessions.len()).sum();
        if saved_count + self.all_terminals().count() >= 128 {
            self.notice = Some("Close a terminal before opening another.".into());
            cx.notify();
            return;
        }
        let root = self.execution_root().unwrap_or_else(|| self.project.clone());
        let chats_root = self.unprojected_root();
        let create_workspace = self.project == chats_root && root.parent() == Some(chats_root.as_path())
            && root.file_name().and_then(|name|name.to_str()).is_some_and(|name|uuid::Uuid::parse_str(name).is_ok());
        self.restore_terminal_tabs(window, cx);
        let backend = self.terminal_backend;
        let id = self.add_terminal(root, create_workspace, backend, None, agent, window, cx);
        self.select_terminal(id, window, cx);
    }

    fn restore_terminal_tabs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.saved_terminal_decks.iter().position(|deck| Some(&deck.owner) == self.terminal_owner.as_ref()) else { return; };
        let deck = self.saved_terminal_decks.remove(index);
        let mut tabs = HashMap::new();
        for resume in deck.sessions {
            let session = resume.id;
            let id = self.add_terminal(resume.project.clone(), false, resume.backend, Some(resume), None, window, cx);
            tabs.insert(session, id);
            self.active_terminal = Some(id);
        }
        if let Some(layout) = deck.layout {
            self.terminal_panes.restore(layout, |id| tabs.get(&id).copied());
        }
        if let Some(active) = deck.active.and_then(|id| tabs.get(&id).copied()) {
            self.active_terminal = Some(active);
        }
    }

    fn add_terminal(
        &mut self,
        root: PathBuf,
        create_workspace: bool,
        backend: terminal::Backend,
        resume: Option<terminal::Resume>,
        agent: Option<(AgentKind, PathBuf)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> EntityId {
        let profile = if cfg!(test) || std::env::args().any(|arg| arg == "--unified-smoke-test") {
            PathBuf::new()
        } else {
            self.attachment_dir.parent().unwrap_or(&self.attachment_dir).to_owned()
        };
        let panel = cx.new(|cx| match agent {
            Some((agent, executable)) => terminal_ui::TerminalPanel::new_agent_terminal(root, create_workspace, backend, agent, executable, profile, cx),
            None => terminal_ui::TerminalPanel::new_workspace(root, create_workspace, backend, profile, resume, cx),
        });
        self.register_terminal(panel, window, cx)
    }

    pub(super) fn register_terminal(&mut self, panel: Entity<terminal_ui::TerminalPanel>, window: &mut Window, cx: &mut Context<Self>) -> EntityId {
        let id = panel.entity_id();
        let session = self.terminal_session(panel, window, cx);
        self.terminal_panes.insert(id, self.active_terminal);
        self.terminals.push(session);
        id
    }

    pub(super) fn attach_parked_terminal(&mut self, owner: TerminalOwner, panel: Entity<terminal_ui::TerminalPanel>, window: &mut Window, cx: &mut Context<Self>) {
        let id = panel.entity_id();
        let session = self.terminal_session(panel, window, cx);
        let deck = self.parked_terminals.entry(owner).or_insert_with(|| TerminalDeck {
            sessions: Vec::new(), active: None, layout: Default::default(), visible: false,
        });
        deck.layout.insert(id, deck.active);
        deck.sessions.push(session);
        deck.active = Some(id);
        deck.visible = true;
        self.sync_split_terminal_visibility(cx);
    }

    fn terminal_session(&mut self, panel: Entity<terminal_ui::TerminalPanel>, window: &mut Window, cx: &mut Context<Self>) -> terminal_ui::Session {
        let subscription = cx.subscribe_in(&panel, window, |this, panel, event, window, cx| {
            if matches!(event, terminal_ui::TerminalEvent::PersistenceChanged) {
                this.queue_workspace_save(cx);
                cx.notify();
                return;
            }
            if matches!(event, terminal_ui::TerminalEvent::New) {
                this.new_terminal_tab_from(panel.entity_id(), window, cx);
                return;
            }
            if !this.terminals.iter().any(|session| session.panel.entity_id() == panel.entity_id()) {
                if matches!(event, terminal_ui::TerminalEvent::Close)
                    && let Some((owner, deck)) = this.parked_terminals.iter_mut().find(|(_, deck)| deck.sessions.iter().any(|session| session.panel.entity_id() == panel.entity_id()))
                {
                    let replacement = deck.layout.remove(panel.entity_id());
                    deck.sessions.retain(|session| session.panel.entity_id() != panel.entity_id());
                    if deck.active == Some(panel.entity_id()) { deck.active = replacement; }
                    if deck.sessions.is_empty() { deck.visible = false; }
                    let visible = deck.visible && this.split_terminal_owners.contains(owner);
                    for session in &deck.sessions {
                        session.panel.update(cx, |panel, _| panel.set_visible(visible && deck.layout.visible(session.panel.entity_id())));
                    }
                    cx.notify();
                }
                return;
            }
            match event {
                terminal_ui::TerminalEvent::Dock(side) => this.dock_terminal(*side, cx),
                terminal_ui::TerminalEvent::DockStarted => this.terminal_dock_hover = None,
                terminal_ui::TerminalEvent::TitleChanged => cx.notify(),
                terminal_ui::TerminalEvent::PersistenceChanged | terminal_ui::TerminalEvent::Focused => {},
                terminal_ui::TerminalEvent::New => unreachable!(),
                terminal_ui::TerminalEvent::Closing => {
                    if this.terminals.len() == 1 && this.terminal_visible {
                        this.close_pane(pane_layout::Pane::Terminal, cx);
                    }
                }
                terminal_ui::TerminalEvent::CloseFailed => {
                    if this.terminals.len() == 1 && this.terminal_visible {
                        this.reopen_closing_pane(pane_layout::Pane::Terminal, cx);
                    } else {
                        this.notice = panel.read(cx).error().map(str::to_owned);
                    }
                    cx.notify();
                }
                terminal_ui::TerminalEvent::Close => {
                    if let Some(index) = this.terminals.iter().position(|session| session.panel.entity_id() == panel.entity_id()) {
                        let replacement = this.terminal_panes.remove(panel.entity_id());
                        this.terminals.remove(index);
                        if this.active_terminal == Some(panel.entity_id()) {
                            this.active_terminal = replacement;
                        }
                        if this.motion && !native::reduced_motion() {
                            for session in &this.terminals {
                                if let Some(id) = session.panel.read(cx).metal_id() {
                                    unified::emit(&serde_json::json!({"type":"dockTerminalAnimateResize","id":id}));
                                }
                            }
                        }
                        if this.terminals.is_empty() {
                            this.close_pane(pane_layout::Pane::Terminal, cx);
                            this.composer.read(cx).focus_handle(cx).focus(window);
                        } else if this.terminal_visible && let Some(panel) = this.active_terminal_panel() {
                            panel.read(cx).focus_handle(cx).focus(window);
                        }
                        this.sync_terminal_visibility(cx);
                        cx.notify();
                    }
                }
            }
        });
        let number = self.next_terminal_number;
        self.next_terminal_number += 1;
        terminal_ui::Session { number, panel, _subscription: subscription }
    }
    pub(super) fn toggle_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.terminal_launcher.is_some() {
            self.finish_terminal_launcher(None, window, cx);
            return;
        }
        self.sync_chat_terminal_owner(cx);
        if self.reopen_closing_pane(pane_layout::Pane::Terminal,cx) {
            self.sync_terminal_visibility(cx);
            if let Some(panel) = self.active_terminal_panel() { panel.read(cx).focus_handle(cx).focus(window); }
            return;
        }
        if self.terminal_visible {
            if self.remote_workspace.active() { self.toggle_pending_remote_terminal(true); }
            if self.active_terminal_panel().is_some_and(|panel| panel.read(cx).focus_handle(cx).is_focused(window)) {
                self.composer.read(cx).focus_handle(cx).focus(window);
            }
            self.close_pane(pane_layout::Pane::Terminal,cx);
            return;
        }
        if self.remote_workspace.active() && self.toggle_pending_remote_terminal(false) {
            cx.notify();
            return;
        }
        if self.quit_pending { return; }
        if self.checkpoint_busy == 0 { self.restore_terminal_tabs(window, cx); }
        if self.terminals.is_empty() {
            if self.remote_workspace.active() {
                self.open_remote_terminal(false, window, cx);
                return;
            }
            if cfg!(test) || std::env::args().any(|arg| arg == "--unified-smoke-test") { self.new_terminal(window, cx); }
            else { self.open_terminal_launcher(window, cx); }
            return;
        }
        if let Some(id) = self.active_terminal { self.select_terminal(id, window, cx); }
    }
    /// GPUI's platform shutdown hook has a 100 ms deadline. Normal Quit waits
    /// before initiating platform shutdown, without blocking the UI thread.
    pub(super) fn quit_after_save(&mut self, window: &Window, cx: &mut Context<Self>) {
        if performance::enabled() { eprintln!("KAI_QUIT_BEGIN already_pending={}", self.quit_pending); }
        if self.quit_pending {
            return;
        }
        self.capture_quit_continuations();
        self.quit_pending = true;
        self.title_work=None;self.title_queue.clear();
        self.background
            .update(cx, |background, cx| background.set_quitting(true, cx));
        self.cancel_pending_launches();
        let broker_shutdown = self.shutdown_app_brokers();
        for session in self.all_terminals() {
            session.panel.update(cx, |terminal, _| terminal.stop());
        }
        self.stop_dictation(cx);
        self.stop_remote_voice(cx);
        self.stop_voice(cx);
        if window.window_handle().window_id() == self.primary_window {
            self.window_state = Some(capture_window(window));
        }
        self.save_workspace_now(cx);
        for run in self.runs.values_mut().filter(|r|r.handle.durable.is_none()) {
            run.status = "Stopping…".into();
        }
        cx.spawn(async move |this, cx| {
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                let Ok(pending) = this.update(cx, |this, cx| {
                    // A launch already in flight can return its handle after
                    // Quit. Include it before allowing the final save/exit.
                    for run in this.runs.values_mut().filter(|r|r.handle.durable.is_none()) {
                        if !run.handle.is_cancelled() {
                            run.handle.cancel();
                        }
                        run.status = "Stopping…".into();
                    }
                    let settled: Vec<_> = this
                        .runs
                        .iter()
                        .filter(|(_, run)| run.handle.settled.is_closed())
                        .map(|(id, _)| id.clone())
                        .collect();
                    // A final full queue takes another bounded UI batch to
                    // observe disconnect and enqueue its canonical source.
                    for id in settled {
                        this.poll(&id, cx);
                    }
                    !broker_shutdown.is_closed() || this.starting || !this.team_starting.is_empty() || this.runs.values().any(|r|r.handle.durable.is_none()) || this.pending_project_creations > 0 || this.checkpoint_busy > 0 || this.file_browser.as_ref().is_some_and(|browser|browser.read(cx).editor_busy(cx)) || this.all_terminals().any(|session| !session.panel.read(cx).is_settled())
                }) else {
                    return;
                };
                if !pending {
                    break;
                }
                if Instant::now() >= deadline {
                    let _ = this.update(cx, |this, cx| {
                        if performance::enabled() {
                            eprintln!("KAI_QUIT_PENDING broker={} launch={} team={} runs={} projects={} checkpoints={} editor={} terminals={}",
                                !broker_shutdown.is_closed(), this.starting, this.team_starting.len(),
                                this.runs.values().filter(|r| r.handle.durable.is_none()).count(),
                                this.pending_project_creations, this.checkpoint_busy,
                                this.file_browser.as_ref().is_some_and(|browser| browser.read(cx).editor_busy(cx)),
                                this.all_terminals().filter(|s| !s.panel.read(cx).is_settled()).count());
                        }
                        this.quit_pending = false;
                        this.notice = Some(
                            "Code is still finishing background work. The app stayed open to preserve it."
                                .into(),
                        );
                        cx.notify();
                    });
                    return;
                }
                Timer::after(Duration::from_millis(16)).await;
            }
            let Ok((history, preferences, saved)) = this.update(cx, |this, cx| {
                this.save_workspace_now(cx);
                (
                    this.engine.flush(),
                    this.preferences.flush(),
                    this.workspace_store.as_ref().map(|store| store.flush()),
                )
            }) else {
                return;
            };
            let result = cx
                .background_executor()
                .spawn(async move {
                    history
                        .recv_timeout(Duration::from_secs(10))
                        .map_err(|error| format!("History save did not finish: {error}"))?;
                    preferences
                        .recv_timeout(Duration::from_secs(10))
                        .map_err(|error| {
                            format!("Project and chat settings save did not finish: {error}")
                        })??;
                    if let Some(saved) = saved {
                        saved
                            .recv_timeout(Duration::from_secs(10))
                            .map_err(|error| format!("Workspace save did not finish: {error}"))?
                            .map_err(|error| format!("Could not save workspace: {error}"))?;
                    }
                    Ok::<_, String>(())
                })
                .await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(()) => {
                    if performance::enabled() { eprintln!("KAI_QUIT_READY"); }
                    unified::emit(&serde_json::json!({"type":"quitReady"}));
                    #[cfg(not(test))]
                    cx.quit();
                    #[cfg(test)]
                    {
                        let _ = (this, cx);
                    }
                }
                Err(error) => {
                    if performance::enabled() { eprintln!("KAI_QUIT_SAVE_FAILED {error}"); }
                    this.quit_pending = false;
                    this.notice = Some(error);
                    cx.notify();
                }
            });
        })
        .detach();
    }
    pub(super) fn change_appearance(
        &mut self,
        appearance: appearance::Appearance,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let appearance = appearance.normalized();
        if self.appearance == appearance {
            return;
        }
        self.appearance = appearance;
        self.apply_personalization(window, cx);
        if let Some(browser) = &self.file_browser {
            browser.update(cx, |browser, cx| browser.set_appearance(appearance, cx));
        }
        if let Some(review) = &self.diff_view {
            review.update(cx, |review, cx| review.set_appearance(appearance, cx));
        }
        self.queue_workspace_save(cx);
        cx.notify();
    }
    fn workspace_fingerprint(&self, cx: &App) -> u64 {
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        self.activity.revision().hash(&mut hash);
        self.project.hash(&mut hash);
        self.new_execution_path.hash(&mut hash);
        self.active.hash(&mut hash);
        self.open_tabs.hash(&mut hash);
        self.agent.command().hash(&mut hash);
        self.model.hash(&mut hash);self.unified_text_morph.hash(&mut hash);self.reasoning_efforts.hash(&mut hash);
        self.codex_approvals.hash(&mut hash);
        self.codex_access.hash(&mut hash);
        self.sidebar_split.hash(&mut hash);
        let mut expanded: Vec<_> = self.expanded_projects.iter().collect();
        expanded.sort_unstable();
        expanded.hash(&mut hash);
        (
            self.allow_edits,
            self.motion,
            self.sidebar,
            self.conversations_visible,
            self.files_visible,
            self.all_projects,
            self.diff_view.is_some(),
        )
            .hash(&mut hash);
        self.composer_revision.hash(&mut hash);
        self.appearance.blur_radius.hash(&mut hash);
        self.appearance.opacity.to_bits().hash(&mut hash);
        self.theme.hash(&mut hash);
        self.voice_choice.hash(&mut hash);
        self.terminal_backend.hash(&mut hash);
        (
            &self.quick_shortcut.key,
            self.quick_shortcut.control,
            self.quick_shortcut.alt,
            self.quick_shortcut.shift,
            self.quick_shortcut.command,
            self.quick_shortcut.key_code,
        )
            .hash(&mut hash);
        if let Some(window) = &self.window_state {
            (
                window.x.to_bits(),
                window.y.to_bits(),
                window.width.to_bits(),
                window.height.to_bits(),
                window.maximized,
                window.fullscreen,
            )
                .hash(&mut hash);
        }
        if let Some(browser) = &self.file_browser {
            browser.read(cx).selected_key().hash(&mut hash);
            browser.read(cx).editor_revision(cx).hash(&mut hash);
        }
        if let Some(review) = &self.diff_view {
            review.read(cx).selected_key().hash(&mut hash);
        }
        hash.finish()
    }
    pub(super) fn track_workspace(&mut self, cx: &mut Context<Self>) {
        if self.workspace_store.is_none() || self.restore_pending.is_some() {
            return;
        }
        let signature = self.workspace_fingerprint(cx);
        if self.workspace_signature != signature {
            self.workspace_signature = signature;
            self.queue_workspace_save(cx);
        }
    }
    pub(super) fn queue_workspace_save(&mut self, cx: &mut Context<Self>) {
        if self.workspace_store.is_none() || self.restore_pending.is_some() {
            return;
        }
        self.workspace_save = Some(cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_millis(250)).await;
            let _ = this.update(cx, |this, cx| {
                this.workspace_save = None;
                this.save_workspace_now(cx);
            });
        }));
    }
    fn transcript_anchor(&self) -> Option<TranscriptAnchor> {
        if self.active.is_none() || self.loading || self.rows.is_empty() {
            return None;
        }
        let offset = self.list.logical_scroll_top();
        let page = self
            .history_pages
            .iter()
            .find(|page| offset.item_ix >= page.start && offset.item_ix < page.start + page.len);
        Some(TranscriptAnchor {
            history_offset: page.map_or(0, |page| page.number),
            row_index: page.map_or(offset.item_ix, |page| offset.item_ix - page.start),
            pixel_offset: offset.offset_in_item.into(),
            at_bottom: self.following_bottom,
        })
    }
    /// Check the proposed metadata before changing durable work or clearing a
    /// draft. Borrowed references keep this proportional to the bounded state.
    pub(super) fn workflow_budget_allows(
        &mut self,
        prompt: Option<&prompt_queue::Prompt>,
        team: Option<&teams::Team>,
        context: Option<(&str, &[context_refs::Reference])>,
        cx: &mut Context<Self>,
    ) -> bool {
        let queue: Vec<_> = self
            .prompt_queue
            .iter()
            .filter(|saved| prompt.is_none_or(|next| saved.id != next.id))
            .chain(prompt)
            .collect();
        let teams: Vec<_> = self.teams.iter().chain(team).collect();
        let mut references: Vec<_> = self
            .chat_context
            .iter()
            .filter(|(scope, refs)| {
                !refs.is_empty() && context.is_none_or(|(replaced, _)| scope.as_str() != replaced)
            })
            .map(|(scope, refs)| (scope.as_str(), refs.as_slice()))
            .collect();
        if let Some((scope, refs)) = context
            && !refs.is_empty()
        {
            references.push((scope, refs));
        }
        if workspace_state::workflow_fits(&queue, &teams, &references) {
            return true;
        }
        self.notice = Some("Saved workflow metadata reached its budget. Remove an unused team or queued prompt, or attached context, before adding more. Your draft and existing work are preserved.".into());
        cx.notify();
        false
    }
    fn terminal_decks_snapshot(&self, cx: &App) -> Vec<SavedTerminalDeck> {
        let mut decks = self.saved_terminal_decks.clone();
        for (owner, tabs, layout, active) in self.terminal_owner.iter()
            .map(|owner| (owner, &self.terminals, &self.terminal_panes, self.active_terminal))
            .chain(self.parked_terminals.iter().map(|(owner, deck)| (owner, &deck.sessions, &deck.layout, deck.active)))
        {
            let saved: HashMap<_, _> = tabs.iter().filter_map(|tab| {
                let resume = tab.panel.read(cx).resume()?;
                Some((tab.panel.entity_id(), resume.id))
            }).collect();
            let sessions: Vec<_> = tabs.iter().filter_map(|tab| tab.panel.read(cx).resume().cloned()).collect();
            if !sessions.is_empty() {
                decks.push(SavedTerminalDeck {
                    owner: owner.clone(), sessions,
                    layout: layout.snapshot(|id| saved.get(&id).copied()),
                    active: active.and_then(|id| saved.get(&id).copied()),
                });
            }
        }
        decks.sort_unstable_by(|a, b| a.owner.cmp(&b.owner));
        decks
    }

    pub(super) fn workspace_snapshot(&self, cx: &App) -> WorkspaceState {
        let mut drafts: std::collections::BTreeMap<_, _> = self
            .open_tabs
            .iter()
            .filter_map(|id| self.drafts.get(id).map(|text| (id.clone(), text.clone())))
            .collect();
        let active_key = self
            .active
            .clone()
            .unwrap_or_else(|| "__new_conversation__".into());
        let text = self.composer.read(cx).text();
        if !text.is_empty() {
            drafts.insert(active_key.clone(), text);
        } else {
            drafts.remove(&active_key);
        }
        let file = self
            .file_browser
            .as_ref()
            .and_then(|browser| browser.read(cx).workspace_selection());
        let mut expanded_projects: Vec<_> = self.expanded_projects.iter().cloned().collect();
        expanded_projects.sort_unstable();
        expanded_projects.truncate(workspace_state::MAX_EXPANDED_PROJECTS);
        let draft_sessions: Vec<_> = self.sessions.iter()
            .filter(|session| self.draft_sessions.contains(&session.id)
                && (drafts.get(&session.id).is_some_and(|draft| !draft.trim().is_empty())
                    || self.chat_clis.get(&session.id).is_some_and(|cli| cli.persistent)
                    || self.attachment_drafts.get(&session.id).is_some_and(|images| !images.is_empty())))
            .take(keyboard::MAX_CHAT_TABS)
            .map(|session| (session.clone(), self.session_models.get(&session.id).cloned().flatten()))
            .collect();
        let keep_tab = |id: &String| !self.draft_sessions.contains(id)
            || draft_sessions.iter().any(|(session, _)| &session.id == id);
        WorkspaceState {
            ninja:self.ninja.draft.clone(),ninja_chats:self.ninja.chats.clone(),ninja_bound:self.ninja.bound.clone(),            workflows:self.workflows.clone(),recent_results:self.recent_results.iter().rev().take(32).cloned().collect::<Vec<_>>().into_iter().rev().collect(),
            startup_surface:self.startup_surface,
            automatic_titles:self.automatic_titles,
            terminal_dock:self.terminal_dock.normalized(),
            remote_terminal_docks: self.remote_terminal_docks.iter().map(|(owner, dock)| (owner.clone(), *dock)).collect(),
            terminal_backend: self.terminal_backend,
            terminal_decks: self.terminal_decks_snapshot(cx),
            image_drafts:self.image_draft_snapshot(),chat_split:self.chat_split_snapshot(),
            cli_sessions:self.cli_snapshot(),cli_draft_project:self.cli_draft_project.clone(),
            inspector_widths:self.inspector_widths.normalized(),
            activity: self.activity.saved.clone(),
            prompt_queue: self.prompt_queue.clone(),
            teams: self.teams.clone(),orchestrator_chats:self.orchestrator_chats.iter().cloned().collect(),
            remote_deliveries: self.remote_deliveries.clone(),
            chat_context: self
                .chat_context
                .iter()
                .map(|(key, refs)| (key.clone(), refs.clone()))
                .collect(),
            window: self.window_state.clone(),
            project: Some(self.project.clone()),
            execution_path: self.execution_root(),
            agent: self.agent.command().to_owned(),
            model: self.model.clone(),
            text_morph:self.unified_text_morph,
            turn_timings:self.turn_timings.clone(),
            reasoning_efforts:self.reasoning_efforts.clone(),
            allow_edits: self.allow_edits,
            codex_approvals: self.codex_approvals,
            codex_access: self.codex_access,
            unrestricted_providers: self.unrestricted_providers.iter().cloned().collect(),
            motion: self.motion,
            shortcut_overrides: self.shortcut_overrides.clone(),
            pane_profile: self.pane_profile,
            workspace_opening_once: self.workspace_opening_once,
            sidebar: self.sidebar,
            sidebar_sizes: self.sidebar_sizes,
            continuations: self.continuation_snapshot(),
            remote_pending_sends: self.remote_workspace.pending_snapshot(),
            remote_open_tabs: self.remote_workspace.saved_tabs(),
            sidebar_split: self.sidebar_split,
            sidebar_projects_collapsed:self.project_tree.projects_collapsed,sidebar_chats_collapsed:self.project_tree.chats_collapsed,
            expanded_projects,
            conversations_visible: self.conversations_visible,
            open_tabs: self.open_tabs.iter().filter(|id| keep_tab(id)).cloned().collect(),
            active_chat: self.active.as_ref().filter(|id| keep_tab(id)).cloned(),
            draft_sessions,
            drafts,
            files_visible: self.files_visible,
            all_projects: self.all_projects,
            selected_file: file.as_ref().map(|file| file.0.clone()),
            file_edit: self
                .file_browser
                .as_ref()
                .and_then(|browser| browser.read(cx).workspace_edit(cx)),
            file_offset: file.as_ref().map_or(0, |file| file.1),
            file_row: file.as_ref().map_or(0, |file| file.2.item_ix),
            file_pixel: file
                .as_ref()
                .map_or(0., |file| file.2.offset_in_item.into()),
            diff_visible: self.diff_view.is_some(),
            selected_diff: self
                .diff_view
                .as_ref()
                .and_then(|view| view.read(cx).selected_path()),
            transcript: self
                .restore_scroll
                .clone()
                .or_else(|| self.transcript_anchor()),
            appearance: self.appearance,
            theme: self.theme.clone(),
            quick_shortcut: self.quick_shortcut.clone(),
            voice_choice: self.voice_choice.clone(),
            ..WorkspaceState::default()
        }
    }
    pub(super) fn save_workspace_now(&mut self, cx: &App) {
        self.workspace_save = None;
        if self.restore_pending.is_some() {
            return;
        }
        if self.workspace_store.is_some()
            && !self.loading
            && self.restore_scroll.is_none()
            && let (Some(id), Some(anchor)) = (&self.active, self.transcript_anchor())
            && !self.runs.contains_key(id)
            && !self.finalizing.contains(id)
        {
            let key = (id.clone(), self.generation, anchor.history_offset);
            if self.saved_viewport.as_ref() != Some(&key) {
                self.engine.save_viewport(
                    id.clone(),
                    self.generation,
                    anchor.history_offset,
                    anchor.at_bottom,
                );
                self.saved_viewport = Some(key);
            }
        }
        if let Some(store) = &self.workspace_store {
            store.save(self.workspace_snapshot(cx));
        }
    }
    pub(super) fn restore_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Cached owner metadata and installed providers are enough to restore
        // the selected chat; unrelated CLI histories continue scanning behind it.
        let cached_selection_ready = !self.agents.is_empty() && self.restore_pending.as_ref().is_none_or(|saved| {
            saved.active_chat.as_ref().is_none_or(|id|self.sessions.iter().any(|session|&session.id==id))
        });
        if !self.preferences_ready || (self.discovering && !cached_selection_ready) {
            return;
        }
        let Some(saved) = self.restore_pending.take() else {
            return;
        };
        for (session, model) in saved.draft_sessions.into_iter().take(keyboard::MAX_CHAT_TABS) {
            if !self.sessions.iter().any(|known| known.id == session.id) {
                self.draft_sessions.insert(session.id.clone());
                self.session_models.insert(session.id.clone(), model);
                self.sessions.push(session);
            }
        }
        let saved_split=saved.chat_split.clone();
        self.restore_image_drafts(saved.image_drafts.clone(),cx);
        self.cli_draft_project=saved.cli_draft_project.clone();
        self.restore_cli_sessions(saved.cli_sessions.clone(),cx);
        self.continuations = saved.continuations;
        self.continuation_restoring = true;
        self.engine.load_metadata(self.continuations.iter().map(|s| s.session.id.clone()).collect());
        self.sidebar_sizes = saved.sidebar_sizes;
        self.sidebar = saved.sidebar;
        self.sidebar_split = saved.sidebar_split;
        self.expanded_projects = saved.expanded_projects.into_iter().collect();
        self.conversations_visible = saved.conversations_visible;
        self.restore_sidebar_sections(saved.sidebar_projects_collapsed,saved.sidebar_chats_collapsed);
        self.sidebar_width = if saved.sidebar {
            self.sidebar_target_width()
        } else {
            0.
        };
        self.sidebar_transition = None;
        self.sidebar_layout_transition = None;
        self.unified_text_morph=saved.text_morph;self.reasoning_efforts=saved.reasoning_efforts.clone();
        self.restore_shortcuts(saved.shortcut_overrides, cx);
        self.pane_profile = saved.pane_profile;
        if unified::active() { workspace_motion::set_pane_profile(self.pane_profile); }
        self.motion = saved.motion && std::env::var_os("ORBIT_REDUCE_MOTION").is_none();
        self.appearance = saved.appearance.normalized();
        self.theme = saved.theme.normalized();
        self.apply_personalization(window, cx);
        self.allow_edits = saved.allow_edits;
        self.inspector_widths=saved.inspector_widths.normalized();
        self.codex_approvals = saved.codex_approvals;
        self.codex_access = saved.codex_access;
        self.unrestricted_providers = saved.unrestricted_providers.into_iter().filter(|id|
            [AgentKind::Claude, AgentKind::Cursor, AgentKind::Antigravity].iter().any(|agent| agent.command() == id)
        ).collect();
        self.prompt_queue = saved.prompt_queue;
        for prompt in &mut self.prompt_queue {
            prompt.held = true;
        }
        self.orchestrator_chats=saved.orchestrator_chats.into_iter().collect();
        self.remote_workspace.restore_pending_sends(saved.remote_pending_sends);
        self.remote_workspace.restore_tabs(saved.remote_open_tabs);
        self.teams = saved.teams;
        self.remote_deliveries = saved.remote_deliveries;
        for team in &mut self.teams {
            team.hold_on_restart();
        }
        self.engine.load_metadata(
            self.teams
                .iter()
                .flat_map(|team| &team.members)
                .map(|member| member.session.clone())
                .collect(),
        );
        self.chat_context = saved.chat_context.into_iter().collect();
        if let Some(project) = saved.project.as_ref().filter(|path| {
            **path == self.unprojected_root()
                || self.projects.iter().any(|project| &project.path == *path)
                || self
                    .sessions
                    .iter()
                    .any(|session| &session.project_path == *path)
        }) {
            self.project = project.clone();
        }
        self.all_projects = saved.all_projects;
        self.activity.saved = saved.activity;
        for ids in self
            .activity
            .saved
            .restore_ids()
            .chunks(engine::ARCHIVED_PAGE_SIZE)
        {
            self.engine.load_metadata(ids.to_vec());
        }
        self.drafts = saved.drafts.into_iter().collect();
        self.open_tabs = saved
            .open_tabs
            .into_iter()
            .filter(|id| {
                !self.hidden_sessions.contains(id)
                    && self.sessions.iter().any(|session| &session.id == id)
            })
            .collect();
        let selected = saved
            .active_chat
            .as_ref()
            .and_then(|id| self.sessions.iter().find(|session| &session.id == id))
            .filter(|session| !self.hidden_sessions.contains(&session.id))
            .cloned();
        self.files_visible = saved.files_visible;
        if let Some(session) = selected {
            self.session_models
                .insert(session.id.clone(), saved.model.clone());
            self.restore_model = Some((session.id.clone(), saved.model.clone()));
            self.restore_scroll = saved.transcript;
            self.open_session(session, window, cx);
        } else {
            self.new_execution_path = saved.execution_path;
            if let Some(agent) = [
                AgentKind::Codex,
                AgentKind::Claude,
                AgentKind::Gemini,
                AgentKind::OpenCode,
                AgentKind::Antigravity,
                AgentKind::Cursor,
                AgentKind::Grok,
                AgentKind::Ninja,
            ]
            .into_iter()
            .find(|agent| agent.command() == saved.agent)
            {
                self.agent = agent;
            }
            self.model = saved.model;
            let draft = self
                .drafts
                .get("__new_conversation__")
                .cloned()
                .unwrap_or_default();
            self.composer
                .update(cx, |input, cx| input.set_text(&draft, cx));
            self.sync_file_root(window, cx);
            self.rebuild_workspace_sessions();
        }
        if let Some(draft) = saved.file_edit {
            self.files_visible = true;
            self.sync_file_root(window, cx);
            if let Some(browser) = &self.file_browser {
                browser.update(cx, |browser, cx| browser.restore_edit(draft, cx));
            }
        }
        if let (Some(file), Some(browser)) = (saved.selected_file, &self.file_browser) {
            browser.update(cx, |browser, cx| {
                browser.restore_selection(
                    file,
                    saved.file_offset,
                    ListOffset {
                        item_ix: saved.file_row,
                        offset_in_item: px(saved.file_pixel),
                    },
                    cx,
                )
            });
        }
        if saved.diff_visible {
            self.open_changes(cx);
            if let (Some(path), Some(review)) = (saved.selected_diff, &self.diff_view) {
                review.update(cx, |review, cx| review.restore_selection(path, cx));
            }
        }
        self.workspace_signature = self.workspace_fingerprint(cx);
        cx.notify();
        self.restore_chat_split(saved_split,window,cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    #[gpui::test]
    fn command_t_opens_a_dock_terminal_tab_without_changing_chats(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        let active = cx.update(|window, cx| view.update(cx, |app, cx| {
            app.new_terminal(window, cx);
            assert_eq!(app.terminals.len(), 1);
            app.active.clone()
        }));
        crate::ui_tests::draw_frame(&view, cx);
        cx.simulate_keystrokes("cmd-t");
        view.update(cx, |app, cx| {
            assert_eq!(app.terminals.len(), 2);
            assert_eq!(app.active, active);
            assert!(app.terminal_launcher.is_none());
            assert!(app.active_terminal_panel().unwrap().read(cx).launcher_agent.is_none());
        });
    }

    #[gpui::test]
    fn terminal_launcher_accepts_number_and_arrow_selection_without_a_live_shell(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        cx.update(|window, cx| view.update(cx, |app, cx| {
            let agent = app.agent;
            app.agents.iter_mut().find(|item| item.kind == agent).unwrap().executable = Some("/bin/cat".into());
            app.open_terminal_launcher(window, cx);
            assert!(app.terminals.is_empty());
        }));
        crate::ui_tests::draw_frame(&view, cx);
        cx.simulate_keystrokes("down tab");
        view.update(cx, |app, cx| {
            assert!(app.terminal_launcher.is_none());
            assert_eq!(app.active_terminal_panel().unwrap().read(cx).launcher_agent, Some(app.agent));
        });
        cx.update(|window, cx| view.update(cx, |app, cx| app.open_terminal_launcher(window, cx)));
        crate::ui_tests::draw_frame(&view, cx);
        cx.simulate_keystrokes("1");
        view.update(cx, |app, cx| {
            assert!(app.terminal_launcher.is_none());
            assert_eq!(app.active_terminal_panel().unwrap().read(cx).launcher_agent, None);
        });
    }

    #[test]
    fn saved_window_is_exact_on_existing_display_and_visible_after_display_removal() {
        let screen = Bounds::new(point(px(0.), px(0.)), size(px(1800.), px(1200.)));
        let state = WindowState {
            x: 100.,
            y: 110.,
            width: 1200.,
            height: 800.,
            ..Default::default()
        };
        let rect = fit_rect(&state, &[screen]);
        assert_eq!(rect.origin, point(px(100.), px(110.)));
        assert_eq!(rect.size, size(px(1200.), px(800.)));
        let moved = WindowState {
            x: 4000.,
            y: -1000.,
            width: 3000.,
            height: 2000.,
            ..state
        };
        let rect = fit_rect(&moved, &[screen]);
        assert_eq!(rect, screen);
    }
}
