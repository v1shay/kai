//! Store only an attachment identity, never CLI credentials or a replay command.
use super::*;
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(super) struct Saved {
    owner: String,
    id: uuid::Uuid,
    root: PathBuf,
    agent: AgentKind,
    shell: bool,
    selected: bool,
    resumes: bool,
    #[serde(default)] cursor:Option<(String,PathBuf)>,
}
impl Saved {
    pub(super) fn owner(&self) -> &str { &self.owner }
    pub(super) fn valid(&self) -> bool {
        !self.owner.is_empty() && self.owner.len() <= 512 && self.root.is_absolute()
    }
}
impl Orbit {
    pub(super) fn image_draft_snapshot(
        &self,
    ) -> std::collections::BTreeMap<String, Vec<(PathBuf, String)>> {
        let mut drafts = self.attachment_drafts.clone();
        let key = self
            .active
            .clone()
            .or_else(|| {
                self.cli_draft_project
                    .as_ref()
                    .map(|_| keyboard::CLI_DRAFT.to_owned())
            })
            .unwrap_or_else(|| "__new_conversation__".into());
        drafts.insert(key, self.attachments.clone());
        drafts
            .into_iter()
            .filter(|(_, v)| !v.is_empty())
            .take(64)
            .map(|(key, files)| {
                (
                    key,
                    files
                        .iter()
                        .take(attachments::MAX_ATTACHMENTS)
                        .map(|file| {
                            file.mark_submitted();
                            (file.path.clone(), file.name.clone())
                        })
                        .collect(),
                )
            })
            .collect()
    }
    pub(super) fn restore_image_drafts(
        &mut self,
        drafts: std::collections::BTreeMap<String, Vec<(PathBuf, String)>>,
        cx: &mut Context<Self>,
    ) {
        let root = self.attachment_dir.clone();
        cx.spawn(async move |this, cx| {
            let drafts = cx
                .background_executor()
                .spawn(async move {
                    drafts
                        .into_iter()
                        .take(64)
                        .map(|(key, files)| {
                            let files = files
                                .into_iter()
                                .take(attachments::MAX_ATTACHMENTS)
                                .filter_map(|(path, name)| {
                                    attachments::load_saved(
                                        &path,
                                        &name,
                                        &root,
                                        &std::sync::atomic::AtomicBool::new(false),
                                    )
                                    .ok()
                                })
                                .collect::<Vec<_>>();
                            (key, files)
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                let current = app
                    .active
                    .clone()
                    .or_else(|| {
                        app.cli_draft_project
                            .as_ref()
                            .map(|_| keyboard::CLI_DRAFT.to_owned())
                    })
                    .unwrap_or_else(|| "__new_conversation__".into());
                for (key, files) in drafts {
                    if !app.attachment_drafts.contains_key(&key) {
                        if key == current && app.attachments.is_empty() {
                            app.attachments = files.clone();
                        }
                        app.attachment_drafts.insert(key, files);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
    pub(super) fn cli_snapshot(&self) -> Vec<Saved> {
        self.chat_clis
            .iter()
            .filter(|(_, c)| c.persistent && !c.preparing)
            .filter_map(|(owner, c)| {
                c.launch.as_ref().map(|(root, _, _)| Saved {
                    owner: owner.clone(),
                    id: c.launch_id,
                    root: root.clone(),
                    agent: c.agent,
                    shell: c.shell,
                    selected: c.selected,
                    resumes: c.resumes_chat,cursor:c.cursor_history.as_ref().and_then(|h|h.saved()),
                })
            })
            .collect()
    }
    pub(super) fn restore_cli_sessions(&mut self, sessions: Vec<Saved>, cx: &mut Context<Self>) {
        if cfg!(test) {
            return;
        }
        let profile = self
            .attachment_dir
            .parent()
            .unwrap_or(&self.attachment_dir)
            .to_owned();
        let home=self.controls_home.clone();
        let known = self.sessions.clone();
        cx.spawn(async move|this,cx|{
            let saved=cx.background_executor().spawn(async move{
                let Some(server)=terminal::SessionServer::find(&profile) else{return Vec::new()};
                let mut sessions = sessions;
                for (resume, native) in server.codex_cli_sessions().unwrap_or_default() {
                    if sessions.iter().any(|s| s.id == resume.id) { continue; }
                    let Some(chat) = known.iter().find(|s| s.agent == AgentKind::Codex && s.external_id.as_deref() == Some(native.as_str())) else { continue; };
                    sessions.push(Saved { owner:chat.id.clone(), id:resume.id, root:resume.project, agent:AgentKind::Codex, shell:false, selected:false, resumes:true, cursor:None });
                }
                sessions.into_iter().filter(|s|s.valid()).map(|s|{let resume=terminal::Resume{id:s.id,project:s.root.clone(),backend:terminal::Backend::Metal};let command=server.metal_attach(&resume,true,None);let history=s.cursor.as_ref().filter(|(_,p)|p.starts_with(profile.join("cursor-sync"))).and_then(|(id,p)|cursor_cli_history::Sync::restore(&home,&s.root,id.clone(),p.clone()).ok());(s,command,history)}).collect::<Vec<_>>()
            }).await;
            let _=this.update(cx,|app,cx|{
                for(saved,command,history)in saved{
                    let command=match command{Ok(c)=>c,Err(e)=>{app.notice=Some(format!("Saved CLI ended; no command was replayed: {e}"));continue}};
                    if app.chat_clis.contains_key(&saved.owner){continue}
                    let arguments=vec!["-lc".into(),command];
                    unified::emit(&serde_json::json!({"type":"chatTerminalOpen","id":saved.owner,"launchId":saved.id,"arguments":arguments,"environment":[],"directory":saved.root,"title":saved.agent.name(),"agent":if saved.shell{"Shell"}else{saved.agent.name()}}));
                    unified::emit(&serde_json::json!({"type":"chatTerminalToggle","id":saved.owner,"launchId":saved.id,"selected":saved.selected}));
                    app.chat_clis.insert(saved.owner,keyboard::ChatCli{shell:saved.shell,persistent:true,launch:Some((saved.root,arguments,Vec::new())),agent:saved.agent,retained:None,preparing:false,launch_id:saved.id,fallback:None,selected:saved.selected,metal_visible:false,resumes_chat:saved.resumes,cursor_history:history,send_when_closed:None});
                }
                app.sync_chat_terminal_owner(cx);cx.notify();
            });
        }).detach();
    }
}
