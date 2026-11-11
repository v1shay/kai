//! Workspace-only split presentation over the existing conversation owner.
use super::*;
use terminal_dock::Side;

#[derive(Clone)]
pub(super) struct ChatDrag {
    session: SessionSummary,
    title: SharedString,
}

impl Render for ChatDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_3()
            .py_2()
            .rounded_md()
            .bg(rgba(0x242424ee))
            .child(self.title.clone())
    }
}

#[derive(Clone, Copy)]
struct ResizeSplit {
    node: usize,
    horizontal: bool,
    bounds: [f32; 4],
}

type PaneRect = [f32; 4];
const FULL_PANE: PaneRect = [0., 0., 1., 1.];

#[derive(Clone)]
enum PaneLayout {
    Pane(String),
    Branch {
        horizontal: bool,
        ratio: f32,
        first: Box<Self>,
        second: Box<Self>,
    },
}

impl PaneLayout {
    fn branch(horizontal: bool, ratio: f32, first: Self, second: Self) -> Self {
        Self::Branch { horizontal, ratio, first: Box::new(first), second: Box::new(second) }
    }

    fn split(&mut self, owner: &str, new: &str, horizontal: bool) -> bool {
        match self {
            Self::Pane(id) if id == owner => {
                *self = Self::branch(horizontal, 0.5, self.clone(), Self::Pane(new.into()));
                true
            }
            Self::Pane(_) => false,
            Self::Branch { first, second, .. } =>
                first.split(owner, new, horizontal) || second.split(owner, new, horizontal),
        }
    }

    fn without(self, owner: &str) -> Option<Self> {
        match self {
            Self::Pane(id) if id == owner => None,
            Self::Pane(_) => Some(self),
            Self::Branch { horizontal, ratio, first, second } => {
                match (first.without(owner), second.without(owner)) {
                    (Some(first), Some(second)) => Some(Self::branch(horizontal, ratio, first, second)),
                    (first, second) => first.or(second),
                }
            }
        }
    }

    fn visit<'a>(
        &'a self,
        node: usize,
        bounds: PaneRect,
        panes: &mut impl FnMut(&'a str, PaneRect),
        dividers: &mut impl FnMut(ResizeSplit, f32),
    ) {
        match self {
            Self::Pane(id) => panes(id, bounds),
            Self::Branch { horizontal, ratio, first, second } => {
                let axis = usize::from(!horizontal);
                let mut before = bounds;
                let mut after = bounds;
                before[axis + 2] *= ratio;
                after[axis] += before[axis + 2];
                after[axis + 2] -= before[axis + 2];
                dividers(ResizeSplit { node, horizontal: *horizontal, bounds }, *ratio);
                first.visit(node * 2, before, panes, dividers);
                second.visit(node * 2 + 1, after, panes, dividers);
            }
        }
    }

    fn resize(&mut self, node: usize, ratio: f32) {
        if let Self::Branch { ratio: current, first, second, .. } = self {
            match node {
                1 => *current = ratio,
                _ => {
                    // The highest bit is the root, followed by the path to the divider.
                    let child_bit = node.ilog2() - 1;
                    let remaining = node ^ (1 << (child_bit + 1)) | (1 << child_bit);
                    let child = if node & (1 << child_bit) == 0 { first } else { second };
                    child.resize(remaining, ratio);
                }
            }
        }
    }

    fn swap(&mut self, a: &str, b: &str) {
        match self {
            Self::Pane(id) if id == a => *id = b.into(),
            Self::Pane(id) if id == b => *id = a.into(),
            Self::Pane(_) => {}
            Self::Branch { first, second, .. } => {
                first.swap(a, b);
                second.swap(a, b);
            }
        }
    }
}
impl Render for ResizeSplit {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size(px(1.))
    }
}

const MAX_SPLIT_PANES: usize = 4;

pub(super) struct Split {
    sessions: Vec<String>,
    motion_from: HashMap<String, [f32; 4]>,
    companion_editors: [(Entity<TextInput>, Subscription); MAX_SPLIT_PANES - 1],
    horizontal: bool,
    ratio: f32,
    layout: Option<PaneLayout>,
    started: Instant,
    tails: HashMap<String, Tail>,
    wallpaper_project: PathBuf,
    expanded_work: HashSet<(String, usize)>,
    expanded_interactions: HashSet<(String, usize)>,
    pending_sends: HashMap<String, String>,
}

struct Tail {
    request: uuid::Uuid,
    rows: Option<Result<Vec<Row>, String>>,
    flow: Option<chat_flow::Index>,
    list: ListState,
}

impl Split {
    fn legacy_layout(&self) -> PaneLayout {
        let pane = |index: usize| PaneLayout::Pane(self.sessions[index].clone());
        let cross = |a, b| PaneLayout::branch(!self.horizontal, 0.5, pane(a), pane(b));
        match self.sessions.len() {
            1 => pane(0),
            2 => PaneLayout::branch(self.horizontal, self.ratio, pane(0), pane(1)),
            3 => PaneLayout::branch(self.horizontal, self.ratio, pane(0), cross(1, 2)),
            _ => PaneLayout::branch(self.horizontal, self.ratio, cross(0, 3), cross(1, 2)),
        }
    }

