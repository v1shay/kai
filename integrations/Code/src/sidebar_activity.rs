//! Summary-only activity and navigation order. No transcript reads or timers.
use super::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, Serialize, Deserialize, Hash)]
#[serde(default)]
pub(super) struct Saved {
    clock: u64,
    opened: BTreeMap<String, u64>,
    pending: BTreeMap<String, u64>,
    prompted: BTreeMap<String, i64>,
}
impl Saved {
    pub fn seed_prompt(&mut self, id: &str, at: i64) {
        self.prompted.entry(id.into()).or_insert(at);
    }
    pub fn normalize(&mut self) {
        self.prompted.retain(|id, at| !id.is_empty() && id.len() <= 512 && !id.chars().any(char::is_control) && *at > 0);
        fn trim(map: &mut BTreeMap<String, u64>, limit: usize) {
            map.retain(|id, _| {
                !id.is_empty() && id.len() <= 512 && !id.chars().any(char::is_control)
            });
            if map.len() > limit {
                let mut ranks: Vec<_> = map.iter().map(|(id, rank)| (id.clone(), *rank)).collect();
                ranks.sort_unstable_by_key(|(_, rank)| std::cmp::Reverse(*rank));
                let keep: HashSet<_> = ranks.into_iter().take(limit).map(|(id, _)| id).collect();
                map.retain(|id, _| keep.contains(id));
            }
        }
        trim(&mut self.opened, 4096);
        trim(&mut self.pending, 1024);
        self.clock = self
            .opened
            .values()
            .chain(self.pending.values())
            .copied()
            .max()
            .unwrap_or(0)
            .max(self.clock)
            .min(u64::MAX - 1);
    }
    pub fn restore_ids(&self) -> Vec<String> {
        let mut opened: Vec<_> = self.opened.iter().collect();
        opened.sort_unstable_by_key(|(_, rank)| std::cmp::Reverse(**rank));
        let mut seen = HashSet::new();
        self.pending
            .keys()
            .chain(opened.into_iter().take(256).map(|(id, _)| id))
            .filter(|id| seen.insert((*id).clone()))
            .cloned()
            .collect()
    }
}
#[derive(Default)]
pub(super) struct Activity {
    pub saved: Saved,
    live: HashSet<String>,
    revision: u64,
}
impl Activity {
    pub fn revision(&self) -> u64 {
        self.revision
    }
    #[cfg(test)]
    pub fn opened_rank(&self, id: &str) -> u64 { self.saved.opened.get(id).copied().unwrap_or(0) }
    pub fn prompt_at(&self, session: &SessionSummary) -> i64 {
        self.saved.prompted.get(&session.id).copied().unwrap_or(session.updated_at.saturating_mul(1000))
    }
    pub fn prompted(&mut self, id: &str, at: i64) {
        self.saved.prompted.insert(id.into(), at);
        self.revision = self.revision.wrapping_add(1);
    }
    pub fn pending_rank(&self, id: &str) -> Option<u64> {
        self.saved.pending.get(id).copied()
    }
    pub fn running(&self, id: &str) -> bool {
        self.live.contains(id)
    }
    pub fn open(&mut self, id: &str) {
        self.revision = self.revision.wrapping_add(1);
        self.saved.clock = self.saved.clock.saturating_add(1);
        self.saved.opened.insert(id.into(), self.saved.clock);
        if !self.running(id) {
            self.saved.pending.remove(id);
        }
        if self.saved.opened.len() > 4096 {
            self.saved.normalize();
        }
    }
    fn sync(&mut self, live: HashSet<String>) -> bool {
        if live == self.live {
            return false;
        }
        for id in live.difference(&self.live) {
            self.saved.clock = self.saved.clock.saturating_add(1);
            self.saved.pending.insert(id.clone(), self.saved.clock);
            self.saved
                .opened
                .entry(id.clone())
                .or_insert(self.saved.clock);
        }
        self.live = live;
        self.revision = self.revision.wrapping_add(1);
        true
    }
}
impl Orbit {
    pub(super) fn sync_sidebar_activity(&mut self, cx: &mut Context<Self>) {
        // Most frames contain the same tiny live set. Avoid allocating IDs or
        // rebuilding summary lists for token chunks and animation frames.
        if self.activity.live.iter().all(|id| {
            self.runs.contains_key(id)
                || self.broker_children.get(id).is_some_and(|count| *count > 0)
        }) && self.runs.keys().all(|id| self.activity.running(id))
            && self
                .broker_children
                .iter()
                .all(|(id, count)| *count == 0 || self.activity.running(id))
        {
            return;
        }
        let live = self
            .runs
            .keys()
            .chain(
                self.broker_children
                    .iter()
                    .filter(|(_, count)| **count > 0)
                    .map(|(id, _)| id),
            )
            .cloned()
            .collect();
        if self.activity.sync(live) {
            self.rebuild_project_tree();
            self.queue_workspace_save(cx);
            cx.notify();
        }
    }
    pub(super) fn sidebar_status_mark(&self, scene: &'static str, active: bool) -> AnyElement {
        let path = kai_icons::status_asset(scene);
        let color = match scene { "failure"=>0xe58c8c, "permission"|"question"=>0xf2b66e, "success"=>0x8fc89a, _=>TEXT };
        let mark = svg().path(path).size(px(14.)).flex_none().text_color(rgb(color));
        if active && self.motion && !native::reduced_motion() {
            mark.with_animation(SharedString::from(format!("sidebar-status-{scene}")),Animation::new(Duration::from_millis(1300)).repeat(),|v,t|v.opacity(0.72+0.28*(t*std::f32::consts::PI).sin())).into_any_element()
        } else { mark.into_any_element() }
    }
    pub(super) fn chat_sidebar_status(&self, id: &str, active: bool) -> AnyElement {
        let scene = self.native_pending.values().find(|p|p.owner==id && p.current(&self.runs)).map(|p|p.indicator_scene())
            .or_else(||self.runs.get(id).map(|r|kai_icons::scene_for_status(&r.status)))
            .unwrap_or(if active{"thinking"}else{"success"});
        self.sidebar_status_mark(scene,active)
    }
    pub(super) fn activity_section(&self, cx: &Context<Self>) -> AnyElement {
        let count = self.project_tree.activity.len();
        if count == 0 {
            return div().into_any_element();
        }
        div()
            .id("active-chats")
            .debug_selector(|| "active-chats".into())
            .flex_none()
            .w_full()
            .px_2()
            .pt_2()
            .child(
                div()
                    .h(px(28.))
                    .px_2()
                    .flex()
                    .items_center()
                    .text_size(theme::ui_px(11.))
                    .text_color(rgb(MUTED))
                    .child("Active"),
            )
            .child(self.activity_list("active-chat-list", count.min(5) as f32 * 44., cx))
            .into_any_element()
    }
    fn activity_list(&self, id: &'static str, height: f32, cx: &Context<Self>) -> AnyElement {
        uniform_list(
            id,
            self.project_tree.activity.len(),
            cx.processor(|this, range: std::ops::Range<usize>, window, cx| {
                range
                    .map(|ix| {
                        let session = this.sessions[this.project_tree.activity[ix]].clone();
                        this.activity_chat_row(session, window, cx)
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .h(px(height))
        .w_full()
        .into_any_element()
    }
    fn activity_chat_row(
        &self,
        session: SessionSummary,
        window: &Window,
        cx: &Context<Self>,
    ) -> AnyElement {
        let running = self.activity.running(&session.id);
        let project = session
            .project_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let mark = self.chat_sidebar_status(&session.id, running && window.is_window_active());
        let animation_id = SharedString::from(format!(
            "activity-enter-{}-{}-{running}",
            session.id,
            self.activity.pending_rank(&session.id).unwrap_or(0)
        ));
        let drag = self.chat_drag(&session);
        let row = div()
            .id(SharedString::from(format!("activity-{}", session.id)))
            .on_drag(drag, |drag, _, _, cx| cx.new(|_| drag.clone()))
            .debug_selector({
                let id = session.id.clone();
                move || format!("activity-{id}")
            })
            .h(px(44.))
            .w_full()
            .px_2()
            .flex()
            .items_center()
            .gap_2()
            .rounded_md()
            .cursor_pointer()
            .hover(|s| s.bg(rgba(GLASS_HOVER)))
            .when(self.active.as_deref() == Some(&session.id), |s| {
                s.bg(rgba(0xffffff14))
            })
            .child(provider_icon(agent_icon(session.agent), 13., agent_color(session.agent)).flex_none())
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_size(theme::ui_px(12.))
                            .text_ellipsis()
                            .child(self.chat_label(&session)),
                    )
                    .child(
                        div()
                            .text_size(theme::ui_px(10.))
                            .text_color(rgb(MUTED))
                            .text_ellipsis()
                            .child(project),
                    ),
            )
            .child(
                div()
                    .size(px(16.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(mark),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                if this.active.as_deref() == Some(&session.id) {
                    this.activity.open(&session.id);
                    this.rebuild_workspace_sessions();
                    this.dismiss_panel(window, cx);
                    this.queue_workspace_save(cx);
                } else {
                    this.open_session(session.clone(), window, cx);
                }
            }));
        if self.motion && !native::reduced_motion() && window.is_window_active() {
            row.with_animation(
                animation_id,
                Animation::new(crate::workspace_motion::CONTENT_DURATION),
                |row, t| {
                    row.opacity(presentation::ease_out(t))
                        .relative()
                        .left(px((1. - presentation::ease_out(t)) * 6.))
                },
            )
            .into_any_element()
        } else {
            row.into_any_element()
        }
    }
    pub(super) fn activity_menu(&self, cx: &Context<Self>) -> AnyElement {
        div()
            .w_full()
            .when(self.project_tree.activity.is_empty(), |s| {
                s.child(
                    div()
                        .px_3()
                        .py_2()
                        .text_color(rgb(MUTED))
                        .child("You're all caught up"),
                )
            })
            .when(!self.project_tree.activity.is_empty(), |s| {
                s.child(self.activity_list(
                    "activity-menu-list",
                    (self.project_tree.activity.len().min(7) * 44) as f32,
                    cx,
                ))
            })
            .child(
                div()
                    .id("activity-archive")
                    .debug_selector(|| "activity-archive".into())
                    .px_3()
                    .py_2()
                    .text_size(theme::ui_px(11.))
                    .text_color(rgb(MUTED))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgba(GLASS_HOVER)))
                    .child("Archived chats")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.dismiss_panel(window, cx);
                        this.toggle_archived_view(cx);
                    })),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    use std::prelude::v1::test;

