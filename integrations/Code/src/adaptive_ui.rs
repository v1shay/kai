//! Per-panel glyph filters. This module never paints a background or changes a theme.
use super::*;
use std::sync::Arc;
use adaptive_text::{Palette, Profile, channels, composite};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
};
#[derive(Default)]
pub(super) struct State {
    pub core: adaptive_text::State,
    configured: Option<adaptive_text::Settings>,
    base: Option<(theme::ResolvedTheme, appearance::Appearance)>,
    wallpaper: Option<gpui::ImageId>,
    invalidated: Option<Instant>,
    frozen: HashMap<WindowId, bool>,
}
thread_local! {static REGIONS:RefCell<HashMap<String,([f32;4],u64)>>=RefCell::new(HashMap::new());static SEQUENCE:Cell<u64>=const{Cell::new(0)};static VISIBLE:RefCell<HashMap<WindowId,(HashSet<String>,HashSet<String>)>>=RefCell::new(HashMap::new());}
fn key(role: &str, w: &Window) -> String {
    format!("{:?}/{role}", w.window_handle().window_id())
}
pub(super) struct Plan {
    enabled: bool,
    id: String,
    role: &'static str,
    filter: Option<GlyphColorFilter>,
}
fn filter(p: Palette) -> GlyphColorFilter {
    let cache = RefCell::new(HashMap::<[u32; 4], Hsla>::new());
    Arc::new(move |color: Hsla| {
        let key = [
            color.h.to_bits(),
            color.s.to_bits(),
            color.l.to_bits(),
            color.a.to_bits(),
        ];
        let mut cache = cache.borrow_mut();
        if let Some(value) = cache.get(&key) {
            return *value;
        }
        let next = p.filter(color);
        if cache.len() >= 128 {
            cache.clear();
        }
        cache.insert(key, next);
        next
    })
}
impl Plan {
    pub fn wrap(self, child: AnyElement) -> AnyElement {
        if self.enabled {
            Panel { child, plan: self }.into_any_element()
        } else {
            child
        }
    }
}
macro_rules! panel {
    ($app:expr,$role:expr,$w:expr,$body:expr) => {{
        let plan = $app.adaptive_plan($role, $w);
        plan.wrap($body.into_any_element())
    }};
}
pub(super) use panel;
struct Panel {
    child: AnyElement,
    plan: Plan,
}
impl IntoElement for Panel {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for Panel {
    type RequestLayoutState = ();
    type PrepaintState = ();
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        w: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(w, cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        w: &mut Window,
        cx: &mut App,
    ) {
        let visible = bounds.intersect(&w.content_mask().bounds);
        if (unified::active() || cfg!(test))
            && visible.size.width > px(2.)
            && visible.size.height > px(2.)
        {
            VISIBLE.with(|all| {
                all.borrow_mut()
                    .entry(w.window_handle().window_id())
                    .or_default()
                    .0
                    .insert(self.plan.id.clone());
            });
            let rect:[f32;4] = [
                bounds.origin.x.into(),
                bounds.origin.y.into(),
                bounds.size.width.into(),
                bounds.size.height.into(),
            ];
            let generation = REGIONS.with(|regions| {
                let mut regions = regions.borrow_mut();
                if regions
                    .get(&self.plan.id)
                    .is_some_and(|(old, _)| old.iter().zip(rect).all(|(a, b)| (*a - b).abs() < 1.))
                {
                    return None;
                }
                let n = SEQUENCE.with(|v| {
                    let next = v.get() + 1;
                    v.set(next);
                    next
                });
                if regions.len() >= 32 && !regions.contains_key(&self.plan.id) {
                    regions.clear();
                }
                regions.insert(self.plan.id.clone(), (rect, n));
                Some(n)
            });
            if let Some(generation) = generation {
                unified::emit(
                    &serde_json::json!({"type":"adaptiveTextRegion","id":self.plan.id,"role":self.plan.role,"window":native::window_number(w),"rect":rect,"generation":generation}),
                );
            }
        }
        w.with_glyph_color_filter(self.plan.filter.clone(), |w| {
            self.child.prepaint(w, cx);
        });
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        w: &mut Window,
        cx: &mut App,
    ) {
        w.with_glyph_color_filter(self.plan.filter.clone(), |w| self.child.paint(w, cx));
    }
}
fn over(rgba: u32, back: [f32; 3]) -> [f32; 3] {
    composite(channels(rgba >> 8), (rgba & 255) as f32 / 255., back)
}
impl Orbit {
    pub(super) fn tick_adaptive(&mut self, w: &mut Window, frozen: bool, cx: &mut Context<Self>) {
        if !unified::active() && !cfg!(test) {
            return;
        }
        let settings = self.theme.adaptive_text.normalized();
        if self.adaptive.configured.as_ref() != Some(&settings) {
            let profiles = if settings.enabled {
                self.adaptive.core.profiles.values().cloned().collect()
            } else {
                Vec::new()
            };
            self.adaptive.configured = Some(settings.clone());
            self.adaptive.core.clear();
            self.adaptive.base = None;
            self.adaptive.frozen.clear();
            REGIONS.with(|v| v.borrow_mut().clear());
            VISIBLE.with(|v| v.borrow_mut().clear());
            unified::emit(
                &serde_json::json!({"type":"adaptiveTextConfigure","enabled":settings.enabled,"settleMs":settings.settle_ms}),
            );
            if settings.enabled {
                self.accept_adaptive(profiles, None, cx);
            }
            cx.refresh_windows();
        }
        if !settings.enabled {
            return;
        }
        let id = w.window_handle().window_id();
        VISIBLE.with(|all| {
            let mut all = all.borrow_mut();
            if all.len() >= 16 && !all.contains_key(&id) {
                all.clear();
            }
            all.entry(id).or_default().0.clear();
        });
        w.on_next_frame(move|w,_|VISIBLE.with(|all|{if let Some((seen,old))=all.borrow_mut().get_mut(&id){if seen!=old{unified::emit(&serde_json::json!({"type":"adaptiveTextVisible","window":native::window_number(w),"ids":seen}));*old=seen.clone();}}}));
        let base = (self.resolved_theme.clone(), self.appearance);
        let art = self.background.read(cx).adaptive_frame_id();
        if self.adaptive.base.as_ref() != Some(&base)
            || (art != self.adaptive.wallpaper
                && self
                    .adaptive
                    .invalidated
                    .is_none_or(|at| at.elapsed() > Duration::from_millis(450)))
        {
            self.adaptive.base = Some(base);
            self.adaptive.wallpaper = art;
            self.adaptive.invalidated = Some(Instant::now());
            let profiles = self.adaptive.core.profiles.values().cloned().collect();
            self.accept_adaptive(profiles, None, cx);
            unified::emit(&serde_json::json!({"type":"adaptiveTextInvalidate"}));
        }
        if self.adaptive.frozen.len() > 16 {
            self.adaptive.frozen.clear();
        }
        if self.adaptive.frozen.insert(id, frozen) != Some(frozen) {
            unified::emit(
                &serde_json::json!({"type":"adaptiveTextFreeze","window":native::window_number(w),"frozen":frozen}),
            );
        }
        let prefix = format!("{id:?}/");
        let now = Instant::now();
        if self
            .adaptive
            .core
            .profiles
            .keys()
            .any(|id| id.starts_with(&prefix) && self.adaptive.core.animating(id, now))
        {
            // Invalidate cached child glyph scenes for this finite color tween.
            // Do not notify the agent owner or republish conversation state.
            w.on_next_frame(|_, cx| cx.refresh_windows());
        }
    }
    pub(super) fn adaptive_plan(&self, role: &'static str, w: &Window) -> Plan {
        let enabled = self.theme.adaptive_text.enabled;
        let id = if enabled { key(role, w) } else { String::new() };
        let palette = enabled
            .then(|| self.adaptive.core.palette(&id, Instant::now()))
            .flatten();
        Plan {
            enabled,
            id,
            role,
            filter: palette.map(filter),
        }
    }
    pub(super) fn accept_adaptive(
        &mut self,
        profiles: Vec<Profile>,
        status: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if !self.theme.adaptive_text.enabled {
            return;
        }
        let old = self.adaptive.core.status.clone();
        if let Some(status) = status {
            self.adaptive.core.status = status.chars().take(128).collect();
        }
        let mut changed = old != self.adaptive.core.status;
        let now = Instant::now();
        for p in profiles.into_iter().take(32) {
            if !adaptive_text::valid(&p) {
                continue;
            }
            let prior = self.adaptive.core.palette(&p.id, now);
            let pixels = p
                .pixels
                .iter()
                .enumerate()
                .map(|(i, value)| {
                    if p.role == "notch" {
                        return *value;
                    }
                    let local = if p.role == "sidebar" || p.role == "projects" {
                        self.resolved_theme.sidebar_rgba()
                    } else if p.role == "mini" {
                        (self.resolved_theme.background << 8)
                            | (self.appearance.tint(0x17171788) & 255)
                    } else {
                        self.resolved_theme.main_rgba()
                    };
                    let value = over(local, over(self.resolved_theme.backdrop_rgba(), *value));
                    if p.role == "chat" || p.role == "mini" {
                        let n = (p.pixels.len() as f32).sqrt().round().max(1.) as usize;
                        self.background
                            .read(cx)
                            .adaptive_sample(
                                ((i % n) as f32 + 0.5) / n as f32,
                                ((i / n) as f32 + 0.5) / n as f32,
                                p.rect[2],
                                p.rect[3],
                                self.active.is_none(),
                                p.role == "mini",
                            )
                            .map_or(value, |(rgb, a)| composite(rgb, a, value))
                    } else {
                        value
                    }
                })
                .collect::<Vec<_>>();
            let id = p.id.clone();
            let notch = p.role == "notch";
            let animated = self.motion && !native::reduced_motion();
            self.adaptive.core.update(
                p,
                &pixels,
                &self.theme.adaptive_text,
                now,
                animated && !notch,
            );
            let next = self.adaptive.core.palette(&id, now);
            changed |= prior != next || self.adaptive.core.animating(&id, now);
            if notch {
                if let Some(palette) = next {
                    unified::emit(
                        &serde_json::json!({"type":"adaptiveTextNotch","decision":{"color":palette.rgb(),"animated":animated}}),
                    );
                }
            }
        }
        if changed {
            cx.refresh_windows();
        }
    }
    pub(super) fn adaptive_settings(&self, cx: &Context<Self>) -> AnyElement {
        let status = match self.adaptive.core.status.as_str() {
            "active" => "Live · text colors only",
            "permission_required" => "Allow screen access to sample the background",
            "capture_error" => "Sampling paused · retry below",
            "paused" => "Paused while surfaces are hidden",
            _ => "Waiting for a visible surface",
        };
        div().flex().flex_col().gap_2().text_size(theme::ui_px(11.)).child(if self.theme.adaptive_text.enabled{status}else{"Off"})
   .child("Light and dark text adapt per panel with a short eased transition. Glass, wallpaper, opacity, blur, icons and pets stay unchanged.")
   .when(self.theme.adaptive_text.enabled&&matches!(self.adaptive.core.status.as_str(),"permission_required"|"capture_error"),|s|s.child(div().id("adaptive-text-permission").cursor_pointer().child("Allow local screen sampling…").on_click(cx.listener(|_,_,_,_|unified::emit(&serde_json::json!({"type":"adaptiveTextPermission"}))))))
   .child("Experimental · samples remain on this Mac. Mixed backgrounds keep one text color per panel; no protective tint is added.").into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    use std::prelude::v1::test;
    #[gpui::test]
    fn adaptive_text_never_filters_background_paint_and_preserves_layout(cx: &mut TestAppContext) {
        let (_, cx) = crate::ui_tests::fixture(cx);
        let calls = std::rc::Rc::new(Cell::new(0));
        let observed = calls.clone();
        let hook: GlyphColorFilter = Arc::new(move |_| {
            observed.set(observed.get() + 1);
            gpui::rgb(0x101318).into()
        });
        let plan = |hook: Option<GlyphColorFilter>| Plan {
            enabled: true,
            id: "test".into(),
            role: "test",
            filter: hook,
        };
        cx.draw(point(px(0.), px(0.)), size(px(200.), px(100.)), |_, _| {
            plan(Some(hook.clone())).wrap(
                div()
                    .size_full()
                    .bg(gpui::rgba(0x39587480))
                    .border_1()
                    .border_color(gpui::rgb(0xffffff))
                    .child(
                        canvas(
                            |_, _, _| {},
                            |b, _, w, _| w.paint_quad(fill(b, gpui::rgba(0xeeeedd33))),
                        )
                        .size_full(),
                    )
                    .into_any_element(),
            )
        });
        assert_eq!(
            calls.get(),
            0,
            "Text color transform touched a background/border/quad"
        );
        cx.draw(point(px(0.), px(0.)), size(px(200.), px(100.)), |_, _| {
            plan(Some(hook.clone())).wrap(
                div()
                    .id("text-contract")
                    .debug_selector(|| "text-contract".into())
                    .size_full()
                    .bg(gpui::rgba(0x39587480))
                    .child("Only text changes")
                    .into_any_element(),
            )
        });
        assert!(
            calls.get() > 0,
            "Real glyph paint did not use its text-only callback"
        );
        let bounds = cx.debug_bounds("text-contract").unwrap();
        cx.draw(point(px(0.), px(0.)), size(px(200.), px(100.)), |_, _| {
            plan(None).wrap(
                div()
                    .id("text-contract")
                    .debug_selector(|| "text-contract".into())
                    .size_full()
                    .bg(gpui::rgba(0x39587480))
                    .child("Only text changes")
                    .into_any_element(),
            )
        });
        assert_eq!(cx.debug_bounds("text-contract").unwrap(), bounds);
    }
    #[gpui::test]
    fn adaptive_text_toggle_preserves_draft_all_theme_surfaces_and_existing_foreground_tokens(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        cx.simulate_input("Keep this draft 🦀");
        view.update_in(cx,|app,w,cx|{
   let before=app.resolved_theme.clone();let original=theme::rgb(0xebebeb);app.theme.adaptive_text.enabled=true;assert_eq!(app.theme.resolve(before.dark,app.appearance),before,"Enabling adaptive text changed the resolved theme");app.tick_adaptive(w,false,cx);
   let body=serde_json::json!({"action":"adaptive_text_samples","status":"active","profiles":[{"id":key("chat",w),"role":"chat","generation":999,"rect":[0,0,600,700],"pixels":[[1,1,1]]}]});
   app.unified_command(&body,w,cx).unwrap();assert!(app.adaptive.core.palette(&key("chat",w),Instant::now()).is_some());assert_eq!(app.resolved_theme,before);assert_eq!(theme::rgb(0xebebeb),original,"Global icon/background palette changed");assert_eq!(app.composer.read(cx).text(),"Keep this draft 🦀");
   app.theme.adaptive_text.enabled=false;app.tick_adaptive(w,false,cx);assert!(app.adaptive.core.profiles.is_empty());assert_eq!(app.resolved_theme,before);
  });
    }
    #[test]
    fn adaptive_text_preferences_cannot_change_any_glass_or_theme_value() {
        for opacity in [0., 0.3, 0.6, 1.] {
            for dark in [false, true] {
                let mut theme = theme::Theme::default();
                let a = appearance::Appearance {
                    opacity,
                    blur_radius: 24,
                };
                let before = theme.resolve(dark, a);
                theme.adaptive_text.enabled = true;
                assert_eq!(theme.resolve(dark, a), before);
            }
        }
    }
}