    fn target_rect(&self, index: usize) -> PaneRect {
        if let Some(layout) = &self.layout {
            let mut result = FULL_PANE;
            layout.visit(1, FULL_PANE, &mut |id, bounds| {
                if id == self.sessions[index] { result = bounds; }
            }, &mut |_, _| {});
            return result;
        }
        if self.sessions.len() == 1 { return [0., 0., 1., 1.]; }
        let r = self.ratio;
        if self.sessions.len() == MAX_SPLIT_PANES {
            return match (self.horizontal, index) {
                (true, 0) => [0., 0., r, 0.5],
                (true, 1) => [r, 0., 1. - r, 0.5],
                (true, 2) => [r, 0.5, 1. - r, 0.5],
                (true, _) => [0., 0.5, r, 0.5],
                (false, 0) => [0., 0., 0.5, r],
                (false, 1) => [0., r, 0.5, 1. - r],
                (false, 2) => [0.5, r, 0.5, 1. - r],
                (false, _) => [0.5, 0., 0.5, r],
            };
        }
        match (self.horizontal, index, self.sessions.len() == 3) {
            (true, 0, _) => [0., 0., r, 1.],
            (true, 1, true) => [r, 0., 1. - r, 0.5],
            (true, 2, true) => [r, 0.5, 1. - r, 0.5],
            (true, _, _) => [r, 0., 1. - r, 1.],
            (false, 0, _) => [0., 0., 1., r],
            (false, 1, true) => [0., r, 0.5, 1. - r],
            (false, 2, true) => [0.5, r, 0.5, 1. - r],
            (false, _, _) => [0., r, 1., 1. - r],
        }
    }

    fn rect(&self, index: usize, progress: f32) -> [f32; 4] {
        let target = self.target_rect(index);
        let Some(from) = self.motion_from.get(&self.sessions[index]) else { return target; };
        std::array::from_fn(|i| from[i] + (target[i] - from[i]) * progress)
    }

    fn begin_layout_motion(&mut self) {
        let elapsed = self.started.elapsed().as_secs_f32() / workspace_motion::PANE_DURATION.as_secs_f32();
        let progress = workspace_motion::ease_out(elapsed.min(1.));
        self.motion_from = self.sessions.iter().enumerate().map(|(index, id)| (id.clone(), self.rect(index, progress))).collect();
        self.started = Instant::now();
    }
}

impl Split {
    pub(super) fn contains(&self, id: &str) -> bool {
        self.sessions.iter().any(|session| session == id)
    }
}

impl Orbit {
    pub(super) fn chat_drag(&self, session: &SessionSummary) -> ChatDrag {
        ChatDrag {
            session: session.clone(),
            title: self.chat_label(session).into(),
        }
    }

    pub(super) fn split_peer(&self) -> Option<&str> {
        if self.ninja_surface_owner().is_some() {
            return None;
        }
        let active = self.active.as_deref()?;
        let split = self.chat_split.as_ref()?;
        if !split.contains(active) { return None; }
        split.sessions.iter().find(|id| id.as_str() != active).map(String::as_str)
    }

    pub(super) fn split_companions(&self) -> impl Iterator<Item = &str> {
        let active = self.active.as_deref();
        self.chat_split.as_ref().filter(|split| self.ninja_surface_owner().is_none()
            && active.is_some_and(|id| split.contains(id))).into_iter()
            .flat_map(move |split| split.sessions.iter().map(String::as_str).filter(move |id| Some(*id) != active))
    }

    pub(super) fn close_chat_split(&mut self, cx: &mut Context<Self>) {
        self.chat_split = None;
        self.sync_split_terminal_visibility(cx);
        cx.notify();
    }

    pub(super) fn remove_split_pane(&mut self, id: &str) -> Option<String> {
        let split = self.chat_split.as_mut()?;
        let index = split.sessions.iter().position(|session| session == id)?;
        split.begin_layout_motion();
        split.sessions.remove(index);
        split.layout = split.layout.take().and_then(|layout| layout.without(id));
        split.motion_from.remove(id);
        split.tails.remove(id);
        split.pending_sends.remove(id);
        split.expanded_work.retain(|(owner, _)| owner != id);
        split.expanded_interactions.retain(|(owner, _)| owner != id);
        let next = split.sessions.get(index).or_else(|| split.sessions.last()).cloned();
        if next.is_none() { self.chat_split = None; }
        next
    }

    pub(super) fn finish_split_close_motion(&mut self) {
        if self.chat_split.as_ref().is_some_and(|split| split.sessions.len() == 1
            && (!self.motion || native::reduced_motion()
                || split.started.elapsed() >= workspace_motion::PANE_DURATION))
        {
            self.chat_split = None;
        }
    }

    pub(super) fn split_chat(
        &mut self,
        session: SessionSummary,
        side: Side,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(current) = self.active.clone().filter(|id| *id != session.id) else {
            return;
        };
        if self.ninja_surface_owner().is_some() || self.hidden_sessions.contains(&session.id) {
            return;
        }
        let id = session.id.clone();
        let sessions = if side.before() {
            vec![id.clone(), current.clone()]
        } else {
            vec![current.clone(), id.clone()]
        };
        let companion_editors = std::array::from_fn(|index| self.split_editor(index, window, cx));
        let wallpaper_project = self.project.clone();
        self.chat_split = Some(Split {
            sessions,
            motion_from: HashMap::new(),
            companion_editors,
            horizontal: side.horizontal(),
            ratio: 0.5,
            layout: None,
            started: Instant::now(),
            tails: HashMap::new(),
            wallpaper_project,
            expanded_work: HashSet::new(),
            expanded_interactions: HashSet::new(),
            pending_sends: HashMap::new(),
        });
        self.sync_split_composer(cx);
        self.refresh_split_tail(&current, cx);
        self.refresh_split_tail(&id, cx);
        if !self.open_tabs.contains(&id) && self.open_tabs.len() < keyboard::MAX_CHAT_TABS {
            self.open_tabs.push(id.clone());
        }
        self.sync_split_terminal_visibility(cx);
        if self.runs.contains_key(&id) {
            self.update_stream(&id, cx);
        }
        native::panel_drag_guard(window, false);
        cx.notify();
    }