    #[test]
    fn completed_activity_requires_a_click_after_completion_and_reappears_for_new_work() {
        let mut activity = Activity::default();
        assert!(activity.sync(HashSet::from(["chat".into()])));
        let first = activity.pending_rank("chat").unwrap();
        activity.open("chat");
        assert!(
            activity.pending_rank("chat").is_some(),
            "Clicking running work cannot acknowledge its later completion"
        );
        assert!(activity.sync(HashSet::new()));
        assert!(!activity.running("chat"));
        assert!(activity.pending_rank("chat").is_some());
        activity.open("another");
        assert!(activity.pending_rank("chat").is_some());
        activity.open("chat");
        assert!(activity.pending_rank("chat").is_none());
        assert!(
            !activity.sync(HashSet::new()),
            "Idle frames cannot change state or schedule work"
        );
        activity.sync(HashSet::from(["chat".into()]));
        assert!(activity.pending_rank("chat").unwrap() > first);
    }

    #[test]
    fn unread_completions_and_open_order_survive_restart_with_bounded_metadata() {
        let mut activity = Activity::default();
        activity.open("older");
        activity.open("newer");
        activity.sync(HashSet::from(["done".into()]));
        activity.sync(HashSet::new());
        let bytes = serde_json::to_vec(&activity.saved).unwrap();
        let saved: Saved = serde_json::from_slice(&bytes).unwrap();
        let restored = Activity {
            saved,
            ..Default::default()
        };
        assert!(restored.pending_rank("done").is_some());
        assert!(!restored.running("done"));
        assert!(restored.opened_rank("newer") > restored.opened_rank("older"));
        let mut saved = restored.saved;
        saved.opened = (0..6_000).map(|n| (format!("chat-{n}"), n)).collect();
        saved.normalize();
        assert_eq!(saved.opened.len(), 4096);
        assert!(saved.opened.contains_key("chat-5999"));
        assert!(!saved.opened.contains_key("chat-0"));
        let ids = saved.restore_ids();
        assert_eq!(
            ids.len(),
            257,
            "Only unread activity and the latest 256 summary IDs are warmed"
        );
        assert!(ids.iter().any(|id| id == "done"));
        assert!(ids.iter().any(|id| id == "chat-5999"));
    }

