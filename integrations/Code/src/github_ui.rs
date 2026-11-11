//! Native GitHub surfaces share the current execution checkout with file review.
//! Reads are bounded and cached. Every mutation is an explicit click against a
//! captured checkout; changing chats/projects cannot retarget an open card.
use super::*;
use anyhow::Context as _;
use github::{Client, PullRequest, Snapshot, SyncPreview};
use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tab {
    Repository,
    Branches,
    Pulls,
    History,
    Account,
    Clone,
    Sync,
}
struct ReadJob(Arc<AtomicBool>);
impl Drop for ReadJob {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}
pub(super) struct State {
    #[cfg(test)]
    freeze_reads: bool,
    root: Option<PathBuf>,
    pub snapshot: Option<Arc<Snapshot>>,
    cache: HashMap<PathBuf, (Instant, Arc<Snapshot>)>,
    read_job: Option<ReadJob>,
    read_id: u64,
    checked: Option<Instant>,
    widget_motion: Option<(Instant, f32, f32)>,
    pr_entries: HashMap<String, Instant>,
    error: Option<String>,
    pub panel: Option<Panel>,
    pub busy: Option<String>,
    account: Option<Arc<github::Account>>,
    history: Vec<github::Commit>,
    mine: Option<github::PullPage>,
    mine_host: Option<String>,
    mine_checked: Option<Instant>,
    mine_job: Option<ReadJob>,
    mine_error: Option<String>,
    widget_offset: usize,
}
impl Default for State {
    fn default() -> Self {
        Self {
            #[cfg(test)]
            freeze_reads: false,
            root: None,
            snapshot: None,
            cache: HashMap::new(),
            read_job: None,
            read_id: 0,
            checked: None,
            widget_motion: None,
            pr_entries: HashMap::new(),
            error: None,
            panel: None,
            busy: None,
            account: None,
            history: vec![],
            mine: None,
            mine_host: None,
            mine_checked: None,
            mine_job: None,
            mine_error: None,
            widget_offset: 0,
        }
    }
}
pub(super) struct Panel {
    root: PathBuf,
    pub(super) tab: Tab,
    at: Instant,
    pub(super) closing: bool,
    closing_from: f32,
    fields: Vec<Entity<TextInput>>,
    _subscriptions: Vec<Subscription>,
    preview: Option<SyncPreview>,
    preview_pending: bool,
    preview_job: Option<ReadJob>,
    error: Option<String>,
    result: Option<String>,
    private: bool,
    draft: bool,
    merge_confirm: Option<PullRequest>,
    mine: bool,
    return_focus: Option<FocusHandle>,
    request: uuid::Uuid,
}
impl Panel {
    fn reveal(&self, motion: bool) -> f32 {
        if !motion || native::reduced_motion() {
            return if self.closing { 0. } else { 1. };
        }
        let t = (self.at.elapsed().as_secs_f32() / workspace_motion::surface_duration(self.closing).as_secs_f32()).min(1.);
        if self.closing {
            self.closing_from * (1. - presentation::ease_out(t))
        } else {
            presentation::ease_out(t).clamp(0., 1.)
        }
    }
}
#[derive(Clone)]
enum Operation {
    Sync {
        preview: SyncPreview,
        message: String,
        push: bool,
    },
    Switch {
        name: String,
        create: bool,
    },
    Fetch,
    Pull,
    Initialize,
    Publish {
        name: String,
        private: bool,
    },
    CreatePull {
        title: String,
        body: String,
        base: String,
        draft: bool,
    },
    PullDiff(PullRequest),
    CheckoutPull(PullRequest),
    Merge(PullRequest),
    CommitDiff(String),
    Account,
    History,
    Clone {
        url: String,
        destination: PathBuf,
    },
    MorePulls,
}
enum Outcome {
    Notice(String),
    Diff(String),
    Account(github::Account),
    History(Vec<github::Commit>),
    Cloned(PathBuf),
    Pulls(github::PullPage),
}
fn button(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    glyph: Option<&'static str>,
    enabled: bool,
    cx: &Context<Orbit>,
    action: impl Fn(&mut Orbit, &mut Window, &mut Context<Orbit>) + 'static,
) -> Stateful<Div> {
    let id = id.into();
    let selector = id.clone();
    let glyph_selector = format!("{id}-glyph");
    let label: SharedString = label.into();
    let icon_only = label.is_empty();
    div()
        .id(id.clone())
        .debug_selector(move || selector.to_string())
        .h(px(28.))
        .px_2()
        .rounded_md()
        .flex()
        .items_center()
        .justify_center()
        .gap_2()
        .flex_none()
        .when(icon_only, |s| s.w(px(28.)).px_0().gap_0())
        .text_size(theme::ui_px(11.))
        .text_color(rgb(TEXT))
        .bg(rgba(0xffffff09))
        .opacity(if enabled { 1. } else { 0.4 })
        .when(enabled, |s| {
            s.cursor_pointer().hover(|s| s.bg(rgba(GLASS_HOVER)))
        })
        .when_some(glyph, |s, g| {
            s.child(
                icon(g, 14., TEXT)
                    .flex_none()
                    .debug_selector(move || glyph_selector.clone()),
            )
        })
        .when(!icon_only, |s| s.child(label))
        .on_click(cx.listener(move |app, _event: &ClickEvent, window, cx| {
            cx.stop_propagation();
            if enabled {
                action(app, window, cx)
            }
        }))
}
pub(super) fn counts(counts: &github::Counts) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap_2()
        .flex_none()
        .font_family(theme::ui_font())
        .text_size(theme::ui_px(10.))
        .when(counts.binary, |s| {
            s.child(div().text_color(rgb(MUTED)).child("Binary"))
        })
        .when(!counts.binary, |s| {
            s.child(
                div()
                    .text_color(rgb(0x8ad9a0))
                    .child(format!("+{}", counts.added)),
            )
            .child(
                div()
                    .text_color(rgb(0xff8e91))
                    .child(format!("−{}", counts.removed)),
            )
        })
        .into_any_element()
}
impl State {
    fn host(&self) -> &str {
        self.snapshot
            .as_ref()
            .and_then(|s| s.checkout.as_ref())
            .and_then(|r| r.hosted.as_ref())
            .map_or("github.com", |r| r.host.as_str())
    }
    fn personal(&self) -> Option<&github::PullPage> {
        (self.mine_host.as_deref() == Some(self.host()))
            .then_some(self.mine.as_ref())
            .flatten()
    }
    fn natural_widget_height(&self) -> f32 {
        let page = self.personal();
        64. + page.map_or(0, |p| p.items.len().min(3)) as f32 * 34.
            + if page.is_some_and(|p| p.total > 3) {
                36.
            } else {
                0.
            }
    }
    fn widget_height(&self, motion: bool) -> f32 {
        let target = self.natural_widget_height();
        if !motion || native::reduced_motion() {
            return target;
        }
        self.widget_motion.map_or(target, |(at, from, to)| {
            let t = (at.elapsed().as_secs_f32() / 0.28).min(1.);
            from + (to - from) * presentation::ease_out(t).clamp(0., 1.)
        })
    }
    fn set_snapshot(&mut self, snapshot: Option<Arc<Snapshot>>, motion: bool) {
        let from = self.widget_height(motion);
        self.snapshot = snapshot;
        let to = self.natural_widget_height();
        self.widget_motion = ((from - to).abs() > 0.5).then(|| (Instant::now(), from, to));
    }
}
impl Orbit {
    fn github_root(&self) -> PathBuf {
        self.execution_root()
            .unwrap_or_else(|| self.project.clone())
    }
    fn refresh_personal_pulls(&mut self, force: bool, cx: &mut Context<Self>) {
        #[cfg(test)]
        if self.github.freeze_reads {
            return;
        }
        let host = self.github.host().to_owned();
        if self.github.mine_host.as_ref() != Some(&host) {
            self.github.mine_job = None;
            self.github.mine = None;
            self.github.mine_checked = None;
            self.github.mine_error = None;
            self.github.mine_host = Some(host.clone());
            self.github.pr_entries.clear();
        }
        if self.github.mine_job.is_some()
            || (!force
                && self
                    .github
                    .mine_checked
                    .is_some_and(|at| at.elapsed() < Duration::from_secs(60)))
        {
            return;
        }
        let login = self
            .github
            .snapshot
            .as_ref()
            .map(|s| s.pulls.login.clone())
            .unwrap_or_default();
        let root = dirs::home_dir().unwrap_or_else(|| self.github_root());
        let cancel = Arc::new(AtomicBool::new(false));
        self.github.mine_job = Some(ReadJob(cancel.clone()));
        self.github.mine_error = None;
        let work_cancel = cancel.clone();
        let work_host = host.clone();
        let job = cx.background_executor().spawn(async move {
            Client::discover()
                .mine(&root, &work_host, &login, &work_cancel)
                .map_err(|e| e.to_string())
        });
        cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |app, cx| {
                if app.github.mine_host.as_ref() != Some(&host)
                    || !app
                        .github
                        .mine_job
                        .as_ref()
                        .is_some_and(|j| Arc::ptr_eq(&j.0, &cancel))
                {
                    return;
                }
                let from = app.github.widget_height(app.motion);
                app.github.mine_job = None;
                app.github.mine_checked = Some(Instant::now());
                match result {
                    Ok(page) => {
                        let old: HashSet<_> = app
                            .github
                            .mine
                            .as_ref()
                            .into_iter()
                            .flat_map(|p| p.items.iter().take(3))
                            .map(|p| p.url.clone())
                            .collect();
                        let visible: HashSet<_> =
                            page.items.iter().take(3).map(|p| p.url.clone()).collect();
                        app.github.pr_entries.retain(|url, _| visible.contains(url));
                        let now = Instant::now();
                        for (index, pr) in page.items.iter().take(3).enumerate() {
                            if !old.contains(&pr.url) {
                                app.github.pr_entries.insert(
                                    pr.url.clone(),
                                    now + Duration::from_millis(index as u64 * 45),
                                );
                            }
                        }
                        app.github.mine = Some(page);
                        app.github.mine_error = None;
                    }
                    Err(error) => app.github.mine_error = Some(error),
                }
                let to = app.github.natural_widget_height();
                app.github.widget_motion =
                    ((from - to).abs() > 0.5).then(|| (Instant::now(), from, to));
                cx.notify();
            });
        })
        .detach();
    }
    pub(super) fn github_after_agent_turn(&mut self, cx: &mut Context<Self>) {
        if self
            .github
            .checked
            .is_none_or(|at| at.elapsed() > Duration::from_secs(15))
        {
            self.github.checked = None;
            if !cfg!(test) && self.github.read_job.is_none() {
                self.refresh_github(false, cx);
            }
        }
    }
    pub(super) fn tick_github(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(p) = self.github.panel.as_ref() {
            if self.motion
                && !native::reduced_motion()
                && p.at.elapsed() < workspace_motion::surface_duration(p.closing)
            {
                request_motion_frame(window, cx);
            }
            if p.closing && p.at.elapsed() >= workspace_motion::CONTROL_DURATION {
                self.github.panel = None;
                cx.notify();
            }
        }
        if self.theme.github_widget
            && self.motion
            && !native::reduced_motion()
            && self.github.pr_entries.values().any(|at| {
                Instant::now().saturating_duration_since(*at) < Duration::from_millis(220)
            })
        {
            request_motion_frame(window, cx);
        }
        if self.theme.github_widget
            && self.motion
            && !native::reduced_motion()
            && self
                .github
                .widget_motion
                .is_some_and(|(at, _, _)| at.elapsed() < Duration::from_millis(280))
        {
            request_motion_frame(window, cx);
        }
        let root = self.github_root();
        if self.github.panel.as_ref().is_some_and(|p| p.root != root) {
            self.github.panel = None;
        }
        if cfg!(test) {
            return;
        }
        if self.github.root.as_ref() != Some(&root) {
            self.github.read_job = None;
            self.github.read_id += 1;
            self.github.root = Some(root.clone());
            let cached = self.github.cache.get(&root).map(|(_, s)| s.clone());
            self.github.set_snapshot(cached, self.motion);
            self.github.checked = self.github.cache.get(&root).map(|(at, _)| *at);
            self.github.error = None;
        }
        if (window.is_window_active() || self.github.checked.is_none())
            && (self.theme.github_widget || self.github.panel.is_some())
            && self.github.busy.is_none()
            && self.github.read_job.is_none()
            && self
                .github
                .checked
                .is_none_or(|at| at.elapsed() > Duration::from_secs(60))
        {
            self.refresh_github(false, cx);
        }
        if (window.is_window_active() || self.github.mine_checked.is_none())
            && (self.theme.github_widget
                || self
                    .github
                    .panel
                    .as_ref()
                    .is_some_and(|p| p.tab == Tab::Pulls && p.mine))
        {
            self.refresh_personal_pulls(false, cx);
        }
    }
    pub(super) fn refresh_github(&mut self, force: bool, cx: &mut Context<Self>) {
        #[cfg(test)]
        if self.github.freeze_reads {
            return;
        }
        if self.github.read_job.is_some() && !force {
            return;
        }
        let root = self.github_root();
        self.github.read_job = None;
        self.github.read_id += 1;
        let id = self.github.read_id;
        let cancel = Arc::new(AtomicBool::new(false));
        self.github.read_job = Some(ReadJob(cancel.clone()));
        self.github.root = Some(root.clone());
        self.github.checked = Some(Instant::now());
        self.github.error = None;
        let read = root.clone();
        let job = cx.background_executor().spawn(async move {
            Client::discover()
                .load(&read, &cancel)
                .map_err(|e| e.to_string())
        });
        cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |app, cx| {
                if app.github.read_id != id {
                    return;
                }
                app.github.read_job = None;
                app.github.checked = Some(Instant::now());
                match result {
                    Ok(snapshot) => {
                        let snapshot = Arc::new(snapshot);
                        if app.github.cache.len() >= 16 {
                            if let Some(old) = app
                                .github
                                .cache
                                .iter()
                                .min_by_key(|(_, v)| v.0)
                                .map(|(k, _)| k.clone())
                            {
                                app.github.cache.remove(&old);
                            }
                        }
                        app.github
                            .cache
                            .insert(root.clone(), (Instant::now(), snapshot.clone()));
                        if app.github_root() == root {
                            app.github.set_snapshot(Some(snapshot.clone()), app.motion);
                            app.refresh_personal_pulls(false, cx);
                            if let Some(p) = app.github.panel.as_mut() {
                                if p.fields[5].read(cx).text().is_empty()
                                    && !snapshot.pulls.default_branch.is_empty()
                                {
                                    p.fields[5].update(cx, |i, cx| {
                                        i.set_text(&snapshot.pulls.default_branch, cx)
                                    });
                                }
                            }
                        }
                    }
                    Err(error) => {
                        if app.github_root() == root {
                            app.github.error = Some(error);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    pub(super) fn open_github(&mut self, tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
        if self.consumed_control_click(control_clicks::Control::Github(tab), window)
            || self
                .github
                .panel
                .as_ref()
                .is_some_and(|p| !p.closing && p.tab == tab)
        {
            self.close_github(window, cx);
            return;
        }
        self.refresh_personal_pulls(false, cx);
        let root = self.github_root();
        self.agent_picker = None;
        self.panel_popover = None;
        self.action_menu = None;
        self.modal = None;
        self.overlay_window = Some(window.window_handle().window_id());
        let placeholders = [
            "Commit message",
            "New branch name",
            "owner/repository",
            "Pull request title",
            "Describe the changes",
            "Base branch",
            "https://github.com/owner/repository",
            "New destination folder",
        ];
        let mut fields = vec![];
        let mut subscriptions = vec![];
        for placeholder in placeholders {
            let input = cx.new(|cx| {
                let mut i = TextInput::new(window, cx);
                i.set_placeholder(placeholder, cx);
                i
            });
            subscriptions.push(cx.subscribe(&input, |_, _, _, cx| cx.notify()));
            fields.push(input);
        }
        if let Some(s) = self.github.snapshot.as_ref() {
            fields[5].update(cx, |i, cx| i.set_text(&s.pulls.default_branch, cx));
        }
        self.github.panel = Some(Panel {
            root,
            tab,
            at: Instant::now(),
            closing: false,
            closing_from: 1.,
            fields,
            _subscriptions: subscriptions,
            preview: None,
            preview_pending: false,
            preview_job: None,
            error: None,
            result: None,
            private: true,
            draft: true,
            merge_confirm: None,
            mine: true,
            return_focus: window.focused(cx),
            request: uuid::Uuid::new_v4(),
        });
        if tab == Tab::Sync {
            self.refresh_sync_preview(cx);
        } else if tab == Tab::Account {
            self.run_github(Operation::Account, cx);
        } else if tab == Tab::History {
            self.run_github(Operation::History, cx);
        } else {
            self.refresh_github(false, cx);
        }
        if let Some(p) = self.github.panel.as_ref().filter(|p| p.tab == Tab::Sync) {
            p.fields[0].read(cx).focus_handle.focus(window);
        }
        cx.notify();
    }
    pub(super) fn close_github(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(p) = self.github.panel.as_mut() {
            if p.closing {
                return;
            }
            p.closing_from = p.reveal(self.motion);
            p.closing = true;
            p.preview_job = None;
            p.at = Instant::now();
            if let Some(f) = p.return_focus.as_ref() {
                f.focus(window);
            }
        }
        cx.notify();
    }
    pub(super) fn open_response_sync(
        &mut self,
        session: Option<String>,
        root: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active != session || self.github_root() != root {
            self.notice =
                Some("Select the response in its current workspace before syncing".into());
            cx.notify();
            return;
        }
        self.open_github(Tab::Sync, window, cx);
    }
    pub(super) fn response_sync_button(&self, ix: usize, cx: &Context<Self>) -> AnyElement {
        let session = self.active.clone();
        let root = self.github_root();
        div()
            .id(("response-sync", ix))
            .debug_selector(move || format!("response-sync-{ix}"))
            .size(px(26.))
            .rounded_md()
            .flex()
            .items_center()
            .justify_center()
            .text_color(rgb(MUTED))
            .cursor_pointer()
            .hover(|s| s.bg(rgba(GLASS_HOVER)).text_color(rgb(TEXT)))
            .child(icon("git-commit", 14., MUTED))
            .on_click(cx.listener(move |app, _, window, cx| {
                app.open_response_sync(session.clone(), root.clone(), window, cx)
            }))
            .into_any_element()
    }
    fn refresh_sync_preview(&mut self, cx: &mut Context<Self>) {
        let Some(p) = self.github.panel.as_mut() else {
            return;
        };
        if p.preview_pending || self.github.busy.is_some() {
            return;
        }
        p.preview_pending = true;
        p.preview = None;
        p.error = None;
        let root = p.root.clone();
        let request = p.request;
        let cancel = Arc::new(AtomicBool::new(false));
        p.preview_job = Some(ReadJob(cancel.clone()));
        let work_cancel = cancel.clone();
        let job = cx
            .background_executor()
            .spawn(async move { github::preview(&root, &work_cancel).map_err(|e| e.to_string()) });
        cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |app, cx| {
                if let Some(p) = app.github.panel.as_mut().filter(|p| {
                    p.request == request
                        && p.preview_job
                            .as_ref()
                            .is_some_and(|job| Arc::ptr_eq(&job.0, &cancel))
                }) {
                    p.preview_job = None;
                    p.preview_pending = false;
                    match result {
                        Ok(v) => p.preview = Some(v),
                        Err(e) => p.error = Some(e),
                    }
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }
    fn github_mutation_blocker(&self) -> Option<String> {
        let root = self
            .github
            .snapshot
            .as_ref()
            .and_then(|s| s.checkout.as_ref())
            .map(|r| r.root.clone())
            .unwrap_or_else(|| self.github_root());
        let belongs =
            |path: &Path| path == root || path.starts_with(&root) || root.starts_with(path);
        if self
            .sessions
            .iter()
            .filter(|s| {
                self.runs.contains_key(&s.id)
                    || self.launches.contains_key(&s.id)
                    || self.finalizing.contains(&s.id)
            })
            .any(|s| belongs(s.execution_path.as_deref().unwrap_or(&s.project_path)))
            || self.unified_children.iter().any(|c| {
                c.connected
                    && matches!(
                        c.status,
                        chat_interactions::InteractionStatus::Started
                            | chat_interactions::InteractionStatus::Working
                    )
                    && belongs(&c.project)
            })
        {
            return Some("Let this workspace's agents finish before changing its Git state".into());
        }
        None
    }
    fn run_github(&mut self, operation: Operation, cx: &mut Context<Self>) {
        if self.github.busy.is_some() {
            return;
        }
        let Some(p) = self.github.panel.as_ref() else {
            return;
        };
        let root = p.root.clone();
        let request = p.request;
        if root != self.github_root() {
            return;
        }
        let readonly = matches!(
            operation,
            Operation::PullDiff(_)
                | Operation::CommitDiff(_)
                | Operation::Account
                | Operation::History
                | Operation::MorePulls
        );
        if matches!(
            operation,
            Operation::Sync { .. }
                | Operation::Switch { .. }
                | Operation::Pull
                | Operation::CheckoutPull(_)
                | Operation::Initialize
        ) {
            if let Some(error) = self.github_mutation_blocker() {
                if let Some(p) = self.github.panel.as_mut() {
                    p.error = Some(error)
                }
                cx.notify();
                return;
            }
        }
        let hosted = self
            .github
            .snapshot
            .as_ref()
            .and_then(|s| s.checkout.as_ref())
            .and_then(|r| r.hosted.clone());
        let cursor = self
            .github
            .snapshot
            .as_ref()
            .and_then(|s| s.pulls.cursor.clone());
        if matches!(operation, Operation::Account) {
            self.github.account = None;
        }
        if matches!(operation, Operation::History) {
            self.github.history.clear();
        }
        let caption = match &operation {
            Operation::Sync { push: true, .. } => "Committing & pushing…",
            Operation::Sync { .. } => "Committing…",
            Operation::Fetch => "Fetching…",
            Operation::Pull => "Pulling…",
            Operation::CreatePull { .. } => "Creating pull request…",
            Operation::Merge(_) => "Merging pull request…",
            Operation::Clone { .. } => "Cloning repository…",
            _ => "Loading source control…",
        };
        self.github.busy = Some(caption.into());
        if let Some(p) = self.github.panel.as_mut() {
            p.error = None;
            p.result = None;
        }
        if !readonly {
            self.checkpoint_busy += 1;
        }
        let work_root = root.clone();
        let job = cx.background_executor().spawn(async move {
            let client = Client::discover();
            let cancel = AtomicBool::new(false);
            let result: anyhow::Result<Outcome> = (|| {
                Ok(match operation {
                    Operation::Sync {
                        preview,
                        message,
                        push,
                    } => {
                        let r = client.sync(&preview, &message, push, &cancel)?;
                        Outcome::Notice(if let Some(warning) = r.warning {
                            format!(
                                "Commit {} saved locally. {warning}",
                                &r.commit[..8.min(r.commit.len())]
                            )
                        } else if let Some(error) = r.push_error {
                            format!(
                                "Commit {} saved locally. Push failed: {error}",
                                &r.commit[..8.min(r.commit.len())]
                            )
                        } else if r.pushed {
                            format!(
                                "Pushed {}{}",
                                &r.commit[..8.min(r.commit.len())],
                                if r.committed {
                                    " · workspace committed"
                                } else {
                                    ""
                                }
                            )
                        } else {
                            format!("Committed {}", &r.commit[..8.min(r.commit.len())])
                        })
                    }
                    Operation::Switch { name, create } => {
                        client.switch_branch(&work_root, &name, create, &cancel)?;
                        Outcome::Notice(format!("Checked out {name}"))
                    }
                    Operation::Fetch => {
                        client.fetch(&work_root, &cancel)?;
                        Outcome::Notice("Fetched remote branches".into())
                    }
                    Operation::Pull => {
                        client.pull(&work_root, &cancel)?;
                        Outcome::Notice("Pulled latest changes with fast-forward only".into())
                    }
                    Operation::Initialize => {
                        client.initialize(&work_root, &cancel)?;
                        Outcome::Notice("Created a local Git repository".into())
                    }
                    Operation::Publish { name, private } => {
                        client.publish(&work_root, &name, private, &cancel)?;
                        Outcome::Notice(format!(
                            "Created {} repository {name}",
                            if private { "private" } else { "public" }
                        ))
                    }
                    Operation::CreatePull {
                        title,
                        body,
                        base,
                        draft,
                    } => Outcome::Notice(
                        client.create_pull(&work_root, &title, &body, &base, draft, &cancel)?,
                    ),
                    Operation::PullDiff(pr) => Outcome::Diff(
                        client.pull_diff(
                            &work_root,
                            pr.repository
                                .as_ref()
                                .or(hosted.as_ref())
                                .context("No GitHub remote")?,
                            pr.number,
                            &cancel,
                        )?,
                    ),
                    Operation::CheckoutPull(pr) => {
                        client.checkout_pull(
                            &work_root,
                            pr.repository
                                .as_ref()
                                .or(hosted.as_ref())
                                .context("No GitHub remote")?,
                            pr.number,
                            &cancel,
                        )?;
                        Outcome::Notice(format!("Checked out PR #{}", pr.number))
                    }
                    Operation::Merge(pr) => {
                        client.merge_pull(
                            &work_root,
                            pr.repository
                                .as_ref()
                                .or(hosted.as_ref())
                                .context("No GitHub remote")?,
                            pr.number,
                            &pr.head_oid,
                            &cancel,
                        )?;
                        Outcome::Notice(format!("Merged PR #{}", pr.number))
                    }
                    Operation::CommitDiff(oid) => {
                        Outcome::Diff(client.commit_diff(&work_root, &oid, &cancel)?)
                    }
                    Operation::Account => Outcome::Account(client.account(
                        &work_root,
                        hosted.as_ref().map_or("github.com", |h| &h.host),
                        &cancel,
                    )?),
                    Operation::History => Outcome::History(client.history(&work_root, &cancel)?),
                    Operation::Clone { url, destination } => {
                        client.clone_repo(&url, &destination, &cancel)?;
                        Outcome::Cloned(destination)
                    }
                    Operation::MorePulls => Outcome::Pulls(client.pulls(
                        &work_root,
                        hosted.as_ref().context("No GitHub remote")?,
                        cursor.as_deref(),
                        &cancel,
                    )?),
                })
            })();
            result.map_err(|e| e.to_string())
        });
        cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |app, cx| {
                app.github.busy = None;
                if !readonly {
                    app.checkpoint_busy = app.checkpoint_busy.saturating_sub(1);
                    app.github.cache.remove(&root);
                    app.github.mine_job = None;
                    app.github.mine_checked = None;
                }
                let current = app.github_root() == root;
                let dialog = app
                    .github
                    .panel
                    .as_ref()
                    .is_some_and(|p| p.request == request);
                match result {
                    Ok(Outcome::Notice(message)) => {
                        if dialog {
                            app.github.panel.as_mut().unwrap().result = Some(message.clone());
                        }
                        app.notice = Some(format!(
                            "{} · {message}",
                            root.file_name().unwrap_or_default().to_string_lossy()
                        ));
                    }
                    Ok(Outcome::Diff(diff)) => {
                        if current && dialog {
                            app.github.panel = None;
                            app.open_github_diff(root.clone(), diff, cx);
                        }
                    }
                    Ok(Outcome::Account(account)) => {
                        if current {
                            app.github.account = Some(Arc::new(account));
                        }
                    }
                    Ok(Outcome::History(commits)) => {
                        if current {
                            app.github.history = commits;
                        }
                    }
                    Ok(Outcome::Cloned(path)) => {
                        if current && dialog {
                            let name = path
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .into_owned();
                            if !app.custom_projects.iter().any(|p| p.path == path) {
                                app.custom_projects.push(Project {
                                    name,
                                    path: path.clone(),
                                });
                                app.rebuild_projects();
                            }
                            app.github.panel.as_mut().unwrap().result =
                                Some(format!("Cloned to {}", path.display()));
                            app.queue_workspace_save(cx);
                        }
                    }
                    Ok(Outcome::Pulls(page)) => {
                        if current {
                            if let Some(snapshot) = app.github.snapshot.as_mut() {
                                let snapshot = Arc::make_mut(snapshot);
                                let mut seen: HashSet<_> =
                                    snapshot.pulls.items.iter().map(|p| p.number).collect();
                                snapshot.pulls.items.extend(
                                    page.items.into_iter().filter(|p| seen.insert(p.number)),
                                );
                                snapshot.pulls.cursor = page.cursor;
                            }
                        }
                    }
                    Err(error) => {
                        if dialog {
                            app.github.panel.as_mut().unwrap().error = Some(error.clone());
                        } else if current {
                            app.notice = Some(error);
                        }
                    }
                }
                if current && !readonly {
                    app.refresh_github(true, cx);
                    if dialog
                        && app
                            .github
                            .panel
                            .as_ref()
                            .is_some_and(|p| p.tab == Tab::Sync)
                    {
                        app.refresh_sync_preview(cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn open_github_diff(&mut self, root: PathBuf, diff: String, cx: &mut Context<Self>) {
        self.files_visible = false;
        self.broker_panel = None;
        let review = cx.new(|cx| diff_view::DiffView::provider(root, Arc::from(diff), cx));
        review.update(cx, |r, cx| {
            r.set_appearance(self.appearance, cx);
            r.set_source_label("GitHub · repository review".into());
            r.set_motion(self.motion)
        });
        self.diff_observer = None;
        self.diff_view = Some(review);
        self.diff_started = Some(Instant::now());
        cx.notify();
    }
    fn github_browse(&mut self, url: String, cx: &mut Context<Self>) {
        let job = cx
            .background_executor()
            .spawn(async move { native::open_web_url(&url) });
        cx.spawn(async move |this, cx| {
            if let Err(e) = job.await {
                let _ = this.update(cx, |app, cx| {
                    app.notice = Some(e);
                    cx.notify();
                });
            }
        })
        .detach();
    }
    fn github_login(&mut self, cx: &mut Context<Self>) {
        let host = self
            .github
            .snapshot
            .as_ref()
            .and_then(|s| s.checkout.as_ref())
            .and_then(|r| r.hosted.as_ref())
            .map_or("github.com".into(), |r| r.host.clone());
        let job = cx.background_executor().spawn(async move {
            let client = Client::discover();
            let gh = client
                .gh
                .ok_or_else(|| "Install GitHub CLI from cli.github.com first".to_owned())?;
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let folder = dirs::data_local_dir()
                .unwrap_or_else(std::env::temp_dir)
                .join("Kai Unified/source-control");
            std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
            let script = folder.join("github-login.command");
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o700)
                .open(&script)
                .map_err(|e| e.to_string())?;
            write!(
                f,
                "#!/bin/sh\nexec '{}' auth login --hostname '{}' --web\n",
                gh.display().to_string().replace('\'', "'\\''"),
                host.replace('\'', "'\\''")
            )
            .map_err(|e| e.to_string())?;
            native::open_login_script(&script)
        });
        cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |app, cx| {
                if let Err(e) = result {
                    app.notice = Some(e);
                }
                cx.notify();
            });
        })
        .detach();
    }
    pub(super) fn github_widget(&self, cx: &Context<Self>) -> AnyElement {
        let personal = self.github.personal();
        let height = self.github.widget_height(self.motion);
        let animate = self.motion && !native::reduced_motion();
        div()
            .id("github-widget-clip")
            .debug_selector(|| "github-widget-clip".into())
            .h(px(height))
            .flex_none()
            .overflow_hidden()
            .child(
                div()
                    .id("github-sidebar-widget")
                    .debug_selector(|| "github-sidebar-widget".into())
                    .mx_2()
                    .mb_2()
                    .p_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgba(0xffffff16))
                    .bg(linear_gradient(
                        135.,
                        linear_color_stop(rgba(0xffffff0c), 0.),
                        linear_color_stop(rgba(0xffffff03), 1.),
                    ))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(icon("kai/integrations/github", 18., TEXT))
                            .child(div().flex_1().text_size(theme::ui_px(12.)).child("GitHub"))
                            .child(button(
                                "github-open",
                                "",
                                Some("chevron-right"),
                                true,
                                cx,
                                |app, w, cx| app.open_github(Tab::Pulls, w, cx),
                            )),
                    )
                    .children(
                        personal
                            .into_iter()
                            .flat_map(|p| p.items.iter().skip(self.github.widget_offset.min(p.items.len().saturating_sub(1)/3*3)).take(3))
                            .map(|pr| {
                                let (glyph, color) = pr.glyph();
                                let url = pr.url.clone();
                                let key = format!("github-widget-pr-{}", pr.url);
                                let reveal = if animate {
                                    self.github.pr_entries.get(&pr.url).map_or(1., |at| {
                                        let t = (Instant::now()
                                            .saturating_duration_since(*at)
                                            .as_secs_f32()
                                            / 0.22)
                                            .min(1.);
                                        presentation::ease_out(t)
                                    })
                                } else {
                                    1.
                                };
                                div()
                                    .id(SharedString::from(key.clone()))
                                    .debug_selector(move || key.clone())
                                    .relative()
                                    .top(px((1. - reveal) * 5.))
                                    .opacity(reveal)
                                    .h(px(26.))
                                    .px_1()
                                    .rounded_md()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(rgba(GLASS_HOVER)))
                                    .child(icon(glyph, 15., color))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.))
                                            .text_size(theme::ui_px(12.))
                                            .text_ellipsis()
                                            .child(format!(
                                                "{} #{} {}",
                                                pr.repository
                                                    .as_ref()
                                                    .map(|r| r.name.as_str())
                                                    .unwrap_or(""),
                                                pr.number,
                                                pr.title
                                            )),
                                    )
                                    .on_click(cx.listener(move |app, _, _, cx| {
                                        cx.stop_propagation();
                                        app.github_browse(url.clone(), cx)
                                    }))
                            }),
                    )
                    .when(personal.is_some_and(|p| p.total > 3), |s| {
                        s.child(button(
                            "github-all-pulls",
                            if self.github.widget_offset==0 {"Next three"} else {"More pull requests"},
                            Some("chevron-down"),
                            true,
                            cx,
                            |app, _, cx| {
                                let count=app.github.personal().map_or(0,|p|p.items.len());
                                app.github.widget_offset=if app.github.widget_offset+3<count{app.github.widget_offset+3}else{0};
                                for (i,url) in app.github.personal().into_iter().flat_map(|p|p.items.iter().skip(app.github.widget_offset).take(3)).map(|p|p.url.clone()).collect::<Vec<_>>().into_iter().enumerate(){app.github.pr_entries.insert(url,Instant::now()+Duration::from_millis(i as u64*35));}
                                cx.notify();
                            },
                        ))
                    }),
            )
            .into_any_element()
    }
    fn github_field(&self, index: usize, label: &str) -> AnyElement {
        let Some(p) = self.github.panel.as_ref() else {
            return div().into_any_element();
        };
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .text_size(theme::ui_px(10.))
                    .text_color(rgb(MUTED))
                    .child(label.to_owned()),
            )
            .child(
                div()
                    .id(SharedString::from(format!("github-field-{index}")))
                    .debug_selector(move || format!("github-field-{index}"))
                    .min_h(px(if index == 4 { 70. } else { 34. }))
                    .p_2()
                    .rounded_md()
                    .bg(rgba(0xffffff08))
                    .border_1()
                    .border_color(rgba(0xffffff14))
                    .child(p.fields[index].clone()),
            )
            .into_any_element()
    }
    fn github_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        if let Some(p) = self.github.panel.as_mut() {
            p.tab = tab;
            if tab != Tab::Sync {
                p.preview_job = None;
                p.preview_pending = false;
            }
            p.error = None;
            p.result = None;
            p.merge_confirm = None;
        }
        if tab == Tab::Account {
            self.run_github(Operation::Account, cx)
        } else if tab == Tab::History {
            self.run_github(Operation::History, cx)
        } else if tab == Tab::Sync {
            self.refresh_sync_preview(cx)
        }
        cx.notify();
    }
    pub(super) fn github_panel_view(&self, window: &Window, cx: &Context<Self>) -> AnyElement {
        let Some(p) = self.github.panel.as_ref() else {
            return div().into_any_element();
        };
        let reveal = p.reveal(self.motion);
        let busy = self.github.busy.is_some();
        let width = (f32::from(window.viewport_size().width) - 32.)
            .min(if p.tab == Tab::Sync { 640. } else { 850. })
            .max(220.);
        let height = (f32::from(window.viewport_size().height) - 70.)
            .min(680.)
            .max(180.);
        let body = match p.tab {
            Tab::Sync => self.github_sync_view(cx),
            Tab::Repository => self.github_repository_view(cx),
            Tab::Branches => self.github_branches_view(cx),
            Tab::Pulls => self.github_pulls_view(cx),
            Tab::History => self.github_history_view(cx),
            Tab::Account => self.github_account_view(cx),
            Tab::Clone => self.github_clone_view(cx),
        };
        anchored()
            .position(point(
                px((f32::from(window.viewport_size().width) - width) / 2.),
                px(34. + 10. * (1. - reveal)),
            ))
            .snap_to_window_with_margin(px(12.))
            .child(
                div()
                    .id("github-panel")
                    .debug_selector(|| "github-panel".into())
                    .w(px(width))
                    .max_h(px(height))
                    .p_4()
                    .rounded_xl()
                    .border_1()
                    .border_color(rgba(0xffffff24))
                    .bg(rgba(appearance::MENU_SURFACE))
                    .shadow_lg()
                    .opacity(reveal)
                    .occlude()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .on_mouse_down_out(cx.listener(|app, event: &MouseDownEvent, window, cx| {
                        if let Some(p) = app.github.panel.as_ref() {
                            app.dismissed_on_press(
                                control_clicks::Control::Github(p.tab),
                                event.position,
                            );
                        }
                        app.close_github(window, cx)
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(icon("kai/integrations/github", 18., TEXT))
                            .child(div().flex_1().text_size(theme::ui_px(15.)).child(
                                if p.tab == Tab::Sync {
                                    "Commit & push"
                                } else {
                                    "GitHub"
                                },
                            ))
                            .child(button(
                                "github-refresh",
                                "",
                                Some("refresh"),
                                !busy,
                                cx,
                                |app, _, cx| {
                                    app.refresh_github(true, cx);
                                    app.refresh_personal_pulls(true, cx);
                                    if app
                                        .github
                                        .panel
                                        .as_ref()
                                        .is_some_and(|p| p.tab == Tab::Sync)
                                    {
                                        app.refresh_sync_preview(cx);
                                    }
                                },
                            ))
                            .child(button(
                                "github-close",
                                "",
                                Some("close"),
                                true,
                                cx,
                                |app, w, cx| app.close_github(w, cx),
                            )),
                    )
                    .when(p.tab != Tab::Sync, |s| {
                        s.child(
                            div().flex().flex_wrap().gap_1().children(
                                [
                                    (Tab::Repository, "Repository"),
                                    (Tab::Branches, "Branches"),
                                    (Tab::Pulls, "Pull requests"),
                                    (Tab::History, "Commits"),
                                    (Tab::Account, "Contributions"),
                                    (Tab::Clone, "Clone"),
                                ]
                                .into_iter()
                                .map(|(tab, label)| {
                                    button(
                                        format!("github-tab-{tab:?}"),
                                        label,
                                        None,
                                        !busy,
                                        cx,
                                        move |app, _, cx| app.github_tab(tab, cx),
                                    )
                                    .when(p.tab == tab, |s| s.bg(rgba(0xffffff1e)))
                                }),
                            ),
                        )
                    })
                    .when_some(self.github.busy.as_ref(), |s, text| {
                        s.child(
                            div()
                                .text_size(theme::ui_px(11.))
                                .text_color(rgb(ACCENT))
                                .child(text.clone()),
                        )
                    })
                    .when_some(
                        p.error.as_ref().or(self.github.error.as_ref()),
                        |s, error| {
                            s.child(
                                div()
                                    .text_size(theme::ui_px(11.))
                                    .text_color(rgb(0xffaaa1))
                                    .child(error.clone()),
                            )
                        },
                    )
                    .when_some(p.result.as_ref(), |s, result| {
                        s.child(
                            div()
                                .debug_selector(|| "github-result".into())
                                .text_size(theme::ui_px(11.))
                                .text_color(rgb(0x8ad9a0))
                                .child(result.clone()),
                        )
                    })
                    .child(
                        div()
                            .id("github-panel-body")
                            .flex_1()
                            .min_h(px(0.))
                            .max_h(px(height - 150.))
                            .overflow_y_scroll()
                            .child(body),
                    ),
            )
            .into_any_element()
    }
    fn github_sync_view(&self, cx: &Context<Self>) -> AnyElement {
        let p = self.github.panel.as_ref().unwrap();
        let ready = p.preview.as_ref();
        let enabled = self.github.busy.is_none() && !p.preview_pending;
        let changes = ready.is_some_and(|v| v.checkout.changes.total > 0);
        let message = p.fields[0].read(cx).text();
        let valid_message = !changes || !message.trim().is_empty();
        let up_to_date = ready.is_some_and(|v| {
            v.checkout.changes.total == 0 && v.checkout.ahead == 0 && v.checkout.upstream.is_some()
        });
        let can_push = enabled
            && !up_to_date
            && valid_message
            && ready.is_some_and(|v| {
                v.checkout.remote.is_some() && (changes || v.checkout.head.is_some())
            });
        let files = ready
            .map(|p| p.checkout.changes.files.clone())
            .unwrap_or_default();
        let count = files.len();
        let mut view = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_size(theme::ui_px(11.))
                    .text_color(rgb(MUTED))
                    .child(format!("Current workspace · {}", p.root.display())),
            )
            .when(p.preview_pending, |s| {
                s.child(
                    div()
                        .text_size(theme::ui_px(11.))
                        .text_color(rgb(MUTED))
                        .child("Reviewing staged, unstaged and untracked changes…"),
                )
            });
        if let Some(preview) = ready {
            let branch = preview.checkout.branch.as_deref().unwrap_or("");
            let remote = preview.checkout.remote.as_deref().unwrap_or("No remote");
            let destination = preview
                .checkout
                .push_hosted
                .as_ref()
                .map(|r| format!(" · {}", r.slug()))
                .unwrap_or_default();
            view = view
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(icon("branch", 14., TEXT))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .text_ellipsis()
                                .child(format!("{branch} → {remote}{destination}")),
                        ),
                )
                .child(
                    div()
                        .text_size(theme::ui_px(11.))
                        .text_color(rgb(MUTED))
                        .child(format!(
                            "{} changed files · {} outgoing commits",
                            preview.checkout.changes.total, preview.checkout.ahead
                        )),
                )
                .child(
                    uniform_list(
                        "github-sync-files",
                        count,
                        cx.processor(move |_, range: std::ops::Range<usize>, _, _| {
                            range
                                .map(|ix| {
                                    let file = &files[ix];
                                    div()
                                        .h(px(28.))
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .child(icon("file-code", 13., MUTED))
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w(px(0.))
                                                .text_size(theme::ui_px(11.))
                                                .text_ellipsis()
                                                .child(file.path.to_string_lossy().into_owned()),
                                        )
                                        .child(counts(&file.counts))
                                        .into_any_element()
                                })
                                .collect::<Vec<_>>()
                        }),
                    )
                    .h(px((count.min(8) * 28) as f32))
                    .w_full(),
                );
        }
        view.when(changes,|s|s.child(self.github_field(0,"Commit message")))
            .child(div().flex().flex_wrap().gap_2()
                .child(button("github-commit-push",if changes{"Commit & push"}else if up_to_date{"Up to date"}else{"Push commits"},Some(if up_to_date{"check"}else{"git-upload"}),can_push,cx,|app,_,cx|{
                    let p=app.github.panel.as_ref().unwrap();if let Some(preview)=p.preview.clone(){let message=p.fields[0].read(cx).text();app.run_github(Operation::Sync{preview,message,push:true},cx);}
                }))
                .child(button("github-commit-only","Commit only",Some("git-commit"),enabled&&changes&&valid_message,cx,|app,_,cx|{
                    let p=app.github.panel.as_ref().unwrap();if let Some(preview)=p.preview.clone(){let message=p.fields[0].read(cx).text();app.run_github(Operation::Sync{preview,message,push:false},cx);}
                }))
                .child(button("github-sync-review","Review diff",Some("file-code"),enabled&&changes,cx,|app,w,cx|{app.close_github(w,cx);app.open_changes(cx);})))
            .when(ready.is_none()&&!p.preview_pending,|s|s.child(button("github-sync-setup","Set up repository",Some("branch"),enabled,cx,|app,_,cx|app.github_tab(Tab::Repository,cx))))
            .child(div().text_size(theme::ui_px(10.)).text_color(rgb(MUTED)).child("Includes staged, unstaged and untracked files in this checkout. Ignored files stay excluded. A changed review must be refreshed; pushes never force."))
            .into_any_element()
    }
    fn github_repository_view(&self, cx: &Context<Self>) -> AnyElement {
        let snapshot = self.github.snapshot.as_ref();
        let repo = snapshot.and_then(|s| s.checkout.as_ref());
        let enabled = self.github.busy.is_none();
        let p = self.github.panel.as_ref().unwrap();
        div().flex().flex_col().gap_3().child(div().text_size(theme::ui_px(11.)).text_color(rgb(MUTED)).child(p.root.display().to_string()))
   .when_some(repo,|s,repo|s.child(div().flex().items_center().gap_2().child(icon("branch",16.,TEXT)).child(repo.branch.clone().unwrap_or_else(||"Detached HEAD".into()))).child(div().text_size(theme::ui_px(11.)).text_color(rgb(MUTED)).child(format!("{} files changed · {} ahead · {} behind",repo.changes.total,repo.ahead,repo.behind)))
    .when_some(repo.hosted.as_ref(),|s,r|{let url=r.url();s.child(button("github-repo-browser",r.slug(),Some("external"),true,cx,move|app,_,cx|app.github_browse(url.clone(),cx)))}))
   .when_some(snapshot.and_then(|s|s.github_error.as_ref()),|s,error|s.child(div().text_size(theme::ui_px(11.)).text_color(rgb(0xffaaa1)).child(error.clone())))
   .when_some(snapshot.filter(|s|!s.pulls.login.is_empty()),|s,snapshot|s.child(div().text_size(theme::ui_px(11.)).text_color(rgb(MUTED)).child(format!("@{} · {} · {} repository",snapshot.pulls.login,snapshot.pulls.permission.to_lowercase(),if snapshot.pulls.private{"private"}else{"public"}))))
   .child(div().flex().flex_wrap().gap_2()
    .child(button("github-repository-sync","Commit & push",Some("git-commit"),enabled&&repo.is_some(),cx,|app,_,cx|app.github_tab(Tab::Sync,cx)))
    .child(button("github-fetch","Fetch",Some("git-download"),enabled&&repo.is_some_and(|r|r.remote.is_some()),cx,|app,_,cx|app.run_github(Operation::Fetch,cx)))
    .child(button("github-pull","Pull",Some("git-download"),enabled&&repo.is_some_and(|r|r.upstream.is_some()),cx,|app,_,cx|app.run_github(Operation::Pull,cx)))
    .child(button("github-login",if snapshot.is_some_and(|s|s.gh_available){"Sign in / manage gh"}else{"Connect GitHub CLI"},Some("lock-keyhole"),enabled,cx,|app,_,cx|app.github_login(cx))))
   .when(repo.is_none(),|s|s.child(button("github-init","Initialize Git repository",Some("branch"),enabled,cx,|app,_,cx|app.run_github(Operation::Initialize,cx))))
   .when(repo.is_none_or(|r|r.remotes.is_empty()),|s|s.child(div().mt_3().text_size(theme::ui_px(12.)).child("Publish this project to GitHub")).child(self.github_field(2,"Repository · owner/name"))
    .child(button("github-visibility",if p.private{"Private repository"}else{"Public repository"},Some(if p.private{"lock-keyhole"}else{"lock-keyhole-open"}),enabled,cx,|app,_,cx|{if let Some(p)=app.github.panel.as_mut(){p.private=!p.private;}cx.notify();}))
    .child(button("github-publish","Create repository",Some("plus"),enabled&&!p.fields[2].read(cx).text().trim().is_empty(),cx,|app,_,cx|{let p=app.github.panel.as_ref().unwrap();app.run_github(Operation::Publish{name:p.fields[2].read(cx).text(),private:p.private},cx);})))
   .child(div().mt_2().text_size(theme::ui_px(10.)).text_color(rgb(MUTED)).child("Uses the selected remote, existing Git credentials and your signed-in gh account. Credentials remain in Git / gh."))
   .into_any_element()
    }
    fn github_branches_view(&self, cx: &Context<Self>) -> AnyElement {
        let repo = self
            .github
            .snapshot
            .as_ref()
            .and_then(|s| s.checkout.as_ref());
        let enabled = self.github.busy.is_none();
        let branches = repo.map(|r| r.branches.clone()).unwrap_or_default();
        let count = branches.len();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(self.github_field(1, "Create a branch from the current HEAD"))
            .child(button(
                "github-create-branch",
                "Create & check out",
                Some("branch"),
                enabled && repo.is_some(),
                cx,
                |app, _, cx| {
                    let name = app.github.panel.as_ref().unwrap().fields[1].read(cx).text();
                    app.run_github(Operation::Switch { name, create: true }, cx);
                },
            ))
            .child(
                uniform_list(
                    "github-branches",
                    count,
                    cx.processor(move |_, range: std::ops::Range<usize>, _, cx| {
                        range
                            .map(|ix| {
                                let branch = &branches[ix];
                                let name = branch.name.clone();
                                button(
                                    format!("github-branch-{}", branch.name),
                                    format!(
                                        "{}{}{}",
                                        if branch.current { "✓ " } else { "" },
                                        branch.name,
                                        if branch.remote { " · remote" } else { "" }
                                    ),
                                    Some("branch"),
                                    enabled && !branch.current,
                                    cx,
                                    move |app, _, cx| {
                                        app.run_github(
                                            Operation::Switch {
                                                name: name.clone(),
                                                create: false,
                                            },
                                            cx,
                                        )
                                    },
                                )
                                .into_any_element()
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .h(px((count.min(10) * 28) as f32))
                .w_full(),
            )
            .into_any_element()
    }
    fn github_pulls_view(&self, cx: &Context<Self>) -> AnyElement {
        let snapshot = self.github.snapshot.as_ref();
        let enabled = self.github.busy.is_none();
        let p = self.github.panel.as_ref().unwrap();
        let mut body = div().flex().flex_col().gap_3().child(button(
            "github-pulls-scope",
            if p.mine {
                "My pull requests · all repositories"
            } else {
                "This project’s pull requests"
            },
            Some("git-pull-request"),
            enabled,
            cx,
            |app, _, cx| {
                let mine = if let Some(p) = app.github.panel.as_mut() {
                    p.mine = !p.mine;
                    p.mine
                } else {
                    false
                };
                if mine {
                    app.refresh_personal_pulls(false, cx);
                    cx.notify();
                } else {
                    cx.notify();
                }
            },
        ));
        let items = if p.mine {
            self.github
                .personal()
                .map(|p| p.items.as_slice())
                .unwrap_or(&[])
        } else {
            snapshot.map(|s| s.pulls.items.as_slice()).unwrap_or(&[])
        };
        if p.mine {
            if let Some(error) = self.github.mine_error.as_ref() {
                body = body.child(
                    div()
                        .text_size(theme::ui_px(11.))
                        .text_color(rgb(0xff8e91))
                        .child(error.clone()),
                );
            } else if self.github.mine_job.is_some() && items.is_empty() {
                body = body.child(
                    div()
                        .text_size(theme::ui_px(11.))
                        .text_color(rgb(MUTED))
                        .child("Loading your pull requests…"),
                );
            }
        }
        for pr in items {
            let (glyph, color) = pr.glyph();
            let url = pr.url.clone();
            let diff = pr.clone();
            let checkout = pr.clone();
            let merge = pr.clone();
            body = body.child(
                div()
                    .p_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgba(0xffffff12))
                    .bg(rgba(0xffffff04))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(icon(glyph, 16., color))
                            .child(
                                div()
                                    .id(SharedString::from(format!("github-pr-{}", pr.number)))
                                    .flex_1()
                                    .min_w(px(0.))
                                    .text_size(theme::ui_px(12.))
                                    .cursor_pointer()
                                    .child(format!("#{} {}", pr.number, pr.title))
                                    .on_click(cx.listener(move |app, _, _, cx| {
                                        app.github_browse(url.clone(), cx)
                                    })),
                            )
                            .child(counts(&github::Counts {
                                added: pr.added,
                                removed: pr.removed,
                                binary: false,
                            })),
                    )
                    .child(
                        div()
                            .text_size(theme::ui_px(10.))
                            .text_color(rgb(MUTED))
                            .child(format!(
                                "{} · {} → {} · {} · {}{}",
                                pr.repository.as_ref().map(|r| r.slug()).unwrap_or_default(),
                                pr.head,
                                pr.base,
                                pr.author,
                                if pr.state == "MERGED" {
                                    "Merged"
                                } else if pr.state == "CLOSED" {
                                    "Closed"
                                } else if pr.draft {
                                    "Draft"
                                } else {
                                    "Open"
                                },
                                if pr.checks.is_empty() {
                                    String::new()
                                } else {
                                    format!(" · checks {}", pr.checks.to_lowercase())
                                }
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .child(button(
                                format!("github-pr-diff-{}", pr.number),
                                "Review diff",
                                Some("file-code"),
                                enabled,
                                cx,
                                move |app, _, cx| {
                                    app.run_github(Operation::PullDiff(diff.clone()), cx)
                                },
                            ))
                            .child(button(
                                format!("github-pr-checkout-{}", pr.number),
                                "Check out",
                                Some("branch"),
                                enabled
                                    && pr.repository.as_ref().is_none_or(|repo| {
                                        snapshot.and_then(|s| s.checkout.as_ref()).is_some_and(
                                            |r| {
                                                r.hosted.as_ref() == Some(repo)
                                                    || r.remote_repos
                                                        .iter()
                                                        .any(|(_, known)| known == repo)
                                            },
                                        )
                                    }),
                                cx,
                                move |app, _, cx| {
                                    app.run_github(Operation::CheckoutPull(checkout.clone()), cx)
                                },
                            ))
                            .child(button(
                                format!("github-pr-merge-{}", pr.number),
                                "Merge…",
                                Some("git-merge"),
                                enabled
                                    && !pr.draft
                                    && (p.mine
                                        || snapshot.is_some_and(|s| {
                                            matches!(
                                                s.pulls.permission.as_str(),
                                                "WRITE" | "MAINTAIN" | "ADMIN"
                                            )
                                        })),
                                cx,
                                move |app, _, cx| {
                                    if let Some(p) = app.github.panel.as_mut() {
                                        p.merge_confirm = Some(merge.clone())
                                    }
                                    cx.notify();
                                },
                            )),
                    ),
            );
        }
        if let Some(pr) = p.merge_confirm.as_ref() {
            let merge = pr.clone();
            body = body.child(
                div()
                    .p_3()
                    .rounded_md()
                    .bg(rgba(0xffb86b12))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(format!(
                        "Squash merge PR #{} into {}? GitHub branch protections and checks apply.",
                        pr.number, pr.base
                    ))
                    .child(button(
                        "github-confirm-merge",
                        "Squash merge",
                        Some("git-merge"),
                        enabled,
                        cx,
                        move |app, _, cx| app.run_github(Operation::Merge(merge.clone()), cx),
                    )),
            );
        }
        body = body
            .when(
                !p.mine && snapshot.is_some_and(|s| s.pulls.cursor.is_some()),
                |s| {
                    s.child(button(
                        "github-more-pulls",
                        "Load more pull requests",
                        None,
                        enabled,
                        cx,
                        |app, _, cx| app.run_github(Operation::MorePulls, cx),
                    ))
                },
            )
            .child(
                div()
                    .mt_3()
                    .text_size(theme::ui_px(13.))
                    .child("New pull request"),
            )
            .child(self.github_field(3, "Title"))
            .child(self.github_field(4, "Description"))
            .child(self.github_field(5, "Base branch"))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(button(
                        "github-pr-draft",
                        if p.draft {
                            "Draft PR"
                        } else {
                            "Ready for review"
                        },
                        Some("git-pull-request-draft"),
                        enabled,
                        cx,
                        |app, _, cx| {
                            if let Some(p) = app.github.panel.as_mut() {
                                p.draft = !p.draft;
                            }
                            cx.notify();
                        },
                    ))
                    .child(button(
                        "github-create-pr",
                        "Create pull request",
                        Some("git-pull-request"),
                        enabled
                            && snapshot
                                .and_then(|s| s.checkout.as_ref())
                                .and_then(|r| r.hosted.as_ref())
                                .is_some(),
                        cx,
                        |app, _, cx| {
                            let p = app.github.panel.as_ref().unwrap();
                            let title = p.fields[3].read(cx).text();
                            let body = p.fields[4].read(cx).text();
                            let base = p.fields[5].read(cx).text();
                            app.run_github(
                                Operation::CreatePull {
                                    title,
                                    body,
                                    base,
                                    draft: p.draft,
                                },
                                cx,
                            );
                        },
                    )),
            );
        body.into_any_element()
    }
    fn github_history_view(&self, cx: &Context<Self>) -> AnyElement {
        let enabled = self.github.busy.is_none();
        div()
            .flex()
            .flex_col()
            .gap_1()
            .children(self.github.history.iter().map(|commit| {
                let oid = commit.oid.clone();
                button(
                    format!("github-commit-{}", commit.oid),
                    format!(
                        "{}  {} · {} · {}",
                        short(&commit.oid, 8),
                        commit.subject,
                        commit.author,
                        commit.date.split('T').next().unwrap_or("")
                    ),
                    Some("git-commit"),
                    enabled,
                    cx,
                    move |app, _, cx| app.run_github(Operation::CommitDiff(oid.clone()), cx),
                )
            }))
            .into_any_element()
    }
    fn github_account_view(&self, cx: &Context<Self>) -> AnyElement {
        let Some(account) = self.github.account.as_ref() else {
            return div()
                .text_size(theme::ui_px(11.))
                .text_color(rgb(MUTED))
                .child("Reading your GitHub contributions…")
                .into_any_element();
        };
        let url = account.url.clone();
        let days = account.days.clone();
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(button(
                "github-account-browser",
                format!("@{} · {} contributions", account.login, account.total),
                Some("external"),
                true,
                cx,
                move |app, _, cx| app.github_browse(url.clone(), cx),
            ))
            .child(
                canvas(
                    |bounds, _, _| bounds,
                    move |bounds, _, window, _| {
                        let columns = (days.len() + 6) / 7;
                        let step = (f32::from(bounds.size.width) / columns.max(1) as f32).min(13.);
                        for (i, (_, count)) in days.iter().enumerate() {
                            let color = match count {
                                0 => 0xffffff10,
                                1..=3 => 0x255e40ff,
                                4..=7 => 0x3a8d58ff,
                                8..=12 => 0x63bb78ff,
                                _ => 0x9ae6a2ff,
                            };
                            window.paint_quad(
                                fill(
                                    Bounds::new(
                                        bounds.origin
                                            + point(
                                                px((i / 7) as f32 * step),
                                                px((i % 7) as f32 * 13.),
                                            ),
                                        size(px((step - 2.).max(2.)), px(10.)),
                                    ),
                                    rgba(color),
                                )
                                .corner_radii(px(2.)),
                            );
                        }
                    },
                )
                .h(px(92.))
                .w_full(),
            )
            .child(
                div()
                    .text_size(theme::ui_px(11.))
                    .text_color(rgb(MUTED))
                    .child(format!(
                        "{} commits · {} pull requests · {} reviews · {} issues",
                        account.commits, account.prs, account.reviews, account.issues
                    )),
            )
            .child(
                div()
                    .text_size(theme::ui_px(12.))
                    .child("Recently active repositories"),
            )
            .children(account.repositories.iter().map(|(name, url, private)| {
                let url = url.clone();
                button(
                    format!("github-account-repo-{name}"),
                    format!("{name}{}", if *private { " · private" } else { "" }),
                    Some("workspace-folder"),
                    true,
                    cx,
                    move |app, _, cx| {
                        if let Some(p) = app.github.panel.as_mut() {
                            p.tab = Tab::Clone;
                            p.fields[6].update(cx, |i, cx| i.set_text(&url, cx));
                        }
                        cx.notify();
                    },
                )
            }))
            .into_any_element()
    }
    fn github_clone_view(&self, cx: &Context<Self>) -> AnyElement {
        let enabled = self.github.busy.is_none();
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(self.github_field(6, "Repository URL"))
            .child(self.github_field(7, "New local folder · full path"))
            .child(button(
                "github-clone",
                "Clone repository",
                Some("git-download"),
                enabled,
                cx,
                |app, _, cx| {
                    let p = app.github.panel.as_ref().unwrap();
                    let url = p.fields[6].read(cx).text();
                    let destination = p.fields[7].read(cx).text();
                    let destination = if let Some(rest) = destination.strip_prefix("~/") {
                        dirs::home_dir().unwrap_or_default().join(rest)
                    } else {
                        PathBuf::from(destination)
                    };
                    app.run_github(Operation::Clone { url, destination }, cx);
                },
            ))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    fn prepare(app: &mut Orbit, cx: &mut Context<Orbit>) {
        let root = app.project.clone();
        app.github.freeze_reads = true;
        app.github.root = Some(root.clone());
        app.github.snapshot = Some(Arc::new(Snapshot {
            checkout: Some(github::Checkout {
                root: root.clone(),
                git_dir: root.join(".git"),
                common_dir: root.join(".git"),
                branch: Some("feature/demo".into()),
                head: Some("a".repeat(40)),
                upstream: Some("origin/feature/demo".into()),
                ahead: 2,
                behind: 0,
                remotes: vec!["origin".into()],
                remote: Some("origin".into()),
                remote_url: Some("git@github.com:demo/project.git".into()),
                push_hosted: Some(github::HostedRepo {
                    host: "github.com".into(),
                    owner: "demo".into(),
                    name: "project".into(),
                }),
                remote_repos: vec![(
                    "origin".into(),
                    github::HostedRepo {
                        host: "github.com".into(),
                        owner: "demo".into(),
                        name: "project".into(),
                    },
                )],
                hosted: Some(github::HostedRepo {
                    host: "github.com".into(),
                    owner: "demo".into(),
                    name: "project".into(),
                }),
                branches: vec![
                    github::Branch {
                        name: "feature/demo".into(),
                        remote: false,
                        current: true,
                        upstream: "origin/feature/demo".into(),
                    },
                    github::Branch {
                        name: "feature/other".into(),
                        remote: false,
                        current: false,
                        upstream: "".into(),
                    },
                ],
                changes: github::Summary::from_files(vec![github::FileChange {
                    path: "src/main.rs".into(),
                    old_path: None,
                    status: "Modified".into(),
                    counts: github::Counts {
                        added: 55,
                        removed: 12,
                        binary: false,
                    },
                }]),
                conflicted: false,
            }),
            pulls: github::PullPage {
                login: "demo".into(),
                parent: None,
                default_branch: "main".into(),
                permission: "WRITE".into(),
                private: true,
                total: 1,
                items: vec![PullRequest {
                    number: 17,
                    repository: Some(github::HostedRepo {
                        host: "github.com".into(),
                        owner: "demo".into(),
                        name: "project".into(),
                    }),
                    title: "Improve native UI".into(),
                    url: "https://github.com/demo/project/pull/17".into(),
                    state: "OPEN".into(),
                    draft: false,
                    head: "feature/demo".into(),
                    base: "main".into(),
                    head_oid: "a".repeat(40),
                    author: "demo".into(),
                    review: "APPROVED".into(),
                    checks: "SUCCESS".into(),
                    mergeable: "MERGEABLE".into(),
                    added: 55,
                    removed: 12,
                    files: 1,
                }],
                cursor: None,
            },
            gh_available: true,
            github_error: None,
        }));
        let mut snapshot = (**app.github.snapshot.as_ref().unwrap()).clone();
        app.github.mine = Some(snapshot.pulls.clone());
        app.github.mine_host = Some("github.com".into());
        app.github.mine_checked = Some(Instant::now());
        snapshot.pulls.items.clear();
        snapshot.pulls.total = 0;
        app.github.snapshot = Some(Arc::new(snapshot));
        cx.notify();
    }
    #[gpui::test]
    fn github_widget_and_tabs_are_native_click_only_and_toggle(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update(cx, prepare);
        crate::ui_tests::draw_frame(&view, cx);
        assert!(cx.debug_bounds("github-sidebar-widget").is_some());
        view.update(cx, |app, _| {
            assert!(
                app.github.panel.is_none(),
                "PRs must appear before the hub is opened"
            );
            assert_eq!(app.github.snapshot.as_ref().unwrap().pulls.total, 0);
            assert_eq!(app.github.personal().unwrap().total, 1);
        });
        assert!(
            cx.debug_bounds("github-widget-pr-https://github.com/demo/project/pull/17")
                .is_some()
        );
        let clip = cx.debug_bounds("github-widget-clip").unwrap();
        let widget = cx.debug_bounds("github-sidebar-widget").unwrap();
        assert!(
            widget.bottom() <= clip.bottom(),
            "sidebar glass was clipped: {widget:?} / {clip:?}"
        );
        crate::ui_tests::click_selector(cx, "github-open");
        crate::ui_tests::draw_frame(&view, cx);
        crate::ui_tests::click_selector(cx, "github-open");
        view.update(cx, |app, _| {
            assert!(
                app.github.panel.as_ref().is_none_or(|p| p.closing),
                "second click reopened GitHub"
            )
        });
        // Finish the finite dismissal before reopening.
        view.update(cx, |app, cx| {
            app.github.panel = None;
            cx.notify();
        });
        crate::ui_tests::draw_frame(&view, cx);
        crate::ui_tests::click_selector(cx, "github-open");
        crate::ui_tests::draw_frame(&view, cx);
        assert!(cx.debug_bounds("github-panel").is_some());
        for (id, glyph_id) in [
            ("github-open", "github-open-glyph"),
            ("github-refresh", "github-refresh-glyph"),
            ("github-close", "github-close-glyph"),
        ] {
            let control = cx.debug_bounds(id).unwrap();
            let glyph = cx.debug_bounds(glyph_id).unwrap();
            assert!(
                (control.center().x - glyph.center().x).abs() < px(0.5),
                "{id} glyph is horizontally displaced"
            );
            assert!(
                (control.center().y - glyph.center().y).abs() < px(0.5),
                "{id} glyph is vertically displaced"
            );
        }
        crate::ui_tests::click_selector(cx, "github-tab-Branches");
        crate::ui_tests::draw_frame(&view, cx);
        assert!(cx.debug_bounds("github-branch-feature/other").is_some());
        crate::ui_tests::click_selector(cx, "github-tab-Pulls");
        crate::ui_tests::draw_frame(&view, cx);
        assert!(cx.debug_bounds("github-pr-diff-17").is_some());
        cx.simulate_keystrokes("escape");
        view.update(cx, |app, _| {
            assert!(app.github.panel.as_ref().is_some_and(|p| p.closing))
        });
    }
    #[gpui::test]
    fn response_commit_card_is_next_to_copy_and_cannot_retarget_another_chat(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update(cx, |app, cx| {
            prepare(app, cx);
            app.active = Some("fixture-session".into());
            app.rows = vec![Row {
                role: Some("assistant:codex".into()),
                block: MessageBlock::Paragraph("I changed the native UI.".into()),
            }];
            app.list.reset(1);
            cx.notify();
        });
        crate::ui_tests::draw_frame(&view, cx);
        let copy = cx.debug_bounds("message-copy-0").unwrap();
        let sync = cx.debug_bounds("response-sync-0").unwrap();
        assert!(sync.left() >= copy.right() - px(1.));
        assert!((sync.center().y - copy.center().y).abs() < px(3.));
        crate::ui_tests::click_selector(cx, "response-sync-0");
        crate::ui_tests::draw_frame(&view, cx);
        view.update(cx, |app, cx| {
            let repo = app
                .github
                .snapshot
                .as_ref()
                .unwrap()
                .checkout
                .as_ref()
                .unwrap()
                .clone();
            let p = app.github.panel.as_mut().unwrap();
            p.request = uuid::Uuid::new_v4();
            p.preview_pending = false;
            p.preview = Some(SyncPreview {
                checkout: repo,
                fingerprint: "fixture".into(),
            });
            p.error = None;
            p.fields[0].update(cx, |i, cx| i.set_text("Native UI polish", cx));
            cx.notify();
        });
        crate::ui_tests::draw_frame(&view, cx);
        let panel = cx.debug_bounds("github-panel").unwrap();
        let push = cx.debug_bounds("github-commit-push").unwrap();
        assert!(
            panel.contains(&push.center()),
            "Commit action escaped the glass card"
        );
        cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                app.github.panel = None;
                let old = app.active.clone();
                app.active = None;
                app.open_response_sync(old, app.project.clone(), window, cx);
                assert!(app.github.panel.is_none());
            })
        });
    }
    #[gpui::test]
    fn personal_pr_cache_is_shared_across_projects_but_scoped_to_its_github_host(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update(cx, |app, cx| {
            prepare(app, cx);
            assert_eq!(app.github.personal().unwrap().total, 1);
            let snapshot = Arc::make_mut(app.github.snapshot.as_mut().unwrap());
            snapshot.checkout.as_mut().unwrap().root = "/another-project".into();
            assert_eq!(app.github.personal().unwrap().total, 1);
            let snapshot = Arc::make_mut(app.github.snapshot.as_mut().unwrap());
            snapshot
                .checkout
                .as_mut()
                .unwrap()
                .hosted
                .as_mut()
                .unwrap()
                .host = "github.corp.example".into();
            assert!(
                app.github.personal().is_none(),
                "Another GitHub host must not display cached public-account PRs"
            );
            assert_eq!(app.github.natural_widget_height(), 64.);
        });
    }
    #[gpui::test]
    #[ignore = "Explicit read-only startup validation using the installed gh login, with no local repository or hub"]
    fn github_live_bootstrap_without_repository_snapshot(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update(cx, |app, cx| {
            app.github = State::default();
            app.project = PathBuf::from("/private/tmp/kai-nonexistent-github-startup-project");
            app.active = None;
            app.refresh_personal_pulls(false, cx);
            assert!(
                app.github.mine_job.is_some(),
                "Account loading must not wait for a local snapshot"
            );
        });
        cx.run_until_parked();
        view.update(cx, |app, _| {
            assert!(app.github.panel.is_none(), "The hub should remain closed");
            assert!(
                app.github.snapshot.is_none(),
                "No project snapshot should be required"
            );
            assert!(app.github.mine_job.is_none());
            assert!(
                app.github.mine_error.is_none(),
                "{:?}",
                app.github.mine_error
            );
            let page = app
                .github
                .personal()
                .expect("Automatic startup did not populate personal PRs");
            assert!(!page.login.is_empty());
            eprintln!(
                "KAI_GITHUB_NO_REPO_STARTUP_OK {} personal PRs for {}",
                page.total, page.login
            );
        });
        crate::ui_tests::draw_frame(&view, cx);
    }
    #[gpui::test]
    fn github_widget_is_optional_and_setting_round_trips(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        crate::ui_tests::draw_frame(&view, cx);
        let before = cx.debug_bounds("sidebar-layout-header").unwrap().top();
        view.update(cx, |app, cx| {
            app.theme.github_widget = false;
            let restored: theme::Theme =
                serde_json::from_str(&serde_json::to_string(&app.theme).unwrap()).unwrap();
            assert!(!restored.github_widget);
            cx.notify();
        });
        crate::ui_tests::draw_frame(&view, cx);
        assert!(
            cx.debug_bounds("sidebar-layout-header").unwrap().top() < before,
            "Hiding the widget did not reclaim sidebar space"
        );
        cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                app.composer.update(cx, |i, cx| i.set_text("/github", cx));
                assert!(app.handle_command(window, cx));
                assert_eq!(app.github.panel.as_ref().unwrap().tab, Tab::Repository);
            })
        });
    }