    fn split_editor(&self, index: usize, window: &mut Window, cx: &mut Context<Self>) -> (Entity<TextInput>, Subscription) {
        let editor = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_placeholder("Ask, build, or explore this project…", cx);
            input
        });
        let focus = editor.read(cx).focus_handle.clone();
        let captured = editor.clone();
        let subscription = cx.on_focus(&focus, window, move |app, window, cx| {
            let Some(id) = app.split_companions().nth(index).map(str::to_owned) else { return; };
            let (range, reversed, _) = captured.read(cx).command_caret();
            let text = captured.read(cx).text();
            if app.focus_split_chat(&id, window, cx) && app.composer.read(cx).text() == text {
                app.composer.update(cx, |input, cx| input.follow_command_caret((range, reversed, None), cx));
            }
        });
        (editor, subscription)
    }

    pub(super) fn sync_split_composer(&self, cx: &mut Context<Self>) {
        let Some(split) = self.chat_split.as_ref() else { return; };
        for (peer, input) in self.split_companions().zip(split.companion_editors.iter().map(|(input, _)| input)) {
            let draft = self.drafts.get(peer).map(String::as_str).unwrap_or_default();
            if input.read(cx).text() != draft {
                input.update(cx, |input, cx| input.set_text(draft, cx));
            }
        }
    }

    pub(super) fn workspace_wallpaper_project(&self) -> &std::path::Path {
        self.chat_split.as_ref().filter(|_| self.split_peer().is_some())
            .map_or(&self.project, |split| &split.wallpaper_project)
    }

    pub(super) fn split_interaction_expanded(&self, session: &SessionSummary, ix: usize) -> bool {
        self.chat_split.as_ref().is_some_and(|split| {
            split
                .expanded_interactions
                .contains(&(session.id.clone(), ix))
        })
    }

    pub(super) fn toggle_split_interaction(&mut self, id: &str, ix: usize, cx: &mut Context<Self>) {
        if let Some(split) = self.chat_split.as_mut() {
            let key = (id.to_owned(), ix);
            if !split.expanded_interactions.remove(&key) {
                split.expanded_interactions.insert(key);
            }
            cx.notify();
        }
    }

    pub(super) fn split_toolbar_slot(&self, owner: Option<&str>) -> usize {
        self.chat_split
            .as_ref()
            .and_then(|split| {
                let id = owner.or(self.active.as_deref())?;
                split
                    .sessions
                    .iter()
                    .position(|session| session == id)
                    .map(|index| index + 1)
            })
            .unwrap_or_default()
    }

    pub(super) fn split_prompt_bounds(&self, cx: &App) -> Option<Bounds<Pixels>> {
        self.chat_split.as_ref()?.companion_editors[0].0.read(cx).bounds()
    }

    pub(super) fn focus_split_chat(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.active.as_deref() == Some(id) {
            return true;
        }
        if !self.split_companions().any(|peer| peer == id) || self.hidden_sessions.contains(id) {
            return false;
        }
        let Some(session) = self
            .sessions
            .iter()
            .find(|session| session.id == id)
            .cloned()
        else {
            return false;
        };
        self.open_session(session, window, cx);
        true
    }

    pub(super) fn defer_split_send(&mut self, cx: &App) -> bool {
        if !self.loading || self.split_peer().is_none() {
            return false;
        }
        let draft = self.composer.read(cx).text();
        if draft.trim().is_empty() && self.attachments.is_empty() {
            return false;
        }
        let id = self.active.clone().unwrap();
        self.chat_split
            .as_mut()
            .unwrap()
            .pending_sends
            .insert(id, draft);
        true
    }

    pub(super) fn take_split_send(&mut self, id: &str, cx: &App) -> bool {
        self.chat_split
            .as_mut()
            .and_then(|split| split.pending_sends.remove(id))
            .is_some_and(|draft| self.composer.read(cx).text() == draft)
    }

    pub(super) fn split_tab_drop(
        &mut self,
        drag: &ChatDrag,
        target: SessionSummary,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if drag.session.id == target.id || self.hidden_sessions.contains(&drag.session.id) {
            return;
        }
        if self.ninja_surface_owner().is_some() || self.active.as_deref() != Some(&target.id) {
            self.open_session(target, window, cx);
        }
        self.split_chat(drag.session.clone(), Side::Right, window, cx);
    }

    pub(super) fn refresh_split_tail(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(split) = self.chat_split.as_mut().filter(|split| split.contains(id)) else {
            return;
        };
        let Some(session) = self
            .sessions
            .iter()
            .find(|session| session.id == id)
            .cloned()
        else {
            return;
        };
        let revision = uuid::Uuid::new_v4();
        split
            .tails
            .entry(id.to_owned())
            .and_modify(|tail| tail.request = revision)
            .or_insert_with(|| Tail {
                request: revision,
                rows: None,
                flow: None,
                list: ListState::new(0, ListAlignment::Bottom, px(300.)),
            });
        let owner = id.to_owned();
        let response = self.engine.split_tail(session);
        cx.spawn(async move |this, cx| {
            let result = response
                .recv()
                .await
                .unwrap_or_else(|_| Err("Conversation history is unavailable.".into()));
            let _ = this.update(cx, |this, cx| {
                if let Some(tail) = this
                    .chat_split
                    .as_mut()
                    .and_then(|split| split.tails.get_mut(&owner))
                    && tail.request == revision
                {
                    tail.rows = Some(result.map(|messages| {
                        let mut rows = Self::history_rows(messages);
                        rows.drain(..rows.len().saturating_sub(96));
                        rows
                    }));
                    tail.flow = tail
                        .rows
                        .as_ref()
                        .and_then(|rows| rows.as_ref().ok())
                        .map(|rows| chat_flow::Index::for_transcript(rows, true, None));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn workspace_split_key(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.ninja_surface_owner().is_some() || self.modal.is_some() { return; }
        let direction = match command {
            "split_beside" => Some(Side::Right),
            "split_below" => Some(Side::Bottom),
            _ => None,
        };
        if command == "split_add" || direction.is_some() {
            if self.chat_split.as_ref().is_some_and(|split| self.split_peer().is_some() && split.sessions.len() >= MAX_SPLIT_PANES) {
                self.notice = Some("This split already has four panes.".into());
                cx.notify();
                return;
            }
            let needed = 1 + usize::from(self.active.is_none());
            let excess = (self.open_tabs.len() + needed).saturating_sub(keyboard::MAX_CHAT_TABS);
            let replaceable: Vec<_> = self.open_tabs.iter().filter(|id| {
                self.active.as_ref() != Some(*id)
                    && !self.chat_split.as_ref().is_some_and(|split| split.contains(id))
                    && !self.chat_clis.contains_key(*id) && !self.runs.contains_key(*id)
                    && !self.settling.contains_key(*id) && !self.finalizing.contains(*id)
                    && self.activity.pending_rank(id).is_none()
            }).take(excess).cloned().collect();
            if replaceable.len() < excess {
                self.notice = Some("Close a tab before adding another split pane.".into());
                cx.notify();
                return;
            }
            self.open_tabs.retain(|id| !replaceable.contains(id));
            let open_cli = self.active_chat_cli().is_some_and(|cli| cli.selected);
            let had_chat = self.active.is_some();
            self.materialize_blank_tab(true, cx);
            if !had_chat { self.sync_chat_terminal_owner(cx); }
            let Some(current) = self.active.clone() else { return; };
            let id = uuid::Uuid::new_v4().to_string();
            let session = SessionSummary {
                id: id.clone(), title: "New session".into(), agent: self.agent,
                project_path: self.project.clone(),
                execution_path: (self.project == self.unprojected_root()).then(|| self.project.join(&id)),
                updated_at: chrono::Utc::now().timestamp(), external_id: None, transcript_path: None,
            };
            self.engine.set_model(session.clone(), self.model.clone());
            self.sessions.insert(0, session.clone());
            self.session_models.insert(id.clone(), self.model.clone());
            self.open_tabs.push(id.clone());
            if let Some(split) = self.chat_split.as_mut().filter(|split| split.sessions.len() > 1 && split.contains(&current)) {
                split.begin_layout_motion();
                if direction.is_some() || split.layout.is_some() {
                    let horizontal = direction.unwrap_or(Side::Right).horizontal();
                    let mut layout = split.layout.take().unwrap_or_else(|| split.legacy_layout());
                    layout.split(&current, &id, horizontal);
                    let original = split.motion_from[&current];
                    split.motion_from.insert(id.clone(), Self::split_entrance(original, horizontal));
                    split.sessions.push(id.clone());
                    split.layout = Some(layout);
                } else {
                    let completing_grid = split.sessions.len() == MAX_SPLIT_PANES - 1;
                    let original = split.target_rect(if completing_grid { 0 } else { 1 });
                    split.motion_from.insert(id.clone(), Self::split_entrance(original, !split.horizontal));
                    split.sessions.push(id.clone());
                    if completing_grid { split.ratio = 0.5; }
                }
                self.refresh_split_tail(&id, cx);
            } else {
                let side = direction.unwrap_or(Side::Right);
                self.split_chat(session.clone(), side, window, cx);
                if let Some(split) = &mut self.chat_split {
                    if direction.is_some() { split.layout = Some(split.legacy_layout()); }
                    split.motion_from.insert(current, FULL_PANE);
                    split.motion_from.insert(id.clone(), Self::split_entrance(FULL_PANE, side.horizontal()));
                }
            }
            self.open_session(session, window, cx);
            if open_cli { self.terminalize_chat(window, cx); }
            self.queue_workspace_save(cx);
        } else {
            let Some(active) = self.active.as_ref() else { return; };
            let Some(split) = self.chat_split.as_mut().filter(|split| split.contains(active)) else { return; };
            let index = split.sessions.iter().position(|id| id == active).unwrap();
            let horizontal = matches!(command, "split_left" | "split_right");
            let before = matches!(command, "split_left" | "split_up");
            if split.layout.is_some() {
                let rect = split.target_rect(index);
                let axis = usize::from(!horizontal);
                let cross = 1 - axis;
                let edge = rect[axis] + if before { 0. } else { rect[axis + 2] };
                let neighbor = (0..split.sessions.len()).filter(|&candidate| candidate != index)
                    .filter_map(|candidate| {
                        let other = split.target_rect(candidate);
                        let other_edge = other[axis] + if before { other[axis + 2] } else { 0. };
                        let overlap = (rect[cross] + rect[cross + 2]).min(other[cross] + other[cross + 2])
                            - rect[cross].max(other[cross]);
                        ((edge - other_edge).abs() < f32::EPSILON && overlap > 0.).then_some((candidate, overlap))
                    }).max_by(|a, b| a.1.total_cmp(&b.1));
                if let Some((neighbor, _)) = neighbor {
                    split.begin_layout_motion();
                    split.layout.as_mut().unwrap().swap(active, &split.sessions[neighbor]);
                    split.sessions.swap(index, neighbor);
                    self.sync_split_composer(cx);
                    cx.notify();
                }
                return;
            }
            if split.sessions.len() == MAX_SPLIT_PANES {
                let rect = split.target_rect(index);
                let axis = usize::from(!horizontal);
                let cross = 1 - axis;
                let neighbor = (0..MAX_SPLIT_PANES).find(|&candidate| {
                    let other = split.target_rect(candidate);
                    other[cross] == rect[cross]
                        && if before { other[axis] < rect[axis] } else { other[axis] > rect[axis] }
                });
                if let Some(neighbor) = neighbor {
                    split.begin_layout_motion();
                    split.sessions.swap(index, neighbor);
                    self.sync_split_composer(cx);
                    cx.notify();
                }
                return;
            }
            let target = if before { 0 } else { split.sessions.len() - 1 };
            if split.horizontal == horizontal && index == target { return; }
            split.begin_layout_motion();
            split.horizontal = horizontal;
            split.sessions.swap(index, target);
            self.sync_split_composer(cx);
        }
        cx.notify();
    }

    fn split_entrance(mut rect: PaneRect, horizontal: bool) -> PaneRect {
        let axis = usize::from(!horizontal);
        rect[axis] += rect[axis + 2];
        rect[axis + 2] = 0.;
        rect
    }

    pub(super) fn workspace_chat_split(
        &self,
        content: AnyElement,
        window: &Window,
        cx: &Context<Self>,
    ) -> AnyElement {
        let mut root = div()
            .id("workspace-chat-split")
            .debug_selector(|| "workspace-chat-split".into())
            .flex_1()
            .min_h(px(0.))
            .min_w(px(0.))
            .relative()
            .overflow_hidden()
            .bg(rgba(self.resolved_theme.main_rgba()))
            .when(self.ninja_surface_owner().is_none(), |root| root.child(
                div().absolute().inset_0().child(self.background.clone()),
            ))
            .on_drag_move(cx.listener(|app, event: &DragMoveEvent<ChatDrag>, _, cx| {
                let target = if app.ninja_surface_owner().is_none()
                    && app
                        .active
                        .as_deref()
                        .is_some_and(|id| id != event.drag(cx).session.id)
                    && event.bounds.contains(&event.event.position)
                {
                    Some(Side::target(event.bounds, event.event.position).unwrap_or(Side::Right))
                } else {
                    None
                };
                if app.chat_split_hover != target {
                    app.chat_split_hover = target;
                    cx.notify();
                }
            }))
            .on_drop(cx.listener(|app, drag: &ChatDrag, window, cx| {
                native::panel_drag_guard(window, false);
                // A fast drop can arrive before a second drag-move event.
                let side = app.chat_split_hover.take().unwrap_or(Side::Right);
                app.split_chat(drag.session.clone(), side, window, cx);
                cx.notify();
            }))
            .when(performance::chat_split_probe_enabled(), |root| {
                root.child(performance::chat_split_probe_area(2))
            });
        if let Some((split, active)) = self.chat_split.as_ref().zip(self.active.as_deref())
            .filter(|(split, active)| self.ninja_surface_owner().is_none() && split.contains(active))
        {
            let mut dividers = Vec::new();
            if let Some(layout) = &split.layout {
                layout.visit(1, FULL_PANE, &mut |_, _| {}, &mut |divider, ratio| dividers.push((divider, ratio)));
            } else if split.sessions.len() > 1 {
                dividers.push((ResizeSplit { node: 0, horizontal: split.horizontal, bounds: FULL_PANE }, split.ratio));
            }
            let elapsed = split.started.elapsed().as_secs_f32() / workspace_motion::PANE_DURATION.as_secs_f32();
            let progress = if self.motion && !native::reduced_motion() && elapsed < 1. {
                request_motion_frame(window, cx);
                workspace_motion::ease_out(elapsed)
            } else { 1. };
            let mut content = Some(content);
            root = root.children(split.sessions.iter().enumerate().map(|(index, id)| {
                let [left, top, width, height] = split.rect(index, progress);
                let pane = if id == active { content.take().unwrap() }
                    else { self.render_split_peer(id, window, cx) };
                div().id(SharedString::from(id.clone())).absolute()
                    .left(relative(left)).top(relative(top)).w(relative(width)).h(relative(height))
                    .min_w(px(0.)).min_h(px(0.)).overflow_hidden().flex()
                    .opacity(if id != active && split.motion_from.is_empty() { progress } else { 1. })
                    .child(pane)
            }))
                .children(dividers.into_iter().map(|(divider, ratio)| {
                    let [left, top, width, height] = divider.bounds;
                    div()
                        .id(("chat-split-divider", divider.node))
                        .absolute()
                        .bg(rgba(0xffffff24))
                        .hover(|d| d.bg(rgba(0x88bfff88)))
                        .when(divider.horizontal, |d| {
                            d.left(relative(left + width * ratio))
                                .ml(px(-2.))
                                .top(relative(top))
                                .h(relative(height))
                                .w(px(5.))
                                .cursor_col_resize()
                        })
                        .when(!divider.horizontal, |d| {
                            d.top(relative(top + height * ratio))
                                .mt(px(-2.))
                                .left(relative(left))
                                .w(relative(width))
                                .h(px(5.))
                                .cursor_row_resize()
                        })
                        .on_mouse_down(MouseButton::Left, |_, window, cx| {
                            native::panel_drag_guard(window, true);
                            cx.stop_propagation();
                        })
                        .on_drag(divider, |drag, _, _, cx| cx.new(|_| *drag))
                }))
                .on_drag_move(
                    cx.listener(|app, event: &DragMoveEvent<ResizeSplit>, _, cx| {
                        let divider = *event.drag(cx);
                        let axis = usize::from(!divider.horizontal);
                        let (offset, total) = if divider.horizontal {
                            (event.event.position.x - event.bounds.left(), event.bounds.size.width)
                        } else {
                            (event.event.position.y - event.bounds.top(), event.bounds.size.height)
                        };
                        let position = f32::from(offset) / f32::from(total).max(1.);
                        let ratio = ((position - divider.bounds[axis]) / divider.bounds[axis + 2]).clamp(0.23, 0.77);
                        if let Some(split) = app.chat_split.as_mut() {
                            split.motion_from.clear();
                            if let Some(layout) = &mut split.layout {
                                layout.resize(divider.node, ratio);
                            } else {
                                split.ratio = ratio;
                            }
                            cx.notify();
                        }
                    }),
                )
                .on_drop(|_: &ResizeSplit, window, _| native::panel_drag_guard(window, false));
        } else {
            root = root.child(content);
        }
        if let Some(side) = self.chat_split_hover.filter(|_| cx.has_active_drag()) {
            root = root.child(
                div()
                    .absolute()
                    .bg(rgba(0x8dbbff38))
                    .when(side.horizontal(), |s| s.h_full().w(relative(0.5)).top_0())
                    .when(!side.horizontal(), |s| s.w_full().h(relative(0.5)).left_0())
                    .when(side == Side::Left, |s| s.left_0())
                    .when(side == Side::Right, |s| s.right_0())
                    .when(side == Side::Top, |s| s.top_0())
                    .when(side == Side::Bottom, |s| s.bottom_0()),
            );
        }
        root.into_any_element()
    }

    fn render_split_peer(&self, peer: &str, window: &Window, cx: &Context<Self>) -> AnyElement {
        let Some(session) = self.sessions.iter().find(|session| session.id == peer) else {
            return div().child("Conversation unavailable").into_any_element();
        };
        let split = self.chat_split.as_ref().unwrap();
        let run = self.runs.get(peer);
        let tail = split.tails.get(peer);
        if let Some(terminal) = self.render_chat_cli(peer, cx) {
            return terminal;
        }
        let transcript = match tail.and_then(|tail| tail.rows.as_ref()) {
            Some(Ok(rows)) => {
                let state = tail.unwrap().list.clone();
                let count = rows.len() + run.map_or(0, |run| run.live_blocks.len().min(48));
                let previous = state.item_count();
                if count != previous {
                    state.splice(
                        count.min(previous)..previous,
                        count.saturating_sub(previous),
                    );
                }
                let id = peer.to_owned();
                div()
                    .relative().flex().flex_col()
                    .pl(px(32.))
                    .size_full()
                    .child(
                        list(
                            state,
                            cx.processor(move |app, ix, _, cx| {
                                app.split_transcript_row(&id, ix, cx)
                            }),
                        )
                        .flex_1().min_w(px(0.)).min_h(px(0.)),
                    )
                    .into_any_element()
            }
            Some(Err(error)) => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(MUTED))
                .child(error.clone())
                .into_any_element(),
            None => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(MUTED))
                .child("Opening conversation…")
                .into_any_element(),
        };
        let content = self
            .conversation_pane()
            .child(self.chat_cli_canvas(peer))
            .child(self.conversation_header(
                self.chat_label(session),
                Some(session),
                run.is_some(),
                false,
                false,
                window,
                cx,
            ))
            .child(
                div()
                    .id("split-chat-transcript")
                    .flex_1()
                    .min_h(px(0.))
                    .min_w(px(0.))
                    .child(transcript),
            )
            .when_some(run, |pane, run| {
                pane.child(Self::conversation_status(
                    provider_icon(agent_icon(run.agent), 16., agent_color(run.agent))
                        .into_any_element(),
                    activity_caption(&run.status),
                ))
            })
            .child(self.conversation_composer(Some(session), split.companion_editors[self.split_companions().position(|id| id == peer).unwrap()].0.clone(), window, cx))
            .into_any_element();
        self.docked_peer_conversation(content, session, window, cx)
    }

    fn split_transcript_row(&self, peer: &str, ix: usize, cx: &Context<Self>) -> AnyElement {
        let empty = || div().h(px(0.)).into_any_element();
        let Some((session, split)) = self
            .sessions
            .iter()
            .find(|session| session.id == peer)
            .zip(self.chat_split.as_ref())
        else {
            return empty();
        };
        let Some(tail) = split.tails.get(peer) else {
            return empty();
        };
        let Some(rows) = tail.rows.as_ref().and_then(|rows| rows.as_ref().ok()) else {
            return empty();
        };
        if ix >= rows.len() {
            let Some(run) = self.runs.get(peer) else {
                return empty();
            };
            let live = run.live_blocks.len().saturating_sub(48) + ix - rows.len();
            return run.live_blocks.get(live).map_or_else(empty, |block| {
                self.split_message(
                    session,
                    10_000 + live,
                    (live == 0).then_some("assistant"),
                    false,
                    block,
                    None,
                    cx,
                )
            });
        }
        let flow = tail.flow.as_ref().unwrap();
        if flow.hidden[ix]
            || flow.commands[ix]
                .as_ref()
                .is_some_and(|span| span.start != ix)
        {
            return empty();
        }
        let mut row_view = div().w_full().px_4();
        if let Some(span) = &flow.work[ix] {
            let expanded = split.expanded_work.contains(&(peer.to_owned(), span.start));
            if ix == span.start {
                let id = peer.to_owned();
                let start = span.start;
                row_view = row_view.child(
                    div().w_full().max_w(px(800.)).mx_auto().child(
                        Self::work_disclosure(ix, "Worked".into(), if expanded { 1. } else { 0. })
                            .on_click(cx.listener(move |app, _, _, cx| {
                                if let Some(split) = app.chat_split.as_mut() {
                                    let key = (id.clone(), start);
                                    if !split.expanded_work.remove(&key) {
                                        split.expanded_work.insert(key);
                                    }
                                    cx.notify();
                                }
                            })),
                    ),
                );
            }
            if !expanded {
                return row_view.into_any_element();
            }
        }
        let copy = flow.message_ends[ix].as_ref().map(|range| {
            let owner = cx.weak_entity();
            let id = peer.to_owned();
            let range = range.clone();
            copy_control::CopyControl::source(format!("message-copy-{ix}"), move |cx| {
                owner
                    .upgrade()
                    .and_then(|owner| {
                        let tail = owner.read(cx).chat_split.as_ref()?.tails.get(&id)?;
                        tail.rows
                            .as_ref()?
                            .as_ref()
                            .ok()?
                            .get(range.clone())
                            .map(copy_control::message_text)
                    })
                    .unwrap_or_default()
            })
            .scope(peer.to_owned())
            .motion(self.motion)
        });
        let command = flow.commands[ix].as_ref().map(|span| {
            let (details, status) = chat_flow::grouped_command(rows, span.clone());
            MessageBlock::Interaction(chat_interactions::ChatInteraction::Status {
                title: if matches!(
                    status,
                    chat_interactions::InteractionStatus::Working
                        | chat_interactions::InteractionStatus::Started
                ) {
                    "Running commands"
                } else {
                    "Ran commands"
                }
                .into(),
                details,
                status,
            })
        });
        let row = &rows[ix];
        row_view
            .child(self.split_message(
                session,
                ix,
                row.role.as_deref(),
                chat_flow::user(row),
                command.as_ref().unwrap_or(&row.block),
                copy,
                cx,
            ))
            .into_any_element()
    }

    fn split_message(
        &self,
        session: &SessionSummary,
        ix: usize,
        role: Option<&str>,
        user: bool,
        block: &MessageBlock,
        copy: Option<copy_control::CopyControl>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let agent = agent_for_role(role, session.agent);
        let content = self.conversation_message_body(ix, user, row_has_header(role), agent);
        let content = self
            .render_message_block(content, ix, block, None, agent, Some(session), cx)
            .when_some(copy, |s, copy| {
                s.child(
                    div()
                        .mt_1()
                        .flex()
                        .items_center()
                        .when(user, |s| s.justify_end())
                        .child(copy),
                )
            });
        div()
            .debug_selector(move || format!("transcript-row-{ix}"))
            .w_full()
            .max_w(px(800.))
            .mx_auto()
            .child(Self::conversation_message_alignment(user, content))
            .into_any_element()
    }
}

#[cfg(test)]
mod keyboard_tests {
    use super::*;
    use std::prelude::v1::test;

    #[gpui::test]
    fn cli_tab_builds_four_equal_panes_at_the_tab_limit(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        cx.update(|window, cx| view.update(cx, |app, cx| {
            app.agent = AgentKind::Claude;
            app.agents.iter_mut().find(|agent| agent.kind == AgentKind::Claude).unwrap().executable =
                Some(PathBuf::from("/bin/sh"));
            app.open_tabs = (0..keyboard::MAX_CHAT_TABS - 1).map(|index| format!("idle-{index}")).collect();
            app.terminalize_chat(window, cx);
            let launch_id = app.chat_clis[keyboard::CLI_DRAFT].launch_id;
            app.unified_command(&serde_json::json!({
                "action": "chat_terminal_command", "id": keyboard::CLI_DRAFT,
                "launchId": launch_id, "command": "split_add"
            }), window, cx).unwrap();
            let second = app.active.clone().unwrap();
            assert_eq!(app.chat_clis[&second].agent, AgentKind::Claude);
            assert!(app.chat_clis[&second].selected);
            app.workspace_split_key("split_add", window, cx);
            let split = app.chat_split.as_ref().unwrap();
            assert_eq!(split.sessions.len(), 3);
            app.workspace_split_key("split_add", window, cx);
            let split = app.chat_split.as_ref().unwrap();
            assert_eq!(split.sessions.len(), MAX_SPLIT_PANES);
            assert_eq!((0..4).map(|i| split.target_rect(i)).collect::<Vec<_>>(), vec![
                [0., 0., 0.5, 0.5], [0.5, 0., 0.5, 0.5],
                [0.5, 0.5, 0.5, 0.5], [0., 0.5, 0.5, 0.5],
            ]);
            assert_eq!(app.open_tabs.len(), keyboard::MAX_CHAT_TABS);
            for pane in &split.sessions {
                assert!(app.open_tabs.contains(pane));
                assert_eq!(app.chat_clis[pane].agent, AgentKind::Claude);
                assert!(app.chat_clis[pane].selected);
            }
            let panes = split.sessions.clone();
            let launches: Vec<_> = panes.iter().map(|pane| app.chat_clis[pane].launch_id).collect();
            let before = split.target_rect(0);
            app.chat_split.as_mut().unwrap().started = Instant::now() - workspace_motion::PANE_DURATION;
            app.close_tab(&panes[3], window, cx);
            let split = app.chat_split.as_ref().unwrap();
            assert_eq!(split.sessions.as_slice(), &panes[..3]);
            assert_eq!(split.rect(0, 0.), before);
            assert_eq!(split.rect(0, 1.), [0., 0., 0.5, 1.]);
            assert_eq!(app.active.as_deref(), Some(panes[2].as_str()));
            assert!(!app.chat_clis.contains_key(&panes[3]));
            for (pane, launch) in panes[..3].iter().zip(&launches) {
                assert_eq!(app.chat_clis[pane].launch_id, *launch);
            }
            app.unified_command(&serde_json::json!({
                "action": "chat_terminal_command", "id": panes[0],
                "launchId": launches[0], "command": "close_tab"
            }), window, cx).unwrap();
            assert_eq!(app.active.as_deref(), Some(panes[2].as_str()));
            assert_eq!(app.chat_split.as_ref().unwrap().sessions.as_slice(), &panes[1..3]);
        }));
    }

    #[gpui::test]
    fn command_d_recursively_splits_the_focused_pane_and_command_w_collapses_its_branch(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        cx.simulate_input("Keep my draft");
        cx.simulate_keystrokes("cmd-d cmd-d");
        let panes = view.update(cx, |app, _| {
            let split = app.chat_split.as_ref().unwrap();
            assert_eq!(split.target_rect(0), [0., 0., 0.5, 1.]);
            assert_eq!(split.target_rect(1), [0.5, 0., 0.25, 1.]);
            assert_eq!(split.target_rect(2), [0.75, 0., 0.25, 1.]);
            split.sessions.clone()
        });
        cx.update(|window, cx| view.update(cx, |app, cx| {
            assert!(app.focus_split_chat(&panes[0], window, cx));
            assert_eq!(app.composer.read(cx).text(), "Keep my draft");
        }));
        cx.simulate_keystrokes("cmd-shift-d");
        view.update(cx, |app, _| {
            let split = app.chat_split.as_ref().unwrap();
            assert_eq!(split.target_rect(0), [0., 0., 0.5, 0.5]);
            assert_eq!(split.target_rect(3), [0., 0.5, 0.5, 0.5]);
        });
        cx.simulate_keystrokes("cmd-w");
        view.update(cx, |app, _| {
            let split = app.chat_split.as_ref().unwrap();
            assert_eq!(split.sessions, panes);
            assert_eq!(split.target_rect(0), [0., 0., 0.5, 1.]);
            assert_eq!(split.target_rect(1), [0.5, 0., 0.25, 1.]);
            assert_eq!(split.target_rect(2), [0.75, 0., 0.25, 1.]);
        });
    }

    #[gpui::test]
    fn terminal_split_shortcuts_keep_existing_cli_launches(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        cx.update(|window, cx| view.update(cx, |app, cx| {
            app.agent = AgentKind::Claude;
            app.agents.iter_mut().find(|agent| agent.kind == AgentKind::Claude).unwrap().executable =
                Some(PathBuf::from("/bin/sh"));
            app.terminalize_chat(window, cx);
            let send = |app: &mut Orbit, window: &mut Window, cx: &mut Context<Orbit>, command| {
                let owner = app.active.clone().unwrap_or_else(|| keyboard::CLI_DRAFT.into());
                app.unified_command(&serde_json::json!({
                    "action": "chat_terminal_command", "id": owner,
                    "launchId": app.chat_clis[&owner].launch_id, "command": command
                }), window, cx).unwrap();
            };
            send(app, window, cx, "split_below");
            let first = app.chat_split.as_ref().unwrap().sessions[0].clone();
            let launch = app.chat_clis[&first].launch_id;
            send(app, window, cx, "split_beside");
            let split = app.chat_split.as_ref().unwrap();
            assert_eq!(split.target_rect(2), [0.5, 0.5, 0.5, 0.5]);
            assert!(split.sessions.iter().all(|id| app.chat_clis[id].selected));
            send(app, window, cx, "close_tab");
            assert_eq!(app.chat_split.as_ref().unwrap().target_rect(1), [0., 0.5, 1., 0.5]);
            assert_eq!(app.chat_clis[&first].launch_id, launch);
        }));
    }

    #[gpui::test]
    fn wallpaper_viewport_does_not_refit_when_panes_or_sidebar_move(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        crate::ui_tests::draw_frame(&view, cx);
        cx.run_until_parked();
        let initial = cx.update(|_, cx| view.read(cx).background.read(cx).snapshot_geometry().0.unwrap());
        cx.update(|window, cx| view.update(cx, |app, cx| {
            app.workspace_split_key("split_add", window, cx);
            app.workspace_split_key("split_add", window, cx);
            app.workspace_split_key("split_up", window, cx);
            app.sidebar_width = 0.;
            cx.notify();
        }));
        crate::ui_tests::draw_frame(&view, cx);
        cx.run_until_parked();
        cx.update(|_, cx| assert_eq!(view.read(cx).background.read(cx).snapshot_geometry().0, Some(initial)));
    }

    #[gpui::test]
    fn tab_keeps_the_original_draft_and_builds_a_three_chat_split(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        cx.update(|window, cx| view.update(cx, |app, cx| {
            app.composer.update(cx, |input, cx| input.set_text("Unsent original draft", cx));
            app.workspace_split_key("split_add", window, cx);
            let split = app.chat_split.as_ref().unwrap();
            let first = split.sessions[0].clone();
            let second = split.sessions[1].clone();
            assert_eq!(app.active.as_deref(), Some(second.as_str()));
            assert_eq!(app.drafts[&first], "Unsent original draft");
            app.workspace_split_key("split_add", window, cx);
            let split = app.chat_split.as_ref().unwrap();
            assert_eq!(split.sessions.len(), 3);
            assert_eq!(split.target_rect(2), [0.5, 0.5, 0.5, 0.5]);
            let third = app.active.clone().unwrap();
            app.workspace_split_key("split_up", window, cx);
            let split = app.chat_split.as_ref().unwrap();
            assert_eq!(split.sessions[0], third);
            assert!(!split.horizontal);
            assert!(app.focus_split_chat(&first, window, cx));
            assert_eq!(app.composer.read(cx).text(), "Unsent original draft");
        }));
    }
}