    #[gpui::test]
    fn every_provider_has_persistent_activity_above_projects_and_notifications_open_chats(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = ui_tests::fixture(cx);
        view.update(cx, |app, cx| {
            app.sidebar_split = false;
            app.sidebar_width = 280.;
            let template = app.sessions[0].clone();
            for (n, provider) in [
                AgentKind::Codex,
                AgentKind::Claude,
                AgentKind::Gemini,
                AgentKind::OpenCode,
                AgentKind::Cursor,
                AgentKind::Grok,
                AgentKind::Antigravity,
            ]
            .into_iter()
            .enumerate()
            {
                let mut session = template.clone();
                session.id = format!("running-{n}");
                session.agent = provider;
                app.runs
                    .insert(session.id.clone(), ui_tests::idle_run("Working".into()));
                app.sessions.push(session);
            }
            app.sync_sidebar_activity(cx);
        });
        ui_tests::draw_frame(&view, cx);
        let activity = cx.debug_bounds("active-chats").unwrap();
        let projects = cx.debug_bounds("sidebar-layout-header").unwrap();
        assert!(activity.bottom() <= projects.top());
        view.update(cx, |app, cx| {
            assert_eq!(app.project_tree.activity.len(), 7);
            app.runs.clear();
            app.sync_sidebar_activity(cx);
            assert_eq!(
                app.project_tree.activity.len(),
                7,
                "Completion must leave every provider visible"
            );
        });
        ui_tests::draw_frame(&view, cx);
        ui_tests::click_selector(cx, "tree-archived");
        ui_tests::draw_frame(&view, cx);
        cx.update(|_, cx| {
            assert!(view.read(cx).modal == Some(Modal::Activity));
            assert!(!view.read(cx).archived_view);
        });
        ui_tests::click_selector(cx, "activity-running-6");
        cx.update(|_, cx| {
            let app = view.read(cx);
            assert_eq!(app.active.as_deref(), Some("running-6"));
            assert!(app.activity.pending_rank("running-6").is_none());
            assert_eq!(app.project_tree.activity.len(), 6);
        });
    }