#[gpui::test]
fn file_viewer_changes_tab_tracks_projects_and_opens_github_reviews(cx:&mut TestAppContext){
    use crate::ui_tests::{fixture,draw_frame,click_selector};
    let(view,cx)=fixture(cx);
    cx.update(|w,cx|view.update(cx,|app,cx|{app.motion=false;app.github.freeze_reads=true;app.toggle_files(w,cx);}));
    draw_frame(&view,cx);assert!(cx.debug_bounds("file-tab-files").is_some());assert!(cx.debug_bounds("file-tab-changes").is_some());
    click_selector(cx,"file-tab-changes");draw_frame(&view,cx);
    view.update(cx,|app,cx|{assert!(!app.files_visible);assert_eq!(app.diff_view.as_ref().unwrap().read(cx).test_root(),app.execution_root().as_deref().unwrap_or(&app.project));});
    assert!(cx.debug_bounds("changes-inspector").is_some());assert!(cx.debug_bounds("file-tab-github").is_some());
    cx.update(|w,cx|view.update(cx,|app,cx|{let next=app.project.join("different-project");std::fs::create_dir_all(&next).unwrap();app.active=None;app.new_execution_path=None;app.project=next.clone();app.sync_file_root(w,cx);assert_eq!(app.diff_view.as_ref().unwrap().read(cx).test_root(),next);}));
    click_selector(cx,"file-tab-files");draw_frame(&view,cx);view.update(cx,|app,_|assert!(app.files_visible&&app.diff_view.is_none()));
    click_selector(cx,"file-tab-github");draw_frame(&view,cx);view.update(cx,|app,_|assert_eq!(app.github.panel.as_ref().unwrap().tab,Tab::Pulls));
    cx.update(|w,cx|view.update(cx,|app,cx|{let captured=app.project.clone();app.close_github(w,cx);app.open_github_diff(captured.clone(),"diff --git a/example.rs b/example.rs\n--- a/example.rs\n+++ b/example.rs\n@@ -1 +1 @@\n-old\n+new\n".into(),cx);app.project=captured.join("other");app.sync_file_root(w,cx);assert_eq!(app.diff_view.as_ref().unwrap().read(cx).test_root(),captured,"A GitHub review must retain its captured repository");}));
}

}
