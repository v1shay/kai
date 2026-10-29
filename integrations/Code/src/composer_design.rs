//! Shared composer identity and discrete native reasoning controls.
use super::*;

pub fn model_glyph(model:&str)->Option<&'static str>{
    let lower=model.to_lowercase();let tokens=lower.split(|c:char|!c.is_alphanumeric()).collect::<Vec<_>>();
    if !tokens.contains(&"gpt") && !tokens.contains(&"openai"){return None}
    [("luna","model-moon"),("terra","model-earth"),("sol","model-sun"),("astra","model-star")].into_iter().find_map(|(family,glyph)|tokens.contains(&family).then_some(glyph))
}
pub fn display_label(value:&str)->String{
    let (model,effort)=value.split_once(" · ").map_or((value,None),|(model,effort)|(model,Some(effort)));
    if !model.starts_with("gpt-") || model_glyph(model).is_none(){return value.into()}
    let parts=model.splitn(3,'-').collect::<Vec<_>>();if parts.len()!=3{return value.into()}
    let name=format!("GPT-{} {}",parts[1],parts[2].split('-').map(title).collect::<Vec<_>>().join(" "));
    effort.map_or_else(||name.clone(),|effort|format!("{name} · {}",title(effort)))
}
pub fn intensity(value:f32)->u32{
    let stops=[0xb6dfff,0x9692ff,0xe879ce,0xff785c,0xff5565];let scaled=value.clamp(0.,1.)*4.;let index=(scaled as usize).min(3);let t=scaled-index as f32;
    let channel=|shift|{let a=((stops[index]>>shift)&255)as f32;let b=((stops[index+1]>>shift)&255)as f32;(a+(b-a)*t).round()as u32};
    (channel(16)<<16)|(channel(8)<<8)|channel(0)
}
pub fn title(value:&str)->String{if value=="xhigh"{"Extra high".into()}else{let mut chars=value.chars();chars.next().map_or_else(String::new,|c|c.to_uppercase().collect::<String>()+chars.as_str())}}
pub fn effort_color(value:&str,fraction:f32)->u32{match value{"none"=>0xa8b4c4,"minimal"=>0x83d6ff,"low"=>0x51d9bd,"medium"=>0x8996ff,"high"=>0xff9870,"xhigh"=>0xf079cd,"max"=>0xf4b942,"ultra"=>0xa855f7,_=>intensity(fraction)}}
#[cfg(test)]
fn index_at(bounds:Bounds<Pixels>,position:Point<Pixels>,count:usize)->Option<usize>{
    if count==0 || f32::from(bounds.size.width)<=18.{return None}
    let fraction=((f32::from(position.x-bounds.origin.x)-9.)/(f32::from(bounds.size.width)-18.)).clamp(0.,1.);
    Some((fraction*(count.saturating_sub(1))as f32).round()as usize)
}
// The same finite Monocode burst paints the selected chip and preview option.
pub(super) fn shimmer_tiles(elapsed:Option<f32>,color:u32,motion:bool)->AnyElement{
    canvas(|bounds,_,_|bounds,move|bounds,_,window,_|{
        if let Some(t)=elapsed.filter(|t|*t<5.){let fade=if motion{((5.-t)/0.4).clamp(0.,1.)}else{0.7};
            for n in 0..160{let col=n%32;let row=n/32;let distance=(((col as f32-15.5)/15.5).powi(2)+((row as f32-2.)/2.).powi(2)).sqrt();let phase=((t-distance*0.42).max(0.)%1.4)/1.4;let filled=(n*73+n*n*19+23)%101<65;let pulse=if !motion{0.5}else if phase<0.26{phase/0.26}else if phase<0.52{1.-(phase-0.26)/0.26*0.82}else{0.18*(1.-(phase-0.52)/0.48)};let feather=((col as f32+0.5)/4.).min((31.5-col as f32)/4.).min(1.)*if row==0||row==4{0.2}else if row==1||row==3{0.65}else{1.};let alpha=(((if filled{pulse}else{0.09})*fade*feather)*210.)as u32;let w=bounds.size.width/32.;let h=bounds.size.height/5.;window.paint_quad(fill(Bounds::new(bounds.origin+point(w*col as f32,h*row as f32),size((w-px(0.7)).max(px(0.)),(h-px(0.7)).max(px(0.)))),rgba((color<<8)|alpha)));}
        }
    }).absolute().size_full().into_any_element()
}
impl Orbit{
    pub(super) fn fast_supported(&self)->bool{
        self.agent==AgentKind::Codex && self.controls.get("codex").is_some_and(|(_,c)|self.model.clone().or_else(||self.effective_model_settings().0).is_some_and(|m|c.defaults.fast_models.contains_key(&m)))
    }
    pub(super) fn fast_enabled(&self)->bool{self.reasoning_efforts.get("codex:service-tier").is_some_and(|v|v=="fast")}
    pub(super) fn request_service_tier(&self)->Option<String>{if self.agent!=AgentKind::Codex{return None}Some(if self.fast_enabled(){self.controls.get("codex").and_then(|(_,c)|self.request_model().or_else(||self.effective_model_settings().0).and_then(|m|c.defaults.fast_models.get(&m).cloned())).unwrap_or_else(||"default".into())}else{"default".into()})}
    pub(super) fn toggle_fast(&mut self,cx:&mut Context<Self>){if self.fast_supported(){let next=if self.fast_enabled(){"default"}else{"fast"};self.reasoning_efforts.insert("codex:service-tier".into(),next.into());self.fast_transition=Some(Instant::now());self.queue_workspace_save(cx);cx.notify();}}
    pub(super) fn fast_button(&self,window:&Window,cx:&Context<Self>)->AnyElement{
        let on=self.fast_enabled();let color=if on{0xffd968}else{MUTED};
        let chip=div().id("fast-mode").debug_selector(||"fast-mode".into()).h(px(26.)).w(px(28.)).relative().overflow_hidden().rounded_md().flex().items_center().justify_center().cursor_pointer()
            .bg(linear_gradient(90.,linear_color_stop(rgba(if on{0xffee9824}else{0xffffff08}),0.),linear_color_stop(rgba(if on{0xffbd3510}else{0xffffff08}),1.)))
            .on_hover(cx.listener(|this,hover:&bool,_,cx|{if *hover{this.fast_transition=Some(Instant::now());cx.notify();}}))
            .on_click(cx.listener(|this,_,_,cx|this.toggle_fast(cx)));
        let bolt=move ||div().relative().child(icon("bolt",15.,color));
        if self.motion && !native::reduced_motion() && window.is_window_active(){
            // Same five-second, distance-staggered tile burst as intensity.
            // Hover/toggle changes the animation identity; ordinary redraws do not.
            let key=format!("fast-chip-{on}-{}",self.fast_transition.map(|at|format!("{at:?}")).unwrap_or_default());
            chip.with_animation(SharedString::from(key),Animation::new(Duration::from_secs(5)),move|chip,t|chip.child(shimmer_tiles(Some(t*5.),0xffd968,true)).child(bolt())).into_any_element()
        }else{chip.child(shimmer_tiles(Some(0.),0xffd968,false)).child(bolt()).into_any_element()}
    }
    pub(super) fn thinking_chip(&self,window:&Window,cx:&Context<Self>)->AnyElement{
        let Some(model)=self.effort_model_choice().filter(|m|!m.reasoning_efforts.is_empty())else{return div().into_any_element()};
        let value=self.unified_effort().map(str::to_owned).or_else(||self.effective_model_settings().1).unwrap_or_default();let i=model.reasoning_efforts.iter().position(|e|e==&value).unwrap_or(0);let t=i as f32/model.reasoning_efforts.len().saturating_sub(1).max(1)as f32;let color=effort_color(&value,t);let label=title(&value);
        let chip=div().id("thinking-chip").debug_selector(||"thinking-chip".into()).px_2().h(px(26.)).relative().overflow_hidden().rounded_md().flex().items_center().text_size(theme::ui_px(11.)).text_color(rgb(color)).bg(linear_gradient(90.,linear_color_stop(rgba((color<<8)|0x24),0.),linear_color_stop(rgba((intensity((t+0.25).min(1.))<<8)|0x10),1.))).cursor_pointer()
        .on_mouse_move(cx.listener(|this,event:&MouseMoveEvent,window,cx|{if this.agent_picker.is_none(){this.toggle_agent_picker(event.position,window,cx);if let Some(p)=this.agent_picker.as_mut(){p.effort_only=true;p.anchor=event.position-point(px(20.),px(230.));}}}))
        .on_click(cx.listener(|this,event:&ClickEvent,window,cx|{if this.agent_picker.as_ref().is_none_or(|p|p.closing){this.toggle_agent_picker(event.position(),window,cx);}if let Some(p)=this.agent_picker.as_mut(){p.effort_only=true;}}));
        if self.motion && !native::reduced_motion() && window.is_window_active(){
            chip.with_animation(SharedString::from(format!("thinking-chip-{}-{value}",self.agent.command())),Animation::new(Duration::from_secs(5)),move|chip,t|chip.child(shimmer_tiles(Some(t*5.),color,true)).child(div().relative().child(label.clone()))).into_any_element()
        }else{chip.child(shimmer_tiles(Some(0.),color,false)).child(div().relative().child(label)).into_any_element()}
    }
    fn effort_fraction(&self,count:usize,current:usize)->f32{
        let target=current as f32/count.saturating_sub(1).max(1)as f32;
        self.agent_picker.as_ref().and_then(|p|p.effort_motion).filter(|_|self.motion).map_or(target,|(at,from,to)|{
            let t=(at.elapsed().as_secs_f32()/0.32).min(1.);from+(to-from)*(1.-(1.-t).powi(3))
        })
    }
    pub(super) fn select_reasoning_effort(&mut self,effort:&str,cx:&mut Context<Self>)->bool{
        let Some(model)=self.effort_model_choice()else{return false};
        let Some(index)=model.reasoning_efforts.iter().position(|e|e==effort)else{return false};
        let current=self.unified_effort().map(str::to_owned).or_else(||self.effective_model_settings().1);
        let old=model.reasoning_efforts.iter().position(|e|Some(e)==current.as_ref()).unwrap_or(0);
        let from=self.effort_fraction(model.reasoning_efforts.len(),old);
        if let Some(picker)=self.agent_picker.as_mut(){picker.effort_motion=Some((Instant::now(),from,index as f32/model.reasoning_efforts.len().saturating_sub(1).max(1)as f32));}
        let key=self.effort_key();self.reasoning_efforts.insert(key,effort.into());self.queue_workspace_save(cx);cx.notify();true
    }
    pub(super) fn reasoning_slider(&self,model:agent_controls::ModelChoice,cx:&Context<Self>)->AnyElement{
        let levels=model.reasoning_efforts;let count=levels.len();
        let selected=self.unified_effort().map(str::to_owned).or_else(||self.effective_model_settings().1);
        div().flex().flex_col().gap_1().child(div().px_2().py_1().text_size(theme::ui_px(10.)).text_color(rgb(MUTED)).child("Thinking intensity"))
        .children(levels.into_iter().enumerate().map(|(i,level)|{
            let color=effort_color(&level,i as f32/count.saturating_sub(1).max(1)as f32);let active=selected.as_ref()==Some(&level);
            let elapsed=self.agent_picker.as_ref().and_then(|p|p.effort_hover.as_ref()).filter(|(v,_)|v==&level).map(|(_,at)|at.elapsed().as_secs_f32());let motion=self.motion && !native::reduced_motion();
            let hover_level=level.clone();let return_to=selected.clone();let select=level.clone();let text=title(&level);
            div().id(SharedString::from(format!("reasoning-{level}"))).debug_selector(move||format!("reasoning-{level}")).h(px(34.)).w_full().relative().overflow_hidden().rounded_md().cursor_pointer().bg(rgba(if active{0xffffff14}else{0x00000000}))
            .child(shimmer_tiles(elapsed,color,motion))
            .child(div().relative().px_3().h_full().flex().items_center().justify_between().text_size(theme::ui_px(12.)).text_color(rgb(color)).child(text).when(active,|s|s.child("✓")))
            .on_hover(cx.listener(move|this,hover:&bool,_,cx|{if let Some(p)=this.agent_picker.as_mut(){if *hover{p.effort_hover=Some((hover_level.clone(),Instant::now()));}else if p.effort_hover.as_ref().is_some_and(|(v,_)|v==&hover_level){p.effort_hover=return_to.clone().map(|value|(value,Instant::now()));}cx.notify();}}))
            .on_click(cx.listener(move|this,_,_,cx|{this.select_reasoning_effort(&select,cx);this.dismiss_agent_picker();cx.notify();}))
        })).into_any_element()
    }
}
#[cfg(test)]mod tests{
 use super::*;use std::prelude::v1::test;
 #[test]fn families_are_exact_and_non_openai_models_do_not_get_planets(){for (model,glyph) in [("gpt-6.1-sol","model-sun"),("openai/gpt-6-astra","model-star"),("GPT 6.1 Terra High","model-earth"),("gpt-6-luna","model-moon")]{assert_eq!(model_glyph(model),Some(glyph));assert!(crate::svg_assets::asset(glyph).is_some());}assert_eq!(model_glyph("claude-sol"),None);assert_eq!(model_glyph("gpt-solution"),None);}
 #[test]fn intensity_is_bounded_and_stop_positions_are_model_relative(){assert_eq!(intensity(-1.),0xb6dfff);assert_eq!(intensity(2.),0xff5565);let bounds=Bounds::new(point(px(0.),px(0.)),size(px(244.),px(32.)));assert_eq!(index_at(bounds,point(px(122.),px(16.)),3),Some(1));assert_eq!(index_at(bounds,point(px(999.),px(16.)),7),Some(6));}
}

