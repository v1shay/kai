//! Orderly-quit recovery. Persist intent before cancellation, then claim it on
//! disk before dispatch. An ambiguous/crashed dispatch is held, never replayed.
use super::*;
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct Saved {
    pub session: SessionSummary,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub service_tier: Option<String>,
    pub allow_edits: bool,
    pub access: runtime::CodexAccess,
    pub approvals: runtime::CodexApprovals,
    pub ninja: Option<ninja::Selection>,
    pub orchestrator: bool,
    pub claimed: bool,
    #[serde(default)] pub submitted_prompt:String,
    #[serde(default)] pub durable:Option<PathBuf>,
    #[serde(default)] pub request:Option<RunRequest>,
    #[serde(default)] pub checkpoint:Option<String>,
}
impl std::fmt::Debug for Saved {fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{f.debug_struct("Saved").field("session",&self.session.id).field("durable",&self.durable).finish()}}
impl Saved {
    pub(super) fn valid(&self) -> bool {
        !self.session.id.is_empty()
            && self.session.id.len() <= 512
            && self.session.project_path.is_absolute()
            && self
                .session
                .execution_path
                .as_ref()
                .is_none_or(|p| p.is_absolute())
            && [&self.model, &self.effort, &self.service_tier]
                .into_iter()
                .all(|s| s.as_ref().is_none_or(|s| s.len() <= 256))
    }
}
const PROMPT: &str = "Kai was closed while this task was running. Continue the unfinished task in this same conversation from its saved progress. First inspect the current files, recent tool results, and conversation to determine what already finished. Do not repeat completed work or external actions. Preserve the original task scope and report the final result. If the task is already complete, report that without making additional changes.";
impl Orbit {
    fn attach_saved_owner(&mut self,saved:Saved,cx:&mut Context<Self>){
        let Some(dir)=saved.durable.clone() else{return};let id=saved.session.id.clone();
        self.continuation_claiming=true;
        cx.spawn(async move|this,cx|{
            let result=cx.background_executor().spawn(async move{runtime::durable::attach(dir)}).await;
            let mut wake=None;
            let _=this.update(cx,|app,cx|{
                app.continuation_claiming=false;
                app.continuations.retain(|entry| entry.session.id != id);
                match result{
                    Ok(handle)=>{
                        wake=Some(handle.wake.clone());
                        if let Some(member) = app.teams.iter_mut().flat_map(|team| &mut team.members)
                            .find(|member| member.session == id && member.attempted && member.status == teams::Status::Held)
                        {
                            member.status = teams::Status::Working;
                        }
                        if let Some(checkpoint)=saved.checkpoint{app.checkpoint_runs.insert(id.clone(),checkpoint);}
                        app.runs.insert(id.clone(),Running{timing:None,agent:saved.session.agent,submitted_prompt:saved.submitted_prompt.clone(),handle,text:String::new(),live_prefix_omitted:false,status:"Reconnecting".into(),stream_start:None,revision:0,parsed_revision:0,parse_pending:false,live_blocks:Vec::new(), file_changes: Default::default(),live_earlier:false,parse_epoch:uuid::Uuid::new_v4(),retry:saved.request,submitted_attachments:Vec::new()});
                    }
                    Err(error) => {
                        for team in &mut app.teams {
                            for member in &mut team.members {
                                if member.session == id && member.attempted && member.status == teams::Status::Queued {
                                    member.status = teams::Status::Held;
                                }
                            }
                        }
                        app.notice = Some(format!("Saved task held without replay: {error}"));
                    }

                }
                app.save_workspace_now(cx);
                app.advance_teams(cx);
                cx.notify();
            });
            if let Some(wake)=wake{loop{if wake.recv().await.is_ok(){Timer::after(Duration::from_millis(16)).await;}match this.update(cx,|app,cx|app.poll(&id,cx)){Ok(true)=>{},_=>break}}}
        }).detach();
    }
    pub(super) fn capture_quit_continuations(&mut self) {
        self.continuation_restoring = false;
        for (id, run) in &self.runs {
            if run.handle.is_cancelled() || self.voice_runs.contains(&(id.clone(), run.parse_epoch))
            {
                continue;
            }
            let Some(request) = &run.retry else {
                continue;
            };
            let Some(session) = self.sessions.iter().find(|s| &s.id == id) else {
                continue;
            };
            if self.continuations.iter().any(|s| s.session.id == *id) {
                continue;
            }
            let mut session = session.clone();
            session.execution_path = Some(request.project.clone());
            self.continuations.push(Saved {
                session,
                model: request.model.clone(),
                effort: request.reasoning_effort.clone(),
                service_tier: request.service_tier.clone(),
                allow_edits: request.allow_edits,
                access: request.codex_access,
                approvals: request.codex_approvals,
                ninja: request.ninja.clone(),
                orchestrator: request.orchestrator,
                claimed: false,submitted_prompt:run.submitted_prompt.clone(),
                durable:run.handle.durable.clone(),
                request:run.handle.durable.as_ref().map(|_|request.clone()),
                checkpoint:self.checkpoint_runs.get(id).cloned(),
            });
        }
    }
    pub(super) fn continuation_snapshot(&self) -> Vec<Saved> {
        let mut out:Vec<_>=self.continuations.iter().filter(|s|!s.claimed).take(32).cloned().collect();
        for(id,run)in &self.runs{
            let Some(dir)=run.handle.durable.clone() else{continue};
            let Some(request)=run.retry.clone() else{continue};
            let Some(session)=self.sessions.iter().find(|s|s.id==*id).cloned() else{continue};
            out.retain(|s|s.session.id!=*id);
            out.push(Saved{session,model:request.model.clone(),effort:request.reasoning_effort.clone(),service_tier:request.service_tier.clone(),allow_edits:request.allow_edits,access:request.codex_access,approvals:request.codex_approvals,ninja:request.ninja.clone(),orchestrator:request.orchestrator,claimed:false,submitted_prompt:run.submitted_prompt.clone(),durable:Some(dir),request:Some(request),checkpoint:self.checkpoint_runs.get(id).cloned()});
        }
        out
    }

