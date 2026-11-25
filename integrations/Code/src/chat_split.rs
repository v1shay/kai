//! Workspace-only split presentation over the existing conversation owner.
use crate::appearance::GlassPopup as _;
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

fn split_direction(command: &str) -> Option<Side> {
    match command {
        "split_beside" => Some(Side::Right),
        "split_below" => Some(Side::Bottom),
        _ => None,
    }
}

#[derive(Clone,Debug,serde::Serialize,serde::Deserialize)]
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

    fn replace(&mut self, old: &str, new: &str) {
        match self {
            Self::Pane(id) if id == old => *id = new.into(),
            Self::Branch { first, second, .. } => { first.replace(old, new); second.replace(old, new); }
            _ => {}
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

pub(super) struct SplitPicker {
    command: String,
    owner: Option<String>,
    focus: FocusHandle,
    selection: usize,
    input: Entity<TextInput>,
    _input_subscription: Subscription,
    _keys: Subscription,
    searching: bool,
    cli: bool,
    provider: Option<AgentKind>,
}

struct SplitChoice {
    agent: AgentKind,
    project: PathBuf,
    execution: Option<PathBuf>,
    shell: bool,
}

pub(super) struct Split {
    pub(super) tab_id: String,
    pub(super) focused: String,
    pub(super) sessions: Vec<String>,
    motion_from: HashMap<String, [f32; 4]>,
    companion_editors: [(Entity<TextInput>, Subscription); MAX_SPLIT_PANES - 1],
    horizontal: bool,
    ratio: f32,
    layout: Option<PaneLayout>,
    started: Instant,
    tails: HashMap<String, Tail>,
    wallpaper_project: PathBuf,
    expanded_work: HashSet<(String, usize)>,
    work_animations: HashMap<(String, usize), interaction_ui::WorkMotion>,
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
        let elapsed = self.started.elapsed().as_secs_f32() / workspace_motion::pane_duration().as_secs_f32();
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

#[derive(Clone,Debug,serde::Serialize,serde::Deserialize)]
pub(super) struct Saved{sessions:Vec<String>,focused:String,horizontal:bool,ratio:f32,layout:Option<PaneLayout>}
impl Orbit {
    pub(super) fn chat_split_snapshot(&self)->Option<Saved>{self.chat_split.as_ref().map(|s|Saved{sessions:s.sessions.clone(),focused:s.focused.clone(),horizontal:s.horizontal,ratio:s.ratio,layout:s.layout.clone()})}
    pub(super) fn restore_chat_split(&mut self,saved:Option<Saved>,window:&mut Window,cx:&mut Context<Self>){
        let Some(saved)=saved.filter(|s|(2..=MAX_SPLIT_PANES).contains(&s.sessions.len())&&s.sessions.iter().all(|id|self.sessions.iter().any(|s|&s.id==id))) else{return};
        let first=saved.sessions.iter().find(|id|self.active.as_ref()!=Some(*id)).and_then(|id|self.sessions.iter().find(|s|&s.id==id)).cloned();
        if let Some(first)=first{
            self.split_chat(first,if saved.horizontal{Side::Right}else{Side::Bottom},window,cx);
            if let Some(split)=&mut self.chat_split{split.sessions=saved.sessions.clone();split.focused=saved.focused;split.ratio=if saved.ratio.is_finite(){saved.ratio.clamp(0.1,0.9)}else{0.5};split.layout=saved.layout;}
            self.sync_split_composer(cx);for id in saved.sessions{self.refresh_split_tail(&id,cx);}
        }
    }

    pub(super) fn chat_drag(&self, session: &SessionSummary) -> ChatDrag {
        ChatDrag {
            session: session.clone(),
            title: self.chat_label(session).into(),
        }
    }

    fn focused_pane_id(&self) -> Option<&str> {
        self.remote_pane_id().or(self.active.as_deref())
    }

    pub(super) fn split_visible_terminal_owners(&self) -> Vec<workspace_ui::TerminalOwner> {
        let Some(active) = self.focused_pane_id() else { return Vec::new(); };
        let Some(split) = self.split_tab_for(active) else { return Vec::new(); };
        split.sessions.iter().filter(|id| id.as_str() != active)
            .filter_map(|id| {
                if id.starts_with("remote-pane:") { self.parked_remote_terminal_owner(id) }
                else { Some(workspace_ui::TerminalOwner::Chat(id.clone())) }
            }).collect()
    }

    pub(super) fn update_split_focus(&mut self, id: &str) {
        if let Some(split) = self.chat_split.as_mut().filter(|split| split.contains(id)) {
            split.focused = id.to_owned();
        }
    }

    pub(super) fn add_chat_split_pane(
        &mut self, source: &str, destination: &str, side: Side, window: &mut Window, cx: &mut Context<Self>,
    ) -> bool {
        self.add_workspace_split_pane(source, destination, Some(side), window, cx)
    }

    pub(super) fn add_workspace_split_pane(
        &mut self, source: &str, destination: &str, direction: Option<Side>,
        window: &mut Window, cx: &mut Context<Self>,
    ) -> bool {
        let side = direction.unwrap_or(Side::Right);
        if source == destination || self.ninja_surface_owner().is_some() {
            return false;
        }
        if let Some(split) = self.chat_split.as_mut().filter(|split| split.contains(source)) {
            if split.contains(destination) { return false; }
            if split.sessions.len() >= MAX_SPLIT_PANES {
                self.notice = Some("This split already has four panes.".into());
                cx.notify();
                return false;
            }
            split.begin_layout_motion();
            if direction.is_some() || split.layout.is_some() {
                let mut layout = split.layout.take().unwrap_or_else(|| split.legacy_layout());
                layout.split(source, destination, side.horizontal());
                if side.before() { layout.swap(source, destination); }
                let origin = split.motion_from[source];
                split.motion_from.insert(destination.to_owned(), Self::split_entrance(origin, side.horizontal()));
                split.layout = Some(layout);
            } else {
                let completing_grid = split.sessions.len() == MAX_SPLIT_PANES - 1;
                let origin = split.target_rect(if completing_grid { 0 } else { 1 });
                split.motion_from.insert(destination.to_owned(), Self::split_entrance(origin, !split.horizontal));
                if completing_grid { split.ratio = 0.5; }
            }
            split.sessions.push(destination.to_owned());
        } else {
            let companion_editors = std::array::from_fn(|index| self.split_editor(index, window, cx));
            let sessions = if side.before() { vec![destination.to_owned(), source.to_owned()] }
                else { vec![source.to_owned(), destination.to_owned()] };
            self.chat_split = Some(Split {
                tab_id: uuid::Uuid::new_v4().to_string(), focused: source.to_owned(), sessions,
                motion_from: HashMap::from([
                    (source.to_owned(), FULL_PANE),
                    (destination.to_owned(), Self::split_entrance(FULL_PANE, side.horizontal())),
                ]),
                companion_editors, horizontal: side.horizontal(), ratio: 0.5, layout: None,
                started: Instant::now(), tails: HashMap::new(), wallpaper_project: self.project.clone(),
                expanded_work: HashSet::new(), work_animations: HashMap::new(),
                expanded_interactions: HashSet::new(), pending_sends: HashMap::new(),
            });
        }
        if direction.is_some() && let Some(split) = self.chat_split.as_mut() {
            if split.layout.is_none() {
                split.layout = Some(split.legacy_layout());
            }
        }
        self.retain_remote_split_panes();
        self.sync_split_composer(cx);
        self.refresh_split_tail(source, cx);
        self.refresh_split_tail(destination, cx);
        self.sync_split_terminal_visibility(cx);
        self.queue_workspace_save(cx);
        cx.notify();
        true
    }

    pub(super) fn split_peer(&self) -> Option<&str> {
        if self.ninja_surface_owner().is_some() {
            return None;
        }
        let active = self.focused_pane_id()?;
        let split = self.chat_split.as_ref()?;
        if !split.contains(active) { return None; }
        split.sessions.iter().find(|id| id.as_str() != active).map(String::as_str)
    }

    pub(super) fn split_companions(&self) -> impl Iterator<Item = &str> {
        let active = self.focused_pane_id();
        self.chat_split.as_ref().filter(|split| self.ninja_surface_owner().is_none()
            && active.is_some_and(|id| split.contains(id))).into_iter()
            .flat_map(move |split| split.sessions.iter().map(String::as_str).filter(move |id| Some(*id) != active && !id.starts_with("remote-pane:")))
    }

    pub(super) fn close_chat_split(&mut self, cx: &mut Context<Self>) {
        self.chat_split = None;
        self.retain_remote_split_panes();
        self.sync_split_terminal_visibility(cx);
        self.sync_remote_cli_visibility(cx);
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
        split.work_animations.retain(|(owner, _), _| owner != id);
        split.expanded_interactions.retain(|(owner, _)| owner != id);
        let next = split.sessions.get(index).or_else(|| split.sessions.last()).cloned();
        if split.focused == id && let Some(next) = &next { split.focused.clone_from(next); }
        if next.is_none() { self.chat_split = None; }
        next
    }

    pub(super) fn finish_split_close_motion(&mut self) {
        if self.chat_split.as_ref().is_some_and(|split| split.sessions.len() == 1
            && (!self.motion || native::reduced_motion()
                || split.started.elapsed() >= workspace_motion::pane_duration()))
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
        let Some(current) = self.focused_pane_id().map(str::to_owned) else { return; };
        if self.hidden_sessions.contains(&session.id) { return; }
        let id = session.id;
        if !self.add_chat_split_pane(&current, &id, side, window, cx) { return; }
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
        if self.focused_pane_id() == Some(id) || self.focus_remote_pane(id, cx) {
            return true;
        }
        if !self.split_companions().any(|peer| peer == id) || self.hidden_sessions.contains(id) {
            return false;
        }
        if self.remote_workspace.active() && self.active.as_deref() == Some(id) {
            self.leave_remote_workspace(cx);
            self.update_split_focus(id);
            cx.notify();
            return true;
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

    pub(super) fn retain_local_split_viewport(&mut self) {
        let Some(id) = self.active.as_ref() else { return; };
        let Some(split) = self.chat_split.as_mut().filter(|split| split.contains(id)) else { return; };
        if self.loading || self.rows.is_empty() || !self.deferred_rows.is_empty() { return; }
        let list = self.list.clone();
        let end = self.runs.get(id).and_then(|run| run.stream_start).unwrap_or(self.rows.len()).min(self.rows.len());
        let rows = self.rows[..end].to_vec();
        let flow = chat_flow::Index::for_transcript(&rows, true, None);
        split.tails.insert(id.clone(), Tail {
            request: uuid::Uuid::new_v4(), rows: Some(Ok(rows)), flow: Some(flow), list,
        });
    }

    pub(super) fn resume_local_split_viewport(&mut self, id: &str) -> bool {
        let Some(tail) = self.chat_split.as_ref().and_then(|split| split.tails.get(id)) else { return false; };
        let Some(Ok(rows)) = &tail.rows else { return false; };
        self.rows = rows.clone();
        self.list = tail.list.clone();
        self.list.splice(self.rows.len().min(self.list.item_count())..self.list.item_count(),
            self.rows.len().saturating_sub(self.list.item_count()));
        let anchor = self.list.logical_scroll_top();
        self.history_anchor = (anchor.item_ix < self.rows.len()).then_some(anchor);
        self.flow_revision = self.flow_revision.wrapping_add(1);
        true
    }

    pub(super) fn refresh_split_tail(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.draft_sessions.contains(id) && !self.chat_clis.get(id).is_some_and(|cli| cli.cursor_history.is_some()) {
            return;
        }
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

    pub(super) fn split_group_badges(&self, cx: &App) -> Vec<tui_badges::Badge> {
        self.chat_split.iter().flat_map(|split| &split.sessions)
            .filter_map(|id| self.sessions.iter().find(|session| session.id == *id))
            .map(|session| self.tab_badges(session, cx).pop().unwrap_or(tui_badges::Badge::Agent(session.agent)))
            .collect()
    }

    pub(super) fn split_tab_for(&self, id: &str) -> Option<&Split> {
        self.chat_split.as_ref().filter(|split| split.sessions.len() > 1 && split.contains(id))
    }

    fn maximize_split_pane(&mut self, cx: &mut Context<Self>) {
        let Some(owner) = self.focused_pane_id().map(str::to_owned) else { return; };
        let Some(split) = self.chat_split.as_mut().filter(|split| split.sessions.len() > 1) else { return; };
        if !split.contains(&owner) { return; }
        split.begin_layout_motion();
        self.pane_maximize_motion = Some((owner.clone(), split.motion_from[&owner], Instant::now()));
        self.remove_split_pane(&owner);
        self.sync_chat_terminal_owner(cx);
        self.sync_split_composer(cx);
        cx.notify();
    }

    pub(super) fn workspace_split_key(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(command,"split_left"|"split_right"|"split_up"|"split_down") && !self.remote_cli_selected() && self.active_chat_cli().is_none_or(|cli|!cli.selected){
            if let Some(panel)=self.active_terminal_panel().filter(|panel|panel.read(cx).focus_handle(cx).is_focused(window)){
                let source=panel.entity_id();self.terminal_keyboard_command(source,command,window,cx);return;
            }
        }
        if self.terminal_launcher.is_some() {
            if command == "split_add" { self.finish_terminal_launcher(None, window, cx); }
            return;
        }
        if self.split_picker.is_some() {
            if matches!(command, "split_add" | "split_beside" | "split_below") { self.dismiss_split_picker(window, cx); }
            return;
        }
        if matches!(command, "split_add" | "split_beside" | "split_below")
            && !cfg!(test) && !std::env::args().any(|arg| arg == "--unified-smoke-test")
        {
            self.open_split_picker(command, window, cx);
            return;
        }
        self.apply_split_command(command, None, window, cx);
    }

    fn open_split_picker(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.split_picker.is_some() || self.ninja_surface_owner().is_some()
            || self.modal.is_some() || self.terminal_launcher.is_some() { return; }
        if self.focused_pane_id().and_then(|id| self.split_tab_for(id)).is_some_and(|split| split.sessions.len() >= MAX_SPLIT_PANES) {
            self.notice = Some("This split already has four panes.".into());
            cx.notify();
            return;
        }
        let focus = cx.focus_handle();
        let input = cx.new(|cx| TextInput::new(window, cx));
        let subscription = cx.observe(&input, |app, _, cx| {
            if let Some(picker) = &mut app.split_picker {
                picker.selection = 0;
                cx.notify();
            }
        });
        let owner = cx.weak_entity();
        let menu_focus = focus.clone();
        let input_focus = input.read(cx).focus_handle.clone();
        let keys = cx.intercept_keystrokes(move |event, window, cx| {
            if !menu_focus.is_focused(window) && !input_focus.is_focused(window) { return; }
            let handled = owner.update(cx, |app, cx| app.handle_split_picker_key(&event.keystroke, window, cx)).unwrap_or(false);
            if handled { cx.stop_propagation(); }
        });
        self.split_picker = Some(SplitPicker {
            command: command.into(), owner: self.focused_pane_id().map(str::to_owned), focus: focus.clone(),
            selection: 1, input, _input_subscription: subscription, _keys: keys,
            searching: false, cli: if self.remote_workspace.active() { self.remote_cli_selected() }
                else { self.active_chat_cli().is_some_and(|cli| cli.selected) }, provider: None,
        });
        self.sync_chat_terminal_owner(cx);
        focus.focus(window);
        cx.notify();
    }

    fn split_picker_options(&self, picker: &SplitPicker, cx: &App) -> Vec<(String, Option<AgentKind>, Option<PathBuf>)> {
        let remote = self.remote_workspace.active();
        if !picker.searching {
            let (agent, project) = if remote {
                (self.remote_split_provider(), self.remote_split_project_label())
            } else {
                let directory = self.execution_root().unwrap_or_else(|| self.project.clone());
                (self.agent, self.project_path_label(&directory))
            };
            let mut choices = vec![
                (if picker.cli { "New terminal" } else { "New chat" }.into(), None, None),
                (format!("{} in {}", agent.name(), project), Some(agent), None),
                ("Choose agent and project…".into(), None, None),
            ];
            if !picker.cli {
                choices.push(("New terminal".into(), None, None));
            }
            return choices;
        }
        let query = picker.input.read(cx).text().trim().to_lowercase();
        let mut choices: Vec<_> = if remote && picker.provider.is_some() {
            self.remote_split_projects().into_iter().map(|(label, path)| (label, None, Some(path))).collect()
        } else if remote {
            self.remote_split_providers().into_iter().map(|agent| (agent.name().to_owned(), Some(agent), None)).collect()
        } else if picker.provider.is_some() {
            self.projects.iter().map(|project| (
                self.project_label(project), None, Some(project.path.clone()),
            )).collect()
        } else {
            self.agents.iter().map(|agent| (agent.name.clone(), Some(agent.kind), None)).collect()
        };
        choices.retain(|(label, _, _)| label.to_lowercase().contains(&query));
        choices.sort_by_key(|(label, _, _)| !label.to_lowercase().starts_with(&query));
        choices.truncate(8);
        choices
    }

    fn choose_split_picker(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picker) = self.split_picker.as_ref() else { return; };
        let Some((_, agent, project)) = self.split_picker_options(picker, cx).into_iter().nth(index) else { return; };
        let remote = self.remote_workspace.active();
        if (!picker.searching && index == 2) || (picker.searching && picker.provider.is_none()) {
            let picker = self.split_picker.as_mut().unwrap();
            let input = picker.input.clone();
            picker.provider = agent;
            picker.searching = true;
            picker.selection = 0;
            input.update(cx, |input, cx| {
                input.set_text("", cx);
                input.set_placeholder(if agent.is_some() { "Find a project…" } else { "Find an agent…" }, cx);
                input.focus_handle.focus(window);
            });
            cx.notify();
            return;
        }
        let picker = self.split_picker.take().unwrap();
        self.sync_chat_terminal_owner(cx);
        if picker.owner.as_deref() != self.focused_pane_id() { cx.notify(); return; }
        let shell = !picker.searching && if picker.cli { index == 0 } else { index == 3 };
        if remote {
            self.split_remote_choice(split_direction(&picker.command), picker.provider.or(agent), project, picker.cli || shell, shell, window, cx);
            return;
        }
        let execution = if project.is_none() { self.execution_root() } else { None };
        let choice = SplitChoice {
            agent: picker.provider.or(agent).unwrap_or(self.agent),
            project: project.unwrap_or_else(|| self.project.clone()),
            execution,
            shell,
        };
        self.apply_split_command(&picker.command, Some(choice), window, cx);
    }

    pub(super) fn dismiss_split_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.split_picker.take().is_none() { return false; }
        self.sync_chat_terminal_owner(cx);
        if !self.restore_remote_cli_focus(window, cx) {
            if let Some(input) = self.remote_prompt_input() { input.read(cx).focus_handle.focus(window); }
            else { self.composer.read(cx).focus_handle.focus(window); }
        }
        cx.notify();
        true
    }

    fn handle_split_picker_key(&mut self, stroke: &Keystroke, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(picker) = self.split_picker.as_ref() else { return false; };
        if stroke.modifiers.platform || stroke.modifiers.control || stroke.modifiers.alt { return false; }
        let length = self.split_picker_options(picker, cx).len();
        let selected = picker.selection.min(length.saturating_sub(1));
        match stroke.key.as_str() {
            "escape" | "tab" => { self.dismiss_split_picker(window, cx); }
            "up" | "down" => {
                if length > 0 {
                    let step = if stroke.key == "up" { length - 1 } else { 1 };
                    self.split_picker.as_mut().unwrap().selection = (selected + step) % length;
                    cx.notify();
                }
            }
            "enter" if !stroke.modifiers.shift => self.choose_split_picker(selected, window, cx),
            "1" | "2" | "3" | "4" if !picker.searching => {
                self.choose_split_picker(stroke.key.as_bytes()[0] as usize - b'1' as usize, window, cx);
            }
            _ => return false,
        }
        true
    }

    pub(super) fn render_split_picker(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let picker = self.split_picker.as_ref()?;
        let options = self.split_picker_options(picker, cx);
        let selected = picker.selection.min(options.len().saturating_sub(1));
        Some(div().id("split-launcher").absolute().inset_0().occlude()
            .track_focus(&picker.focus).flex().items_center().justify_center()
            .child(div().w(px(330.)).max_w(relative(0.9)).min_w(px(0.)).p_2()
                .flex().flex_col().rounded_lg().border_1()
                .border_color(rgba(0xebebeb1c)).glass_popup().occlude()
                .when(picker.searching, |menu| menu.child(
                    div().px_2().py_1().flex().flex_col()
                        .when_some(picker.provider, |menu, agent| menu.child(format!("{} in", agent.name())))
                        .child(picker.input.clone()),
                ))
                .when(options.is_empty(), |menu| menu.child(div().p_2().child("No matches")))
                .children(options.into_iter().enumerate().map(|(index, (label, agent, project))| {
                    let browse = !picker.searching && index == 2;
                    let row = div().id(("split-launch-choice", index)).h(px(34.)).px_2().rounded_md().flex()
                        .items_center().gap_2().cursor_pointer().text_size(theme::ui_px(12.))
                        .when(index == selected, |row| row.bg(rgba(GLASS_HOVER)))
                        .hover(|row| row.bg(rgba(GLASS_HOVER)))
                        .on_click(cx.listener(move |app, _, window, cx| app.choose_split_picker(index, window, cx)));
                    if browse {
                        row.child(tui_badges::stack(self.agents.iter().map(|agent| tui_badges::Badge::Agent(agent.kind))))
                            .tooltip(|_, cx| cx.new(|_| tui_badges::Label("Choose agent and project…".into())).into())
                    } else {
                        row.child(if let Some(agent) = agent { provider_icon(agent_icon(agent), 15., agent_color(agent)).into_any_element() }
                            else { icon(if project.is_some() { "folder" } else if picker.cli { "terminal" } else { "plus" }, 15., MUTED).into_any_element() })
                            .child(div().flex_1().truncate().child(label))
                            .when(!picker.searching, |row| row.child(div().text_color(rgb(MUTED)).child(format!("{}", index + 1))))
                    }
                })))
            .into_any_element())
    }

    fn apply_split_command(&mut self, command: &str, choice: Option<SplitChoice>, window: &mut Window, cx: &mut Context<Self>) {
        if self.ninja_surface_owner().is_some() || self.modal.is_some() || self.terminal_launcher.is_some() { return; }
        if command == "maximize_pane" {
            self.maximize_split_pane(cx);
            return;
        }
        let direction = split_direction(command);
        if command == "split_add" || direction.is_some() {
            if self.remote_workspace.active() {
                self.split_remote_choice(direction, None, None, self.remote_cli_selected(), false, window, cx);
                return;
            }
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
            let shell = choice.as_ref().is_some_and(|choice| choice.shell);
            let agent = choice.as_ref().map_or(self.agent, |choice| choice.agent);
            let project = choice.as_ref().map_or_else(|| self.project.clone(), |choice| choice.project.clone());
            let model = if agent == self.agent { self.model.clone() } else { self.agent_models.get(agent.command()).cloned().flatten() };
            let had_chat = self.active.is_some();
            self.materialize_blank_tab(true, cx);
            if !had_chat { self.sync_chat_terminal_owner(cx); }
            let Some(current) = self.active.clone() else { return; };
            let id = uuid::Uuid::new_v4().to_string();
            let session = SessionSummary {
                id: id.clone(), title: "New session".into(), agent,
                execution_path: choice.and_then(|choice| choice.execution)
                    .or_else(|| (project == self.unprojected_root()).then(|| project.join(&id))),
                project_path: project,
                updated_at: chrono::Utc::now().timestamp(), external_id: None, transcript_path: None,
            };
            self.draft_sessions.insert(id.clone());
            self.sessions.push(session.clone());
            self.session_models.insert(id.clone(), model);
            self.open_tabs.push(id.clone());
            self.add_workspace_split_pane(&current, &id, direction, window, cx);
            self.open_session(session, window, cx);
            if open_cli || shell { self.terminalize_chat_as(shell, window, cx); }
            self.queue_workspace_save(cx);
        } else {
            let Some(active) = self.focused_pane_id().map(str::to_owned) else { return; };
            let Some(split) = self.chat_split.as_mut().filter(|split| split.contains(&active)) else { return; };
            let index = split.sessions.iter().position(|id| id == &active).unwrap();
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
                    split.layout.as_mut().unwrap().swap(&active, &split.sessions[neighbor]);
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

    pub(super) fn pane_launch_feedback(&self, owner: String, pending: bool, content: AnyElement) -> AnyElement {
        div().relative().size_full().min_w(px(0.)).min_h(px(0.)).flex().child(content)
            .child(canvas(|bounds, _, _| bounds, move |_, bounds, _, _| {
                unified::pane_feedback(&owner, if pending { "loading" } else { "ready" }, bounds);
            }).absolute().inset_0()).into_any_element()
    }

    fn local_cli_pending(&self, owner: &str, cx: &App) -> bool {
        self.chat_clis.get(owner).is_some_and(|cli| cli.selected && !cli.metal_visible &&
            cli.fallback.as_ref().is_none_or(|(panel, _)| panel.read(cx).launching()))
    }

    pub(super) fn workspace_chat_split(
        &mut self,
        content: AnyElement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let owner = self.focused_pane_id().unwrap_or(keyboard::CLI_DRAFT).to_owned();
        let pending = if self.remote_workspace.active() { self.remote_feedback_pending(cx) }
            else { self.local_cli_pending(&owner, cx) };
        let content = self.pane_launch_feedback(owner, pending, content);
        let shared_wallpaper = self.theme.wallpaper_side_surfaces || self.theme.wallpaper_top_bottom;
        let wallpaper_bounds = self.background.read(cx).surface_bounds();
        let mut root = div()
            .id("workspace-chat-split")
            .debug_selector(|| "workspace-chat-split".into())
            .flex_1()
            .min_h(px(0.))
            .min_w(px(0.))
            .relative()
            .overflow_hidden()
            .when(!shared_wallpaper, |root| root.bg(rgba(self.resolved_theme.main_rgba())))
            .child(canvas(move |bounds, _, _| wallpaper_bounds.set(bounds), |_, _, _, _| {}).absolute().inset_0())
            .when(!shared_wallpaper && self.ninja_surface_owner().is_none(), |root| root.child(
                div().absolute().inset_0().child(background::cached(self.background.clone(), true)),
            ))
            .on_drag_move(cx.listener(|app, event: &DragMoveEvent<remote_workspace::RemoteChatDrag>, _, cx| {
                let target = app.focused_pane_id().and_then(|_| {
                    event.bounds.contains(&event.event.position)
                        .then(|| Side::target(event.bounds, event.event.position).unwrap_or(Side::Right))
                });
                if app.chat_split_hover != target {
                    app.chat_split_hover = target;
                    cx.notify();
                }
            }))
            .on_drop(cx.listener(|app, drag: &remote_workspace::RemoteChatDrag, window, cx| {
                native::panel_drag_guard(window, false);
                let side = app.chat_split_hover.take().unwrap_or(Side::Right);
                if let Some(source) = app.focused_pane_id().map(str::to_owned) {
                    app.split_remote_chat(&source, drag, side, window, cx);
                }
            }))
            .on_drag_move(cx.listener(|app, event: &DragMoveEvent<ChatDrag>, _, cx| {
                let target = if app.ninja_surface_owner().is_none()
                    && app
                        .focused_pane_id()
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
        if let Some((split, active)) = self.chat_split.as_ref().zip(self.focused_pane_id().map(str::to_owned))
            .filter(|(split, active)| self.ninja_surface_owner().is_none() && split.contains(active))
        {
            let mut dividers = Vec::new();
            if let Some(layout) = &split.layout {
                layout.visit(1, FULL_PANE, &mut |_, _| {}, &mut |divider, ratio| dividers.push((divider, ratio)));
            } else if split.sessions.len() > 1 {
                dividers.push((ResizeSplit { node: 0, horizontal: split.horizontal, bounds: FULL_PANE }, split.ratio));
            }
            let elapsed = split.started.elapsed().as_secs_f32() / workspace_motion::pane_duration().as_secs_f32();
            let progress = if self.motion && !native::reduced_motion() && elapsed < 1. {
                request_motion_frame(window, cx);
                workspace_motion::ease_out(elapsed)
            } else { 1. };
            let panes = split.sessions.iter().enumerate().map(|(index, pane)| {
                (pane.clone(), split.rect(index, progress))
            }).collect::<Vec<_>>();
            let fade_unfocused = split.motion_from.is_empty();
            let mut content = Some(content);
            for (id, [left, top, width, height]) in panes {
                let focused = id == active;
                let pane = if focused {
                    content.take().unwrap()
                } else if id.starts_with("remote-pane:") {
                    self.render_parked_remote_pane(&id, window, cx).unwrap_or_else(|| div().child("Conversation unavailable").into_any_element())
                } else {
                    self.pane_launch_feedback(id.clone(), self.local_cli_pending(&id, cx),
                        self.render_split_peer(&id, window, cx))
                };
                let target = id.clone();
                root = root.child(div().id(SharedString::from(id)).absolute()
                    .left(relative(left)).top(relative(top)).w(relative(width)).h(relative(height))
                    .min_w(px(0.)).min_h(px(0.)).overflow_hidden().flex()
                    .opacity(if !focused && fade_unfocused { progress } else { 1. })
                    .child(pane)
                    .when(!focused, |pane| pane.capture_any_mouse_down(cx.listener(
                        move |app, event: &gpui::MouseDownEvent, window, cx| {
                            if event.button == MouseButton::Left { app.focus_split_chat(&target, window, cx); }
                        },
                    ))));

            }
            root = root
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
                        let ratio = ((position - divider.bounds[axis]) / divider.bounds[axis + 2]).clamp(0.06, 0.94);
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
                .on_drop(cx.listener(|app, drag: &ResizeSplit, window, cx| {
                    native::panel_drag_guard(window, false);
                    let Some(split) = app.chat_split.as_ref() else { return; };
                    let mut ratio = split.ratio;
                    if let Some(layout) = &split.layout {
                        layout.visit(1, FULL_PANE, &mut |_, _| {}, &mut |divider, value| {
                            if divider.node == drag.node { ratio = value; }
                        });
                    }
                    if ratio > 0.08 && ratio < 0.92 {
                        app.queue_workspace_save(cx);
                        return;
                    }
                    let axis = usize::from(!drag.horizontal);
                    let edge = drag.bounds[axis] + if ratio <= 0.08 { 0. } else { drag.bounds[axis + 2] };
                    let cross = 1 - axis;
                    let closing: Vec<_> = split.sessions.iter().enumerate().filter_map(|(index, id)| {
                        let rect = split.target_rect(index);
                        let pane_edge = rect[axis] + if ratio <= 0.08 { 0. } else { rect[axis + 2] };
                        let overlap = (rect[cross] + rect[cross + 2]).min(drag.bounds[cross] + drag.bounds[cross + 2])
                            - rect[cross].max(drag.bounds[cross]);
                        ((pane_edge - edge).abs() < 0.001 && overlap > 0.).then(|| id.clone())
                    }).collect();
                    for id in closing {
                        if id.starts_with("remote-pane:") { app.close_remote_split_pane(&id, window, cx); }
                        else { app.close_tab(&id, window, cx); }
                    }
                }));
        } else if let Some((_, origin, started)) = self.pane_maximize_motion.as_ref()
            .filter(|(owner, _, started)| self.motion && !native::reduced_motion()
                && self.active.as_ref() == Some(owner) && started.elapsed() < workspace_motion::pane_duration())
        {
            request_motion_frame(window, cx);
            let progress = workspace_motion::ease_out(started.elapsed().as_secs_f32() / workspace_motion::pane_duration().as_secs_f32());
            let [left, top, width, height] = std::array::from_fn(|i| origin[i] + (FULL_PANE[i] - origin[i]) * progress);
            root = root.child(div().absolute().left(relative(left)).top(relative(top))
                .w(relative(width)).h(relative(height)).overflow_hidden().flex().child(content));
        } else {
            // Give docked and undocked chat identical definite bounds. A
            // terminal's intrinsic grid height must not size the chat column.
            root = root.child(div().absolute().inset_0().min_w(px(0.)).min_h(px(0.))
                .overflow_hidden().flex().child(content));
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

    pub(super) fn cached_split_transcript(&self, owner: &str, cx: &Context<Self>) -> Option<AnyElement> {
        let tail = self.chat_split.as_ref()?.tails.get(owner)?;
        if self.active.as_deref() == Some(owner) {
            let width = self.list.viewport_bounds().size.width;
            return Some(list(self.list.clone(), cx.processor(move |app, index, window, cx| {
                app.transcript_row(index, width, window, cx, false)
            })).flex_1().min_w(px(0.)).min_h(px(0.)).into_any_element());
        }
        let rows = tail.rows.as_ref()?.as_ref().ok()?;
        let state = tail.list.clone();
        let count = rows.len() + self.runs.get(owner).map_or(0, |run| run.live_blocks.len().min(48));
        let previous = state.item_count();
        if count != previous {
            state.splice(count.min(previous)..previous, count.saturating_sub(previous));
        }
        let owner = owner.to_owned();
        let width = state.viewport_bounds().size.width;
        Some(div().relative().flex().flex_col().size_full()
            .child(list(state, cx.processor(move |app, index, window, cx| {
                app.split_transcript_row(&owner, index, width, window, cx)
            })).flex_1().min_w(px(0.)).min_h(px(0.)))
            .into_any_element())
    }

    fn render_split_peer(&self, peer: &str, window: &Window, cx: &Context<Self>) -> AnyElement {
        let Some(session) = self.sessions.iter().find(|session| session.id == peer) else {
            return div().child("Conversation unavailable").into_any_element();
        };
        let split = self.chat_split.as_ref().unwrap();
        let run = self.runs.get(peer);
        let tail = split.tails.get(peer);
        if let Some(terminal) = self.render_chat_cli(peer, window, cx) {
            return self.docked_peer_conversation(terminal, session, window, cx);
        }
        let transcript = self.cached_split_transcript(peer, cx).unwrap_or_else(|| {
            if let Some(Err(error)) = tail.and_then(|tail| tail.rows.as_ref()) {
                div().size_full().flex().items_center().justify_center()
                    .text_color(rgb(MUTED)).child(error.clone()).into_any_element()
            } else if self.draft_sessions.contains(peer) {
                div().size_full().into_any_element()
            } else {
                div().size_full().flex().items_center().justify_center()
                    .text_color(rgb(MUTED)).child("Opening conversation…").into_any_element()
            }
        });
        let content = self
            .conversation_pane()
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
                    Some(run.file_changes),
                ))
            })
            .child(self.conversation_composer(Some(session), split.companion_editors[self.split_companions().position(|id| id == peer).unwrap()].0.clone(), window, cx))
            .into_any_element();
        self.docked_peer_conversation(content, session, window, cx)
    }

    pub(super) fn tick_split_work(&mut self, motion: bool) -> bool {
        let Some(split) = self.chat_split.as_mut() else { return false; };
        split.work_animations.retain(|(owner, start), animation| {
            let Some(tail) = split.tails.get(owner) else { return false; };
            let range = tail.flow.as_ref().and_then(|flow| flow.work.get(*start)).cloned().flatten();
            interaction_ui::remeasure_work_rows(&tail.list, range);
            motion && animation.active()
        });
        !split.work_animations.is_empty()
    }

    fn toggle_split_work(&mut self, owner: &str, start: usize, cx: &mut Context<Self>) {
        let Some(split) = self.chat_split.as_mut() else { return; };
        let key = (owner.to_owned(), start);
        let expanded = split.expanded_work.contains(&key);
        let from = split.work_animations.get(&key).map_or(f32::from(expanded), |motion| motion.progress());
        if expanded { split.expanded_work.remove(&key); } else { split.expanded_work.insert(key.clone()); }
        if self.motion && !native::reduced_motion() {
            split.work_animations.insert(key, interaction_ui::WorkMotion::new(from, !expanded));
        }
        if let Some(tail) = split.tails.get(owner) {
            let range = tail.flow.as_ref().and_then(|flow| flow.work.get(start)).cloned().flatten();
            interaction_ui::remeasure_work_rows(&tail.list, range);
        }
        cx.notify();
    }

    fn split_transcript_row(&self, peer: &str, ix: usize, width: Pixels, window: &Window, cx: &Context<Self>) -> AnyElement {
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
        let mut column = div().w_full().max_w(px(800.)).flex().flex_col();
        let mut reveal = 1.;
        if let Some(span) = &flow.work[ix] {
            let key = (peer.to_owned(), span.start);
            reveal = split.work_animations.get(&key)
                .filter(|_| self.motion && !native::reduced_motion())
                .map_or(f32::from(split.expanded_work.contains(&key)), |motion| motion.progress());
            if ix == span.start {
                let (id, start) = key;
                column = column.child(Self::work_disclosure(ix, "Worked".into(), reveal,
                    cx.listener(move |app, _, _, cx| app.toggle_split_work(&id, start, cx))));
            }
            if reveal <= 0.001 {
                return div().w_full().px_4().flex().justify_center().child(column).into_any_element();
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
                title: chat_flow::tool_summary(rows, span.clone()),
                details,
                status,
            })
        });
        let row = &rows[ix];
        let body = self.split_message(
            session, ix, row.role.as_deref(), chat_flow::user(row),
            command.as_ref().unwrap_or(&row.block), copy, cx,
        );
        let width = (width - window.rem_size() * 2.).clamp(px(0.), px(800.));
        div().w_full().px_4().flex().justify_center()
            .child(column.child(interaction_ui::reveal_work_body(body, reveal, width)))
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
                        .gap_1().flex_wrap()
                        .child(copy)
                        .when(!user, |s| s.child(self.conversation_sync_button(ix, Some(session), cx))),
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
            app.chat_split.as_mut().unwrap().started = Instant::now() - workspace_motion::pane_duration();
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
    fn split_picker_uses_the_saved_project_name_and_creates_a_gui_chat(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        cx.update(|window, cx| view.update(cx, |app, cx| {
            let project = app.projects[0].path.clone();
            app.lifecycle.project_names.insert(project.clone(), "Saved project label".into());
            app.open_split_picker("split_beside", window, cx);
            app.split_picker.as_mut().unwrap().selection = 2;
            app.choose_split_picker(2, window, cx);
            let picker = app.split_picker.as_ref().unwrap();
            assert!(picker.searching);
            let choices = app.split_picker_options(picker, cx);
            let provider = choices.iter().position(|(_, agent, _)| *agent == Some(AgentKind::Codex)).unwrap();
            app.choose_split_picker(provider, window, cx);
            let picker = app.split_picker.as_ref().unwrap();
            let choices = app.split_picker_options(picker, cx);
            let named = choices.iter().position(|(name, _, _)| name == "Saved project label").unwrap();
            app.split_picker.as_mut().unwrap().selection = named;
            let enter = Keystroke::parse("enter").unwrap();
            assert!(app.handle_split_picker_key(&enter, window, cx));
            assert_eq!(app.project, project);
            assert_eq!(app.agent, AgentKind::Codex);
            assert!(app.split_peer().is_some());
            assert!(app.active_chat_cli().is_none());
        }));
    }

    #[gpui::test]
    fn split_panes_share_one_tab_and_blank_panes_stay_out_of_history(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        cx.simulate_keystrokes("cmd-d cmd-d");
        cx.update(|window, cx| view.update(cx, |app, cx| {
            let panes = app.chat_split.as_ref().unwrap().sessions.clone();
            assert_eq!(panes.len(), 3);
            assert_eq!(app.open_tab_sessions().len(), 1);
            assert_eq!(app.open_tab_sessions()[0].id, panes[2]);
            app.rebuild_workspace_sessions();
            assert!(app.workspace_sessions.iter().all(|session| !panes.contains(&session.id)));
            assert!(app.workspace_snapshot(cx).draft_sessions.is_empty());
            let native = app.unified_snapshot(cx);
            for group in ["chats", "standaloneChats"] {
                assert!(native[group].as_array().unwrap().iter().all(|chat| {
                    panes.iter().all(|id| chat["id"] != format!("code:{id}"))
                }));
            }
            app.close_tab(&panes[1], window, cx);
            assert_eq!(app.open_tab_sessions().len(), 1);
            assert!(app.closing_tabs.is_empty());
        }));
    }

    #[gpui::test]
    fn maximizing_one_pane_keeps_the_other_three_in_the_same_group(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        cx.simulate_keystrokes("cmd-d cmd-d cmd-shift-d");
        cx.update(|window, cx| view.update(cx, |app, cx| {
            let focused = app.active.clone().unwrap();
            let peers: Vec<_> = app.split_companions().map(str::to_owned).collect();
            app.workspace_split_key("maximize_pane", window, cx);
            assert_eq!(app.active.as_deref(), Some(focused.as_str()));
            assert!(app.split_peer().is_none());
            let group = app.open_tab_sessions().into_iter().find(|session| session.id != focused).unwrap().clone();
            assert_eq!(app.open_tab_sessions().len(), 2);
            app.open_session(group, window, cx);
            assert_eq!(app.chat_split.as_ref().unwrap().sessions, peers);
            assert!(app.open_tabs.contains(&focused));
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

impl Orbit {
    pub(super) fn replace_split_chat(&mut self, owner: &str, replacement: &str) {
        if owner == replacement { return; }
        let Some(split) = self.chat_split.as_mut().filter(|s| s.contains(owner)) else { return; };
        // Swapping uses a temporary identity so both positions remain intact.
        let swap = split.contains(replacement);
        let temporary = uuid::Uuid::new_v4().to_string();
        if let Some(layout) = &mut split.layout {
            if swap { layout.replace(replacement, &temporary); }
            layout.replace(owner, replacement);
            if swap { layout.replace(&temporary, owner); }
        }
        for id in &mut split.sessions {
            if id == owner { *id = replacement.into(); }
            else if swap && id == replacement { *id = owner.into(); }
        }
        split.focused = replacement.into();
        split.motion_from.clear();
    }
}

#[cfg(test)]
mod replacement_history_tests {
    use super::*;
    use gpui::TestAppContext;
    use std::prelude::v1::test;
    #[gpui::test]
    fn daily_driver_swapped_replacement_loads_its_saved_peer_transcript(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        cx.update(|window, cx| view.update(cx, |app, cx| {
            app.active = Some("fixture-session".into());
            let mut second = app.sessions[0].clone(); second.id = "second".into();
            let mut third = second.clone(); third.id = "third".into();
            app.engine.save(third.clone(), "assistant", "SAVED_THIRD_TRANSCRIPT".into());
            app.sessions.extend([second.clone(), third.clone()]);
            app.split_chat(second.clone(), Side::Right, window, cx);
            app.toggle_chat_switcher(Some("fixture-session".into()), point(px(400.), px(70.)), window, cx);
            app.choose_pane_chat(Some("fixture-session".into()), third, window, cx);
            app.toggle_chat_switcher(Some("third".into()), point(px(400.), px(70.)), window, cx);
            app.choose_pane_chat(Some("third".into()), second, window, cx);
        }));
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            cx.run_until_parked();
            let loaded = view.update(cx, |app, _| app.chat_split.as_ref().and_then(|s| s.tails.get("third"))
                .and_then(|t| t.rows.as_ref()).and_then(|rows| rows.as_ref().ok())
                .is_some_and(|rows| rows.iter().any(|row| matches!(&row.block, MessageBlock::Paragraph(t) if t == "SAVED_THIRD_TRANSCRIPT"))));
            if loaded { break; }
            assert!(Instant::now() < deadline, "The newly visible peer never loaded its saved transcript");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