#[cfg(test)]mod ui_regressions{
 use super::*;use std::prelude::v1::test;
 #[gpui::test]fn permission_badge_is_independent_atomic_and_owned(cx:&mut TestAppContext){
  let (view,cx)=crate::ui_tests::fixture(cx);
  cx.update(|window,cx|view.update(cx,|app,cx|{
   app.agent=AgentKind::Codex;app.allow_edits=false;
   let mut body=serde_json::json!({"action":"workspace_permissions","ownerToken":app.unified_token(),"access":"full"});app.unified_command(&body,window,cx).unwrap();
   assert!(app.allow_edits);assert_eq!(app.codex_access,runtime::CodexAccess::Full);assert_eq!(app.codex_approvals,runtime::CodexApprovals::Ask);
   body["access"]=serde_json::Value::Null;body["approvals"]=serde_json::json!("never");app.unified_command(&body,window,cx).unwrap();assert_eq!(app.codex_access,runtime::CodexAccess::Full);
   body["access"]=serde_json::json!("read-only");body["approvals"]=serde_json::json!("invalid");assert!(app.unified_command(&body,window,cx).is_err());assert!(app.allow_edits);
   body["ownerToken"]=serde_json::json!("stale");body["approvals"]=serde_json::json!("ask");assert!(app.unified_command(&body,window,cx).is_err());assert_eq!(app.codex_approvals,runtime::CodexApprovals::Never);
   let snapshot=app.unified_snapshot(cx);assert_eq!(snapshot["workspaceAccess"],"full");assert_eq!(snapshot["workspaceApprovalPolicy"],"never");
  }));
 }
 #[gpui::test]fn model_specific_thinking_choices_preserve_draft(cx:&mut TestAppContext){
  let (view,cx)=crate::ui_tests::fixture(cx);
  cx.update(|window,cx|view.update(cx,|app,cx|{
   app.agent=AgentKind::Codex;app.model=Some("gpt-6.1-sol".into());app.native_models.insert(AgentKind::Codex,vec![agent_controls::ModelChoice{id:"gpt-6.1-sol".into(),label:"Sol".into(),reasoning_efforts:vec!["low".into(),"medium".into(),"high".into()]}]);app.composer.update(cx,|input,cx|input.set_text("Keep this draft",cx));app.toggle_agent_picker(point(px(700.),px(640.)),window,cx);
  }));
  crate::ui_tests::draw_frame(&view,cx);let bounds=cx.debug_bounds("reasoning-high").expect("model-specific thinking choice");cx.simulate_click(bounds.center(),Modifiers::none());
  view.update(cx,|app,cx|{assert_eq!(app.unified_effort(),Some("high"));assert_eq!(app.request_effort().as_deref(),Some("high"));assert_eq!(app.composer.read(cx).text(),"Keep this draft");assert!(!app.select_reasoning_effort("ultra",cx));assert_eq!(app.unified_effort(),Some("high"));assert!(app.agent_picker.as_ref().is_none_or(|p|p.closing));});
 }
 #[gpui::test]fn fast_mode_requires_advertised_support_and_reaches_the_request(cx:&mut TestAppContext){
  let(view,cx)=crate::ui_tests::fixture(cx);view.update(cx,|app,cx|{
   app.agent=AgentKind::Codex;app.model=Some("fast-fixture".into());app.toggle_fast(cx);assert!(!app.fast_enabled());
   let mut info=agent_controls::ControlInfo{defaults:Default::default(),auth:agent_controls::AuthState::Unknown,models:vec![],note:None,account:None,limits:vec![],catalog:None};info.defaults.fast_models.insert("fast-fixture".into(),"priority".into());app.controls.insert("codex",(Instant::now(),info));
   app.toggle_fast(cx);assert_eq!(app.request_service_tier().as_deref(),Some("priority"));let saved=app.workspace_snapshot(cx).normalized();assert_eq!(saved.reasoning_efforts.get("codex:service-tier").map(String::as_str),Some("fast"));
   app.agent=AgentKind::Gemini;assert!(app.request_service_tier().is_none());app.agent=AgentKind::Codex;app.toggle_fast(cx);assert_eq!(app.request_service_tier().as_deref(),Some("default"));
  });
 }
 #[gpui::test]fn thinking_preview_defaults_to_selected_and_returns_after_hover(cx:&mut TestAppContext){
  let(view,cx)=crate::ui_tests::fixture(cx);
  cx.update(|window,cx|view.update(cx,|app,cx|{
   app.agent=AgentKind::Codex;app.model=Some("thinking-preview-fixture".into());app.native_models.insert(AgentKind::Codex,vec![agent_controls::ModelChoice{id:"thinking-preview-fixture".into(),label:"Preview".into(),reasoning_efforts:vec!["low".into(),"medium".into(),"high".into()]}]);let key=app.effort_key();app.reasoning_efforts.insert(key,"medium".into());app.toggle_agent_picker(point(px(650.),px(620.)),window,cx);
   assert_eq!(app.agent_picker.as_ref().unwrap().effort_hover.as_ref().map(|(v,_)|v.as_str()),Some("medium"));
  }));
  // The first event causes the manual test frame to be replaced by the window's
  // own frame; a second move tests hover against the settled window hitboxes.
  crate::ui_tests::draw_frame(&view,cx);let low=cx.debug_bounds("reasoning-low").unwrap();cx.simulate_mouse_move(low.center(),None,Modifiers::none());cx.simulate_mouse_move(low.center()+point(px(1.),px(0.)),None,Modifiers::none());crate::ui_tests::draw_frame(&view,cx);
  view.update(cx,|app,_|{assert_eq!(app.agent_picker.as_ref().unwrap().effort_hover.as_ref().map(|(v,_)|v.as_str()),Some("low"));assert_eq!(app.unified_effort(),Some("medium"));});
  let picker=cx.debug_bounds("agent-picker").unwrap();cx.simulate_mouse_move(picker.origin+point(px(2.),px(2.)),None,Modifiers::none());crate::ui_tests::draw_frame(&view,cx);
  view.update(cx,|app,_|{assert_eq!(app.agent_picker.as_ref().unwrap().effort_hover.as_ref().map(|(v,_)|v.as_str()),Some("medium"));assert_eq!(app.unified_effort(),Some("medium"));});
 }
}
