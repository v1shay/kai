//! The paired client owns microphone and speaker. The host owns the native
//! provider session and publishes its answer only to the initiating peer.
use super::*;
use serde_json::{Value, json};

pub(super) struct HostCall {
    client: String,
    session_id: String,
    epoch: uuid::Uuid,
    lease: Instant,
    opened_at: Instant,
    started: bool,
    answer: Option<String>,
    closed: bool,
    error: Option<String>,
}

impl Orbit {
    pub(super) fn remote_voice_command(
        &mut self,
        client: &str,
        action: &str,
        args: &Value,
        session: SessionSummary,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        let session_id = args["voice_session"]
            .as_str()
            .filter(|id| uuid::Uuid::parse_str(id).is_ok())
            .ok_or("Invalid RealTime session")?;
        let id = session.id.clone();
        if action == "voice_start" {
            if self.remote.voice.len() >= 16 && !self.remote.voice.contains_key(&id) {
                return Err("The host has reached its RealTime session limit".into());
            }
            if session.agent != AgentKind::Codex {
                return Err("This host chat’s provider does not support remote RealTime".into());
            }
            if self.chat_execution_busy(&id) || self.chat_has_queued_team_task(&id) {
                return Err("Finish this chat’s current work before starting RealTime".into());
            }
            if self.quit_pending
                || self.runs.len() + self.launches.len() + self.team_starting.len() >= 4
            {
                return Err("The host cannot start another session right now. Try again when it has a free slot.".into());
            }
            let project = self
                .prepare_voice_workspace(&session)
                .map_err(|error| error.to_string())?;
            let executable = self
                .agents
                .iter()
                .find(|agent| agent.kind == AgentKind::Codex)
                .and_then(|agent| agent.executable.clone())
                .ok_or("Codex is unavailable on the host")?;
            let (model, effort) = self.orchestration_selection(
                args,
                AgentKind::Codex,
                &project,
                self.session_models.get(&id).cloned().flatten(),
            )?;
            let effort = effort.or_else(|| self.reasoning_efforts
                .get(&format!("codex:{id}:{}", model.as_deref().unwrap_or_default())).cloned())
                .or_else(|| self.usage.get(&id).and_then(|usage| usage.reasoning_effort.clone()));
            let params = voice::Start {
                session_id: session_id.into(),
                sdp: args["sdp"]
                    .as_str()
                    .ok_or("A WebRTC offer is required")?
                    .into(),
                voice: voice::configured_voice(
                    args["voice"].as_str().unwrap_or(&self.voice_choice),
                ),
            }
            .params()
            .map_err(|error| error.to_string())?;
            let service_tier = if args["fast"] == true {
                Some(
                    self.remote_service_tier(AgentKind::Codex, &project, model.as_deref())
                        .ok_or("The selected host model does not support Fast mode")?,
                )
            } else if args["fast"].is_null() && self.fast_enabled() {
                self.remote_service_tier(AgentKind::Codex, &project, model.as_deref())
            } else {
                None
            };
            let orchestrator = args["orchestrator"]
                .as_bool()
                .unwrap_or_else(|| self.orchestrator_chats.contains(&id));
            let request = RunRequest {
                identity: None,
                ninja: None,
                orchestrator,
                service_tier,
                reasoning_effort: effort,
                agent: AgentKind::Codex,
                executable,
                project: project.clone(),
                prompt: String::new(),
                resume_id: session.external_id.clone(),
                fork_session: imported_codex_needs_fork(&session),
                model,
                images: Vec::new(),
                allow_edits: self.allow_edits && args["allow_edits"].as_bool().unwrap_or(true),
                codex_approvals: serde_json::from_value(args["codex_approvals"].clone()).unwrap_or(runtime::CodexApprovals::Ask),
                codex_access: serde_json::from_value(args["codex_access"].clone()).unwrap_or(runtime::CodexAccess::Workspace),
                provider_unrestricted: false,
                trust_workspace: self
                    .trusted_workspaces
                    .contains(&(AgentKind::Codex, project)),
            };
            let handle = runtime::launch_native_command(request, "thread/realtime/start", params)
                .map_err(|error| error.to_string())?;
            if !self.sessions.iter().any(|known| known.id == id) {
                self.engine.update_session(session.clone());
                self.sessions.insert(0, session);
                self.rebuild_workspace_sessions();
            }
            if orchestrator {
                self.orchestrator_chats.insert(id.clone());
            }
            let epoch = uuid::Uuid::new_v4();
            let wake = handle.wake.clone();
            self.voice_runs.insert((id.clone(), epoch));
            self.remote.voice.insert(
                id.clone(),
                HostCall {
                    client: client.into(),
                    session_id: session_id.into(),
                    epoch,
                    lease: Instant::now(),
                    opened_at: Instant::now(),
                    started: false,
                    answer: None,
                    closed: false,
                    error: None,
                },
            );
            self.runs.insert(
                id.clone(),
                Running {
                    timing: None,
                    agent: AgentKind::Codex,
                    submitted_prompt: String::new(),
                    handle,
                    text: String::new(),
                    live_prefix_omitted: false,
                    status: "Connecting Codex voice".into(),
                    stream_start: None,
                    revision: 0,
                    parsed_revision: 0,
                    parse_pending: false,
                    live_blocks: Vec::new(),
                    file_changes: Default::default(),
                    live_earlier: false,
                    parse_epoch: epoch,
                    retry: None,
                    submitted_attachments: Vec::new(),
                },
            );
            let owner = id.clone();
            cx.spawn(async move |this, cx| {
                while wake.recv().await.is_ok() {
                    Timer::after(Duration::from_millis(16)).await;
                    if !matches!(this.update(cx, |app, cx| app.poll(&owner, cx)), Ok(true)) {
                        return;
                    }
                }
                let _ = this.update(cx, |app, cx| app.poll(&owner, cx));
            })
            .detach();
            cx.spawn(async move |this, cx| {
                loop {
                    Timer::after(Duration::from_secs(5)).await;
                    if !matches!(
                        this.update(cx, |app, cx| {
                            let Some(call) =
                                app.remote.voice.get_mut(&id).filter(|call| call.epoch == epoch)
                            else {
                                return false;
                            };
                            let expired = call.lease.elapsed() >= Duration::from_secs(40);
                            let startup_timed_out = !call.closed
                                && (!call.started || call.answer.is_none())
                                && call.opened_at.elapsed() >= Duration::from_secs(45);
                            if expired || startup_timed_out {
                                if let Some(run) =
                                    app.runs.get(&id).filter(|run| run.parse_epoch == epoch)
                                {
                                    run.handle.cancel();
                                }
                                if startup_timed_out {
                                    call.closed = true;
                                    call.error = Some("Codex did not finish starting RealTime on the host. Try again.".into());
                                }
                                if expired {
                                    app.remote.voice.remove(&id);
                                }
                                cx.notify();
                                return !expired;
                            }
                            true
                        }),
                        Ok(true)
                    ) {
                        break;
                    }
                }
            })
            .detach();
            cx.notify();
            return Ok(json!({"accepted":true}));
        }
        let call = self
            .remote
            .voice
            .get_mut(&id)
            .filter(|call| call.client == client && call.session_id == session_id)
            .ok_or("This RealTime session is closed or belongs to another device")?;
        let run = self
            .runs
            .get(&id)
            .filter(|run| run.parse_epoch == call.epoch);
        match action {
            "voice_status" => {
                if !call.closed {
                    call.lease = Instant::now();
                }
                let preview = run.map(|run| {
                    let text = run.text.as_str();
                    &text[text.ceil_char_boundary(text.len().saturating_sub(32 * 1024))..]
                });
                Ok(
                    json!({"started":call.started,"answer":call.answer,"closed":call.closed || run.is_none(),
                    "error":call.error,"preview":preview}),
                )
            }
            "voice_interrupt" => {
                run.ok_or("This RealTime session is closed")?
                    .handle
                    .interrupt()
                    .map_err(|error| error.to_string())?;
                Ok(json!({"accepted":true}))
            }
            "voice_end" => {
                if let Some(run) = run {
                    if !call.started || call.answer.is_none() || run
                        .handle
                        .native_command("thread/realtime/stop", json!({}))
                        .is_err()
                    {
                        run.handle.cancel();
                    }
                }
                call.closed = true;
                Ok(json!({"accepted":true}))
            }
            _ => Err("Unsupported RealTime command".into()),
        }
    }

