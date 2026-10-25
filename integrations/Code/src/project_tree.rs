//! Cached summary-only project tree. Flattening happens on metadata changes,
//! never while painting or streaming a response.
use super::*;

#[derive(Clone)]
enum Node {
    Project(usize),
    Chat(usize, Option<PathBuf>, usize),
    Placeholder(usize),
    RecentHeader,
}
#[derive(Default)]
pub(super) struct ProjectTree {
    pub activity: Vec<usize>,
    rows: Vec<Node>,
    recent_roots: Vec<PathBuf>,
    pending: HashMap<PathBuf, u64>,
    ready: HashSet<PathBuf>,
    failed: HashSet<PathBuf>,
    motions: HashMap<PathBuf, (Instant, bool, f32)>,
    sequence: u64,
    restored: bool,
    #[cfg(test)]
    rendered: usize,
}

impl Orbit {
    pub(super) fn unprojected_root(&self) -> PathBuf {
        discovery::unprojected_root(&self.controls_home)
    }

    /// Keep navigation separate from the CLI cwd. Each standalone chat owns a
    /// fresh workspace; no filesystem work or agent launch happens on a click.
    pub(super) fn new_unprojected_conversation(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.new_conversation(window, cx);
        self.project = self.unprojected_root();
        self.new_execution_path = Some(self.project.join(uuid::Uuid::new_v4().to_string()));
        self.all_projects = false;
        self.archived_view = false;
        self.rebuild_workspace_sessions();
        self.sync_file_root(window, cx);
        self.queue_workspace_save(cx);
        cx.notify();
    }

