//! Team orchestration reuses the same bounded native transports, request tray,
//! Stop handling and lazy transcript storage as ordinary chats.
use super::*;

pub(super) struct MemberDraft {
    agent: AgentKind,
    input: Entity<TextInput>,
    _subscription: Subscription,
}
pub(super) struct Panel {
    members: Vec<MemberDraft>,
    allow_edits: bool,
    started: Instant,
}
impl Panel {
    pub(super) fn animating(&self) -> bool {
        self.started.elapsed() < Duration::from_millis(320)
    }
}
impl Orbit {
    pub(super) fn toggle_team_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.team_panel.take().is_some() {
            cx.notify();
            return;
        }
        let mut members = Vec::new();
        for agent in self.agents.iter().filter(|a| a.available) {
            let kind = agent.kind;
            let input = cx.new(|cx| {
                let mut input = TextInput::new(window, cx);
                input.set_placeholder("Assign a specific task…", cx);
                input
            });
            let subscription = cx.subscribe_in(&input, window, |_, _, _, _, cx| cx.notify());
            members.push(MemberDraft {
                agent: kind,
                input,
                _subscription: subscription,
            });
        }
        self.team_panel = Some(Panel {
            members,
            allow_edits: false,
            started: Instant::now(),
        });
        cx.notify();
    }
    fn create_team(&mut self, cx: &mut Context<Self>) {
        if self.quit_pending || self.teams.len() >= teams::MAX_TEAMS || self.starting_current() {
            return;
        }
        if self
            .context_pending
            .values()
            .any(|p| p.scope == self.context_scope())
        {
            self.notice = Some("Wait for attached chat context to finish loading.".into());
            cx.notify();
            return;
        }
        let Some(panel) = &self.team_panel else {
            return;
        };
        let members: Vec<_> = panel
            .members
            .iter()
            .filter_map(|m| {
                let task = m.input.read(cx).text();
                (!task.trim().is_empty()).then(|| teams::Member {
                    session: uuid::Uuid::new_v4().to_string(),
                    agent: m.agent,
                    model: self.agent_models.get(m.agent.command()).cloned().flatten(),
                    task: task.trim().into(),
                    status: teams::Status::Queued,
                    attempted: false,
                    cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                })
            })
            .collect();
        let team = teams::Team {
            id: uuid::Uuid::new_v4().to_string(),
            title: "Agent team".into(),
            project: self
                .execution_root()
                .unwrap_or_else(|| self.project.clone()),
            allow_edits: panel.allow_edits,
            created_at: chrono::Utc::now().timestamp(),
            members,
            context: self.context_references(),
        };
        if !team.valid() {
            self.notice = Some("Assign tasks to two to four agents, up to 4 KB each.".into());
            cx.notify();
            return;
        }
        if !self.workflow_budget_allows(None, Some(&team), None, cx) {
            return;
        }
        self.teams.push(team);
        if let Some(panel) = &mut self.team_panel {
            for member in &panel.members {
                member.input.update(cx, |input, cx| input.set_text("", cx));
            }
        }
        self.queue_workspace_save(cx);
        cx.notify();
    }
    pub(super) fn team_finished(
        &mut self,
        session: &str,
        failed: bool,
        stopped: bool,
        cx: &mut Context<Self>,
    ) {
        for team in &mut self.teams {
            if let Some(member) = team.members.iter_mut().find(|m| m.session == session) {
                member.status = if failed {
                    teams::Status::Failed
                } else if stopped {
                    teams::Status::Stopped
                } else {
                    teams::Status::Complete
                };
                if failed || stopped {
                    for next in &mut team.members {
                        if next.status == teams::Status::Queued {
                            next.status = teams::Status::Held;
                        }
                    }
                }
                self.queue_workspace_save(cx);
                return;
            }
        }
    }
    pub(super) fn advance_teams(&mut self, cx: &mut Context<Self>) {
        if self.quit_pending
            || self.starting_current()
            || self.checkpoint_busy > 0
            || self.runs.len() + self.team_starting.len() + self.launches.len() >= 4
        {
            return;
        }
        let next = self.teams.iter().find_map(|team| {
            // Shared-directory writes serialize. Read-only members may overlap;
            // no hidden worktree, Git mutation or broader permission is implied.
            let pending_overlap = self.teams.iter().any(|other| {
                teams::overlapping(&other.project, &team.project)
                    && (team.allow_edits || other.allow_edits)
                    && other.members.iter().any(|m| m.status.active())
            });
            let running_overlap = self.runs.values().any(|r| {
                r.retry.as_ref().is_some_and(|request| {
                    teams::overlapping(&request.project, &team.project)
                        && (team.allow_edits || request.allow_edits)
                })
            });
            if pending_overlap
                || running_overlap
                || (team.allow_edits && (!self.settling.is_empty() || self.checkpoint_busy > 0))
            {
                return None;
            }
            team.members
                .iter()
                .find(|m| m.status == teams::Status::Queued)
                .map(|member| (team.clone(), member.clone()))
        });
        let Some((team, member)) = next else {
            return;
        };
        let Some(executable) = self
            .agents
            .iter()
            .find(|a| a.kind == member.agent && a.available)
            .and_then(|a| a.executable.clone())
        else {
            self.team_finished(&member.session, true, false, cx);
            return;
        };
        let references = team.context.clone();
        let prompt = match context_refs::with_context(
            &format!(
                "You are a member of a user-created coding team. Your assigned task follows. Other team members may be investigating this project; stay within your assigned scope and the current native permissions. Report actual findings, checks, changed files and blockers.\n\n{}",
                member.task
            ),
            &references,
        ) {
            Ok(p) => p,
            Err(error) => {
                self.notice = Some(error.to_string());
                self.team_finished(&member.session, true, false, cx);
                return;
            }
        };
        let session = self
            .sessions
            .iter()
            .find(|session| session.id == member.session)
            .cloned()
            .unwrap_or_else(|| SessionSummary {
                id: member.session.clone(),
                title: short(&member.task.replace('\n', " "), 46),
                agent: member.agent,
                project_path: team.project.clone(),
                execution_path: None,
                updated_at: chrono::Utc::now().timestamp(),
                external_id: None,
                transcript_path: None,
            });
        if member.attempted && session.external_id.is_none() {
            for team in &mut self.teams {
                for saved in &mut team.members {
                    if saved.session == member.session {
                        saved.status = teams::Status::Held;
                    }
                }
            }
            self.notice = Some("This task has a previous attempt without a recovered native session. Open its chat and inspect the result before restarting it.".into());
            self.queue_workspace_save(cx);
            cx.notify();
            return;
        }
        let prompt = if member.attempted {
            format!(
                "Continue this team's assigned task in its existing native session. Inspect completed work first and do not repeat completed side effects.\n\n{prompt}"
            )
        } else {
            prompt
        };
        let request = RunRequest { reasoning_effort:None,
            agent: member.agent,
            executable,
            project: team.project.clone(),
            prompt,
            resume_id: session.external_id.clone(),
            fork_session: false,
            model: member.model.clone(),
            images: Vec::new(),
            allow_edits: team.allow_edits,
            codex_approvals: self.codex_approvals,
            trust_workspace: self
                .trusted_workspaces
                .contains(&(member.agent, team.project.clone())),
        };
        for t in &mut self.teams {
            if t.id == team.id {
                for m in &mut t.members {
                    if m.session == member.session {
                        m.status = teams::Status::Starting;
                        m.attempted = true;
                        m.cancel.store(false, std::sync::atomic::Ordering::Release);
                    }
                }
            }
        }
        self.team_starting.insert(member.session.clone());
        self.sessions.retain(|saved| saved.id != session.id);
        self.sessions.insert(0, session.clone());
        self.rebuild_workspace_sessions();
        self.engine.update_session(session.clone());
        self.save_workspace_now(cx);
        let retry = request.clone();
        let durable = self.workspace_store.as_ref().map(|store| store.flush());
        let history = self.engine.flush();
        let repository = self.checkpoint_repository.clone();
        let orphan_repository = repository.clone();
        let cancel = member.cancel.clone();
        let checkpoint_session = session.id.clone();
        let checkpoint_title = member.task.clone();
        let timing_key=uuid::Uuid::new_v4().to_string();let turn_started=Instant::now();let checkpoint_key=timing_key.clone();
        self.begin_turn_clock(&session.id,&timing_key,turn_started,cx);
        let job = cx.background_executor().spawn(async move {
            if let Some(durable) = durable {
                durable
                    .recv_timeout(Duration::from_secs(10))
                    .map_err(|e| anyhow::anyhow!("Team state could not be saved: {e}"))?
                    .map_err(anyhow::Error::msg)?;
            }
            history
                .recv_timeout(Duration::from_secs(10))
                .map_err(|e| anyhow::anyhow!("Team history could not be saved: {e}"))?;
            anyhow::ensure!(
                !cancel.load(std::sync::atomic::Ordering::Acquire),
                "Team task stopped before launch"
            );
            let key=checkpoint_key;
            let checkpoint = if request.allow_edits {
                Some(
                    repository
                        .as_ref()
                        .map_err(|e| anyhow::anyhow!(e.clone()))?
                        .capture_before(
                            &request.project,
                            &checkpoint_session,
                            &key,
                            &checkpoint_title,
                            &cancel,
                        )?,
                )
            } else {
                Some(
                    repository
                        .as_ref()
                        .map_err(|e| anyhow::anyhow!(e.clone()))?
                        .record_read_only(
                            &request.project,
                            &checkpoint_session,
                            &key,
                            &checkpoint_title,
                        )?,
                )
            };
            if cancel.load(std::sync::atomic::Ordering::Acquire) {
                if let (Ok(repository), Some(id)) = (&repository, &checkpoint) {
                    let _ = repository.abandon(id);
                }
                anyhow::bail!("Team task stopped before launch");
            }
            let native_input = store::NativeInput::from_prompt(&request.prompt);
            match runtime::launch(request) {
                Ok(handle) => Ok((handle, checkpoint, native_input)),
                Err(error) => {
                    if let (Ok(repository), Some(id)) = (&repository, &checkpoint) {
                        let _ = repository.abandon(id);
                    }
                    Err(error)
                }
            }
        });
        cx.spawn(async move |this, cx| {
            let result = job.await;
            let id = session.id.clone();
            let wake = match result {
                Ok((handle, checkpoint, native_input)) => {
                    let wake = handle.wake.clone();
                    let mut cleanup =
                        checkpoint
                            .as_ref()
                            .filter(|_| retry.allow_edits)
                            .and_then(|id| {
                                orphan_repository.as_ref().ok().map(|repository| {
                                    repository.cleanup_on_orphan(id.clone(), handle.settled.clone())
                                })
                            });
                    if this
                        .update(cx, |this, cx| {
                            this.team_starting.remove(&id);
                            let mut should_stop = this.quit_pending;
                            for t in &mut this.teams {
                                for m in &mut t.members {
                                    if m.session == id {
                                        if m.status == teams::Status::Starting {
                                            m.status = teams::Status::Working;
                                        } else {
                                            should_stop = true;
                                        }
                                    }
                                }
                            }
                            if should_stop {
                                handle.cancel();
                            }
                            let mut saved = member.task.clone();
                            if let Some(checkpoint) = checkpoint {
                                saved.push_str(&format!("\n\n<!--code-checkpoint:{checkpoint}-->"));
                                if retry.allow_edits {
                                    if let Some(t)=this.turn_timings.iter_mut().rev().find(|t|t.key==timing_key){t.checkpoint=Some(checkpoint.clone());}
                                    this.checkpoint_runs.insert(id.clone(), checkpoint);
                                }
                            }
                            this.engine
                                .save_prompt(session.clone(), saved, native_input);
                            this.runs.insert(
                                id.clone(),
                                Running { timing:Some(chat_timing::Live{key:timing_key.clone(),started:turn_started}),
                                    agent: retry.agent,
                                    submitted_prompt: member.task,
                                    handle,
                                    text: String::new(),
                                    live_prefix_omitted: false,
                                    status: "Thinking".into(),
                                    stream_start: None,
                                    revision: 0,
                                    parsed_revision: 0,
                                    parse_pending: false,
                                    live_blocks: Vec::new(),
                                    live_earlier: false,
                                    parse_epoch: uuid::Uuid::new_v4(),
                                    retry: Some(retry),
                                    submitted_attachments: Vec::new(),
                                },
                            );
                            this.queue_workspace_save(cx);
                            cx.notify();
                        })
                        .is_err()
                    {
                        return;
                    }
                    if let Some(cleanup) = &mut cleanup {
                        cleanup.disarm();
                    }
                    wake
                }
                Err(error) => {
                    let _ = this.update(cx, |this, cx| {
                        this.team_starting.remove(&id);
                        this.finish_turn_clock(&timing_key,turn_started,"failed",cx);
                        let stopped = member.cancel.load(std::sync::atomic::Ordering::Acquire)
                            || this.quit_pending;
                        this.team_finished(&id, !stopped, stopped, cx);
                        this.engine.finish(
                            session,
                            chat_interactions::serialize_interaction(
                                &chat_interactions::ChatInteraction::Status {
                                    title: if stopped {
                                        "Team task stopped"
                                    } else {
                                        "Team task could not start"
                                    }
                                    .into(),
                                    details: error.to_string(),
                                    status: if stopped {
                                        chat_interactions::InteractionStatus::Cancelled
                                    } else {
                                        chat_interactions::InteractionStatus::Failed
                                    },
                                },
                            ),
                        );
                        if !stopped {
                            this.notice = Some(error.to_string());
                        }
                        cx.notify();
                    });
                    return;
                }
            };
            loop {
                if wake.recv().await.is_ok() {
                    Timer::after(Duration::from_millis(16)).await;
                }
                if !matches!(this.update(cx, |this, cx| this.poll(&id, cx)), Ok(true)) {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }
    pub(super) fn team_panel_view(&self, window: &Window, cx: &Context<Self>) -> AnyElement {
        let Some(panel) = &self.team_panel else {
            return div().into_any_element();
        };
        let reveal = if self.motion {
            presentation::spring((panel.started.elapsed().as_secs_f32() / 0.32).min(1.))
                .clamp(0., 1.)
        } else {
            1.
        };
        let mut content = div()
            .id("team-content")
            .max_h(px(500.))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_2();
        for member in &panel.members {
            content = content.child(
                div()
                    .p_2()
                    .rounded_md()
                    .bg(rgba(GLASS_SURFACE))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(icon(
                                agent_icon(member.agent),
                                14.,
                                agent_color(member.agent),
                            ))
                            .child(
                                div()
                                    .text_size(crate::theme::ui_px(12.))
                                    .child(member.agent.name()),
                            ),
                    )
                    .child(
                        div()
                            .mt_2()
                            .min_h(px(34.))
                            .max_h(px(80.))
                            .child(member.input.clone()),
                    ),
            );
        }
        content = content.child(
            div()
                .id("team-edit-mode")
                .p_2()
                .rounded_md()
                .cursor_pointer()
                .hover(|s| s.bg(rgba(GLASS_HOVER)))
                .text_size(crate::theme::ui_px(11.))
                .child(if panel.allow_edits {
                    "Workspace edits · tasks run one at a time"
                } else {
                    "Read only · tasks may run together"
                })
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(panel) = &mut this.team_panel {
                        panel.allow_edits = !panel.allow_edits;
                    }
                    cx.notify();
                })),
        );
        content = content.child(
            div()
                .id("start-team")
                .p_2()
                .rounded_md()
                .cursor_pointer()
                .bg(rgba(GLASS_HOVER))
                .text_size(crate::theme::ui_px(12.))
                .child("Start assigned tasks")
                .on_click(cx.listener(|this, _, _, cx| this.create_team(cx))),
        );
        for team in self.teams.iter().rev() {
            let team_id = team.id.clone();
            let stop_id = team_id.clone();
            let dismiss_id = team_id.clone();
            content = content.child(
                div()
                    .mt_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .text_size(crate::theme::ui_px(12.))
                            .child(team.title.clone()),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("resume-{team_id}")))
                            .text_size(crate::theme::ui_px(10.))
                            .cursor_pointer()
                            .child("Resume held")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(team) = this.teams.iter_mut().find(|t| t.id == team_id)
                                {
                                    for member in &mut team.members {
                                        if member.status == teams::Status::Held {
                                            member.status = teams::Status::Queued;
                                        }
                                    }
                                }
                                this.queue_workspace_save(cx);
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("stop-{stop_id}")))
                            .text_size(crate::theme::ui_px(10.))
                            .cursor_pointer()
                            .child("Stop")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(team) = this.teams.iter_mut().find(|t| t.id == stop_id)
                                {
                                    for member in &mut team.members {
                                        member
                                            .cancel
                                            .store(true, std::sync::atomic::Ordering::Release);
                                        if let Some(run) = this.runs.get(&member.session) {
                                            run.handle.cancel();
                                        }
                                        if member.status == teams::Status::Queued {
                                            member.status = teams::Status::Held;
                                        } else if member.status == teams::Status::Starting {
                                            member.status = teams::Status::Stopped;
                                        }
                                    }
                                }
                                this.queue_workspace_save(cx);
                                cx.notify();
                            })),
                    )
                    .when(
                        !team.members.iter().any(|member| member.status.active()),
                        |s| {
                            s.child(
                                div()
                                    .id(SharedString::from(format!("dismiss-team-{dismiss_id}")))
                                    .text_size(crate::theme::ui_px(10.))
                                    .cursor_pointer()
                                    .child("Dismiss")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if this
                                            .teams
                                            .iter()
                                            .find(|team| team.id == dismiss_id)
                                            .is_some_and(|team| {
                                                !team
                                                    .members
                                                    .iter()
                                                    .any(|member| member.status.active())
                                            })
                                        {
                                            this.teams.retain(|team| team.id != dismiss_id);
                                            this.queue_workspace_save(cx);
                                            cx.notify();
                                        }
                                    })),
                            )
                        },
                    ),
            );
            for member in &team.members {
                let id = member.session.clone();
                let state = self
                    .runs
                    .get(&id)
                    .map(|r| r.status.clone())
                    .unwrap_or_else(|| member.status.label().into());
                let approvals = self
                    .native_pending
                    .values()
                    .any(|request| request.owner == id);
                content = content.child(
                    div()
                        .id(SharedString::from(format!("team-member-{id}")))
                        .p_2()
                        .rounded_md()
                        .cursor_pointer()
                        .hover(|s| s.bg(rgba(GLASS_HOVER)))
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .items_center()
                                .child(icon(
                                    agent_icon(member.agent),
                                    14.,
                                    agent_color(member.agent),
                                ))
                                .child(
                                    div()
                                        .text_size(crate::theme::ui_px(12.))
                                        .child(member.agent.name()),
                                )
                                .child(div().flex_1()),
                        )
                        .child(
                            div()
                                .text_size(crate::theme::ui_px(11.))
                                .text_ellipsis()
                                .child(short(&member.task, 64)),
                        )
                        .child(
                            div()
                                .text_size(crate::theme::ui_px(10.))
                                .text_color(rgb(if approvals { 0xd9bf83 } else { MUTED }))
                                .child(if approvals {
                                    "Needs your decision".to_owned()
                                } else {
                                    state
                                }),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if let Some(session) =
                                this.sessions.iter().find(|s| s.id == id).cloned()
                            {
                                this.open_session(session, window, cx);
                            }
                        })),
                );
            }
        }
        anchored()
            .position(point(
                (window.viewport_size().width - px(410.)).max(px(8.)),
                px(82. + (1. - reveal) * 6.),
            ))
            .snap_to_window_with_margin(px(8.))
            .child(
                div()
                    .id("team-panel")
                    .debug_selector(|| "team-panel".into())
                    .w(px(390.))
                    .p_3()
                    .rounded_lg()
                    .bg(rgba(appearance::MENU_SURFACE))
                    .border_1()
                    .border_color(rgba(0xebebeb1c))
                    .opacity(reveal)
                    .occlude()
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.team_panel = None;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .mb_2()
                            .text_size(crate::theme::ui_px(13.))
                            .child("Agent teams"),
                    )
                    .child(content),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        os::unix::fs::PermissionsExt,
        prelude::v1::test,
        sync::{Arc, atomic::AtomicBool},
    };
    fn member(agent: AgentKind, task: &str) -> teams::Member {
        teams::Member {
            session: uuid::Uuid::new_v4().to_string(),
            agent,
            model: None,
            task: task.into(),
            status: teams::Status::Queued,
            attempted: false,
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }
    fn team(project: PathBuf) -> teams::Team {
        teams::Team {
            id: uuid::Uuid::new_v4().to_string(),
            title: "Fixture team".into(),
            project,
            allow_edits: false,
            created_at: 0,
            context: Vec::new(),
            members: vec![
                member(AgentKind::Codex, "CODEX_ASSIGNED_TASK"),
                member(AgentKind::Gemini, "GEMINI_ASSIGNED_TASK"),
            ],
        }
    }
    #[gpui::test]
    fn duplicate_team_start_clears_inputs_and_failure_holds_unstarted_members(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                app.agents.iter_mut().for_each(|agent| {
                    agent.available = matches!(agent.kind, AgentKind::Codex | AgentKind::Gemini)
                });
                app.toggle_team_panel(window, cx);
                let panel = app.team_panel.as_ref().unwrap();
                assert_eq!(panel.members.len(), 2);
                for draft in &panel.members {
                    draft
                        .input
                        .update(cx, |input, cx| input.set_text("Inspect assigned scope", cx));
                }
                app.create_team(cx);
                app.create_team(cx);
                assert_eq!(app.teams.len(), 1);
                assert!(
                    app.team_panel
                        .as_ref()
                        .unwrap()
                        .members
                        .iter()
                        .all(|draft| draft.input.read(cx).text().is_empty())
                );
                let first = app.teams[0].members[0].session.clone();
                app.team_finished(&first, true, false, cx);
                assert_eq!(app.teams[0].members[0].status, teams::Status::Failed);
                assert_eq!(app.teams[0].members[1].status, teams::Status::Held);
            })
        });
    }
    #[gpui::test]
    fn attempted_team_resume_without_native_identity_holds_instead_of_replaying_task(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update(cx, |app, cx| {
            let mut team = team(app.project.clone());
            team.members[0].attempted = true;
            let agent = app
                .agents
                .iter_mut()
                .find(|agent| agent.kind == AgentKind::Codex)
                .unwrap();
            agent.available = true;
            agent.executable = Some(app.project.join("must-not-launch"));
            app.teams.push(team);
            app.advance_teams(cx);
            assert_eq!(app.teams[0].members[0].status, teams::Status::Held);
            assert!(app.team_starting.is_empty() && app.runs.is_empty());
            assert!(
                app.notice
                    .as_ref()
                    .is_some_and(|notice| notice.contains("previous attempt"))
            );
        });
    }
    fn fixture(path: &std::path::Path, source: &str) {
        std::fs::write(path, format!("#!/usr/bin/python3\n{source}")).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    #[gpui::test]
    fn two_native_provider_team_runs_are_concurrent_and_keep_prompts_approvals_and_sessions_separate(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        let (project, ids) = view.update(cx, |app, _| {
            let project = app.project.clone();
            let plan = team(project.clone());
            let ids = plan
                .members
                .iter()
                .map(|member| member.session.clone())
                .collect::<Vec<_>>();
            app.teams.push(plan);
            (project, ids)
        });
        let codex = project.join("codex-team-fixture");
        let gemini = project.join("gemini-team-fixture");
        fixture(
            &codex,
            r#"
import sys,json,time,pathlib
thread='12345678-1234-4234-8234-123456789012'
def emit(v):print(json.dumps(v),flush=True)
for line in sys.stdin:
 v=json.loads(line);m=v.get('method');p=v.get('params',{})
 if m=='initialize':emit({'id':v['id'],'result':{}})
 elif m=='config/read':emit({'id':v['id'],'result':{'config':{'model_provider':'openai'}}})
 elif m=='thread/start':
  assert p['sandbox']=='read-only'
  emit({'id':v['id'],'result':{'thread':{'id':thread}}})
 elif m=='turn/start':
  pathlib.Path('codex.prompt').write_text(''.join(x.get('text','') for x in p['input']))
  pathlib.Path('codex.started').touch()
  deadline=time.monotonic()+8
  while not pathlib.Path('gemini.started').exists():
   assert time.monotonic()<deadline,'Gemini did not launch concurrently'
   time.sleep(.01)
  emit({'id':v['id'],'result':{'turn':{'id':'codex-turn'}}})
  emit({'id':91,'method':'item/commandExecution/requestApproval','params':{'threadId':thread,'turnId':'codex-turn','itemId':'tool','command':'pwd'}})
 elif v.get('id')==91:
  assert v['result']['decision']=='decline'
  pathlib.Path('codex.declined').touch()
  emit({'method':'item/agentMessage/delta','params':{'threadId':thread,'itemId':'answer','delta':'CODEX_TEAM_RESULT'}})
  emit({'method':'turn/completed','params':{'threadId':thread,'turn':{'id':'codex-turn','status':'completed'}}})
"#,
        );
        fixture(
            &gemini,
            r#"
import sys,json,time,pathlib
def emit(v):print(json.dumps(v),flush=True)
for line in sys.stdin:
 v=json.loads(line);m=v.get('method');p=v.get('params',{})
 if m=='initialize':emit({'id':v['id'],'result':{'protocolVersion':1,'agentCapabilities':{}}})
 elif m=='session/new':emit({'id':v['id'],'result':{'sessionId':'gemini-team-native','modes':{'availableModes':[{'id':'plan'}]}}})
 elif m=='session/set_mode':
  assert p['modeId']=='plan'
  emit({'id':v['id'],'result':{}})
 elif m=='session/prompt':
  pathlib.Path('gemini.prompt').write_text(p['prompt'][0]['text'])
  pathlib.Path('gemini.started').touch()
  deadline=time.monotonic()+8
  while not pathlib.Path('codex.started').exists():
   assert time.monotonic()<deadline,'Codex did not launch concurrently'
   time.sleep(.01)
  emit({'id':92,'method':'session/request_permission','params':{'sessionId':'gemini-team-native','toolCall':{'toolCallId':'tool','title':'Read project','rawInput':{'command':'pwd'}},'options':[{'optionId':'reject-once','name':'Decline','kind':'reject_once'}]}})
 elif v.get('id')==92:
  assert v['result']=={'outcome':{'outcome':'selected','optionId':'reject-once'}}
  pathlib.Path('gemini.declined').touch()
  emit({'method':'session/update','params':{'sessionId':'gemini-team-native','update':{'sessionUpdate':'agent_message_chunk','content':{'text':'GEMINI_TEAM_RESULT'}}}})
  emit({'id':5,'result':{'stopReason':'end_turn'}})
"#,
        );
        view.update(cx, |app, cx| {
            for (kind, path) in [(AgentKind::Codex, codex), (AgentKind::Gemini, gemini)] {
                let agent = app
                    .agents
                    .iter_mut()
                    .find(|agent| agent.kind == kind)
                    .unwrap();
                agent.available = true;
                agent.executable = Some(path);
            }
            app.advance_teams(cx);
        });
        let mut decisions = std::collections::HashSet::new();
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            cx.run_until_parked();
            let done = view.update(cx, |app, cx| {
                while let Ok(event) = app.engine.events.try_recv() {
                    if matches!(
                        event,
                        Event::PagedFinished(_) | Event::Parsed(_, _) | Event::FinishError(_, _)
                    ) {
                        app.receive(event, cx);
                    }
                }
                app.advance_teams(cx);
                for (token, request) in &app.native_pending {
                    if !decisions.insert(token.clone()) {
                        continue;
                    }
                    let run = app.runs.get(&request.owner).unwrap();
                    let answer = if request.owner == ids[1] {
                        "reject-once"
                    } else {
                        "decline"
                    };
                    run.handle
                        .respond(token, runtime::NativeAnswer::Choice(answer.into()))
                        .unwrap();
                }
                app.teams[0]
                    .members
                    .iter()
                    .all(|member| member.status == teams::Status::Complete)
                    && app.team_starting.is_empty()
                    && app.runs.is_empty()
                    && app.finalizing.is_empty()
                    && app.settling.is_empty()
                    && app.checkpoint_busy == 0
            });
            if done {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "Two-provider native team did not complete"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(decisions.len(), 2);
        assert!(
            project.join("codex.declined").is_file() && project.join("gemini.declined").is_file()
        );
        let first = std::fs::read_to_string(project.join("codex.prompt")).unwrap();
        let second = std::fs::read_to_string(project.join("gemini.prompt")).unwrap();
        assert!(first.contains("CODEX_ASSIGNED_TASK") && !first.contains("GEMINI_ASSIGNED_TASK"));
        assert!(second.contains("GEMINI_ASSIGNED_TASK") && !second.contains("CODEX_ASSIGNED_TASK"));
        view.update(cx, |app, _| {
            assert!(app.sessions.iter().any(|session| session.id == ids[0]
                && session.external_id.as_deref() == Some("12345678-1234-4234-8234-123456789012")));
            assert!(app.sessions.iter().any(|session| session.id == ids[1]
                && session.external_id.as_deref() == Some("gemini-team-native")));
        });
    }
}
