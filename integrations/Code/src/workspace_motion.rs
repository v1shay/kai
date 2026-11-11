//! One system monotonic clock for Swift contours and Code components. Interrupted
//! transitions capture each component's current progress before changing direction.
use gpui::{div,px,Div,Styled};
pub(super) const CONTROL_DURATION: std::time::Duration = std::time::Duration::from_millis(160);
pub(super) const PANE_DURATION: std::time::Duration = std::time::Duration::from_millis(220);

pub(super) use crate::presentation::ease_out;
pub(super) const CONTENT_DURATION: std::time::Duration = std::time::Duration::from_millis(320);
pub(super) fn surface_duration(closing: bool) -> std::time::Duration {
    if closing { CONTROL_DURATION } else { CONTENT_DURATION }
}

#[derive(Clone,Copy)]pub(super) enum Part{Header,Sidebar,Content,Inspector,Composer}
impl Part{fn index(self)->usize{match self{Self::Header=>0,Self::Sidebar=>1,Self::Content=>2,Self::Inspector=>3,Self::Composer=>4}}fn delay(self)->f32{[0.06,0.13,0.23,0.29,0.39][self.index()]}}
const PARTS:[Part;5]=[Part::Header,Part::Sidebar,Part::Content,Part::Inspector,Part::Composer];
pub(super) fn now()->f64{let mut t:libc::timespec=unsafe{std::mem::zeroed()};unsafe{libc::clock_gettime(libc::CLOCK_MONOTONIC_RAW,&mut t)};t.tv_sec as f64+t.tv_nsec as f64/1_000_000_000.}
#[derive(Default)]pub(super) struct Transition{started:f64,phase:Phase,from:[f32;5]}
#[derive(Clone,Copy,Default)]enum Phase{#[default] Hidden,Opening,Closing}
#[derive(Clone,Copy)]pub(super) struct Frame{elapsed:f32,phase:Phase,from:[f32;5],enabled:bool,motion:bool}
impl Transition{
 pub fn set_at(&mut self,phase:&str,started:f64){let old=self.frame(true);self.from=PARTS.map(|part|old.progress(part));self.phase=match phase{"expand"|"reopen"|"visible"=>Phase::Opening,"collapse"=>Phase::Closing,_=>Phase::Hidden};if phase=="reopen"{self.from=[0.;5];}if phase=="visible"{self.from=[1.;5];}self.started=started;}
 pub fn frame(&self,motion:bool)->Frame{Frame{elapsed:(now()-self.started).max(0.) as f32,phase:self.phase,from:self.from,enabled:true,motion}}
}
impl Frame{
 pub fn identity()->Self{Self{elapsed:1.,phase:Phase::Opening,from:[0.;5],enabled:false,motion:false}}
 pub fn animating(self)->bool{self.enabled&&self.motion&&match self.phase{Phase::Opening=>self.from!=[1.;5]&&self.elapsed<0.82,Phase::Closing=>self.from!=[0.;5]&&self.elapsed<0.35,Phase::Hidden=>false}}
 fn progress(self,part:Part)->f32{
  if !self.enabled{return 1.}if !self.motion{return if matches!(self.phase,Phase::Hidden){0.}else{1.}}
  let (time,target)=match self.phase{Phase::Opening=>((self.elapsed-part.delay())/0.42,1.),Phase::Closing=>((self.elapsed-(0.39-part.delay())*0.30)/0.20,0.),Phase::Hidden=>return 0.};
  let p=ease_out(time);self.from[part.index()]+(target-self.from[part.index()])*p
 }
 pub fn style(self,element:Div,part:Part)->Div{let p=self.progress(part);element.opacity(p).relative().top(px((1.-p)*14.))}
 pub fn content_visible(self)->bool{!self.enabled||self.progress(Part::Content)>0.02}
 pub fn container(self,part:Part)->Div{self.style(div(),part)}
}
#[cfg(test)]mod tests{
 use super::*;
 #[test]fn opening_arrives_from_header_to_composer_then_stops(){let mut f=Frame{elapsed:0.2,phase:Phase::Opening,from:[0.;5],enabled:true,motion:true};assert!(f.progress(Part::Header)>f.progress(Part::Content));assert_eq!(f.progress(Part::Composer),0.);f.elapsed=0.84;assert_eq!(f.progress(Part::Composer),1.);assert!(!f.animating());}
 #[test]fn dismissal_retires_composer_before_header(){let f=Frame{elapsed:0.08,phase:Phase::Closing,from:[1.;5],enabled:true,motion:true};assert!(f.progress(Part::Composer)<f.progress(Part::Header));}
 #[test]fn reversal_starts_at_each_components_current_progress(){let mut t=Transition::default();t.set_at("expand",now()-0.3);let before=t.frame(true);t.set_at("collapse",now());let after=t.frame(true);for p in PARTS{assert!((after.progress(p)-before.progress(p)).abs()<0.001);}}
 #[test]fn receiver_delay_uses_origin_clock(){let mut t=Transition::default();t.set_at("expand",now()-1.);assert!(!t.frame(true).animating());assert_eq!(t.frame(true).progress(Part::Composer),1.);}
 #[test]fn retained_workspace_is_full_and_reopening_replays_component_entrances(){let mut t=Transition::default();t.set_at("visible",now());assert!(!t.frame(true).animating());for p in PARTS{assert_eq!(t.frame(true).progress(p),1.);}t.set_at("reopen",now());for p in PARTS{assert_eq!(t.frame(true).progress(p),0.);}assert!(t.frame(true).animating());}
 #[test]fn reduced_motion_and_standalone_have_no_choreography_loop(){let mut f=Frame::identity();assert!(!f.animating());assert_eq!(f.progress(Part::Content),1.);f.enabled=true;f.phase=Phase::Hidden;assert_eq!(f.progress(Part::Header),0.);assert!(!f.animating());}
}
