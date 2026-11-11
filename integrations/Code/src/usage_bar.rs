//! One account quota cache for the workspace, independent of selected chat.
use super::*;
#[derive(Default)]
pub(super) struct State {
    windows: HashMap<String,(Instant,Vec<usage::LimitWindow>)>,
    pending: HashSet<String>,
    refreshed: Option<Instant>,
}
impl State {
    pub(super) fn snapshot(&self,key:&str)->Option<(Instant,&Vec<usage::LimitWindow>)>{
        self.windows.get(key).filter(|(_,v)|!v.is_empty()).map(|(at,v)|(*at,v))
    }
}
impl Orbit {
    pub(super) fn refresh_usage_bar(&mut self,force:bool,cx:&mut Context<Self>){
        if self.agents.is_empty() || (cfg!(test) && !force){return}
        if !force && self.usage_bar.refreshed.is_some_and(|t|t.elapsed()<Duration::from_secs(300)){return}
        self.usage_bar.refreshed=Some(Instant::now());
        for agent in self.agents.clone().into_iter().filter(|a|a.available && a.kind!=AgentKind::Ninja){
            let key=agent.kind.command();
            if !self.usage_bar.pending.insert(key.into()){continue}
            let home=self.controls_home.clone();let project=self.model_project();
            cx.spawn(async move|this,cx|{
                let limits=cx.background_executor().spawn(async move{
                    if agent.kind==AgentKind::Codex {
                        agent.executable.map(|path|agent_controls::inspect_project(agent.kind,&path,&home,&project).limits).unwrap_or_default()
                    }else{agent_controls::account_limits(agent.kind,agent.executable.as_deref(),&home)}
                }).await;
                let _=this.update(cx,|app,cx|{app.usage_bar.pending.remove(key);app.usage_bar.windows.insert(key.into(),(Instant::now(),limits));cx.notify();});
            }).detach();
        }
        cx.spawn(async move|this,cx|{Timer::after(Duration::from_secs(300)).await;let _=this.update(cx,|_,cx|cx.notify());}).detach();
    }
    pub(super) fn account_usage_bar(&self,cx:&Context<Self>)->AnyElement{
        let now=chrono::Utc::now().timestamp();
        let providers:Vec<_>=self.agents.iter().filter(|a|a.available&&a.kind!=AgentKind::Ninja).filter_map(|a|{
            let windows=self.provider_limit_windows(a.kind);
            (!windows.is_empty()).then(||(a.kind,windows.iter().map(|w|w.compact_remaining(now,false)).collect::<Vec<_>>().join("  /  ")))
        }).collect();
        if providers.is_empty(){return div().into_any_element()}
        div().id("account-usage-bar").debug_selector(||"account-usage-bar".into()).h(px(28.)).flex_none().w_full().min_w(px(0.)).px_3().flex().items_center().gap_4().border_t_1().border_color(rgba(0xffffff16)).text_size(theme::ui_px(11.)).text_color(rgb(MUTED)).overflow_x_scroll()
        .child("Left")
        .children(providers.into_iter().map(|(kind,text)|{
            div().id(SharedString::from(format!("account-usage-{}",kind.command()))).debug_selector(move||format!("account-usage-{}",kind.command())).flex_none().flex().items_center().gap_1().child(provider_icon(agent_icon(kind),16.,agent_color(kind))).child(text)
        }))
        .child(div().id("refresh-account-usage").flex_none().cursor_pointer().child(icon("refresh",13.,MUTED)).on_click(cx.listener(|app,_,_,cx|{app.refresh_usage_bar(true,cx);cx.notify();}))).into_any_element()
    }
}

#[cfg(test)] mod tests {
    use super::*;
    use std::prelude::v1::test;
    #[gpui::test] fn welcome_respects_project_and_standalone_navigation(cx:&mut TestAppContext){
        let(app,cx)=crate::ui_tests::fixture(cx);
        app.update(cx,|app,_|{app.project=PathBuf::from("/tmp/my-project");assert_eq!(app.build_greeting(),"What do you want to build in my-project?");app.projects.push(Project{path:app.project.clone(),name:"Frontend name".into()});assert_eq!(app.build_greeting(),"What do you want to build in Frontend name?");app.lifecycle.project_names.insert(app.project.clone(),"My custom project".into());assert_eq!(app.build_greeting(),"What do you want to build in My custom project?");app.project=app.unprojected_root();assert_eq!(app.build_greeting(),"What do you want to build?");});
    }
    #[gpui::test] fn quota_bar_keeps_all_account_scopes_and_chat_selection(cx:&mut TestAppContext){
        let(app,cx)=crate::ui_tests::fixture(cx);
        let selected=app.read_with(cx,|app,_|app.active.clone());
        app.update(cx,|app,_|{app.usage_bar.windows.insert("cursor-agent".into(),(Instant::now(),vec![usage::LimitWindow{label:"Billing cycle".into(),used_percent:5.,duration_minutes:None,resets_at:Some(1800000000),reported_at_ms:None}]));app.agents.push(Agent{kind:AgentKind::Cursor,name:"Cursor".into(),executable:None,available:true});});
        cx.draw(point(px(0.),px(0.)),size(px(1000.),px(650.)),|_,_|app.clone().into_any_element());
        assert!(cx.debug_bounds("account-usage-bar").is_some());
        assert_eq!(app.read_with(cx,|app,_|app.active.clone()),selected);
    }
    #[test] fn compact_labels_never_leak_bucket_names_or_repeat_time_windows(){
        let now=1800000000;
        let quota=usage::LimitWindow{label:"codex primary weekly limit".into(),used_percent:5.,duration_minutes:Some(10080),resets_at:Some(now+2*86400+3*3600),reported_at_ms:None};
        assert_eq!(quota.compact_remaining(now,false),"95% · 2d 3h");
        assert_eq!(quota.compact_remaining(now,true),"95% left · 2d 3h");
    }
    #[gpui::test] fn account_quota_is_shared_by_bar_footer_and_provider_switches(cx:&mut TestAppContext){
        let(app,cx)=crate::ui_tests::fixture(cx);
        app.update(cx,|app,_|{
            let quota=usage::LimitWindow{label:"native weekly".into(),used_percent:9.,duration_minutes:Some(10080),resets_at:Some(1800000000),reported_at_ms:None};
            app.usage_bar.windows.insert("cursor-agent".into(),(Instant::now(),vec![quota.clone()]));
            app.agent=AgentKind::Cursor;
            assert_eq!(app.reported_limit_windows(),app.provider_limit_windows(AgentKind::Cursor));
            assert_eq!(app.reported_limit_windows()[0],quota);
            assert!(app.context_usage().is_none(),"A provider switch must not reuse the previous provider's context");
            assert!(!app.usage_summary().contains("Cursor"));
            app.agent=AgentKind::Claude;assert!(app.reported_limit_windows().is_empty());assert!(app.usage_summary().is_empty());
            app.agents=vec![Agent{kind:AgentKind::Claude,name:"Claude".into(),executable:None,available:true}];
        });
        cx.draw(point(px(0.),px(0.)),size(px(1000.),px(650.)),|_,_|app.clone().into_any_element());
        assert!(cx.debug_bounds("account-usage-claude").is_none());
        assert!(cx.debug_bounds("provider-usage-footer").is_none());
    }

}
