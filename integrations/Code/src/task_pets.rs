//! Native Kai assignments, reused by inline child activity and the composer.
use super::*;
#[derive(Clone,Debug,PartialEq,serde::Deserialize)]
pub(super) struct Visual {
    pub path:PathBuf,pub rows:usize,pub row:usize,pub columns:Vec<usize>,
    #[serde(rename="durationsMs")]pub durations:Vec<u64>,pub colors:Vec<u32>,
}
#[cfg(test)]mod tests{
    use super::*;use std::prelude::v1::test;
    #[test]fn sprite_timing_and_validation_keep_exact_native_pet_frames(){
        let mut pet=Visual{path:"/tmp/pet.png".into(),rows:9,row:3,columns:vec![2,4,7],durations:vec![100,200,100],colors:vec![0xf0c080,0xa07040,0x201008]};assert!(pet.valid());assert_eq!(pet.column(0.),2);assert_eq!(pet.column(0.3),4);assert_eq!(pet.column(0.8),7);pet.columns[0]=8;assert!(!pet.valid());pet.columns[0]=2;pet.durations.pop();assert!(!pet.valid());
    }
    #[gpui::test]fn owned_children_in_worktrees_keep_inline_pets_in_activity_and_the_viewer(cx:&mut TestAppContext){
        let(view,cx)=crate::ui_tests::fixture(cx);
        let catalog:serde_json::Value=serde_json::from_str(include_str!("../../../kai_pets/animation_catalog.json")).unwrap();let path=PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../kai_pets").join(catalog["pets"][0]["spritesheet"].as_str().unwrap());
        let mut keys=vec![];
        cx.update(|window,cx|view.update(cx,|app,cx|{
            app.active=Some("fixture-session".into());app.loading=false;app.sessions[0].external_id=Some("native-parent".into());app.finalizing.insert("fixture-session".into());
            app.rows=vec![Row{role:Some("user".into()),block:MessageBlock::Paragraph("Use two agents".into())}];
            for n in 0..2{let child=runtime::broker::ChildSnapshot{name:None,scope:None,task:None,model:None,effort:None,workspace:None,provider:AgentKind::Codex,project:app.project.join(format!("worktree-{n}")),parent_id:"native-parent".into(),id:format!("native-child-{n}"),turn_id:Some(format!("turn-{n}")),status:chat_interactions::InteractionStatus::Working,preview:"Working on its task".into(),requests:vec![],transcript_bytes:0,transcript_revision:0,connected:true};
                assert_eq!(app.child_owner(&child,&[]).unwrap().id,"fixture-session");let key=unified::child_id(&child);keys.push(key.clone());app.task_pet_visuals.insert(key,Visual{path:path.clone(),rows:catalog["pets"][0]["grid"]["rows"].as_u64().unwrap()as usize,row:n,columns:vec![0],durations:vec![100],colors:vec![0xabcdef]});
                app.rows.push(Row{role:Some("assistant:codex".into()),block:MessageBlock::Interaction(chat_interactions::ChatInteraction::Delegation{title:"Subagent working".into(),details:"Its own task".into(),status:chat_interactions::InteractionStatus::Working,agent:Some(child.id.clone())})});app.unified_children.push(child);
            }
            app.toggle_children(window,cx);app.broker_panel.as_mut().unwrap().children=app.unified_children.clone();app.list.reset(app.rows.len());cx.notify();
        }));
        crate::ui_tests::draw_frame(&view,cx);
        for key in &keys{assert!(cx.debug_bounds(Box::leak(format!("inline-task-pet-{key}").into_boxed_str())).is_some(),"A running child lost its inline pet")}
        view.update(cx,|app,cx|{let state=app.unified_snapshot(cx);let history=state["history"].as_array().unwrap();for key in keys{assert!(history.iter().any(|m|m["childId"]==key),"The extended notch lost the child's pet identity");}});
    }
}
impl Visual {
    pub fn valid(&self)->bool{self.path.is_absolute()&&self.rows>0&&self.rows<=64&&self.row<self.rows&&!self.columns.is_empty()&&self.columns.len()<=128&&self.columns.len()==self.durations.len()&&self.columns.iter().all(|c|*c<8)&&self.durations.iter().all(|t|*t>=16&&*t<=60_000)&&!self.colors.is_empty()&&self.colors.len()<=8&&self.colors.iter().all(|c|*c<=0xffffff)}
    fn column(&self,fraction:f32)->usize{let total=self.durations.iter().sum::<u64>();let mut at=(fraction.clamp(0.,0.999999)*total as f32)as u64;for(i,duration)in self.durations.iter().enumerate(){if at<*duration{return self.columns[i]}at-=duration;}self.columns[0]}
}
impl Orbit {
    pub(super) fn task_pet(&self,id:&str,size:f32,moving:bool)->Option<AnyElement>{
        let visual=self.task_pet_visuals.get(id)?.clone();
        let sheet=img(visual.path.clone()).absolute().w(px(size*8.)).h(px(size*visual.rows as f32)).top(px(-(visual.row as f32)*size));
        let sheet=if moving&&self.motion&&!native::reduced_motion(){let key=SharedString::from(format!("{id}:{}:{}",visual.path.display(),visual.row));let duration=Duration::from_millis(visual.durations.iter().sum());sheet.with_animation(key,Animation::new(duration).repeat(),move|s,t|s.left(px(-(visual.column(t)as f32)*size))).into_any_element()}else{sheet.left(px(-(visual.columns[0]as f32)*size)).into_any_element()};
        let label=format!("inline-task-pet-{id}");Some(div().debug_selector(move||label.clone()).size(px(size)).flex_none().relative().overflow_hidden().child(sheet).into_any_element())
    }
    pub(super) fn send_symbol(&self,stopping:bool)->AnyElement{
        let colors=self.task_pet_visuals.get("selected").map(|v|v.colors.clone()).unwrap_or_else(||vec![0xb7c8dc,0x75869c,0x303744]);
        let radial=canvas(|bounds,_,_|bounds,move|bounds,_,window,_|{let radius=bounds.size.width/2.;for i in (1..=32).rev(){let t=i as f32/32.;let(a,b,f)=if t<0.52{(colors[0],colors[colors.len()/2],t/0.52)}else{(colors[colors.len()/2],colors[colors.len()-1],(t-0.52)/0.48)};let ch=|n:u32|{let x=((a>>n)&255u32)as f32;let y=((b>>n)&255u32)as f32;(x+(y-x)*f)as u32};let color=(ch(16)<<16)|(ch(8)<<8)|ch(0);let r=radius*t;window.paint_quad(fill(Bounds::new(bounds.center()-point(r,r),size(r*2.,r*2.)),rgb(color)).corner_radii(r));}}).absolute().inset_0();
        div().size(px(32.)).relative().flex().items_center().justify_center().rounded_full().overflow_hidden().border_1().border_color(rgba(0xffffff5c))
            .child(radial)
            .child(div().absolute().inset_0().rounded_full().bg(linear_gradient(135.,linear_color_stop(rgba(0xffffff30),0.),linear_color_stop(rgba(0x00000040),1.))))
            .child(icon(if stopping{"stop"}else{"notch-send"},if stopping{13.}else{25.},0xffffff)).into_any_element()
    }
}