    #[gpui::test]
    fn priority_uses_last_prompt_across_projects_and_acknowledging_current_chat_keeps_viewport(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = ui_tests::fixture(cx);
        view.update(cx, |app, cx| {
            app.sidebar_split = false;
            app.all_projects = true;
            let mut old = app.sessions[0].clone();
            old.id = "old-but-visited".into();
            old.updated_at = 1;
            old.project_path = app.project.join("other-project");
            app.sessions.push(old);
            app.activity.prompted("fixture-session", 1000);
            app.activity.prompted("old-but-visited", 2000);
            app.rebuild_workspace_sessions();
            assert_eq!(app.workspace_sessions[0].id, "old-but-visited");
            app.activity.open("fixture-session");
            app.rebuild_workspace_sessions();
            assert_eq!(app.workspace_sessions[0].id, "old-but-visited");
            app.activity.prompted("fixture-session", 3000);
            app.rebuild_workspace_sessions();
            assert_eq!(app.workspace_sessions[0].id, "fixture-session");
            app.active = Some("fixture-session".into());
            app.loading = false;
            app.rows = vec![Row {
                role: Some("assistant:codex".into()),
                block: MessageBlock::Paragraph("Keep this view".into()),
            }];
            app.list.reset(1);
            app.activity.sync(HashSet::from(["fixture-session".into()]));
            app.activity.sync(HashSet::new());
            app.rebuild_workspace_sessions();
            cx.notify();
        });
        ui_tests::draw_frame(&view, cx);
        let before = cx.update(|_, cx| view.read(cx).list.logical_scroll_top());
        ui_tests::click_selector(cx, "activity-fixture-session");
        cx.update(|_, cx| {
            let app = view.read(cx);
            assert!(app.activity.pending_rank("fixture-session").is_none());
            assert!(
                !app.loading,
                "Acknowledging the already open chat must not reload it"
            );
            assert_eq!(app.rows.len(), 1);
            let after = app.list.logical_scroll_top();
            assert_eq!(before.item_ix, after.item_ix);
            assert_eq!(before.offset_in_item, after.offset_in_item);
            assert!(
                app.all_projects,
                "Opening from Priority must preserve that mode"
            );
        });
    }
}
