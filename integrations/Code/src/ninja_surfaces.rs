//! The existing notch/mini bridge presents the selected personal Ninja owner.
//! There is one draft, one conversation and one submit path across surfaces.
use super::*;
use serde_json::{Value, json};

impl Orbit {
    pub(super) fn remember_ninja_draft(&mut self, cx: &App) {
        if let Some(p) = &self.ninjas.panel {
            let key = format!(
                "{}:{}:{}",
                p.host,
                p.profile.id,
                p.chat.as_deref().unwrap_or("draft")
            );
            let text = p.fields[3].read(cx).text();
            if text.is_empty() {
                self.ninjas.drafts.remove(&key);
            } else {
                if self.ninjas.drafts.len() >= 24 && !self.ninjas.drafts.contains_key(&key) {
                    if let Some(old) = self.ninjas.drafts.keys().next().cloned() {
                        self.ninjas.drafts.remove(&old);
                    }
                }
                self.ninjas.drafts.insert(key, text);
            }
        }
        self.ninjas.save();
    }
    pub(super) fn ninja_surface_owner(&self) -> Option<String> {
        let p = self
            .ninjas
            .panel
            .as_ref()
            .filter(|p| p.profile.revision > 0)?;
        Some(format!(
            "personal:{}:{}:{}",
            p.host,
            p.profile.id,
            p.chat.as_deref().unwrap_or("draft")
        ))
    }
    pub(super) fn ninja_surface_command(
        &mut self,
        v: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<bool, String> {
        let action = v["action"].as_str().unwrap_or("");
        if action == "menu_bar_open" {
            match v["kind"].as_str() {
                Some("ninja") => {
                    let (host, id) = v["id"]
                        .as_str()
                        .and_then(|s| s.rsplit_once(':'))
                        .ok_or("Missing Ninja identity")?;
                    if !self
                        .ninjas
                        .panel
                        .as_ref()
                        .is_some_and(|p| p.host == host && p.profile.id == id)
                    {
                        self.open_ninjas(Some(host.into()), Some(id.into()), window, cx);
                    }
                }
                Some("provider") => {
                    let agent = self
                        .agents
                        .iter()
                        .map(|a| a.kind)
                        .find(|a| a.command() == v["id"].as_str().unwrap_or(""))
                        .ok_or("Unknown provider")?;
                    self.remember_ninja_draft(cx);
                    self.ninjas.desktop = None;
                    self.ninjas.panel = None;
                    if let Some(s) = self
                        .sessions
                        .iter()
                        .filter(|s| s.agent == agent && !self.hidden_sessions.contains(&s.id))
                        .max_by_key(|s| s.updated_at)
                        .cloned()
                    {
                        self.open_session(s, window, cx);
                    } else {
                        self.new_unprojected_conversation(window, cx);
                        self.agent = agent;
                        self.refresh_controls(false, cx);
                    }
                }
                _ => {}
            }
            if cx.has_global::<quick_chat::Controller>() {
                let owner=cx.weak_entity();
                // QuickChat::new/show reads Orbit. Return this entity's lease
                // before opening the second surface over the same entity.
                cx.defer(move |cx| {
                    let result=cx.update_global::<quick_chat::Controller,_>(|controller,cx|controller.toggle_menu(cx));
                    if let Err(error)=result { let _=owner.update(cx,|app,cx|{app.notice=Some(error.to_string());cx.notify();}); }
                });
            }
            return Ok(true);
        }
        if action == "chat" {
            if let Some(raw) = v["id"].as_str().and_then(|s| s.strip_prefix("ninja-chat:")) {
                let mut parts = raw.rsplitn(3, ':');
                let chat = parts.next().ok_or("Missing chat")?;
                let id = parts.next().ok_or("Missing Ninja")?;
                let host = parts.next().ok_or("Missing host")?;
                if !self
                    .ninjas
                    .panel
                    .as_ref()
                    .is_some_and(|p| p.host == host && p.profile.id == id)
                {
                    self.open_ninjas(Some(host.into()), Some(id.into()), window, cx);
                }
                self.open_ninja_chat(chat.into(),cx);
                return Ok(true);
            }
        }
        if action == "chat"
            && v["id"].as_str().is_some_and(|s| s.starts_with("code:"))
            && self.ninja_surface_owner().is_some()
        {
            if v["ownerToken"].as_str() != self.ninja_surface_owner().as_deref() {
                return Err("Selected conversation changed".into());
            }
            let id = v["id"].as_str().unwrap().trim_start_matches("code:");
            let target = self
                .sessions
                .iter()
                .find(|s| s.id == id && !self.hidden_sessions.contains(id))
                .cloned()
                .ok_or("Chat is unavailable")?;
            self.remember_ninja_draft(cx);
            self.ninjas.desktop = None;
            self.ninjas.panel = None;
            self.open_session(target, window, cx);
            return Ok(true);
        }
        let Some(owner) = self.ninja_surface_owner() else {
            return Ok(false);
        };
        if !matches!(
            action,
            "send"
                | "draft"
                | "new"
                | "new_project"
                | "agent"
                | "settings"
                | "workspace_expand"
                | "workspace_permissions"
                | "orchestrator"
                | "attachments"
                | "interrupt"
                | "history_more"
                | "history_latest"
        ) {
            return Ok(false);
        }
        if v["ownerToken"].as_str() != Some(&owner) {
            return Err("The selected Ninja conversation changed; your draft is preserved".into());
        }
        match action {
            "send" | "draft" => {
                let text = v["text"]
                    .as_str()
                    .filter(|s| s.len() <= input::MAX_DRAFT_BYTES && !s.contains('\0'))
                    .ok_or("Invalid Ninja message")?;
                let p = self.ninjas.panel.as_ref().unwrap();
                if v["draftBase"].as_str() != Some(p.fields[3].read(cx).text().as_str()) {
                    return Err("The Ninja draft changed on another surface".into());
                }
                p.fields[3].update(cx, |i, cx| i.set_text(text, cx));
                self.remember_ninja_draft(cx);
                if action == "send" {
                    self.ninja_submit_task(cx);
                }
            }
            "new" | "new_project" => {
                self.remember_ninja_draft(cx);
                let p = self.ninjas.panel.as_mut().unwrap();
                p.chat = None;
                p.task = None;
                p.messages.clear();
                p.fields[3].update(cx, |i, cx| i.set_text("", cx));
            }
            "attachments"=>{let paths=v["paths"].as_array().ok_or("Missing reference files")?.iter().filter_map(Value::as_str).map(PathBuf::from).collect::<Vec<_>>();if !paths.is_empty(){self.ninja_reference_paths(paths,cx);}},
            "orchestrator"=>{let p=self.ninjas.panel.as_mut().unwrap();p.profile.orchestrator=!p.profile.orchestrator;},
            "agent" => {
                let p = self.ninjas.panel.as_ref().unwrap();
                let requested = v["agent"].as_str().ok_or("Missing provider")?;
                let agent = self
                    .ninjas
                    .snapshots
                    .get(&p.host)
                    .into_iter()
                    .flat_map(|s| s["providers"].as_array().into_iter().flatten())
                    .filter(|p| p["installed"] == true)
                    .filter_map(|p| serde_json::from_value::<AgentKind>(p["agent"].clone()).ok())
                    .find(|a| a.command() == requested)
                    .ok_or("That provider is not installed on this host")?;
                let ninja = (agent == AgentKind::Ninja).then(|| self.ninja_selection());
                let p = self.ninjas.panel.as_mut().unwrap();
                p.profile.agent = agent;
                p.profile.model = None;
                p.profile.effort = None;
                p.profile.ninja = ninja;
                p.models.clear();
                p.models_requested = false;
                let profile = p.profile.clone();
                self.ninja_operation(json!({"action":"save_profile","profile":profile}), cx);
            }
            "settings" => {
                let model = v["model"].as_str().ok_or("Missing model")?;
                let p = self.ninjas.panel.as_mut().unwrap();
                let choice = p
                    .models
                    .iter()
                    .find(|m| m["id"] == model)
                    .ok_or("This host does not advertise that model")?;
                let effort = v["effort"].as_str().filter(|s| !s.is_empty());
                if effort.is_some_and(|e| {
                    !choice["efforts"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .any(|v| v == e)
                }) {
                    return Err("Unsupported thinking level".into());
                }
                p.profile.model = Some(model.into());
                p.profile.effort = effort.map(str::to_owned);
                let profile = p.profile.clone();
                self.ninja_operation(json!({"action":"save_profile","profile":profile}), cx);
            }
            "workspace_permissions" => {
                let p = self.ninjas.panel.as_mut().unwrap();
                if let Some(access) = v["access"].as_str() {
                    if !["read-only", "workspace", "full"].contains(&access) {
                        return Err("Unknown workspace access".into());
                    }
                    p.profile.edits = access != "read-only";
                    p.profile.access = if access == "full" {
                        runtime::CodexAccess::Full
                    } else {
                        runtime::CodexAccess::Workspace
                    };
                }
                if let Some(policy) = v["approvals"].as_str() {
                    p.profile.approvals = match policy {
                        "ask" => runtime::CodexApprovals::Ask,
                        "auto-review" => runtime::CodexApprovals::AutoReview,
                        "never" => runtime::CodexApprovals::Never,
                        _ => return Err("Unknown approval policy".into()),
                    };
                }
                let profile = p.profile.clone();
                self.ninja_operation(json!({"action":"save_profile","profile":profile}), cx);
            }
            "interrupt" => {
                if self.ninjas.call.take().is_none() {
                    let p = self.ninjas.panel.as_ref().unwrap();
                    let task = self
                        .ninjas
                        .snapshots
                        .get(&p.host)
                        .and_then(|v| v["tasks"].as_array())
                        .and_then(|tasks| {
                            tasks.iter().find(|t| {
                                t["profile"] == p.profile.id
                                    && t["chat"].as_str() == p.chat.as_deref()
                                    && ["running", "approval", "queued"]
                                        .contains(&t["state"].as_str().unwrap_or(""))
                            })
                        })
                        .and_then(|t| t["id"].as_str())
                        .map(str::to_owned);
                    if let Some(id) = task {
                        self.ninja_operation(json!({"action":"stop","id":id}), cx);
                    }
                }
            }
            "workspace_expand" => crate::unified::reveal(),
            "history_more" | "history_latest" => self.ninja_conversation(cx),
            _ => {}
        }
        cx.notify();
        Ok(true)
    }
    pub(super) fn ninja_surface_snapshot(&self, s: &mut Value, cx: &App) {
        // Personal chats also appear in the normal native conversation picker.
        let mut chats = Vec::new();
        for h in &self.ninjas.hosts {
            if let Some(v) = self.ninjas.snapshots.get(&h.id) {
                for p in v["profiles"].as_array().into_iter().flatten() {
                    let id = p["id"].as_str().unwrap_or("");
                    let mut seen = HashSet::new();
                    for t in v["tasks"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|t| {
                            t["profile"] == id && seen.insert(t["chat"].clone().to_string())
                        })
                        .take(12)
                    {
                        chats.push(json!({"id":format!("ninja-chat:{}:{}:{}",h.id,id,t["chat"].as_str().unwrap_or("")),"name":format!("{} · {}",p["name"].as_str().unwrap_or("Ninja"),self.ninja_chat_label(&h.id,id,t)),"status":t["state"],"agent":"ninja"}));
                    }
                }
            }
        }
        if let Some(list) = s["standaloneChats"].as_array_mut() {
            list.extend(chats.clone());
        }
        let Some(p) = self
            .ninjas
            .panel
            .as_ref()
            .filter(|p| p.profile.revision > 0)
        else {
            return;
        };
        let snapshot = self.ninjas.snapshots.get(&p.host);
        let mut tasks = p.messages.clone();
        for t in snapshot
            .into_iter()
            .flat_map(|s| s["tasks"].as_array().into_iter().flatten())
            .filter(|t| t["profile"] == p.profile.id && t["chat"].as_str() == p.chat.as_deref())
        {
            if let Some(old) = tasks.iter_mut().find(|v| v["id"] == t["id"]) {
                if old["finished"] != t["finished"]
                    || ["running", "approval", "queued"]
                        .contains(&t["state"].as_str().unwrap_or(""))
                {
                    *old = t.clone();
                }
            } else {
                tasks.push(t.clone());
            }
        }
        tasks.sort_by_key(|t| t["created"].as_i64().unwrap_or(0));
        let mut history = vec![];
        for t in &tasks {
            let id = t["id"].as_str().unwrap_or("");
            if t["call"].is_string() {
                for (i, line) in t["utterances"].as_array().into_iter().flatten().enumerate() {
                    history.push(json!({"id":format!("{id}-voice-{i}"),"kind":line["role"],"lines":[{"text":line["text"],"style":"plain"}],"agent":"ninja"}));
                }
            } else {
                history.push(json!({"id":format!("{id}-user"),"kind":"user","lines":[{"text":t["prompt"],"style":"plain"}],"agent":"ninja"}));
                let text = crate::ninjas::readable_brief(t["brief"].as_str().unwrap_or(""));
                if !text.is_empty() {
                    history.push(json!({"id":format!("{id}-reply"),"kind":"assistant","lines":[{"text":text,"style":"plain"}],"agent":"ninja"}));
                }
            }
        }
        let active = tasks.iter().rev().find(|t| {
            ["running", "approval", "queued"].contains(&t["state"].as_str().unwrap_or(""))
        });
        let thread = format!(
            "ninja-chat:{}:{}:{}",
            p.host,
            p.profile.id,
            p.chat.as_deref().unwrap_or("draft")
        );
        let models=p.models.iter().map(|m|json!({"model":m["id"],"displayName":m["label"],"supportedReasoningEfforts":m["efforts"].as_array().into_iter().flatten().map(|e|json!({"reasoningEffort":e})).collect::<Vec<_>>()})).collect::<Vec<_>>();
        let override_ = json!({"orchestratorMode":p.profile.orchestrator,"orchestratorAvailable":true,"personalNinja":{"id":p.profile.id,"name":p.profile.name,"style":p.profile.style,"host":p.host},"threadId":thread,"threadName":p.chat.as_ref().map(|chat|self.lifecycle.chat_names.get(&Self::ninja_chat_key(&p.host,&p.profile.id,chat)).cloned().or_else(||self.ninja_chat_seed(&Self::ninja_chat_key(&p.host,&p.profile.id,chat))).unwrap_or_else(||p.profile.name.clone())).unwrap_or_else(||p.profile.name.clone()),"ownerToken":self.ninja_surface_owner(),"project":p.profile.project,"composerDraft":p.fields[3].read(cx).text(),"history":history,"historyLoading":p.history_loading,"modelsLoading":p.models_loading.is_some(),"historyHasMore":false,"historyEarlier":false,"readOnly":false,"composerBusy":p.busy,"selectedAgent":p.profile.agent.command(),"selectedModel":p.profile.model,"effectiveModel":active.and_then(|t|t["model"].as_str()).or(p.profile.model.as_deref()),"reasoningEffort":p.profile.effort,"models":models,"chats":chats,"activeTurnId":active.and_then(|t|t["id"].as_str()),"sessionActivity":{"scene":active.map(crate::ninjas_ui::task_scene).unwrap_or("off"),"text":active.and_then(|t|t["status"].as_str()).unwrap_or("")},"attachments":[]});
        for (key, value) in override_.as_object().unwrap() {
            s[key] = value.clone();
        }
        s["agents"]=json!(snapshot.into_iter().flat_map(|v|v["providers"].as_array().into_iter().flatten()).filter_map(|p|serde_json::from_value::<AgentKind>(p["agent"].clone()).ok().map(|a|json!({"id":a.command(),"name":a.name(),"available":p["installed"]==true}))).collect::<Vec<_>>());
        s["workspaceAccess"] = json!(if !p.profile.edits {
            "read-only"
        } else if p.profile.access == runtime::CodexAccess::Full {
            "full"
        } else {
            "workspace"
        });
        s["workspaceApprovalPolicy"] = json!(match p.profile.approvals {
            runtime::CodexApprovals::Ask => "ask",
            runtime::CodexApprovals::AutoReview => "auto-review",
            runtime::CodexApprovals::Never => "never",
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    #[gpui::test]
    fn mini_and_menu_hotkey_destinations_switch_the_same_shared_window(cx:&mut TestAppContext){
        let(view,cx)=crate::ui_tests::fixture(cx);
        cx.update(|_,cx|{cx.set_global(quick_chat::Controller::new(view));cx.update_global::<quick_chat::Controller,_>(|c,cx|{c.toggle_menu(cx).unwrap();assert!(c.is_menu_surface());c.toggle(cx).unwrap();assert!(!c.is_menu_surface());c.toggle_menu(cx).unwrap();assert!(c.is_menu_surface());c.dismiss(cx);});});
    }
    #[gpui::test]
    fn menu_bar_opens_mini_code_after_returning_the_shared_entity_lease(cx:&mut TestAppContext){
        let(view,cx)=crate::ui_tests::fixture(cx);
        cx.update(|w,cx|{
            cx.set_global(quick_chat::Controller::new(view.clone()));
            view.update(cx,|app,cx|{app.unified_command(&json!({"action":"menu_bar_open","kind":"kai"}),w,cx).unwrap();});
        });
        cx.run_until_parked();
        cx.update(|_,cx|{cx.update_global::<quick_chat::Controller,_>(|c,cx|c.dismiss(cx));});
    }
    #[gpui::test]
    fn notch_owns_the_selected_ninja_history_and_draft(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        cx.update(|w,cx|view.update(cx,|app,cx|{
            app.composer.update(cx,|i,cx|i.set_text("Untouched Code draft",cx));
            app.open_ninjas(Some("local".into()),None,w,cx);
            let p=app.ninjas.panel.as_mut().unwrap();p.profile.revision=1;p.profile.name="Ice".into();p.models_loading=Some(uuid::Uuid::new_v4());p.chat=Some("chat-a".into());p.messages=vec![json!({"id":"turn-a","created":1,"profile":p.profile.id,"chat":"chat-a","prompt":"Hello Ice","brief":"My own answer","state":"completed","finished":2})];
            let owner=app.ninja_surface_owner().unwrap();let snapshot=app.unified_snapshot(cx);
            assert_eq!(snapshot["threadName"],"Ice");assert_eq!(snapshot["modelsLoading"],true);assert_eq!(snapshot["composerBusy"],false);assert_eq!(snapshot["personalNinja"]["name"],"Ice");assert_eq!(snapshot["history"][1]["lines"][0]["text"],"My own answer");
            assert!(app.unified_command(&json!({"action":"draft","ownerToken":"stale","draftBase":"","text":"wrong"}),w,cx).is_err());
            app.unified_command(&json!({"action":"draft","ownerToken":owner,"draftBase":"","text":"Ninja-only draft"}),w,cx).unwrap();
            assert_eq!(app.ninjas.panel.as_ref().unwrap().fields[3].read(cx).text(),"Ninja-only draft");assert_eq!(app.composer.read(cx).text(),"Untouched Code draft");
            // The global Send action in mini Code must never submit the hidden
            // Code draft while its visible Ninja is busy.
            app.ninjas.panel.as_mut().unwrap().busy=true;app.send(w,cx);
            assert_eq!(app.composer.read(cx).text(),"Untouched Code draft");assert_eq!(app.ninjas.panel.as_ref().unwrap().fields[3].read(cx).text(),"Ninja-only draft");app.ninjas.panel.as_mut().unwrap().busy=false;
            app.unified_command(&json!({"action":"new","ownerToken":owner}),w,cx).unwrap();assert!(app.ninjas.panel.as_ref().unwrap().messages.is_empty());assert!(app.ninjas.panel.as_ref().unwrap().chat.is_none());assert_eq!(app.composer.read(cx).text(),"Untouched Code draft");assert!(app.ninjas.drafts.values().any(|s|s=="Ninja-only draft"));
        }));
    }
}