    /// Only Code-owned, UUID-named standalone workspaces may be created by the
    /// launch worker. Never recreate a missing user-selected project directory.
    pub(super) fn unprojected_workspace(&self, session: &SessionSummary) -> Option<PathBuf> {
        let root = self.unprojected_root();
        let workspace = session.execution_path.as_ref()?;
        (session.project_path == root
            && workspace.parent() == Some(root.as_path())
            && workspace
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| uuid::Uuid::parse_str(name).is_ok()))
        .then(|| workspace.clone())
    }

    pub(super) fn invalidate_project_branches(&mut self) {
        self.project_tree.ready.clear();
        self.project_tree.failed.clear();
        // Replies from discovery before this refresh must not mark a branch
        // current again. Visible expanded rows request fresh summaries lazily.
        self.project_tree.pending.clear();
    }

    pub(super) fn sidebar_target_width(&self) -> f32 {
        if self.sidebar_split { 200. } else { 280. }
    }

    pub(super) fn rebuild_project_tree(&mut self) {
        self.project_tree.activity = self
            .sessions
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                !self.hidden_sessions.contains(&s.id) && self.activity.pending_rank(&s.id).is_some()
            })
            .map(|(ix, _)| ix)
            .collect();
        self.project_tree.activity.sort_by_key(|ix| {
            std::cmp::Reverse(self.activity.pending_rank(&self.sessions[*ix].id))
        });
        let paths: HashMap<_, _> = self
            .projects
            .iter()
            .enumerate()
            .map(|(ix, p)| (p.path.as_path(), ix))
            .collect();
        let compare = |a: &usize, b: &usize| {
            let key = |ix: usize| {
                let s = &self.sessions[ix];
                (
                    std::cmp::Reverse(if self.all_projects {
                        self.activity.opened_rank(&s.id)
                    } else {
                        0
                    }),
                    !self.lifecycle.pinned_chats.contains(&s.id),
                    std::cmp::Reverse(s.updated_at),
                    s.id.as_str(),
                )
            };
            key(*a).cmp(&key(*b))
        };
        let storage_roots: HashSet<_> = ["Kai", "Orbit", "Code"]
            .into_iter()
            .map(|app| {
                self.controls_home
                    .join("Library/Application Support")
                    .join(app)
                    .join("Chats")
            })
            .collect();
        self.project_tree.recent_roots = self
            .projects
            .iter()
            .filter(|p| storage_roots.contains(&p.path))
            .map(|p| p.path.clone())
            .collect();
        let mut all: Vec<_> = self
            .sessions
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                !self.hidden_sessions.contains(&s.id)
                    && self.lifecycle.archived_chats.contains(&s.id) == self.archived_view
            })
            .map(|(ix, _)| ix)
            .collect();
        all.sort_by(compare);
        let mut groups = vec![Vec::new(); self.projects.len()];
        let mut ungrouped = Vec::new();
        // Populate branches in the same pin/recency order as the flat list:
        // one sort per metadata change, no transcript loading or per-frame work.
        for ix in &all {
            let owner = self.sessions[*ix]
                .project_path
                .ancestors()
                .find_map(|path| paths.get(path));
            if let Some(project) = owner {
                groups[*project].push(*ix);
            }
            if owner.is_none_or(|project| storage_roots.contains(&self.projects[*project].path)) {
                ungrouped.push(*ix);
            }
        }
        let mut rows = Vec::new();
        if self.all_projects {
            rows.extend(
                all.into_iter()
                    .enumerate()
                    .map(|(n, ix)| Node::Chat(ix, None, n)),
            );
        } else {
            for (ix, project) in self.projects.iter().enumerate() {
                if storage_roots.contains(&project.path) {
                    continue;
                }
                rows.push(Node::Project(ix));
                if !self.expanded_projects.contains(&project.path)
                    && self
                        .project_tree
                        .motions
                        .get(&project.path)
                        .is_none_or(|(_, open, _)| *open)
                {
                    continue;
                }
                if groups[ix].is_empty() {
                    rows.push(Node::Placeholder(ix));
                } else {
                    rows.extend(
                        groups[ix].iter().enumerate().map(|(n, session)| {
                            Node::Chat(*session, Some(project.path.clone()), n)
                        }),
                    );
                }
            }
            rows.push(Node::RecentHeader);
            rows.extend(
                ungrouped
                    .into_iter()
                    .enumerate()
                    .map(|(n, ix)| Node::Chat(ix, None, n)),
            );
        }
        self.project_tree.rows = rows;
    }

    fn request_project_branch(&mut self, path: PathBuf) {
        if self.project_tree.ready.contains(&path) || self.project_tree.pending.contains_key(&path)
        {
            return;
        }
        self.project_tree.sequence = self.project_tree.sequence.wrapping_add(1);
        let token = self.project_tree.sequence;
        self.project_tree.pending.insert(path.clone(), token);
        self.project_tree.failed.remove(&path);
        self.engine.load_sidebar_project(path, token);
    }

    pub(super) fn receive_project_branch(
        &mut self,
        path: PathBuf,
        token: u64,
        sessions: Result<Vec<SessionSummary>, String>,
        cx: &mut Context<Self>,
    ) {
        if self.project_tree.pending.get(&path) != Some(&token) {
            return;
        }
        self.project_tree.pending.remove(&path);
        match sessions {
            Ok(sessions) => {
                self.project_tree.ready.insert(path);
                let mut positions: HashMap<_, _> = self
                    .sessions
                    .iter()
                    .enumerate()
                    .map(|(ix, s)| (s.id.clone(), ix))
                    .collect();
                for mut session in sessions {
                    if let Some(ix) = positions.get(&session.id) {
                        merge_session_metadata(&mut session, &self.sessions[*ix]);
                        self.sessions[*ix] = session;
                    } else {
                        positions.insert(session.id.clone(), self.sessions.len());
                        self.sessions.push(session);
                    }
                }
            }
            Err(_) => {
                self.project_tree.failed.insert(path);
            }
        }
        self.rebuild_workspace_sessions();
        cx.notify();
    }

    fn toggle_project_branch(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let open = !self.expanded_projects.contains(&path);
        let from = self.project_fold_fraction(&path).clamp(0., 1.);
        if open {
            self.expanded_projects.insert(path.clone());
            self.project_tree.ready.remove(&path);
            self.request_project_branch(path.clone());
        } else {
            self.expanded_projects.remove(&path);
        }
        if self.motion {
            self.project_tree
                .motions
                .insert(path, (Instant::now(), open, from));
        } else {
            self.project_tree.motions.remove(&path);
        }
        self.all_projects = false;
        if let Some(saved) = &mut self.restore_pending {
            saved.expanded_projects = self.expanded_projects.iter().cloned().collect();
        }
        self.rebuild_project_tree();
        self.queue_workspace_save(cx);
        cx.notify();
    }

    pub(super) fn toggle_sidebar_layout(&mut self, cx: &mut Context<Self>) {
        let progress = self.sidebar_layout_progress();
        self.sidebar_split = !self.sidebar_split;
        self.sidebar_layout_transition = self.motion.then_some((Instant::now(), progress));
        self.conversations_visible = true;
        self.sidebar = true;
        if let Some(saved) = &mut self.restore_pending {
            saved.sidebar_split = self.sidebar_split;
            saved.sidebar = true;
            saved.conversations_visible = true;
        }
        self.sidebar_transition = self.sidebar.then_some((Instant::now(), self.sidebar_width));
        if !self.motion {
            self.sidebar_width = if self.sidebar {
                self.sidebar_target_width()
            } else {
                0.
            };
            self.sidebar_transition = None;
        }
        self.rebuild_project_tree();
        self.queue_workspace_save(cx);
        cx.notify();
    }

    fn project_fold_fraction(&self, path: &std::path::Path) -> f32 {
        let target = if self.expanded_projects.contains(path) {
            1.
        } else {
            0.
        };
        self.project_tree
            .motions
            .get(path)
            .map_or(target, |(start, open, from)| {
                let p = presentation::spring((start.elapsed().as_secs_f32() / 0.24).min(1.));
                let target = if *open { 1. } else { 0. };
                from + (target - from) * p
            })
    }

    pub(super) fn sidebar_layout_progress(&self) -> f32 {
        let target = if self.sidebar_split { 1. } else { 0. };
        self.sidebar_layout_transition
            .map_or(target, |(start, from)| {
                (from
                    + (target - from)
                        * presentation::spring((start.elapsed().as_secs_f32() / 0.65).min(1.)))
                .clamp(0., 1.)
            })
    }

    pub(super) fn tick_project_tree(&mut self, window: &Window, cx: &mut Context<Self>) {
        if let Some((start, _)) = self.sidebar_layout_transition {
            if !self.motion || start.elapsed() >= Duration::from_millis(650) {
                self.sidebar_layout_transition = None;
            } else if window.is_window_active() {
                request_motion_frame(window, cx);
            }
        }
        if !self.sidebar_split && !self.project_tree.restored && !self.projects.is_empty() {
            self.project_tree.restored = true;
            self.rebuild_project_tree();
        }
        let before = self.project_tree.motions.len();
        self.project_tree
            .motions
            .retain(|_, (start, _, _)| self.motion && start.elapsed() < Duration::from_millis(240));
        if before != self.project_tree.motions.len() {
            self.rebuild_project_tree();
        }
        if !self.project_tree.motions.is_empty()
            && !self.sidebar_split
            && self.sidebar
            && window.is_window_active()
        {
            request_motion_frame(window, cx);
        }
    }

    fn tree_menu(
        &mut self,
        target: MenuTarget,
        event: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.modal = None;
        self.action_menu = Some(ActionMenu {
            target,
            anchor: event.position() + point(px(10.), px(-20.)),
            started: Instant::now(),
            closing: false,
            closing_from: 1.,
        });
        self.focus.focus(window);
        cx.notify();
    }

    pub(super) fn project_tree_list(&self, cx: &Context<Self>) -> impl IntoElement {
        div()
            .flex_1()
            .min_h(px(0.))
            .w_full()
            .flex()
            .flex_col()
            .child(
                uniform_list(
                    "project-chat-tree",
                    self.project_tree.rows.len(),
                    cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                        range
                            .map(|ix| this.project_tree_row(ix, cx))
                            .collect::<Vec<_>>()
                    }),
                )
                .debug_selector(|| "project-chat-tree".into())
                .flex_1()
                .min_h(px(0.))
                .w_full()
                .px_2(),
            )
            .when(
                self.archived_view && self.archive_offset < self.archive_ids.len(),
                |s| {
                    s.child(
                        div()
                            .id("tree-archive-more")
                            .h(px(28.))
                            .px_3()
                            .text_size(crate::theme::ui_px(11.))
                            .cursor_pointer()
                            .text_color(rgb(MUTED))
                            .child("Load more archived chats")
                            .on_click(cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.load_archived_page(cx);
                            })),
                    )
                },
            )
            .child(
                div()
                    .id("tree-new-chat")
                    .debug_selector(|| "tree-new-chat".into())
                    .h(px(36.))
                    .mx_2()
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|s| s.bg(rgba(GLASS_HOVER)))
                    .text_color(rgb(MUTED))
                    .child(icon("plus", 14., MUTED))
                    .child("New conversation")
                    .child(div().flex_1())
                    .child(div().text_size(crate::theme::ui_px(10.)).child("⌘N"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        cx.stop_propagation();
                        this.new_conversation(window, cx);
                    })),
            )
    }

    fn project_tree_row(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        #[cfg(test)]
        {
            self.project_tree.rendered += 1;
        }
        let Some(node) = self.project_tree.rows.get(ix).cloned() else {
            return div().into_any_element();
        };
        let row = div().h(px(32.)).w_full().min_w(px(0.));
        match node {
            Node::RecentHeader => {
                for path in self.project_tree.recent_roots.clone() {
                    if !self.project_tree.failed.contains(&path) {
                        self.request_project_branch(path);
                    }
                }
                row.flex()
                    .items_center()
                    .px_2()
                    .text_size(crate::theme::ui_px(11.))
                    .text_color(rgb(MUTED))
                    .debug_selector(|| "sidebar-recents".into())
                    .child("Chats")
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("new-unprojected-chat")
                            .debug_selector(|| "new-unprojected-chat".into())
                            .size(px(24.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|s| s.bg(rgba(GLASS_HOVER)))
                            .child(icon("plus", 14., MUTED))
                            .on_click(cx.listener(|this, _, window, cx| {
                                cx.stop_propagation();
                                this.new_unprojected_conversation(window, cx);
                            })),
                    )
                    .into_any_element()
            }
            Node::Project(project_ix) => {
                let project = self.projects[project_ix].clone();
                let path = project.path.clone();
                let menu_project = project.clone();
                let new_project = project.clone();
                let open = self.expanded_projects.contains(&path);
                if open && !self.project_tree.failed.contains(&path) {
                    self.request_project_branch(path.clone());
                }
                let fraction = self.project_fold_fraction(&path);
                row.child(
                    div()
                        .id(("tree-project", project_ix))
                        .debug_selector(move || format!("project-{project_ix}"))
                        .group("tree-project")
                        .h(px(32.))
                        .w_full()
                        .min_w(px(0.))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .px_2()
                        .rounded_md()
                        .cursor_pointer()
                        .hover(|s| s.bg(rgba(GLASS_HOVER)))
                        .child(icon("chevron-right", 10., MUTED).with_transformation(
                            Transformation::rotate(radians(fraction * std::f32::consts::FRAC_PI_2)),
                        ))
                        .child(
                            icon(
                                if self.lifecycle.pinned_projects.contains(&path) {
                                    "pin"
                                } else {
                                    "folder"
                                },
                                14.,
                                MUTED,
                            )
                            .flex_none(),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(self.project_label(&project)),
                        )
                        .child(
                            div()
                                .id(("tree-project-new", project_ix))
                                .size(px(20.))
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_sm()
                                .opacity(0.)
                                .group_hover("tree-project", |s| s.opacity(1.))
                                .hover(|s| s.bg(rgba(GLASS_HOVER)))
                                .child(icon("plus", 12., MUTED))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.project = new_project.path.clone();
                                    this.all_projects = false;
                                    this.new_conversation(window, cx);
                                    this.new_execution_path = None;
                                    this.rebuild_workspace_sessions();
                                    this.sync_file_root(window, cx);
                                })),
                        )
                        .child(
                            div()
                                .id(("tree-project-menu", project_ix))
                                .debug_selector(move || format!("project-menu-{project_ix}"))
                                .size(px(20.))
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_sm()
                                .opacity(0.)
                                .group_hover("tree-project", |s| s.opacity(1.))
                                .hover(|s| s.bg(rgba(GLASS_HOVER)))
                                .child(icon("more", 12., MUTED))
                                .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                                    cx.stop_propagation();
                                    this.tree_menu(
                                        MenuTarget::Project(menu_project.clone()),
                                        e,
                                        window,
                                        cx,
                                    );
                                })),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.toggle_project_branch(path.clone(), cx);
                        })),
                )
                .into_any_element()
            }
            Node::Chat(session_ix, parent, ordinal) => {
                let Some(session) = self.sessions.get(session_ix).cloned() else {
                    return row.into_any_element();
                };
                if let Some(path) = &parent
                    && self.expanded_projects.contains(path)
                    && !self.project_tree.failed.contains(path)
                {
                    self.request_project_branch(path.clone());
                }
                let chat_project = parent.clone();
                let menu_session = session.clone();
                let selected = self.active.as_deref() == Some(&session.id);
                let running = self.runs.contains_key(&session.id)
                    || self
                        .broker_children
                        .get(&session.id)
                        .is_some_and(|n| *n > 0);
                let closing = parent
                    .as_ref()
                    .is_some_and(|p| !self.expanded_projects.contains(p));
                let entrance = parent
                    .as_ref()
                    .and_then(|p| self.project_tree.motions.get(p).map(|_| p))
                    .map(|p| {
                        let fraction = self.project_fold_fraction(p).clamp(0., 1.);
                        (
                            fraction,
                            6. * (1. - fraction) + (ordinal.min(7) as f32 * 0.15) * (1. - fraction),
                        )
                    });
                row.child(
                    div()
                        .id(("tree-chat", session_ix))
                        .debug_selector({
                            let id = session.id.clone();
                            move || format!("tree-chat-{id}")
                        })
                        .group("tree-chat")
                        .h(px(32.))
                        .w_full()
                        .min_w(px(0.))
                        .pl(px(if parent.is_some() { 28. } else { 8. }))
                        .pr_2()
                        .flex()
                        .items_center()
                        .gap_2()
                        .rounded_md()
                        .cursor_pointer()
                        .overflow_hidden()
                        .when(selected, |s| s.bg(rgba(0xffffff14)))
                        .hover(|s| s.bg(rgba(GLASS_HOVER)))
                        .when_some(entrance, |s, (opacity, offset)| {
                            s.opacity(opacity).relative().left(px(offset))
                        })
                        .child(
                            icon(agent_icon(session.agent), 12., agent_color(session.agent))
                                .flex_none(),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .text_ellipsis()
                                .overflow_hidden()
                                .text_size(crate::theme::ui_px(12.))
                                .flex()
                                .flex_col()
                                .child(div().text_ellipsis().child(self.chat_label(&session)))
                                .when(self.all_projects, |s| {
                                    s.child(
                                        div()
                                            .text_size(theme::ui_px(10.))
                                            .text_color(rgb(MUTED))
                                            .text_ellipsis()
                                            .child(
                                                session
                                                    .project_path
                                                    .file_name()
                                                    .unwrap_or_default()
                                                    .to_string_lossy()
                                                    .into_owned(),
                                            ),
                                    )
                                }),
                        )
                        .when(running, |s| {
                            s.child(
                                div()
                                    .size(px(4.))
                                    .rounded_full()
                                    .bg(rgb(0x9fc6a8))
                                    .flex_none(),
                            )
                        })
                        .when(self.lifecycle.pinned_chats.contains(&session.id), |s| {
                            s.child(icon("pin", 10., MUTED).flex_none())
                        })
                        .child(
                            div()
                                .id(("tree-chat-menu", session_ix))
                                .debug_selector(move || format!("tree-chat-menu-{session_ix}"))
                                .size(px(18.))
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_sm()
                                .opacity(if selected { 1. } else { 0. })
                                .group_hover("tree-chat", |s| s.opacity(1.))
                                .hover(|s| s.bg(rgba(GLASS_HOVER)))
                                .child(icon("more", 12., MUTED))
                                .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                                    cx.stop_propagation();
                                    this.tree_menu(
                                        MenuTarget::Chat(menu_session.clone()),
                                        e,
                                        window,
                                        cx,
                                    );
                                })),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            if closing {
                                return;
                            }
                            if let Some(project) = &chat_project {
                                this.project = project.clone();
                                this.all_projects = false;
                            }
                            this.open_session(session.clone(), window, cx);
                        })),
                )
                .into_any_element()
            }
            Node::Placeholder(project_ix) => {
                let path = self.projects[project_ix].path.clone();
                let failed = self.project_tree.failed.contains(&path);
                row.child(
                    div()
                        .id(("tree-empty", project_ix))
                        .h(px(32.))
                        .pl(px(30.))
                        .text_size(crate::theme::ui_px(11.))
                        .text_color(rgb(MUTED))
                        .flex()
                        .items_center()
                        .child(if failed {
                            "Retry loading chats"
                        } else if self.project_tree.pending.contains_key(&path) {
                            "Loading chats…"
                        } else {
                            "No conversations yet"
                        })
                        .when(failed, |s| {
                            s.cursor_pointer()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.request_project_branch(path.clone());
                                    cx.notify();
                                }))
                        }),
                )
                .into_any_element()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_tests::{click_selector, draw_frame, fixture};
    use gpui::{TestAppContext, VisualTestContext};
    use std::prelude::v1::test;

    fn unified(cx: &mut TestAppContext) -> (Entity<Orbit>, &mut VisualTestContext) {
        let (view, cx) = fixture(cx);
        view.update(cx, |app, cx| {
            app.sidebar_split = false;
            app.sidebar_width = 280.;
            app.custom_projects = vec![
                Project {
                    name: "One".into(),
                    path: app.project.join("one"),
                },
                Project {
                    name: "Two".into(),
                    path: app.project.join("two"),
                },
            ];
            app.source_projects.clear();
            for p in &app.custom_projects {
                std::fs::create_dir_all(&p.path).unwrap();
            }
            app.expanded_projects.clear();
            app.all_projects = false;
            app.rebuild_projects();
            let base = app.sessions[0].clone();
            app.sessions = (0..2)
                .map(|n| {
                    let mut s = base.clone();
                    s.id = format!("chat-{n}");
                    s.title = format!("Conversation {n}");
                    s.project_path = app.custom_projects[n].path.clone();
                    s
                })
                .collect();
            app.active = Some("chat-0".into());
            app.composer
                .update(cx, |input, cx| input.set_text("Draft stays here", cx));
            app.rebuild_workspace_sessions();
        });
        draw_frame(&view, cx);
        (view, cx)
    }

    fn double_click(cx: &mut VisualTestContext, position: Point<Pixels>) {
        cx.simulate_event(MouseDownEvent {
            position,
            button: MouseButton::Left,
            click_count: 2,
            ..Default::default()
        });
        cx.simulate_event(MouseUpEvent {
            position,
            button: MouseButton::Left,
            click_count: 2,
            ..Default::default()
        });
    }

    #[gpui::test]
    fn multiple_project_branches_keep_active_chat_and_draft_until_chat_clicked(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = unified(cx);
        click_selector(cx, "project-0");
        draw_frame(&view, cx);
        click_selector(cx, "project-1");
        draw_frame(&view, cx);
        cx.update(|_, cx| {
            let app = view.read(cx);
            assert_eq!(app.expanded_projects.len(), 2);
            assert_eq!(app.active.as_deref(), Some("chat-0"));
            assert_eq!(app.composer.read(cx).text(), "Draft stays here");
            assert_eq!(app.rows.len(), 0);
        });
        click_selector(cx, "tree-chat-chat-1");
        cx.update(|_, cx| {
            let app = view.read(cx);
            assert_eq!(app.active.as_deref(), Some("chat-1"));
            assert_eq!(app.drafts["chat-0"], "Draft stays here");
            assert_eq!(app.project, app.custom_projects[1].path);
        });
        draw_frame(&view, cx);
        click_selector(cx, "project-0");
        draw_frame(&view, cx);
        // A selected chat remains open when its parent folder is collapsed.
        click_selector(cx, "project-1");
        cx.update(|_, cx| {
            let app = view.read(cx);
            assert!(app.expanded_projects.is_empty());
            assert_eq!(app.active.as_deref(), Some("chat-1"));
        });
    }

    #[gpui::test]
    fn discovery_refresh_invalidates_branch_summaries_and_stale_replies(cx: &mut TestAppContext) {
        let (view, cx) = unified(cx);
        view.update(cx, |app, cx| {
            let path = app.custom_projects[0].path.clone();
            app.project_tree.ready.insert(path.clone());
            app.project_tree.pending.insert(path.clone(), 7);
            app.receive(
                Event::Ready(discovery::Discovery::default(), Vec::new(), None),
                cx,
            );
            assert!(app.project_tree.ready.is_empty());
            assert!(app.project_tree.pending.is_empty());
            app.receive_project_branch(path, 7, Ok(Vec::new()), cx);
            assert!(app.project_tree.ready.is_empty());
        });
    }

    #[gpui::test]
    fn restoring_active_chat_preserves_collapsed_project(cx: &mut TestAppContext) {
        let (view, cx) = unified(cx);
        view.update_in(cx, |app, window, cx| {
            let session = app.sessions[0].clone();
            app.restore_model = Some((session.id.clone(), None));
            app.open_session(session, window, cx);
            assert!(app.expanded_projects.is_empty());
        });
    }

    #[gpui::test]
    fn double_clicking_split_controls_keeps_the_layout(cx: &mut TestAppContext) {
        let (view, cx) = unified(cx);
        view.update(cx, |app, cx| app.toggle_sidebar_layout(cx));
        draw_frame(&view, cx);
        for selector in ["toggle-archived-chats", "new-session", "workspace-search"] {
            let bounds = cx.debug_bounds(selector).unwrap();
            double_click(cx, bounds.center());
            draw_frame(&view, cx);
            cx.update(|_, cx| assert!(view.read(cx).sidebar_split));
        }
    }

    #[gpui::test]
    fn double_click_changes_layout_without_losing_expansion_or_draft(cx: &mut TestAppContext) {
        let (view, cx) = unified(cx);
        click_selector(cx, "project-0");
        draw_frame(&view, cx);
        let before = cx.update(|_, cx| view.read(cx).expanded_projects.clone());
        let header = cx.debug_bounds("sidebar-layout-header").unwrap();
        double_click(cx, header.center());
        draw_frame(&view, cx);
        cx.update(|_, cx| {
            let app = view.read(cx);
            assert!(app.sidebar_split);
            assert_eq!(app.sidebar_width, 200.);
            assert_eq!(app.expanded_projects, before);
            assert_eq!(app.composer.read(cx).text(), "Draft stays here");
        });
        let split = cx.debug_bounds("workspace-pane").unwrap();
        double_click(cx, point(split.center().x, split.top() + px(20.)));
        draw_frame(&view, cx);
        cx.update(|_, cx| {
            let app = view.read(cx);
            assert!(!app.sidebar_split);
            assert_eq!(app.sidebar_width, 280.);
            assert_eq!(app.expanded_projects, before);
        });
        let chat = cx.debug_bounds("chat-pane").unwrap();
        assert_eq!(chat.left(), px(280.));
    }

    #[gpui::test]
    fn unified_sidebar_can_be_reopened_and_archive_and_recent_views_remain_available(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = unified(cx);
        click_selector(cx, "inbox");
        draw_frame(&view, cx);
        cx.update(|_, cx| assert!(view.read(cx).all_projects));
        click_selector(cx, "sidebar-layout-header");
        draw_frame(&view, cx);
        cx.update(|_, cx| assert!(!view.read(cx).all_projects));
        click_selector(cx, "tree-archived");
        draw_frame(&view, cx);
        cx.update(|_, cx| assert!(view.read(cx).modal == Some(Modal::Activity)));
        click_selector(cx, "activity-archive");
        draw_frame(&view, cx);
        cx.update(|_, cx| assert!(view.read(cx).archived_view));
        view.update(cx, |app, cx| {
            app.toggle_sidebar();
            cx.notify();
        });
        draw_frame(&view, cx);
        click_selector(cx, "expand");
        draw_frame(&view, cx);
        cx.update(|_, cx| {
            let app = view.read(cx);
            assert!(app.sidebar);
            assert_eq!(app.sidebar_width, 280.);
        });
    }

    #[gpui::test]
    fn unprojected_chat_button_leaves_the_project_and_preserves_its_draft(cx: &mut TestAppContext) {
        let (view, cx) = unified(cx);
        click_selector(cx, "new-unprojected-chat");
        let first = cx.update(|_, cx| {
            let app = view.read(cx);
            assert!(app.active.is_none());
            assert_eq!(app.drafts["chat-0"], "Draft stays here");
            assert!(app.composer.read(cx).text().is_empty());
            assert_eq!(app.project, app.unprojected_root());
            assert!(!app.all_projects && !app.archived_view);
            let workspace = app.execution_root().unwrap();
            assert_eq!(workspace.parent(), Some(app.project.as_path()));
            assert!(!workspace.exists(), "Clicking must not do filesystem work");
            assert!(app.runs.is_empty() && !app.starting);
            assert!(!app.projects.iter().any(|p| p.path == app.project));
            workspace
        });
        click_selector(cx, "new-unprojected-chat");
        cx.update(|_, cx| {
            let app = view.read(cx);
            assert_ne!(app.execution_root().as_ref(), Some(&first));
            let mut session = app.sessions[0].clone();
            session.project_path = app.project.clone();
            session.execution_path = app.execution_root();
            assert!(app.unprojected_workspace(&session).is_some());
            session.execution_path = Some(app.project.join("not-a-managed-chat"));
            assert!(app.unprojected_workspace(&session).is_none());
            session.execution_path = Some(first);
            session.project_path = app.custom_projects[0].path.clone();
            assert!(app.unprojected_workspace(&session).is_none());
        });
        view.update_in(cx, |app, window, cx| {
            app.composer.update(cx, |input, cx| {
                input.set_text("Unsent standalone draft", cx)
            });
            let saved = app.workspace_snapshot(cx);
            let workspace = app.execution_root();
            app.project = app.custom_projects[0].path.clone();
            app.new_execution_path = None;
            app.composer.update(cx, |input, cx| input.set_text("", cx));
            app.preferences_ready = true;
            app.restore_pending = Some(saved);
            app.restore_workspace(window, cx);
            assert_eq!(app.project, app.unprojected_root());
            assert_eq!(app.execution_root(), workspace);
            assert_eq!(app.composer.read(cx).text(), "Unsent standalone draft");
            app.new_conversation(window, cx);
            assert_eq!(app.project, app.unprojected_root());
            assert_ne!(
                app.execution_root(),
                workspace,
                "New chat must not reuse the last standalone cwd"
            );
        });
    }

    #[gpui::test]
    fn unprojected_chat_sends_in_its_own_workspace_and_persists(cx: &mut TestAppContext) {
        use std::os::unix::fs::PermissionsExt;
        let (view, cx) = unified(cx);
        let (executable, database) = cx.update(|_, cx| {
            let app = view.read(cx);
            let scratch = app.controls_home.parent().unwrap();
            (
                scratch.join("claude-fixture.py"),
                scratch.join("data/history.sqlite"),
            )
        });
        std::fs::write(&executable, r#"#!/usr/bin/python3
import json, os, sys
def emit(v): print(json.dumps(v), flush=True)
v = json.loads(sys.stdin.readline())
emit({'type':'control_response','response':{'subtype':'success','request_id':v['request_id'],'response':{'commands':[]}}})
v = json.loads(sys.stdin.readline())
open('received.json', 'w').write(json.dumps({'cwd':os.getcwd(), 'input':v}))
emit({'type':'system','subtype':'init','session_id':'standalone-native','slash_commands':[]})
emit({'type':'assistant','message':{'id':'reply','content':[{'type':'text','text':'Standalone reply'}]}})
emit({'type':'result','subtype':'success','is_error':False,'result':'Standalone reply'})
"#).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        click_selector(cx, "new-unprojected-chat");
        let (workspace, root, id) = view.update_in(cx, |app, window, cx| {
            app.agent = AgentKind::Claude;
            app.agents = vec![Agent {
                kind: AgentKind::Claude,
                name: "Fixture Claude".into(),
                executable: Some(executable),
                available: true,
            }];
            app.composer
                .update(cx, |input, cx| input.set_text("Standalone prompt", cx));
            let workspace = app.execution_root().unwrap();
            app.send(window, cx);
            let id = app.active.clone().expect("Send created a chat");
            (workspace, app.project.clone(), id)
        });
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            cx.run_until_parked();
            view.update(cx, |app, cx| {
                while let Ok(event) = app.engine.events.try_recv() {
                    if matches!(
                        event,
                        Event::Parsed(_, _) | Event::PagedFinished(_) | Event::FinishError(_, _)
                    ) {
                        app.receive(event, cx);
                    }
                }
            });
            if cx.update(|_, cx| {
                let app = view.read(cx);
                !app.starting && app.runs.is_empty() && app.finalizing.is_empty()
            }) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "Standalone prompt did not finish"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let captured: serde_json::Value =
            serde_json::from_slice(&std::fs::read(workspace.join("received.json")).unwrap())
                .unwrap();
        assert_eq!(
            PathBuf::from(captured["cwd"].as_str().unwrap()),
            workspace.canonicalize().unwrap()
        );
        assert!(captured["input"].to_string().contains("Standalone prompt"));
        let flush = cx.update(|_, cx| view.read(cx).engine.flush());
        flush.recv_timeout(Duration::from_secs(5)).unwrap();
        let stored = store::Store::open(&database)
            .unwrap()
            .session(&id)
            .unwrap()
            .unwrap();
        assert_eq!(stored.project_path, root);
        assert_eq!(stored.execution_path, Some(workspace));
        assert_eq!(stored.external_id.as_deref(), Some("standalone-native"));
        cx.update(|_, cx| {
            let app = view.read(cx);
            assert!(app.project_tree.rows.iter().any(|node| matches!(node,
                Node::Chat(ix, None, _) if app.sessions[*ix].id == id)));
            assert!(!app.projects.iter().any(|p| p.path == root));
            assert!(app.rows.iter().any(|row| matches!(&row.block,
                MessageBlock::Paragraph(text) if text.contains("Standalone reply"))));
        });
    }

    #[gpui::test]
    fn ungrouped_chats_follow_projects_without_a_storage_folder_or_duplicates(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = unified(cx);
        view.update(cx, |app, _| {
            let storage = app
                .controls_home
                .join("Library/Application Support/Kai/Chats");
            let real = app.project.join("actual-repository/Chats");
            app.custom_projects.extend([
                Project {
                    name: "Chats".into(),
                    path: storage.clone(),
                },
                Project {
                    name: "Chats".into(),
                    path: real.clone(),
                },
            ]);
            let mut loose = app.sessions[0].clone();
            loose.id = "ungrouped".into();
            loose.project_path = storage.clone();
            loose.execution_path = Some(storage.join("original-working-directory"));
            let mut grouped = loose.clone();
            grouped.id = "grouped".into();
            grouped.project_path = real;
            app.sessions.extend([loose, grouped]);
            app.rebuild_projects();
            let header = app
                .project_tree
                .rows
                .iter()
                .position(|n| matches!(n, Node::RecentHeader))
                .unwrap();
            assert_eq!(
                app.project_tree.rows[..header]
                    .iter()
                    .filter(|n| matches!(n, Node::Project(_)))
                    .count(),
                3
            );
            assert!(
                app.project_tree.rows[header + 1..]
                    .iter()
                    .all(|n| matches!(n, Node::Chat(_, None, _)))
            );
            let flat: Vec<_> = app.project_tree.rows[header + 1..]
                .iter()
                .filter_map(|n| match n {
                    Node::Chat(ix, None, _) => Some(app.sessions[*ix].id.as_str()),
                    _ => None,
                })
                .collect();
            assert_eq!(flat, ["ungrouped"]);
            assert!(app.rows.is_empty());
        });
        draw_frame(&view, cx);
        click_selector(cx, "tree-chat-ungrouped");
        view.update(cx, |app, _| {
            assert_eq!(app.active.as_deref(), Some("ungrouped"));
            let session = app.sessions.iter().find(|s| s.id == "ungrouped").unwrap();
            assert!(
                session
                    .execution_path
                    .as_ref()
                    .unwrap()
                    .ends_with("original-working-directory")
            );
        });
    }

    #[gpui::test]
    fn huge_expanded_tree_renders_only_visible_summaries_and_no_transcripts(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = unified(cx);
        view.update(cx, |app, _| {
            let base = app.sessions[0].clone();
            app.sessions = (0..10_000)
                .map(|n| {
                    let mut s = base.clone();
                    s.id = format!("many-{n}");
                    s.title = format!("Chat {n}");
                    s
                })
                .collect();
            app.expanded_projects.insert(base.project_path.clone());
            app.project_tree.ready.insert(base.project_path);
            app.rebuild_workspace_sessions();
            app.project_tree.rendered = 0;
        });
        draw_frame(&view, cx);
        cx.update(|_, cx| {
            let app = view.read(cx);
            assert_eq!(
                app.project_tree
                    .rows
                    .iter()
                    .filter(|row| matches!(row, Node::Chat(..)))
                    .count(),
                10_000
            );
            assert_eq!(
                app.project_tree
                    .rows
                    .iter()
                    .filter(|row| matches!(row, Node::Project(_)))
                    .count(),
                2
            );
            assert!(app.project_tree.rendered < 100);
            assert!(app.rows.is_empty());
        });
    }

    #[gpui::test]
    fn rapid_fold_reversal_is_continuous_and_all_animation_work_stops(cx: &mut TestAppContext) {
        let (view, cx) = unified(cx);
        cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                app.motion = true;
                let path = app.custom_projects[0].path.clone();
                app.toggle_project_branch(path.clone(), cx);
                app.project_tree.motions.insert(
                    path.clone(),
                    (Instant::now() - Duration::from_millis(100), true, 0.),
                );
                let before = app.project_fold_fraction(&path).clamp(0., 1.);
                app.toggle_project_branch(path.clone(), cx);
                assert!((app.project_fold_fraction(&path) - before).abs() < 0.01);
                app.project_tree.motions.insert(
                    path.clone(),
                    (Instant::now() - Duration::from_millis(80), false, before),
                );
                let before = app.project_fold_fraction(&path).clamp(0., 1.);
                app.toggle_project_branch(path.clone(), cx);
                assert!((app.project_fold_fraction(&path) - before).abs() < 0.01);
                app.project_tree
                    .motions
                    .insert(path, (Instant::now() - Duration::from_secs(1), false, 1.));
                app.expanded_projects.clear();
                app.tick_project_tree(window, cx);
                assert!(app.project_tree.motions.is_empty());
                assert_eq!(
                    app.project_tree
                        .rows
                        .iter()
                        .filter(|row| matches!(row, Node::Project(_)))
                        .count(),
                    2
                );
                assert!(
                    !app.project_tree
                        .rows
                        .iter()
                        .any(|row| matches!(row, Node::Chat(..)))
                );
            })
        });
    }

    #[gpui::test]
    fn collapsed_projects_do_not_fetch_and_branch_menus_keep_their_targets(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = unified(cx);
        cx.update(|_, cx| assert!(view.read(cx).project_tree.pending.is_empty()));
        click_selector(cx, "project-menu-0");
        cx.update(|_,cx|{let app=view.read(cx);assert!(matches!(&app.action_menu.as_ref().unwrap().target,MenuTarget::Project(p) if p.path==app.custom_projects[0].path));assert_eq!(app.active.as_deref(),Some("chat-0"));});
        view.update(cx, |app, _| {
            app.action_menu = None;
        });
        click_selector(cx, "project-0");
        draw_frame(&view, cx);
        click_selector(cx, "tree-chat-menu-0");
        cx.update(|_,cx|{let app=view.read(cx);assert!(matches!(&app.action_menu.as_ref().unwrap().target,MenuTarget::Chat(s) if s.id=="chat-0"));assert_eq!(app.expanded_projects.len(),1);});
    }

    #[gpui::test]
    fn closest_project_owns_chats_and_stale_branch_results_cannot_replace_state(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = unified(cx);
        view.update(cx, |app, cx| {
            let parent = app.custom_projects[0].path.clone();
            let nested = parent.join("nested");
            app.custom_projects.push(Project {
                name: "Nested".into(),
                path: nested.clone(),
            });
            app.sessions[0].project_path = nested.clone();
            app.expanded_projects = HashSet::from([parent.clone(), nested.clone()]);
            app.rebuild_projects();
            app.rebuild_workspace_sessions();
            assert_eq!(
                app.project_tree
                    .rows
                    .iter()
                    .filter(|n| matches!(n, Node::Chat(0, Some(_), _)))
                    .count(),
                1
            );
            app.project_tree.pending.insert(parent.clone(), 42);
            let count = app.sessions.len();
            app.receive_project_branch(parent.clone(), 41, Ok(vec![]), cx);
            assert_eq!(app.project_tree.pending.get(&parent), Some(&42));
            assert_eq!(app.sessions.len(), count);
            app.receive_project_branch(parent, 42, Err("fixture".into()), cx);
            assert!(app.project_tree.pending.is_empty());
            assert_eq!(app.active.as_deref(), Some("chat-0"));
        });
    }
}
