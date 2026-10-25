//! Finite choreography for real workspace components. Layout remains fixed;
//! only opacity/paint offsets change, and idle surfaces schedule no frames.
use gpui::{div,px,Div,Styled};
use std::time::Instant;
#[derive(Clone,Copy)]
pub(super) enum Part{Header,Sidebar,Content,Inspector,Composer}
impl Part{fn delay(self)->f32{match self{Self::Header=>0.06,Self::Sidebar=>0.13,Self::Content=>0.23,Self::Inspector=>0.29,Self::Composer=>0.39}}}
#[derive(Default)]
pub(super) struct Transition{started:Option<Instant>,phase:Phase}
#[derive(Clone,Copy,Default)]enum Phase{#[default] Hidden,Opening,Closing}
#[derive(Clone,Copy)]pub(super) struct Frame{elapsed:f32,phase:Phase,enabled:bool,motion:bool}
impl Transition{
 pub fn set(&mut self,phase:&str){self.phase=match phase{"expand"=>Phase::Opening,"collapse"=>Phase::Closing,_=>Phase::Hidden};self.started=Some(Instant::now());}
 pub fn frame(&self,motion:bool)->Frame{Frame{elapsed:self.started.map_or(1.,|t|t.elapsed().as_secs_f32()),phase:self.phase,enabled:true,motion}}
}
impl Frame{
 pub fn identity()->Self{Self{elapsed:1.,phase:Phase::Opening,enabled:false,motion:false}}
 pub fn animating(self)->bool{self.enabled&&self.motion&&match self.phase{Phase::Opening=>self.elapsed<0.82,Phase::Closing=>self.elapsed<0.35,Phase::Hidden=>false}}
 fn progress(self,part:Part)->f32{
  if !self.enabled{return 1.}
  if !self.motion{return if matches!(self.phase,Phase::Hidden){0.}else{1.}}
  let (time,exit)=match self.phase{Phase::Opening=>((self.elapsed-part.delay())/0.42,false),Phase::Closing=>((self.elapsed-(0.39-part.delay())*0.30)/0.20,true),Phase::Hidden=>return 0.};
  let t=time.clamp(0.,1.);let p=1.-(1.-t).powi(3);if exit{1.-p}else{p}
 }
 pub fn style(self,element:Div,part:Part)->Div{let p=self.progress(part);element.opacity(p).relative().top(px((1.-p)*14.))}
 pub fn content_visible(self)->bool{!self.enabled||self.progress(Part::Content)>0.02}
 pub fn container(self,part:Part)->Div{self.style(div(),part)}
}
#[cfg(test)]mod tests{
 use super::*;
 #[test]fn opening_arrives_from_header_to_composer_then_stops(){let mut f=Frame{elapsed:0.2,phase:Phase::Opening,enabled:true,motion:true};assert!(f.progress(Part::Header)>f.progress(Part::Content));assert_eq!(f.progress(Part::Composer),0.);f.elapsed=0.84;assert_eq!(f.progress(Part::Composer),1.);assert!(!f.animating());}
 #[test]fn dismissal_retires_composer_before_header(){let f=Frame{elapsed:0.08,phase:Phase::Closing,enabled:true,motion:true};assert!(f.progress(Part::Composer)<f.progress(Part::Header));}
 #[test]fn reduced_motion_and_standalone_have_no_choreography_loop(){let mut f=Frame::identity();assert!(!f.animating());assert_eq!(f.progress(Part::Content),1.);f.enabled=true;f.phase=Phase::Hidden;assert_eq!(f.progress(Part::Header),0.);assert!(!f.animating());}
}
