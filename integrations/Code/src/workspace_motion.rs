//! One system monotonic clock for Swift contours and Code components. Interrupted
//! transitions capture each component's current progress before changing direction.
use gpui::{div,px,Div,Styled};
pub(super) const CONTROL_DURATION: std::time::Duration = std::time::Duration::from_millis(160);
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaneProfile {
    Quick,
    #[default]
    Standard,
    Relaxed,
}

impl PaneProfile {
    pub const ALL: [Self; 3] = [Self::Quick, Self::Standard, Self::Relaxed];

    pub fn label(self) -> &'static str {
        match self {
            Self::Quick => "Quick",
            Self::Standard => "Standard",
            Self::Relaxed => "Relaxed",
        }
    }

    pub fn millis(self) -> u64 {
        match self {
            Self::Quick => 160,
            Self::Standard => 220,
            Self::Relaxed => 300,
        }
    }
}

static PANE_MILLIS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(220);

pub(super) fn pane_duration() -> std::time::Duration {
    std::time::Duration::from_millis(PANE_MILLIS.load(std::sync::atomic::Ordering::Relaxed))
}

pub(super) fn set_pane_profile(profile: PaneProfile) {
    let millis = profile.millis();
    PANE_MILLIS.store(millis, std::sync::atomic::Ordering::Relaxed);
    crate::unified::emit(&serde_json::json!({"type": "paneMotion", "durationMs": millis}));
}

pub(super) use crate::presentation::ease_out;
pub(super) const CONTENT_DURATION: std::time::Duration = std::time::Duration::from_millis(320);
pub(super) fn surface_duration(closing: bool) -> std::time::Duration {
    if closing { CONTROL_DURATION } else { CONTENT_DURATION }
}