    pub(super) fn advance_continuations(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.continuation_restoring || self.workspace_store.is_none()
            || self.quit_pending
            || self.restore_pending.is_some()
            || !self.preferences_ready
            || self.loading
            || self.continuation_claiming
            || self.starting
            || self.checkpoint_busy > 0
            || self.handoff_pending.is_some()
        {
            return;
        }
        let Some(index) = self.continuations.iter().position(|s| {
            !s.claimed
                && !self.runs.contains_key(&s.session.id)
                && !self.finalizing.contains(&s.session.id)
        }) else {
            return;
        };
        let id = self.continuations[index].session.id.clone();
        let Some(current) = self.sessions.iter().find(|s| s.id == id).cloned() else {
            if self.discovering {
                return;
            }
            self.continuations.remove(index);
            self.notice = Some(
                "A saved task could not be found. Its continuation is held for review.".into(),
            );
            self.queue_workspace_save(cx);
            return;
        };
        if self.continuations[index].durable.is_some(){
            let saved=self.continuations[index].clone();
            self.attach_saved_owner(saved,cx);return;
        }
        // A native thread identity is required: a follow-up in a new session
        // would lose the task's original context.
        if current.external_id.is_none()
            || self.hidden_sessions.contains(&id)
            || self.chat_clis.contains_key(&id)
        {
            self.continuations[index].claimed = true;
            self.notice = Some(
                "A saved task needs review before continuing. Open its chat to resume.".into(),
            );
            self.queue_workspace_save(cx);
            return;
        }
        self.continuations[index].session.external_id = current.external_id;
        self.continuations[index].claimed = true;
        let saved = self.continuations[index].clone();
        let Some(store) = &self.workspace_store else {
            return;
        };
        store.save(self.workspace_snapshot(cx));
        let ack = store.flush();
        self.continuation_claiming = true;
        let window_handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    ack.recv_timeout(Duration::from_secs(10))
                        .map_err(|e| e.to_string())
                        .and_then(|saved| saved.map_err(|e| e.to_string()))
                })
                .await;
            let _ = window_handle.update(cx, |_, window, cx| {
                let _ = this.update(cx, |app, cx| {
                    app.continuation_claiming = false;
                    if let Err(error) = result {
                        app.notice = Some(format!(
                            "Could not save continuation: {error}. The task was not submitted."
                        ));
                        cx.notify();
                        return;
                    }
                    if app.quit_pending {
                        return;
                    }
                    app.launch_continuation(saved, window, cx);
                });
            });
        })
        .detach();
    }
    fn launch_continuation(&mut self, saved: Saved, window: &mut Window, cx: &mut Context<Self>) {
        let agent = saved.session.agent;
        let executable = self
            .agents
            .iter()
            .find(|a| a.kind == agent && a.available)
            .and_then(|a| a.executable.clone());
        let Some(executable) = executable else {
            self.notice = Some(format!(
                "Install or sign in to {} to resume its saved task.",
                agent_name(agent)
            ));
            cx.notify();
            return;
        };
        let root = saved
            .session
            .execution_path
            .clone()
            .unwrap_or_else(|| saved.session.project_path.clone());
        if !root.is_dir() {
            self.notice = Some(
                "A saved task’s checkout is unavailable. Reopen its project to continue.".into(),
            );
            cx.notify();
            return;
        }
        // Existing drafts/selection are untouched, including the active blank tab.
        self.remember_draft(cx);
        let active = self.active.clone();
        let project = self.project.clone();
        let agent_before = self.agent;
        let model = self.model.clone();
        let blank = self.composer.read(cx).text();
        let blank_attachments = self.attachments.clone();
        let blank_path = self.new_execution_path.clone();
        let request = RunRequest {
            identity: None,
            ninja: saved.ninja,
            orchestrator: saved.orchestrator,
            agent,
            executable,
            project: root,
            prompt: PROMPT.into(),
            resume_id: saved.session.external_id.clone(),
            fork_session: false,
            model: saved.model.clone(),
            service_tier: saved.service_tier,
            reasoning_effort: saved.effort,
            images: vec![],
            allow_edits: saved.allow_edits,
            trust_workspace: false,
            codex_approvals: saved.approvals,
            codex_access: saved.access,
            provider_unrestricted: false,
        };
        let scope = saved.session.id.clone();
        self.open_session(saved.session.clone(), window, cx);
        self.submit_captured(
            handoff_ui::Submission {
                session: saved.session,
                request,
                selected_model: saved.model,
                prompt: PROMPT.into(),
                original_prompt: PROMPT.into(),
                saved_prompt: PROMPT.into(),
                attachments: vec![],
                references: vec![],
                context_scope: scope,
                native_command: false,
                queued_prompt_id: None,
                parallel: false,
            },
            window,
            cx,
        );
        if let Some(session) =
            active.and_then(|id| self.sessions.iter().find(|s| s.id == id).cloned())
        {
            self.open_session(session, window, cx);
        } else {
            self.active = None;
            self.project = project;
            self.agent = agent_before;
            self.model = model;
            self.new_execution_path = blank_path;
            self.attachments = blank_attachments;
            self.composer
                .update(cx, |input, cx| input.set_text(&blank, cx));
            self.sync_chat_terminal_owner(cx);
        }
        self.notice = Some("Continuing saved work…".into());
        cx.notify();
    }
}
