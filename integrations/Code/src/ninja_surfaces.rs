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
                | "composer_command"
                | "ninja_artifact"
                | "interaction_details"
        ) {
            return Ok(false);
        }
        if v["ownerToken"].as_str() != Some(&owner) {
            return Err("The selected Ninja conversation changed; your draft is preserved".into());
        }
        match action {
            "interaction_details"=>{
                let raw=v["id"].as_str().and_then(|id|id.strip_prefix("ninja-work:")).ok_or("Missing Ninja work group")?;
                let(task,index)=raw.rsplit_once(':').ok_or("Invalid Ninja work group")?;let index=index.parse::<usize>().map_err(|_|"Invalid Ninja work group")?;
                let p=self.ninjas.panel.as_ref().unwrap();let owner=p.messages.iter().chain(p.history_tasks.borrow().iter()).any(|t|t["id"]==task&&t["chat"].as_str()==p.chat.as_deref());if !owner{return Err("This work belongs to another conversation".into())}
                let key=crate::ninja_chat::block_key(&format!("{task}:work"),index);if !self.expanded_tools.remove(&key){self.expanded_tools.insert(key);}cx.notify();
            },
            "ninja_artifact"=>{let path=v["path"].as_str().ok_or("Missing generated file")?;self.ninja_request_artifact(path.into(),true,cx);},
            "composer_command"=>{
                let name=v["name"].as_str().filter(|n|crate::ninja_chat::COMMANDS.iter().any(|(known,_)|n==known)).ok_or("Unknown Ninja command")?;
                let p=self.ninjas.panel.as_ref().unwrap();
                let draft=v["draft"].as_str().filter(|s|s.len()<=input::MAX_DRAFT_BYTES&&!s.contains('\0')).ok_or("Invalid Ninja draft")?;
                let caret=v["caret"].as_u64().ok_or("Missing caret")? as usize;
                let input=p.fields[3].clone();input.update(cx,|i,cx|i.set_text(draft,cx));
                if !input.update(cx,|i,cx|i.set_command_caret_utf16(caret,cx)){return Err("Invalid caret".into())}
                input.update(cx,|i,cx|{if let Some(span)=i.command_span(){let mut text=i.text().to_string();text.replace_range(span,"");i.set_text(&text,cx);}});
                self.ninja_command(name,window,cx)?;
                // Native model selection stays in the notch picker. Other host
                // controls reveal their actual shared chat instead of Code settings.
                if matches!(name,"model"|"agent"|"thinking")&&v["pickerSurface"]=="notch"{if let Some(p)=&mut self.ninjas.panel{p.control=None;}}
                else if self.ninjas.panel.as_ref().is_some_and(|p|p.control.is_some()){crate::unified::reveal();}
            },
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
            "orchestrator"=>{let p=self.ninjas.panel.as_mut().unwrap();p.profile.orchestrator=!p.profile.orchestrator;p.control_at=Instant::now();self.ninja_persist_controls(cx);},
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
                self.ninja_persist_controls(cx);
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
                p.effort_preview=effort.map(|s|(s.to_owned(),Instant::now()));
                p.control_at=Instant::now();
                p.profile.model = Some(model.into());
                p.profile.effort = effort.map(str::to_owned);
                if let Some(n)=&mut p.profile.ninja{n.model=model.into();n.thinking=effort.map(str::to_owned);}
                self.ninja_persist_controls(cx);
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
                self.ninja_persist_controls(cx);
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
        let tasks=crate::ninja_chat::merged_tasks(&p.messages,snapshot,&p.profile.id,p.chat.as_deref());
        let mut history = vec![];
        for t in &tasks {
            let id = t["id"].as_str().unwrap_or("");
            if t["call"].is_string() {
                for (i, line) in t["utterances"].as_array().into_iter().flatten().enumerate() {
                    history.push(json!({"id":format!("{id}-voice-{i}"),"kind":line["role"],"lines":[{"text":line["text"],"style":"plain"}],"agent":"ninja"}));
                }
            } else {
                history.push(json!({"id":format!("{id}-user"),"kind":"user","lines":[{"text":t["prompt"],"style":"plain"}],"agent":"ninja"}));
                let blocks=self.ninja_blocks(false,t["brief"].as_str().unwrap_or(""),id);
                let answer=crate::ninja_chat::answer_range(&blocks);
                let show_work=self.expanded_tools.contains(&crate::ninja_chat::block_key(&format!("{id}:work"),0));
                if blocks.iter().enumerate().any(|(i,b)|!crate::ninja_chat::response_block(b,i,&answer)){
                    let steps=blocks.iter().filter(|b|crate::ninja_chat::work_block(b)).count();
                    history.push(json!({"id":format!("{id}-work"),"kind":"activity","lines":[{"text":format!("{} · {steps} steps",crate::ninja_chat::work_label(t)),"style":"activity"}],"detailId":format!("ninja-work:{id}:0"),"agent":"ninja","speechText":""}));
                }
                let work=(0..blocks.len()).filter(|i|!crate::ninja_chat::response_block(&blocks[*i],*i,&answer)&&show_work);
                let response=(0..blocks.len()).filter(|i|crate::ninja_chat::response_block(&blocks[*i],*i,&answer));
                for i in work.chain(response){
                    let block=&blocks[i];
                    let (kind,text,style,preview)=match block{
                        MessageBlock::Paragraph(s)|MessageBlock::Quote(s)=>("assistant",s.clone(),"text",None),
                        MessageBlock::Heading(s)=>("assistant",s.clone(),"heading",None),
                        MessageBlock::RichParagraph{text,..}|MessageBlock::ListItem{text,..}=>("assistant",text.clone(),"text",None),
                        MessageBlock::Code{text,..}=>("assistant",text.clone(),"code",None),
                        MessageBlock::Attachment{name,target}=>{("assistant",name.clone(),"text",self.ninja_native_preview(target))},
                        MessageBlock::Activity{label,details}=>("activity",format!("{label}\n{details}"),"code",None),
                        MessageBlock::Interaction(chat_interactions::ChatInteraction::Status{title,details,..}|chat_interactions::ChatInteraction::Delegation{title,details,..})=>("activity",format!("{title}\n{details}"),"code",None),
                        MessageBlock::TableRow{cells,..}=>("assistant",cells.join(" | "),"text",None),
                        MessageBlock::Voice{text,..}=>("assistant",text.clone(),"voice",None),
                        MessageBlock::Interaction(chat_interactions::ChatInteraction::NativeRequest(r))=>("activity",format!("{}\n{}",r.title,r.detail),"code",None),
                        MessageBlock::Interaction(chat_interactions::ChatInteraction::Question(q))=>("activity",format!("{}\n{}",q.prompt,q.answer.as_deref().unwrap_or("")),"text",None),
                        MessageBlock::Interaction(chat_interactions::ChatInteraction::Answer{question,answer})=>("activity",format!("{question}\n{answer}"),"text",None),
                        MessageBlock::Interaction(chat_interactions::ChatInteraction::Speaker{text,..})=>("assistant",text.clone(),"text",None),
                    };
                    let link=if let MessageBlock::Attachment{target,..}=block{Some(self.ninja_native_url(target))}else{None};
                    history.push(json!({"id":format!("{id}-reply-{i}"),"kind":kind,"lines":[{"text":text,"style":style,"previewPNG":preview,"linkTarget":link,"links":self.ninja_native_links(block)}],"agent":"ninja","speechText":if kind=="activity"||style=="code"{""}else{&text}}));
                }
            }
        }
        let active = tasks.iter().rev().find(|t| {
            crate::ninja_chat::live(t)
        });
        let thread = format!(
            "ninja-chat:{}:{}:{}",
            p.host,
            p.profile.id,
            p.chat.as_deref().unwrap_or("draft")
        );
        let models=p.models.iter().map(|m|json!({"model":m["id"],"displayName":m["label"],"supportedReasoningEfforts":m["efforts"].as_array().into_iter().flatten().map(|e|json!({"reasoningEffort":e})).collect::<Vec<_>>()})).collect::<Vec<_>>();
        let override_ = json!({"orchestratorMode":p.profile.orchestrator,"orchestratorAvailable":true,"personalNinja":{"id":p.profile.id,"name":p.profile.name,"style":p.profile.style,"host":p.host},"threadId":thread,"threadName":p.chat.as_ref().map(|chat|self.lifecycle.chat_names.get(&Self::ninja_chat_key(&p.host,&p.profile.id,chat)).cloned().or_else(||self.ninja_chat_seed(&Self::ninja_chat_key(&p.host,&p.profile.id,chat))).unwrap_or_else(||p.profile.name.clone())).unwrap_or_else(||p.profile.name.clone()),"ownerToken":self.ninja_surface_owner(),"project":p.profile.project,"composerDraft":p.fields[3].read(cx).text(),"history":crate::unified::budget_history(history),"historyLoading":p.history_loading,"modelsLoading":p.models_loading.is_some(),"historyHasMore":false,"historyEarlier":false,"readOnly":false,"composerBusy":p.busy,"selectedAgent":p.profile.agent.command(),"selectedModel":p.profile.model,"effectiveModel":active.and_then(|t|t["model"].as_str()).or(p.profile.model.as_deref()),"reasoningEffort":p.profile.effort,"models":models,"chats":chats,"activeTurnId":active.and_then(|t|t["id"].as_str()),"sessionActivity":{"scene":active.map(|t|crate::ninjas_ui::task_scene(t)).unwrap_or("off"),"text":active.and_then(|t|t["status"].as_str()).unwrap_or("")},"attachments":[]});
        for (key, value) in override_.as_object().unwrap() {
            s[key] = value.clone();
        }
        let usage=tasks.iter().rev().find_map(|t|crate::ninja_chat::usage_label(&t["usage"]));
        let context=tasks.iter().rev().find_map(|t|t["usage"]["context_used"].as_u64().zip(t["usage"]["context_limit"].as_u64()).filter(|(_,limit)|*limit>0));
        s["usageSummary"]=json!(usage.unwrap_or_default());
        s["contextWindow"]=json!(context.map(|(used,limit)|json!({"used":used,"limit":limit,"fraction":(used as f64/limit as f64).clamp(0.,1.)})));
        s["usageWindows"]=json!([]);s["usageAccount"]=Value::Null;
        s["slashCommands"]=json!(crate::ninja_chat::COMMANDS.iter().map(|(name,description)|json!({"name":name,"description":description})).collect::<Vec<_>>());
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
    fn native_work_disclosure_uses_the_same_group_and_rejects_a_changed_owner(cx:&mut TestAppContext){
        let(view,cx)=crate::ui_tests::fixture(cx);
        cx.update(|w,cx|view.update(cx,|app,cx|{
            app.open_ninjas(Some("local".into()),None,w,cx);let p=app.ninjas.panel.as_mut().unwrap();p.profile.revision=1;p.chat=Some("chat".into());
            let tool=crate::chat_interactions::serialize_interaction(&crate::chat_interactions::ChatInteraction::Delegation{title:"Terminal output".into(),details:"Verified fixture output".into(),status:crate::chat_interactions::InteractionStatus::Completed,agent:None});
            p.messages=vec![json!({"id":"turn","profile":p.profile.id,"chat":"chat","brief":format!("{tool}\n\nDone"),"prompt":"Check","state":"completed","finished":2})];
            let owner=app.ninja_surface_owner().unwrap();let before=app.unified_snapshot(cx);let detail=before["history"].as_array().unwrap().iter().find_map(|v|v["detailId"].as_str()).unwrap().to_owned();assert!(!before["history"].to_string().contains("Verified fixture output"));
            app.unified_command(&json!({"action":"interaction_details","ownerToken":owner,"id":detail}),w,cx).unwrap();assert!(app.unified_snapshot(cx)["history"].to_string().contains("Verified fixture output"));
            app.unified_command(&json!({"action":"interaction_details","ownerToken":owner,"id":detail}),w,cx).unwrap();assert!(!app.unified_snapshot(cx)["history"].to_string().contains("Verified fixture output"));
            app.ninjas.panel.as_mut().unwrap().chat=Some("other".into());assert!(app.unified_command(&json!({"action":"interaction_details","ownerToken":owner,"id":detail}),w,cx).is_err());
        }));
    }
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