#[derive(Clone,Copy)]pub(super) enum Part{Header,Sidebar,Content,Inspector,Composer}
impl Part{fn index(self)->usize{match self{Self::Header=>0,Self::Sidebar=>1,Self::Content=>2,Self::Inspector=>3,Self::Composer=>4}}fn delay(self)->f32{[0.06,0.13,0.23,0.29,0.39][self.index()]}}
const PARTS:[Part;5]=[Part::Header,Part::Sidebar,Part::Content,Part::Inspector,Part::Composer];
pub(super) fn now()->f64{let mut t:libc::timespec=unsafe{std::mem::zeroed()};unsafe{libc::clock_gettime(libc::CLOCK_MONOTONIC_RAW,&mut t)};t.tv_sec as f64+t.tv_nsec as f64/1_000_000_000.}
#[derive(Default)]
pub(super) struct Transition {
    started: f64,
    wallpaper_started: f64,
    phase: Phase,
    from: [f32; 5],
}
#[derive(Clone, Copy, Default)]
enum Phase {
    #[default]
    Hidden,
    Opening,
    Closing,
}
#[derive(Clone, Copy)]
pub(super) struct Frame {
    started: f64,
    elapsed: f32,
    phase: Phase,
    from: [f32; 5],
    enabled: bool,
    motion: bool,
}
impl Transition {
    pub fn set_at(&mut self, phase: &str, started: f64) {
        let continuing_wallpaper = phase == "visible" && matches!(self.phase, Phase::Opening);
        if matches!(phase, "expand" | "reopen" | "visible") && !continuing_wallpaper {
            self.wallpaper_started = started;
        }
        let old = self.frame(true);
        self.from = PARTS.map(|part| old.progress(part));
        self.phase = match phase {
            "expand" | "reopen" | "visible" => Phase::Opening,
            "collapse" => Phase::Closing,
            _ => Phase::Hidden,
        };
        if phase == "reopen" {
            self.from = [0.; 5];
        }
        if phase == "visible" {
            self.from = [1.; 5];
        }
        self.started = started;
    }
    pub fn frame(&self, motion: bool) -> Frame {
        Frame {
            started: self.wallpaper_started,
            elapsed: (now() - self.started).max(0.) as f32,
            phase: self.phase,
            from: self.from,
            enabled: true,
            motion,
        }
    }
}
impl Frame{
    pub fn identity() -> Self {
        Self {
            started: 0.,
            elapsed: 1.,
            phase: Phase::Opening,
            from: [0.; 5],
            enabled: false,
            motion: false,
        }
    }
 pub fn animating(self)->bool{self.enabled&&self.motion&&match self.phase{Phase::Opening=>self.from!=[1.;5]&&self.elapsed<0.82,Phase::Closing=>self.from!=[0.;5]&&self.elapsed<0.35,Phase::Hidden=>false}}
 fn progress(self,part:Part)->f32{
  if !self.enabled{return 1.}if !self.motion{return if matches!(self.phase,Phase::Hidden){0.}else{1.}}
  let (time,target)=match self.phase{Phase::Opening=>((self.elapsed-part.delay())/0.42,1.),Phase::Closing=>((self.elapsed-(0.39-part.delay())*0.30)/0.20,0.),Phase::Hidden=>return 0.};
  let p=ease_out(time);self.from[part.index()]+(target-self.from[part.index()])*p
 }
 pub fn style(self,element:Div,part:Part)->Div{let p=self.progress(part);element.opacity(p).relative().top(px((1.-p)*14.))}
    /// Wallpaper has its own arrival clock. Complete initial component styles
    /// still begin an artwork opening, and a late image starts when paint can use it.
    pub fn wallpaper_opening(self) -> Option<(f64, bool)> {
        (self.enabled && matches!(self.phase, Phase::Opening))
            .then_some((self.started, self.motion))
    }
    #[cfg(test)]
    pub fn wallpaper_reveal(self) -> Option<f32> {
        (self.enabled && self.motion && matches!(self.phase, Phase::Opening) && self.elapsed < 0.68)
            .then(|| ((self.elapsed - 0.08) / 0.60).clamp(0., 1.))
    }
 pub fn content_visible(self)->bool{!self.enabled||self.progress(Part::Content)>0.02}
 pub fn container(self,part:Part)->Div{self.style(div(),part)}
}
#[cfg(test)]mod tests{
 use super::*;
 #[test]fn opening_arrives_from_header_to_composer_then_stops(){let mut f=Frame{started:0.,elapsed:0.2,phase:Phase::Opening,from:[0.;5],enabled:true,motion:true};assert!(f.progress(Part::Header)>f.progress(Part::Content));assert_eq!(f.progress(Part::Composer),0.);f.elapsed=0.84;assert_eq!(f.progress(Part::Composer),1.);assert!(!f.animating());}
 #[test]fn dismissal_retires_composer_before_header(){let f=Frame{started:0.,elapsed:0.08,phase:Phase::Closing,from:[1.;5],enabled:true,motion:true};assert!(f.progress(Part::Composer)<f.progress(Part::Header));}
 #[test]fn reversal_starts_at_each_components_current_progress(){let mut t=Transition::default();t.set_at("expand",now()-0.3);let before=t.frame(true);t.set_at("collapse",now());let after=t.frame(true);for p in PARTS{assert!((after.progress(p)-before.progress(p)).abs()<0.001);}}
 #[test]fn receiver_delay_uses_origin_clock(){let mut t=Transition::default();t.set_at("expand",now()-1.);assert!(!t.frame(true).animating());assert_eq!(t.frame(true).progress(Part::Composer),1.);}
 #[test]fn retained_workspace_is_full_and_reopening_replays_component_entrances(){let mut t=Transition::default();t.set_at("visible",now());assert!(!t.frame(true).animating());for p in PARTS{assert_eq!(t.frame(true).progress(p),1.);}t.set_at("reopen",now());for p in PARTS{assert_eq!(t.frame(true).progress(p),0.);}assert!(t.frame(true).animating());}
 #[test]fn reduced_motion_and_standalone_have_no_choreography_loop(){let mut f=Frame::identity();assert!(!f.animating());assert_eq!(f.progress(Part::Content),1.);f.enabled=true;f.phase=Phase::Hidden;assert_eq!(f.progress(Part::Header),0.);assert!(!f.animating());}
}

#[cfg(test)]
mod animation_visibility_tests {
    use gpui::{prelude::*, *};
    use std::{cell::Cell, rc::Rc, time::Duration};
    use std::prelude::v1::test;

