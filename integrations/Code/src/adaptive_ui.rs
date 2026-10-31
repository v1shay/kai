//! Local palette scopes preserve GPUI's own layout, event routing and renderer.
use super::*;
use adaptive_contrast::{Profile, channels, composite};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
};
#[derive(Default)]
pub(super) struct State {
    pub core: adaptive_contrast::State,
    pub tokens: HashMap<String, theme::ResolvedTheme>,
    configured: Option<adaptive_contrast::Settings>,
    frozen: HashMap<WindowId, bool>,
    temporal: HashMap<String, ([f32; 3], Instant)>,
    base: Option<(theme::ResolvedTheme, appearance::Appearance)>,
    wallpaper: Option<gpui::ImageId>,
    invalidated: Option<Instant>,
}
thread_local! {static REGIONS:RefCell<HashMap<String,([f32;4],u64)>>=RefCell::new(HashMap::new());}
thread_local! {static REGION_SEQUENCE:Cell<u64>=const{Cell::new(0)};}
thread_local! {static VISIBLE:RefCell<HashMap<WindowId,(std::collections::HashSet<String>,std::collections::HashSet<String>)>>=RefCell::new(HashMap::new());}
pub(super) fn key(role: &str, window: &Window) -> String {
    format!("{:?}/{role}", window.window_handle().window_id())
}
pub(super) struct Plan {
    enabled: bool,
    id: String,
    role: &'static str,
    palette: Option<theme::ResolvedTheme>,
    fill: Option<u32>,
}
impl Plan {
    pub fn scope(&self) -> theme::PaletteScope {
        theme::PaletteScope::new(self.palette.as_ref())
    }
    pub fn wrap(self, child: AnyElement) -> AnyElement {
        if !self.enabled {
            return child;
        }
        ScopedPanel { child, plan: self }.into_any_element()
    }
}
macro_rules! panel {
    ($app:expr,$role:expr,$window:expr,$body:expr) => {{
        let plan = $app.adaptive_plan($role, $window);
        let _scope = plan.scope();
        let child = $body.into_any_element();
        plan.wrap(child)
    }};
}
pub(super) use panel;
struct ScopedPanel {
    child: AnyElement,
    plan: Plan,
}
impl IntoElement for ScopedPanel {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for ScopedPanel {
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
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let _scope = self.plan.scope();
        let style = self.plan.palette.as_ref().map(|p| TextStyleRefinement {
            color: Some(gpui::rgb(p.foreground).into()),
            ..Default::default()
        });
        (
            window.with_text_style(style, |window| self.child.request_layout(window, cx)),
            (),
        )
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let _scope = self.plan.scope();
        let visible = bounds.intersect(&window.content_mask().bounds);
        if (unified::active() || cfg!(test)) && self.plan.role != "terminal"
            && visible.size.width > px(2.)
            && visible.size.height > px(2.)
        {
            VISIBLE.with(|all| {
                all.borrow_mut()
                    .entry(window.window_handle().window_id())
                    .or_default()
                    .0
                    .insert(self.plan.id.clone());
            });
            let rect = [
                f32::from(bounds.origin.x),
                f32::from(bounds.origin.y),
                f32::from(bounds.size.width),
                f32::from(bounds.size.height),
            ];
            let next = REGIONS.with(|regions| {
                let mut regions = regions.borrow_mut();
                if regions
                    .get(&self.plan.id)
                    .is_some_and(|(old, _)| old.iter().zip(rect).all(|(a, b)| (*a - b).abs() < 1.))
                {
                    return None;
                }
                let generation = REGION_SEQUENCE.with(|seq| {
                    let next = seq.get().saturating_add(1);
                    seq.set(next);
                    next
                });
                if regions.len() >= 32 && !regions.contains_key(&self.plan.id) {
                    regions.clear();
                }
                regions.insert(self.plan.id.clone(), (rect, generation));
                Some(generation)
            });
            if let Some(generation) = next {
                unified::emit(
                    &serde_json::json!({"type":"adaptiveContrastRegion","id":self.plan.id,"role":self.plan.role,"window":native::window_number(window),"rect":rect,"generation":generation}),
                );
            }
        }
        let style = self.plan.palette.as_ref().map(|p| TextStyleRefinement {
            color: Some(gpui::rgb(p.foreground).into()),
            ..Default::default()
        });
        window.with_text_style(style, |window| {
            self.child.prepaint(window, cx);
        });
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let _scope = self.plan.scope();
        if let Some(fill) = self.plan.fill {
            window.paint_quad(fill_quad(bounds, gpui::rgba(fill)));
        }
        let style = self.plan.palette.as_ref().map(|p| TextStyleRefinement {
            color: Some(gpui::rgb(p.foreground).into()),
            ..Default::default()
        });
        window.with_text_style(style, |window| self.child.paint(window, cx));
    }
}
fn fill_quad(bounds: Bounds<Pixels>, color: Rgba) -> PaintQuad {
    fill(bounds, color)
}
fn over_rgba(rgba: u32, back: [f32; 3]) -> [f32; 3] {
    composite(channels(rgba >> 8), (rgba & 255) as f32 / 255., back)
}
impl Orbit {
    pub(super) fn tick_adaptive(&mut self, window: &Window, frozen: bool, cx: &mut Context<Self>) {
        // The first GPUI frame can precede native attachment. Never cache an
        // undelivered configuration; the first attached frame must send it.
        if !unified::active() && !cfg!(test){return;}
        let settings = self.theme.adaptive_contrast.normalized();
        if self.adaptive.configured.as_ref() != Some(&settings) {
            let retained = if settings.enabled
                && self.adaptive.configured.as_ref().is_some_and(|s| s.enabled)
            {
                Some((
                    self.adaptive.core.profiles.clone(),
                    self.adaptive.core.status.clone(),
                ))
            } else {
                None
            };
            self.adaptive.configured = Some(settings.clone());
            self.adaptive.core.clear();
            if let Some((profiles, status)) = retained {
                self.adaptive.core.profiles = profiles;
                self.adaptive.core.status = status;
            }
            self.adaptive.tokens.clear();
            self.adaptive.temporal.clear();
            self.adaptive.base = None;
            self.adaptive.frozen.clear();
            REGIONS.with(|regions| regions.borrow_mut().clear());
            unified::emit(
                &serde_json::json!({"type":"adaptiveContrastConfigure","enabled":settings.enabled,"target":settings.target,"settleMs":settings.settle_ms}),
            );
            cx.refresh_windows();
        }
        if settings.enabled {
            let id = window.window_handle().window_id();
            VISIBLE.with(|all| {let mut all=all.borrow_mut();if all.len()>=16 && !all.contains_key(&id){all.clear();}all.entry(id).or_default().0.clear();});
            window.on_next_frame(move |window,_|{
                VISIBLE.with(|all|{let mut all=all.borrow_mut();if let Some((seen,old))=all.get_mut(&id){if seen!=old{
                    unified::emit(&serde_json::json!({"type":"adaptiveContrastVisible","window":native::window_number(window),"ids":seen}));
                    *old=seen.clone();
                }}});
            });
            let base = (self.resolved_theme.clone(), self.appearance);
            let artwork = self.background.read(cx).adaptive_frame_id();
            let base_changed = self.adaptive.base.as_ref() != Some(&base);
            let artwork_changed = artwork != self.adaptive.wallpaper
                && self
                    .adaptive
                    .invalidated
                    .is_none_or(|at| at.elapsed() > Duration::from_millis(450));
            if base_changed || artwork_changed {
                self.adaptive.base = Some(base);
                self.adaptive.wallpaper = artwork;
                self.adaptive.invalidated = Some(Instant::now());
                let profiles = self.adaptive.core.profiles.values().cloned().collect();
                self.accept_adaptive(profiles, None, cx);
                unified::emit(&serde_json::json!({"type":"adaptiveContrastInvalidate"}));
            }
        } else {
            self.adaptive.base = None;
            self.adaptive.wallpaper = None;
        }
        if self.adaptive.frozen.len()>16{self.adaptive.frozen.clear();}
        if settings.enabled
            && self
                .adaptive
                .frozen
                .insert(window.window_handle().window_id(), frozen)
                != Some(frozen)
        {
            unified::emit(
                &serde_json::json!({"type":"adaptiveContrastFreeze","window":native::window_number(window),"frozen":frozen}),
            );
        }
    }
    pub(super) fn adaptive_plan(&self, role: &'static str, window: &Window) -> Plan {
        let enabled = self.theme.adaptive_contrast.enabled;
        if !enabled {
            return Plan {
                enabled: false,
                id: String::new(),
                role,
                palette: None,
                fill: None,
            };
        }
        let id = key(role, window);
        let palette = if !enabled {
            None
        } else if role == "terminal" {
            Some(self.resolved_theme.clone())
        } else {
            self.adaptive.tokens.get(&id).cloned()
        };
        let fill = if enabled && role == "terminal" {
            Some((self.resolved_theme.background << 8) | 255)
        } else if role == "chat" || role == "mini" || role == "sidebar" || role == "projects" {
            None
        } else {
            palette.as_ref().map(|p| p.main_rgba())
        };
        Plan {
            enabled,
            id,
            role,
            palette,
            fill,
        }
    }
    pub(super) fn adaptive_guard(&self, role: &str, window: &Window) -> AnyElement {
        let colour = self
            .theme
            .adaptive_contrast
            .enabled
            .then(|| self.adaptive.core.decisions.get(&key(role, window)))
            .flatten()
            .map(|d| (d.tint << 8) | (d.alpha * 255.).round() as u32);
        div()
            .absolute()
            .inset_0()
            .when_some(colour, |s, c| s.bg(gpui::rgba(c)))
            .into_any_element()
    }
    pub(super) fn accept_adaptive(
        &mut self,
        profiles: Vec<Profile>,
        status: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if !self.theme.adaptive_contrast.enabled {
            return;
        }
        let old_status = self.adaptive.core.status.clone();
        if let Some(status) = status {
            self.adaptive.core.status = status.chars().take(256).collect();
        }
        if self.adaptive.tokens.len() > 64 {
            self.adaptive.tokens.clear();
            self.adaptive.temporal.clear();
            self.adaptive.core.clear();
        }
        let now = Instant::now();
        let before = self.adaptive.core.revision;
        let mut palette_changed = false;
        for profile in profiles.into_iter().take(32) {
            if !adaptive_contrast::valid_pixels(&profile.pixels) || profile.id.len() > 160 {
                continue;
            }
            if self
                .adaptive
                .core
                .profiles
                .get(&profile.id)
                .is_some_and(|old| old.generation > profile.generation)
            {
                continue;
            }
            let raw = profile.pixels.clone();
            let mut pixels = Vec::with_capacity(raw.len() + 2);
            for (i, p) in raw.iter().enumerate() {
                let native = over_rgba(self.resolved_theme.backdrop_rgba(), *p);
                let local = over_rgba(
                    if profile.role == "sidebar" || profile.role == "projects" {
                        self.resolved_theme.sidebar_rgba()
                    } else {
                        self.resolved_theme.main_rgba()
                    },
                    native,
                );
                let local = if profile.role == "chat" || profile.role == "mini" {
                    let n = (raw.len() as f32).sqrt().round().max(1.) as usize;
                    let point = ((i % n) as f32 + 0.5, (i / n) as f32 + 0.5);
                    self.background
                        .read(cx)
                        .adaptive_sample(
                            point.0 / n as f32,
                            point.1 / n as f32,
                            profile.rect[2],
                            profile.rect[3],
                            self.active.is_none(),
                            profile.role == "mini",
                        )
                        .map_or(local, |(rgb, a)| composite(rgb, a, local))
                } else if profile.role == "notch" {
                    *p
                } else {
                    local
                };
                pixels.push(local);
            }
            let mean: [f32; 3] =
                std::array::from_fn(|c| raw.iter().map(|p| p[c]).sum::<f32>() / raw.len() as f32);
            let temporal = self
                .adaptive
                .temporal
                .entry(profile.id.clone())
                .or_insert((mean, now));
            if !profile.moving
                && mean
                    .iter()
                    .zip(temporal.0)
                    .any(|(a, b)| (*a - b).abs() > 0.10)
            {
                temporal.1 = now + Duration::from_secs(2);
            }
            temporal.0 = mean;
            if profile.moving
                || now < temporal.1
                || ((profile.role == "chat" || profile.role == "mini")
                    && self.background.read(cx).adaptive_animated())
            {
                pixels.extend([[0.; 3], [1.; 3]]);
            }
            let settings = adaptive_contrast::Settings {
                target: (self.theme.adaptive_contrast.target
                    + if self.theme.adaptive_contrast.target >= 7. {
                        1.5
                    } else {
                        1.
                    })
                .min(10.),
                ..self.theme.adaptive_contrast.clone()
            };
            if let Some(decision) = self
                .adaptive
                .core
                .update(&profile.id, &pixels, &settings, now)
            {
                let palette = self.resolved_theme.adaptive_tokens(
                    &self.theme,
                    self.appearance,
                    &decision,
                    &pixels,
                );
                palette_changed |= self.adaptive.tokens.get(&profile.id) != Some(&palette);
                self.adaptive.tokens.insert(profile.id.clone(), palette);
                if profile.role == "notch" {
                    unified::emit(
                        &serde_json::json!({"type":"adaptiveContrastNotch","decision":decision}),
                    );
                }
            }
            self.adaptive.core.accept(vec![profile]);
        }
        self.adaptive
            .tokens
            .retain(|id, _| self.adaptive.core.profiles.contains_key(id));
        self.adaptive
            .temporal
            .retain(|id, _| self.adaptive.core.profiles.contains_key(id));
        if self.adaptive.core.revision != before
            || palette_changed
            || old_status != self.adaptive.core.status
        {
            cx.refresh_windows();
        }
        // A color-only refresh must not republish/rebuild the agent transcript.
    }
    pub(super) fn adaptive_settings(&self, cx: &Context<Self>) -> AnyElement {
        let enabled = self.theme.adaptive_contrast.enabled;
        let status = match self.adaptive.core.status.as_str() {
            "active" => "Live · local only",
            "permission_required" => "Screen access is needed for live backdrops",
            "capture_error" => "Live sampling paused · retry below",
            "paused" => "Paused while panels are hidden",
            "suspended" => "Paused while the display is asleep or locked",
            _ => "Waiting for a visible surface",
        };
        let alpha = |rgba: u32| (rgba & 255) as f32 / 255.;
        let native = alpha(self.resolved_theme.backdrop_rgba());
        let main = native + (1. - native) * alpha(self.resolved_theme.main_rgba());
        let sidebar = native + (1. - native) * alpha(self.resolved_theme.sidebar_rgba());
        div().id("adaptive-contrast-settings").debug_selector(||"adaptive-contrast-settings".into()).flex().flex_col().gap_2().text_size(theme::ui_px(11.))
            .child(if enabled{status.to_owned()}else{"Off · existing appearance is unchanged".into()})
            .when(enabled && matches!(self.adaptive.core.status.as_str(),"permission_required"|"capture_error"),|s|s.child(div().id("adaptive-allow-screen").debug_selector(||"adaptive-allow-screen".into()).px_3().py_2().rounded_md().cursor_pointer().bg(rgba(GLASS_HOVER)).child("Allow local backdrop sampling…").on_click(cx.listener(|_,_,_,_|unified::emit(&serde_json::json!({"type":"adaptiveContrastPermission"}))))))
            .child(format!("Base tint: conversation {:.0}% · sidebar {:.0}%",main*100.,sidebar*100.))
            .child("One ink color per panel; extra protection for mixed backgrounds. Samples stay on this Mac. No captured image is saved or sent to an agent.")
            .child(div().text_size(theme::ui_px(10.)).text_color(rgb(MUTED)).child("Contrast playground · responds to your glass opacity"))
            .child(self.adaptive_playground())
            .into_any_element()
    }
    fn adaptive_playground(&self) -> AnyElement {
        let native = self.resolved_theme.backdrop_rgba();
        let local = self.resolved_theme.main_rgba();
        div()
            .flex()
            .flex_wrap()
            .gap_2()
            .children(
                [("Dark", 0), ("Bright", 1), ("Split", 2), ("Color", 3)]
                    .into_iter()
                    .map(|(title, kind)| {
                        let samples: Vec<_> = (0..16)
                            .map(|i| match kind {
                                0 => [0.; 3],
                                1 => [1.; 3],
                                2 => {
                                    if i < 8 {
                                        [0.; 3]
                                    } else {
                                        [1.; 3]
                                    }
                                }
                                _ => channels([0x267ce3, 0xc45050, 0xecc95e, 0x17653f][i % 4]),
                            })
                            .collect();
                        let shown: Vec<_> = samples
                            .iter()
                            .map(|p| over_rgba(local, over_rgba(native, *p)))
                            .collect();
                        let d = adaptive_contrast::solve(
                            &shown,
                            self.theme.adaptive_contrast.target,
                            None,
                        );
                        div()
                            .w(px(126.))
                            .h(px(72.))
                            .relative()
                            .rounded_md()
                            .overflow_hidden()
                            .child(
                                div()
                                    .absolute()
                                    .inset_0()
                                    .flex()
                                    .children(samples.iter().map(|p| {
                                        div().flex_1().h_full().bg(gpui::rgb(
                                            (((p[0] * 255.) as u32) << 16)
                                                | (((p[1] * 255.) as u32) << 8)
                                                | (p[2] * 255.) as u32,
                                        ))
                                    })),
                            )
                            .child(div().absolute().inset_0().bg(gpui::rgba(native)))
                            .child(div().absolute().inset_0().bg(gpui::rgba(local)))
                            .child(
                                div()
                                    .absolute()
                                    .inset_0()
                                    .bg(gpui::rgba((d.tint << 8) | (d.alpha * 255.) as u32)),
                            )
                            .child(
                                div()
                                    .relative()
                                    .p_2()
                                    .text_color(gpui::rgb(d.ink.rgb()))
                                    .child(title)
                                    .child(
                                        div()
                                            .text_size(px(10.))
                                            .child(format!("Aa  {:.1}:1", d.minimum)),
                                    ),
                            )
                    }),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    use std::prelude::v1::test;
    #[gpui::test]
    fn adaptive_panels_are_independent_and_disable_restores_draft_and_geometry(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        cx.simulate_input("Keep this 🦀 draft");
        crate::ui_tests::draw_frame(&view, cx);
        let before = cx.debug_bounds("new-chat-content").unwrap();
        let (base, keys) = cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                let base = app.resolved_theme.clone();
                app.theme.adaptive_contrast.enabled = true;
                app.appearance.opacity = 0.;
                app.resolved_theme = app.theme.resolve(true, app.appearance);
                app.tick_adaptive(window, false, cx);
                let keys = [key("sidebar", window), key("chat", window)];
                let profiles = keys
                    .iter()
                    .enumerate()
                    .map(|(i, id)| Profile {
                        id: id.clone(),
                        role: if i == 0 { "sidebar" } else { "chat" }.into(),
                        generation: 100,
                        pixels: vec![[if i == 0 { 0. } else { 1. }; 3]; 64],
                        rect: [0., 0., 600., 700.],
                        moving: false,
                    })
                    .collect();
                app.accept_adaptive(profiles, Some("active".into()), cx);
                assert_ne!(
                    app.adaptive.core.decisions[&keys[0]].ink,
                    app.adaptive.core.decisions[&keys[1]].ink
                );
                (base, keys)
            })
        });
        crate::ui_tests::draw_frame(&view, cx);
        assert_eq!(
            cx.debug_bounds("new-chat-content").unwrap(),
            before,
            "contrast changed layout"
        );
        cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                assert_eq!(app.composer.read(cx).text(), "Keep this 🦀 draft");
                app.theme.adaptive_contrast.target = 7.;
                app.tick_adaptive(window, false, cx);
                assert_eq!(app.adaptive.core.status, "active");
                assert!(app.adaptive.core.profiles.contains_key(&keys[0]));
                assert!(app.adaptive.tokens.contains_key(&keys[1]));
                app.theme.adaptive_contrast.enabled = false;
                app.appearance = appearance::Appearance::default();
                app.resolved_theme = base.clone();
                app.tick_adaptive(window, false, cx);
                assert!(app.adaptive.tokens.is_empty());
                assert!(app.adaptive.core.decisions.is_empty());
                assert_eq!(app.composer.read(cx).text(), "Keep this 🦀 draft");
                assert!(!app.adaptive_plan("chat", window).enabled);
            })
        });
        crate::ui_tests::draw_frame(&view, cx);
        assert_eq!(cx.debug_bounds("new-chat-content").unwrap(), before);
    }
    #[gpui::test]
    fn backdrop_samples_use_global_bridge_without_a_chat_owner_token(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update_in(cx,|app,window,cx|{
            app.theme.adaptive_contrast.enabled=true;app.tick_adaptive(window,false,cx);
            let body=serde_json::json!({"action":"adaptive_contrast_samples","status":"active","profiles":[{"id":key("chat",window),"role":"chat","generation":99,"rect":[0,0,600,700],"pixels":[[1,1,1]]}]});
            app.unified_command(&body,window,cx).expect("global sampling must not require an active chat");
            assert_eq!(app.adaptive.core.status,"active");assert!(!app.adaptive.tokens.is_empty());
            app.theme.adaptive_contrast.enabled=false;app.tick_adaptive(window,false,cx);
            app.unified_command(&body,window,cx).unwrap();assert!(app.adaptive.tokens.is_empty());
        });
    }
    #[gpui::test]
    fn lazy_child_render_and_canvas_phases_inherit_only_their_own_palette(cx: &mut TestAppContext) {
        use std::{cell::RefCell, rc::Rc};
        type Seen = Rc<RefCell<Vec<(String, Rgba, Hsla)>>>;
        struct Probe {
            label: String,
            seen: Seen,
        }
        impl Render for Probe {
            fn render(&mut self, w: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                self.seen.borrow_mut().push((
                    format!("{}-render", self.label),
                    theme::rgb(0xebebeb),
                    w.text_style().color,
                ));
                let a = self.seen.clone();
                let b = a.clone();
                let l = self.label.clone();
                let m = l.clone();
                canvas(
                    move |_, w, _| {
                        a.borrow_mut().push((
                            format!("{l}-prepaint"),
                            theme::rgb(0xebebeb),
                            w.text_style().color,
                        ));
                    },
                    move |_, _, w, _| {
                        b.borrow_mut().push((
                            format!("{m}-paint"),
                            theme::rgb(0xebebeb),
                            w.text_style().color,
                        ))
                    },
                )
                .size_full()
            }
        }
        let seen: Seen = Default::default();
        let mut cx = VisualTestContext::from_window(
            cx.add_window(|_, _| Probe {
                label: "root".into(),
                seen: seen.clone(),
            })
            .into(),
            cx,
        );
        let settings = theme::Theme::default();
        let base = settings.resolve(true, appearance::Appearance::default());
        theme::install_resolved(base.clone());
        let make = |ink, pixels: [[f32; 3]; 1]| {
            let d = adaptive_contrast::solve_for(ink, &pixels, 5.5);
            base.adaptive_tokens(&settings, appearance::Appearance::default(), &d, &pixels)
        };
        let light = make(adaptive_contrast::Ink::Light, [[0.; 3]]);
        let dark = make(adaptive_contrast::Ink::Dark, [[1.; 3]]);
        let p = cx.new(|_| Probe {
            label: "light".into(),
            seen: seen.clone(),
        });
        let q = cx.new(|_| Probe {
            label: "dark".into(),
            seen: seen.clone(),
        });
        cx.draw(point(px(0.), px(0.)), size(px(400.), px(200.)), |_, _| {
            let plan = |id: &str, palette| Plan {
                enabled: true,
                id: id.into(),
                role: "terminal",
                palette: Some(palette),
                fill: None,
            };
            div()
                .size_full()
                .flex()
                .child(
                    plan("a", light.clone()).wrap(
                        div()
                            .w(px(200.))
                            .h_full()
                            .child(p.clone())
                            .into_any_element(),
                    ),
                )
                .child(
                    plan("b", dark.clone()).wrap(
                        div()
                            .w(px(200.))
                            .h_full()
                            .child(q.clone())
                            .into_any_element(),
                    ),
                )
                .into_any_element()
        });
        for label in ["light", "dark"] {
            for phase in ["render", "prepaint", "paint"] {
                let e = seen
                    .borrow()
                    .iter()
                    .find(|(s, _, _)| *s == format!("{label}-{phase}"))
                    .cloned()
                    .unwrap();
                let expected = gpui::rgb(if label == "light" {
                    light.foreground
                } else {
                    dark.foreground
                });
                assert_eq!(e.1, expected);
                assert_eq!(e.2, Hsla::from(expected));
            }
        }
        assert_eq!(theme::rgb(0xebebeb), gpui::rgb(base.foreground));
    }
}