    pub(super) fn remote_voice_signal(&mut self, id: &str, event: &voice::Signal) -> bool {
        let Some(call) = self.remote.voice.get_mut(id).filter(|call| {
            self.runs
                .get(id)
                .is_some_and(|run| run.parse_epoch == call.epoch)
        }) else {
            return false;
        };
        match event {
            voice::Signal::Started { session_id } if session_id == &call.session_id => {
                call.started = true
            }
            voice::Signal::Answer { session_id, sdp } if session_id == &call.session_id => {
                call.answer = Some(sdp.clone())
            }
            voice::Signal::Closed { session_id, reason } if session_id == &call.session_id => {
                call.closed = true;
                call.error = reason
                    .as_ref()
                    .filter(|reason| {
                        !matches!(
                            reason.as_str(),
                            "userRequested" | "clientRequested" | "stopped" | "completed"
                        )
                    })
                    .cloned();
            }
            _ => {}
        }
        true
    }

    pub(super) fn finish_remote_voice_run(&mut self, id: &str, failed: Option<String>) {
        if let Some(call) = self.remote.voice.get_mut(id).filter(|call| {
            self.runs
                .get(id)
                .is_some_and(|run| run.parse_epoch == call.epoch)
        }) {
            call.closed = true;
            if failed.is_some() {
                call.error = failed;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;

    #[gpui::test]
    fn remote_voice_waits_for_a_real_writer_but_not_for_history_rendering(cx: &mut gpui::TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update(cx, |app, cx| {
            let mut chat = app.sessions[0].clone();
            chat.agent = AgentKind::Codex;
            let provider = app.agents.iter_mut().find(|a| a.kind == chat.agent).unwrap();
            provider.executable = Some("/usr/bin/false".into());
            let id = chat.id.clone();
            let args = json!({"voice_session": uuid::Uuid::new_v4(), "sdp": "v=0\r\nfixture"});
            app.finalizing.insert(id.clone());
            assert!(app.remote_voice_command("kai:fixture", "voice_start", &args, chat.clone(), cx).is_err());
            app.final_history_load = Some((id.clone(), app.generation));
            let (sender, receiver) = async_channel::bounded(1);
            app.settling.insert(id.clone(), receiver);
            assert!(app.remote_voice_command("kai:fixture", "voice_start", &args, chat.clone(), cx).is_err());
            drop(sender);
            let result = app.remote_voice_command("kai:fixture", "voice_start", &args, chat, cx).unwrap();
            assert_eq!(result["accepted"], true);
            app.runs[&id].handle.cancel();
        });
    }

    #[gpui::test]
    fn web_rtc_answer_requires_the_device_that_opened_the_host_chat(cx: &mut gpui::TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update(cx, |app, cx| {
            let chat = app.sessions[0].clone();
            let identity = uuid::Uuid::new_v4();
            app.remote.voice.insert(
                chat.id.clone(),
                HostCall {
                    client: "kai:originating-device".into(),
                    session_id: identity.to_string(),
                    epoch: uuid::Uuid::new_v4(),
                    lease: Instant::now(),
                    opened_at: Instant::now(),
                    started: true,
                    answer: Some("v=0\r\nprivate SDP".into()),
                    closed: false,
                    error: None,
                },
            );
            let args = json!({"voice_session":identity});
            assert!(
                app.remote_voice_command(
                    "kai:other-device",
                    "voice_status",
                    &args,
                    chat.clone(),
                    cx
                )
                .is_err()
            );
            assert!(
                app.remote_voice_command(
                    "kai:originating-device",
                    "voice_status",
                    &json!({"voice_session":uuid::Uuid::new_v4()}),
                    chat.clone(),
                    cx
                )
                .is_err()
            );
            let response = app
                .remote_voice_command("kai:originating-device", "voice_status", &args, chat, cx)
                .unwrap();
            assert_eq!(response["answer"], "v=0\r\nprivate SDP");
        });
    }
}