    struct ProbeView { x: Pixels, repeat: bool, scheduled: Rc<Cell<bool>> }
    struct ProbeElement { inner: AnimationElement<Div>, scheduled: Rc<Cell<bool>> }
    impl IntoElement for ProbeElement { type Element = Self; fn into_element(self) -> Self { self } }
    impl Render for ProbeView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().relative().overflow_hidden().child(ProbeElement {
                inner: div().size(px(20.)).absolute().left(self.x).with_animation(
                    "visibility-probe", if self.repeat { Animation::new(Duration::from_secs(1)).repeat() }
                    else { Animation::new(Duration::from_secs(1)) }, |element, _| element,
                ),
                scheduled: self.scheduled.clone(),
            })
        }
    }
    impl Element for ProbeElement {
        type RequestLayoutState = <AnimationElement<Div> as Element>::RequestLayoutState;
        type PrepaintState = bool;
        fn id(&self) -> Option<ElementId> { self.inner.id() }
        fn source_location(&self) -> Option<&'static std::panic::Location<'static>> { None }
        fn request_layout(&mut self, id: Option<&GlobalElementId>, inspector: Option<&InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, Self::RequestLayoutState) {
            self.inner.request_layout(id, inspector, window, cx)
        }
        fn prepaint(&mut self, id: Option<&GlobalElementId>, inspector: Option<&InspectorElementId>, bounds: Bounds<Pixels>, state: &mut Self::RequestLayoutState, window: &mut Window, cx: &mut App) -> bool {
            let scheduled = self.inner.prepaint(id, inspector, bounds, state, window, cx);
            self.scheduled.set(scheduled);
            scheduled
        }
        fn paint(&mut self, id: Option<&GlobalElementId>, inspector: Option<&InspectorElementId>, bounds: Bounds<Pixels>, state: &mut Self::RequestLayoutState, prepaint: &mut bool, window: &mut Window, cx: &mut App) {
            self.inner.paint(id, inspector, bounds, state, prepaint, window, cx);
        }
    }
    #[gpui::test]
    fn clipped_loops_park_but_partial_and_finite_entrances_continue(cx: &mut TestAppContext) {
        let scheduled = Rc::new(Cell::new(false));
        let initial = scheduled.clone();
        let (view, cx) = cx.add_window_view(move |_, _| ProbeView { x: px(10.), repeat: true, scheduled: initial });
        let width = cx.update(|window, _| window.viewport_size().width);
        for (x, repeat, expected) in [(px(10.), true, true), (width - px(5.), true, true),
            (width + px(10.), true, false), (px(-30.), true, false), (width + px(10.), false, true)] {
            view.update(cx, |view, cx| { view.x = x; view.repeat = repeat; cx.notify(); });
            cx.draw(point(px(0.), px(0.)), size(width, px(20.)), |_, _| view.clone().into_any_element());
            assert_eq!(scheduled.get(), expected, "x={x:?}, repeat={repeat}");
        }
    }
}

#[cfg(test)]
mod wallpaper_opening_tests {
    use super::*;
    #[test]
    fn cold_primed_components_still_have_an_independent_wallpaper_opening() {
        let mut transition = Transition::default();
        transition.set_at("visible", now());
        let frame = transition.frame(true);
        assert!(!frame.animating());
        for part in PARTS {
            assert_eq!(frame.progress(part), 1.);
        }
        assert!(
            frame.wallpaper_reveal().is_some(),
            "Cold complete styles suppressed the wallpaper intro"
        );
        assert!(frame.wallpaper_opening().is_some());
    }
    #[test]
    fn visible_acknowledgement_keeps_the_current_wallpaper_opening_epoch() {
        let mut transition = Transition::default();
        let origin = now() - 0.3;
        transition.set_at("reopen", origin);
        transition.set_at("visible", now());
        let frame = transition.frame(true);
        assert!(!frame.animating());
        assert_eq!(frame.wallpaper_opening(), Some((origin, true)));
        transition.set_at("hidden", now());
        let cold = now();
        transition.set_at("visible", cold);
        assert_eq!(
            transition.frame(true).wallpaper_opening(),
            Some((cold, true))
        );
    }
    #[test]
    fn late_wallpaper_keeps_the_opening_identity_after_components_finish() {
        let mut transition = Transition::default();
        let origin = now() - 3.;
        transition.set_at("visible", origin);
        assert!(!transition.frame(true).animating());
        assert_eq!(
            transition.frame(true).wallpaper_opening(),
            Some((origin, true))
        );
        assert_eq!(
            transition.frame(false).wallpaper_opening(),
            Some((origin, false))
        );
        transition.set_at("collapse", now());
        assert_eq!(transition.frame(true).wallpaper_opening(), None);
        transition.set_at("reopen", now());
        assert!(transition.frame(true).wallpaper_opening().is_some());
    }
}
