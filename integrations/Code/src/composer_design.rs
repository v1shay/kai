//! Shared composer identity and discrete native reasoning controls.
use super::*;
use std::{cell::Cell,rc::Rc};

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
fn index_at(bounds:Bounds<Pixels>,position:Point<Pixels>,count:usize)->Option<usize>{
    if count==0 || f32::from(bounds.size.width)<=18.{return None}
    let fraction=((f32::from(position.x-bounds.origin.x)-9.)/(f32::from(bounds.size.width)-18.)).clamp(0.,1.);
    Some((fraction*(count.saturating_sub(1))as f32).round()as usize)
}
impl Orbit{
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
        const WIDTH:f32=244.;let levels=model.reasoning_efforts;let count=levels.len();
        let selected=self.unified_effort().map(str::to_owned).or_else(||self.effective_model_settings().1);
        let index=levels.iter().position(|level|Some(level)==selected.as_ref());let fraction=self.effort_fraction(count,index.unwrap_or(0));
        let focus=self.agent_picker.as_ref().map(|picker|picker.effort_focus.clone()).unwrap_or_else(||self.focus.clone());
        let down_focus=focus.clone();
        let key_levels=levels.clone();
        let bounds=Rc::new(Cell::new(Bounds::default()));let painted=bounds.clone();let down=bounds.clone();let drag=bounds.clone();
        let down_levels=levels.clone();let drag_levels=levels.clone();let fill=intensity(fraction);let start=intensity((fraction-0.4).max(0.));
        div().px_2().py_2().flex().flex_col().gap_1()
            .child(div().flex().justify_between().text_size(theme::ui_px(10.)).text_color(rgb(MUTED)).child("Thinking").child(div().text_color(rgb(fill)).child(index.map(|i|title(&levels[i])).unwrap_or_else(||"Choose a level".into()))))
            .child(div().id("reasoning-slider").debug_selector(||"reasoning-slider".into()).track_focus(&focus).w(px(WIDTH)).h(px(32.)).relative().cursor_pointer()
                .child(canvas(move|next,_,_|painted.set(next),|_,_,_,_|{}).size_full())
                .child(div().absolute().left(px(9.)).right(px(9.)).top(px(13.)).h(px(6.)).rounded_full().bg(rgba(0xffffff1c)))
                .child(div().absolute().left(px(9.)).top(px(13.)).h(px(6.)).w(px(((WIDTH-18.)*fraction).max(6.))).rounded_full().bg(linear_gradient(90.,linear_color_stop(rgb(start),0.),linear_color_stop(rgb(fill),1.))))
                .children(levels.iter().enumerate().map(|(i,_)|div().absolute().left(px(7.5+(WIDTH-18.)*i as f32/count.saturating_sub(1).max(1)as f32)).top(px(14.5)).size(px(3.)).rounded_full().bg(rgba(0xffffff88))))
                .child(div().absolute().left(px((WIDTH-18.)*fraction)).top(px(7.)).size(px(18.)).rounded_full().bg(rgb(0xf4f6ff)).border_1().border_color(rgb(fill)).shadow_md().opacity(if index.is_some(){1.}else{0.5}))
                .on_mouse_down(MouseButton::Left,cx.listener(move|this,event:&MouseDownEvent,window,cx|{cx.stop_propagation();down_focus.focus(window);if let Some(picker)=this.agent_picker.as_mut(){picker.effort_dragging=true;}if let Some(index)=index_at(down.get(),event.position,down_levels.len()){this.select_reasoning_effort(&down_levels[index],cx);}}))
                .on_mouse_move(cx.listener(move|this,event:&MouseMoveEvent,_,cx|{if event.pressed_button==Some(MouseButton::Left) && this.agent_picker.as_ref().is_some_and(|p|p.effort_dragging){if let Some(index)=index_at(drag.get(),event.position,drag_levels.len()){if this.unified_effort()!=Some(drag_levels[index].as_str()){this.select_reasoning_effort(&drag_levels[index],cx);}}}}))
                .on_mouse_up(MouseButton::Left,cx.listener(|this,_,_,_|{if let Some(picker)=this.agent_picker.as_mut(){picker.effort_dragging=false;}}))
                .on_mouse_up_out(MouseButton::Left,cx.listener(|this,_,_,_|{if let Some(picker)=this.agent_picker.as_mut(){picker.effort_dragging=false;}}))
                .on_key_down(cx.listener(move|this,event:&KeyDownEvent,_,cx|{
                    let current=this.unified_effort().map(str::to_owned).or_else(||this.effective_model_settings().1);
                    let index=key_levels.iter().position(|value|Some(value)==current.as_ref()).unwrap_or(0);
                    let target=match event.keystroke.key.as_str(){"left"|"down"=>Some(index.saturating_sub(1)),"right"|"up"=>Some((index+1).min(key_levels.len().saturating_sub(1))),"home"=>Some(0),"end"=>Some(key_levels.len().saturating_sub(1)),_=>None};
                    if let Some(index)=target{if let Some(level)=key_levels.get(index){cx.stop_propagation();this.select_reasoning_effort(level,cx);}}
                })))
            .child(div().flex().justify_between().children(levels.into_iter().enumerate().map(|(i,level)|{
                let text=title(&level);div().id(SharedString::from(format!("reasoning-{level}"))).debug_selector({let level=level.clone();move||format!("reasoning-{level}")}).cursor_pointer().text_size(theme::ui_px(9.)).text_color(rgb(if index==Some(i){fill}else{MUTED})).child(text).on_click(cx.listener(move|this,_,_,cx|{this.select_reasoning_effort(&level,cx);}))
            }))).into_any_element()
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
 #[gpui::test]fn model_specific_slider_changes_real_settings_and_preserves_draft(cx:&mut TestAppContext){
  let (view,cx)=crate::ui_tests::fixture(cx);
  cx.update(|window,cx|view.update(cx,|app,cx|{
   app.agent=AgentKind::Codex;app.model=Some("gpt-6.1-sol".into());app.native_models.insert(AgentKind::Codex,vec![agent_controls::ModelChoice{id:"gpt-6.1-sol".into(),label:"Sol".into(),reasoning_efforts:vec!["low".into(),"medium".into(),"high".into()]}]);app.composer.update(cx,|input,cx|input.set_text("Keep this draft",cx));app.toggle_agent_picker(point(px(700.),px(640.)),window,cx);
  }));
  crate::ui_tests::draw_frame(&view,cx);let bounds=cx.debug_bounds("reasoning-slider").expect("native gradient slider");cx.simulate_click(point(bounds.right()-px(9.),bounds.center().y),Modifiers::none());
  view.update(cx,|app,cx|{assert_eq!(app.unified_effort(),Some("high"));assert_eq!(app.request_effort().as_deref(),Some("high"));assert_eq!(app.composer.read(cx).text(),"Keep this draft");assert!(!app.select_reasoning_effort("ultra",cx));assert_eq!(app.unified_effort(),Some("high"));assert!(app.agent_picker.as_ref().unwrap().animating());});
 }
}
