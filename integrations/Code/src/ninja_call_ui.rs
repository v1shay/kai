//! A personal Ninja call keeps media on this Mac and the native Codex runtime
//! on the execution host. Signaling never goes into the persisted UI snapshot.
use super::*;
use crate::{
    ninjas_ui::{Host, call},
    voice_bridge::{Activity, Media, MediaEvent},
};

pub(super) struct Call {
    pub host: Host,
    pub profile: String,
    pub name: String,
    pub session: String,
    pub task: Option<String>,
    pub media: Media,
    pub started: Instant,
    pub connected: bool,
    pub backend: bool,
    pub muted: bool,
    pub activity: Activity,
}
impl Call {
    pub fn scene(&self) -> &'static str {
        if !self.connected || !self.backend {
            "thinking"
        } else if self.muted {
            "waiting"
        } else {
            match self.activity {
                Activity::Listening => "listening",
                Activity::Thinking => "thinking",
                Activity::Speaking => "listening",
            }
        }
    }
}
impl Drop for Call {
    fn drop(&mut self) {
        self.media.stop();
        if let Some(task) = self.task.take() {
            let host = self.host.clone();
            let session = self.session.clone();
            std::thread::spawn(move || {
                let _ = call(
                    &host,
                    &serde_json::json!({"action":"call_end","id":task,"session":session}),
                );
            });
        }
    }
}
impl Orbit {
    pub(super) fn ninja_call_toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.ninjas.call.take().is_some() {
            cx.notify();
            return;
        }
        if self.voice_call.is_some() {
            self.notice = Some("End the current call before calling a Ninja".into());
            cx.notify();
            return;
        }
        let Some(p) = self
            .ninjas
            .panel
            .as_ref()
            .filter(|p| p.profile.revision > 0)
        else {
            return;
        };
        let Some(host) = self.ninjas.hosts.iter().find(|h| h.id == p.host).cloned() else {
            return;
        };
        self.voice_error = None;
        let session = uuid::Uuid::new_v4().to_string();
        let media = match Media::start(window, &session) {
            Ok(media) => media,
            Err(e) => {
                self.notice = Some(e.to_string());
                cx.notify();
                return;
            }
        };
        let wake = media.wake.clone();
        self.ninjas.call = Some(Call {
            host,
            profile: p.profile.id.clone(),
            name: p.profile.name.clone(),
            session: session.clone(),
            task: None,
            media,
            started: Instant::now(),
            connected: false,
            backend: false,
            muted: false,
            activity: Activity::Thinking,
        });
        cx.spawn(async move |this, cx| {
            while wake.recv().await.is_ok() {
                if !this
                    .update(cx, |app, cx| app.ninja_drain_media(&session, cx))
                    .unwrap_or(false)
                {
                    break;
                }
            }
        })
        .detach();
        let session = self.ninjas.call.as_ref().unwrap().session.clone();
        cx.spawn(async move|this,cx|{Timer::after(Duration::from_secs(60)).await;let _=this.update(cx,|app,cx|{
            if app.ninjas.call.as_ref().is_some_and(|c|c.session==session&&!(c.connected&&c.backend)) {
                app.ninjas.call.take();app.notice=Some("The host did not connect the call. Check its Codex sign-in and microphone access on this Mac.".into());cx.notify();
            }
        });}).detach();
        cx.notify();
    }
    fn ninja_drain_media(&mut self, session: &str, cx: &mut Context<Self>) -> bool {
        if !self
            .ninjas
            .call
            .as_ref()
            .is_some_and(|c| c.session == session)
        {
            return false;
        }
        loop {
            let event = self
                .ninjas
                .call
                .as_ref()
                .and_then(|c| c.media.try_recv().ok());
            let Some(event) = event else { break };
            match event {
                MediaEvent::Offer { session_id, sdp } if session_id == session => {
                    let c = self.ninjas.call.as_ref().unwrap();
                    let host = c.host.clone();
                    let profile = c.profile.clone();
                    let owner = c.session.clone();
                    let chat = self
                        .ninjas
                        .panel
                        .as_ref()
                        .filter(|p| p.host == host.id && p.profile.id == profile)
                        .and_then(|p| p.chat.clone());
                    let request = serde_json::json!({"action":"call_start","id":profile,"chat":chat,"session":owner,"sdp":sdp,"voice":voice::configured_voice(&self.voice_choice)});
                    cx.spawn(async move|this,cx|{
                        let h=host.clone();let result=cx.background_executor().spawn(async move{call(&h,&request)}).await;
                        let mut orphan=result.as_ref().ok().and_then(|v|v["id"].as_str()).map(str::to_owned);
                        let next=this.update(cx,|app,cx|{
                            let Some(c)=app.ninjas.call.as_mut().filter(|c|c.session==owner)else{orphan=result.as_ref().ok().and_then(|v|v["id"].as_str()).map(str::to_owned);return None};
                            match result {
                                Ok(t)=>{let task=t["id"].as_str()?.to_owned();c.task=Some(task.clone());orphan=None;
                                    if let Some(p)=app.ninjas.panel.as_mut().filter(|p|p.host==host.id&&p.profile.id==c.profile){p.chat=t["chat"].as_str().map(str::to_owned);p.task=Some(t);}
                                    cx.notify();Some(task)
                                },
                                Err(e)=>{app.ninjas.call.take();app.voice_error=Some(e.to_string());app.notice=Some(e.to_string());cx.notify();None}
                            }
                        }).ok().flatten();
                        if let Some(id)=orphan {let remote=host.clone();let sid=owner.clone();cx.background_executor().spawn(async move{let _=call(&remote,&serde_json::json!({"action":"call_end","id":id,"session":sid}));}).await;}
                        if let Some(task)=next {
                            let _=this.update(cx,|app,cx|app.ninja_poll_call(host,task,owner,cx));
                        }
                    }).detach();
                }
                MediaEvent::Connected { session_id } if session_id == session => {
                    let c = self.ninjas.call.as_mut().unwrap();
                    c.connected = true;
                    if c.backend {
                        if let Err(e) = c.media.activate() {
                            self.ninjas.call.take();
                            self.notice = Some(e.to_string());
                            return false;
                        }
                    }
                }
                MediaEvent::Activity {
                    session_id,
                    activity,
                } if session_id == session => {
                    self.ninjas.call.as_mut().unwrap().activity = activity;
                }
                MediaEvent::Error {
                    session_id,
                    message,
                } if session_id == session => {
                    self.ninjas.call.take();
                    self.notice = Some(message);
                    cx.notify();
                    return false;
                }
                _ => {}
            }
        }
        cx.notify();
        true
    }
    fn ninja_poll_call(
        &mut self,
        host: Host,
        task: String,
        session: String,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            let mut cursor = 0;
            loop {
                let h = host.clone();
                let id = task.clone();
                let result = cx
                    .background_executor()
                    .spawn(async move {
                        call(
                            &h,
                            &serde_json::json!({"action":"call_poll","id":id,"cursor":cursor}),
                        )
                    })
                    .await;
                let keep = this
                    .update(cx, |app, cx| {
                        let Some(c) = app.ninjas.call.as_mut().filter(|c| c.session == session)
                        else {
                            return false;
                        };
                        let result = (|| -> anyhow::Result<bool> {
                            let value = result?;
                            cursor = value["cursor"].as_u64().unwrap_or(cursor);
                            for v in value["signals"].as_array().into_iter().flatten() {
                                let signal: voice::Signal = serde_json::from_value(v.clone())?;
                                match signal {
                                    voice::Signal::Started { session_id }
                                        if session_id == session =>
                                    {
                                        c.backend = true;
                                        if c.connected {
                                            c.media.activate()?;
                                        }
                                    }
                                    voice::Signal::Answer { session_id, sdp }
                                        if session_id == session =>
                                    {
                                        c.media.answer(&sdp)?
                                    }
                                    voice::Signal::Closed { session_id, reason }
                                        if session_id == session =>
                                    {
                                        if let Some(reason) = reason.filter(|r| {
                                            ![
                                                "userRequested",
                                                "clientRequested",
                                                "stopped",
                                                "completed",
                                                "Completed",
                                            ]
                                            .contains(&r.as_str())
                                        }) {
                                            anyhow::bail!("{reason}");
                                        }
                                        return Ok(false);
                                    }
                                    _ => {}
                                }
                            }
                            Ok(true)
                        })();
                        match result {
                            Ok(true) => {
                                cx.notify();
                                true
                            }
                            other => {
                                if let Err(e) = other {
                                    app.voice_error = Some(e.to_string());
                                    app.notice = Some(e.to_string());
                                }
                                app.ninjas.call.take();
                                cx.notify();
                                false
                            }
                        }
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
                // Host long-poll sleeps for revisions; this guard prevents a
                // fast status stream from monopolizing a core.
                Timer::after(Duration::from_millis(100)).await;
            }
        })
        .detach();
    }
}
