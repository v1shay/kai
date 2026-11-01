//! Measured, single-row composer controls. Only finite layout transitions request frames.
use super::*;
use std::{cell::Cell, rc::Rc};

const PROJECT: usize = 0;
const MODEL: usize = 1;
const ORCHESTRATE: usize = 2;
const THINKING: usize = 3;
const FAST: usize = 4;
const PERMISSIONS: usize = 5;
const DELEGATE: usize = 6;
const VOICE: usize = 7;
const ATTACH: usize = 8;
const MORE: usize = 9;
const COUNT: usize = 10;
const GAP: f32 = 5.;
const DURATION: f32 = 0.24;
const KEYS: [&str; COUNT] = [
    "project",
    "model",
    "orchestrate",
    "thinking",
    "fast",
    "permissions",
    "delegate",
    "voice",
    "attach",
    "more",
];

#[derive(Clone, Copy, Default, Debug, PartialEq)]
struct Plan {
    widths: [f32; COUNT],
    detail: [f32; COUNT],
}
impl Plan {
    fn footprint(&self) -> f32 {
        self.widths
            .iter()
            .filter(|w| **w > 0.)
            .map(|w| w + GAP)
            .sum::<f32>()
    }
    fn mix(self, to: Self, t: f32) -> Self {
        Self {
            widths: std::array::from_fn(|i| self.widths[i] + (to.widths[i] - self.widths[i]) * t),
            detail: std::array::from_fn(|i| self.detail[i] + (to.detail[i] - self.detail[i]) * t),
        }
    }
    fn compact(&mut self, index: usize, width: f32) {
        if self.widths[index] > 0. {
            self.widths[index] = width;
            self.detail[index] = 0.;
        }
    }
    fn hide(&mut self, index: usize) {
        if self.widths[index] > 0. {
            self.widths[index] = 0.;
            self.detail[index] = 0.;
            self.widths[MORE] = 28.;
        }
    }
}
fn fit(mut plan: Plan, available: f32) -> Plan {
    let available = available.max(66.);
    // Retain useful names as long as possible. Mic and send have their own
    // fixed trailing group and never participate in this budget.
    if plan.footprint() > available {
        plan.compact(PERMISSIONS, 28.);
        plan.compact(VOICE, 28.);
    }
    if plan.footprint() > available {
        plan.compact(ORCHESTRATE, 30.);
        plan.compact(THINKING, 28.);
    }
    if plan.footprint() > available {
        for i in [PROJECT, DELEGATE, FAST, ORCHESTRATE, PERMISSIONS] {
            plan.hide(i);
        }
    }
    if plan.footprint() > available {
        for i in [VOICE, ATTACH, THINKING] {
            plan.hide(i);
        }
    }
    let rest = plan.footprint() - plan.widths[MODEL];
    plan.widths[MODEL] = plan.widths[MODEL].min((available - rest).max(28.));
    plan
}

pub(super) struct State {
    width: Rc<Cell<f32>>,
    from: Plan,
    target: Plan,
    started: Instant,
}
impl Default for State {
    fn default() -> Self {
        Self {
            width: Rc::new(Cell::new(0.)),
            from: Plan::default(),
            target: Plan::default(),
            started: Instant::now(),
        }
    }
}
impl State {
    fn sample(&self, motion: bool) -> Plan {
        if !motion || self.started.elapsed().as_secs_f32() >= DURATION {
            return self.target;
        }
        let t = self.started.elapsed().as_secs_f32() / DURATION;
        self.from.mix(self.target, 1. - (1. - t).powi(3))
    }
    fn update(&mut self, target: Plan, motion: bool) -> Plan {
        if target != self.target {
            self.from = self.sample(motion);
            // First presentation already uses a fitting layout. Later changes
            // animate from the current pose, including interrupted resizes.
            if self.target == Plan::default() {
                self.from = target;
            }
            self.target = target;
            self.started = Instant::now();
        }
        self.sample(motion)
    }
}

pub(super) fn text_width(window: &Window, text: &str) -> f32 {
    let run = TextRun {
        len: text.len(),
        font: font(theme::ui_font()),
        color: rgb(TEXT).into(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    f32::from(
        window
            .text_system()
            .shape_line(text.to_owned().into(), theme::ui_px(11.), &[run], None)
            .width,
    )
    .ceil()
}

#[derive(Clone, Copy)]
enum Action {
    Project,
    Model,
    Thinking,
    Orchestrate,
    Fast,
    Permissions,
    Delegate,
    Voice,
    Attach,
}

impl Orbit {
    fn toolbar_action(
        &mut self,
        action: Action,
        anchor: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            Action::Project => {
                self.toggle_panel_from_click(Modal::Projects, anchor, true, window, cx)
            }
            Action::Model | Action::Thinking => self.toggle_picker_from_click(
                matches!(action, Action::Thinking),
                anchor,
                window,
                cx,
            ),
            Action::Orchestrate => self.toggle_orchestrator(window, cx),
            Action::Fast => {
                self.dismiss_panel(window, cx);
                self.toggle_fast(cx);
            }
            Action::Permissions => {
                self.toggle_panel_from_click(Modal::Permissions, anchor, true, window, cx)
            }
            Action::Voice => {
                self.toggle_panel_from_click(Modal::Realtime, anchor, true, window, cx)
            }
            Action::Attach => {
                self.dismiss_panel(window, cx);
                self.choose_images(cx);
            }
            Action::Delegate => {
                self.dismiss_panel(window, cx);
                let draft = self.composer.read(cx).text();
                if commands::name(draft.trim()) != Some("delegate") {
                    self.composer.update(cx, |input, cx| {
                        input.set_text(&format!("/delegate {draft}"), cx)
                    });
                }
                self.composer.read(cx).focus_handle.focus(window);
                cx.notify();
            }
        }
    }
    fn workspace_folder_symbol(&self)->AnyElement{
        let progress=self.panel_popover.as_ref().filter(|p|p.kind==Modal::Projects)
            .map_or(0.,|p|p.reveal(self.motion&&!native::reduced_motion()).clamp(0.,1.));
        div().size(px(16.)).relative()
            .child(icon("workspace-folder",16.,MUTED).absolute().opacity(1.-progress))
            .child(icon("workspace-folder-open",16.,TEXT).absolute().top(px(-progress)).opacity(progress))
            .into_any_element()
    }
    fn toolbar_plan(&self, window: &Window) -> Plan {
        let mut plan = Plan::default();
        let model = composer_design::display_label(
            self.effective_model_label()
                .split(" · ")
                .next()
                .unwrap_or(""),
        );
        plan.widths[PROJECT] = 26.;
        plan.widths[MODEL] =
            (44. + if composer_design::model_glyph(&model).is_some() {
                18.
            } else {
                0.
            } + text_width(window, &model))
            .min(238.);
        {
            plan.widths[ORCHESTRATE] =
                16. + text_width(
                    window,
                    if self.orchestrator_active() {
                        "Orchestrator"
                    } else {
                        "Orchestrate"
                    },
                ) + if self.orchestrator_active() { 18. } else { 0. };
        }
        if self
            .effort_model_choice()
            .is_some_and(|m| !m.reasoning_efforts.is_empty())
        {
            let value = self
                .unified_effort()
                .map(str::to_owned)
                .or_else(|| self.effective_model_settings().1)
                .unwrap_or_default();
            let label = if value.is_empty() {
                "Thinking".into()
            } else {
                composer_design::title(&value)
            };
            plan.widths[THINKING] = 16. + text_width(window, &label);
        }
        if self.fast_supported() {
            plan.widths[FAST] = 28.;
        }
        plan.widths[PERMISSIONS] = 28.;
        if self.modal == Some(Modal::ComposerControls) {
            plan.widths[MORE] = 28.;
        }
        for i in [MODEL, ORCHESTRATE, THINKING] {
            plan.detail[i] = 1.;
        }
        plan
    }
    fn toolbar_permission_label(&self) -> &'static str {
        if !self.allow_edits {
            "Read only"
        } else if matches!(self.agent,AgentKind::Codex|AgentKind::Ninja) && self.codex_access == runtime::CodexAccess::Full
        {
            "Full access"
        } else {
            "Build"
        }
    }
    fn toolbar_detail_button(
        &self,
        id: &'static str,
        glyph: &'static str,
        label: &'static str,
        detail: f32,
        action: Action,
        cx: &Context<Self>,
    ) -> AnyElement {
        div()
            .id(id)
            .debug_selector(move || id.into())
            .w_full()
            .h(px(28.))
            .px(px(6. + 2. * detail))
            .flex()
            .items_center()
            .rounded_md()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_size(theme::ui_px(11.))
            .cursor_pointer()
            .hover(|s| s.bg(rgba(GLASS_HOVER)))
            .bg(rgba(
                if self.theme.borderless_composer {
                    0
                } else if matches!(action, Action::Voice) && self.voice_call.is_some() {
                    0x9fbaee26
                } else if matches!(action, Action::Permissions) {
                    0xffffff14
                } else {
                    0xffffff00
                },
            ))
            .child(
                icon(
                    glyph,
                    14.,
                    if matches!(action, Action::Voice) && self.voice_call.is_some() {
                        ACCENT
                    } else {
                        MUTED
                    },
                )
                .flex_none(),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .opacity(detail)
                    .pl(px(4. * detail))
                    .child(label),
            )
            .when(detail > 0.01, |s| {
                s.child(
                    div()
                        .w(px(10. * detail))
                        .flex_none()
                        .overflow_hidden()
                        .opacity(detail)
                        .child(icon("chevron-down", 9., MUTED)),
                )
            })
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                this.toolbar_action(
                    action,
                    event.position() + point(px(-12.), px(-12.)),
                    window,
                    cx,
                )
            }))
            .into_any_element()
    }
    pub(super) fn composer_toolbar(
        &self,
        quick: bool,
        input: Entity<TextInput>,
        stopping: bool,
        window: &Window,
        cx: &Context<Self>,
    ) -> AnyElement {
        let motion = self.motion && !native::reduced_motion() && window.is_window_active();
        let window_id = window.window_handle().window_id();
        let (plan, target, measured, animating) = {
            let mut states = self.composer_toolbars.borrow_mut();
            if !states.contains_key(&window_id) && states.len() >= 4 {
                states.clear();
            }
            let state = states.entry(window_id).or_default();
            let target = fit(self.toolbar_plan(window), state.width.get());
            let plan = state.update(target, motion);
            (
                plan,
                target,
                state.width.clone(),
                motion
                    && state.from != state.target
                    && state.started.elapsed().as_secs_f32() < DURATION,
            )
        };
        if animating {
            request_motion_frame(window, cx);
        }
        let owner = cx.weak_entity();
        let measure = canvas(
            move |bounds, window, _| {
                // This canvas measures the control area, after padding and the fixed
                // microphone/send group have been laid out. No viewport guesses.
                let width = f32::from(bounds.size.width);
                if (measured.get() - width).abs() > 0.5 {
                    measured.set(width);
                    let owner = owner.clone();
                    window.on_next_frame(move |_, cx| {
                        let _ = owner.update(cx, |_, cx| cx.notify());
                    });
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        let mut controls = div()
            .id("composer-adaptive-controls")
            .debug_selector(|| "composer-adaptive-controls".into())
            .flex_1()
            .min_w(px(0.))
            .h(px(32.))
            .relative()
            .overflow_hidden()
            .flex()
            .items_center()
            .child(measure);
        for i in [PROJECT, PERMISSIONS, MODEL, THINKING, FAST, ORCHESTRATE, MORE] {
            if plan.widths[i] < 0.1 {
                continue;
            }
            let fraction = (plan.widths[i] / target.widths[i].max(28.)).min(1.);
            let content = match i {
                PROJECT => div()
                    .id("choose-project")
                    .debug_selector(|| "choose-project".into())
                    .size(px(26.))
                    .rounded_md()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .bg(rgba(if self.theme.borderless_composer { 0 } else { 0xffffff14 }))
                    .child(self.workspace_folder_symbol())
                    .on_click(cx.listener(|this, event: &ClickEvent, window, cx| {
                        this.toolbar_action(Action::Project, event.position(), window, cx)
                    }))
                    .into_any_element(),
                MODEL => {
                    let id = if quick {
                        "quick-agent-model"
                    } else {
                        "choose-agent"
                    };
                    let label = composer_design::display_label(
                        self.effective_model_label()
                            .split(" · ")
                            .next()
                            .unwrap_or(""),
                    );
                    let glyph_reveal = ((plan.widths[MODEL] - 78.) / 24.).clamp(0., 1.);
                    let text_reveal = ((plan.widths[MODEL] - 32.) / 42.).clamp(0., 1.);
                    let chevron_reveal = ((plan.widths[MODEL] - 58.) / 12.).clamp(0., 1.);
                    div()
                        .id(id)
                        .debug_selector(move || id.into())
                        .w_full()
                        .min_w(px(0.))
                        .h(px(28.))
                        .px(px(6.))
                        .flex()
                        .items_center()
                        .rounded_md()
                        .bg(rgba(if self.theme.borderless_composer { 0 } else { 0xffffff14 }))
                        .overflow_hidden()
                        .text_size(theme::ui_px(11.))
                        .cursor_pointer()
                        .hover(|s| s.bg(rgba(GLASS_HOVER)))
                        .child(
                            icon(agent_icon(self.agent), 14., agent_color(self.agent)).flex_none(),
                        )
                        .when_some(composer_design::model_glyph(&label), |s, glyph| {
                            s.child(
                                div()
                                    .w(px(14. * glyph_reveal))
                                    .h(px(14.))
                                    .ml(px(4. * glyph_reveal))
                                    .flex_none()
                                    .overflow_hidden()
                                    .opacity(glyph_reveal)
                                    .child(icon(glyph, 14., TEXT)),
                            )
                        })
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .ml(px(4. * text_reveal))
                                .opacity(text_reveal)
                                .child(label),
                        )
                        .child(
                            div()
                                .w(px(10. * chevron_reveal))
                                .h(px(14.))
                                .ml(px(4. * chevron_reveal))
                                .flex_none()
                                .overflow_hidden()
                                .opacity(chevron_reveal)
                                .child(icon("chevron-down", 10., MUTED)),
                        )
                        .on_click(cx.listener(|this, event: &ClickEvent, window, cx| {
                            this.toolbar_action(Action::Model, event.position(), window, cx)
                        }))
                        .into_any_element()
                }
                ORCHESTRATE => self.orchestrator_chip_adaptive(window, cx, plan.detail[i]),
                THINKING => self.thinking_chip_adaptive(window, cx, plan.detail[i]),
                FAST => self.fast_button(window, cx),
                PERMISSIONS => {
                    let color = if !self.allow_edits { 0xd5dce5 } else if matches!(self.agent,AgentKind::Codex|AgentKind::Ninja) && self.codex_access == runtime::CodexAccess::Full { 0xff7d75 } else { 0xffb86b };
                    let glyph=if !self.allow_edits {"lock-keyhole"} else if matches!(self.agent,AgentKind::Codex|AgentKind::Ninja) && self.codex_access==runtime::CodexAccess::Full {"lock-keyhole-open"} else {"shield"};
                    div().id(if quick { "quick-permissions" } else { "mode" })
                        .debug_selector(move || if quick { "quick-permissions".into() } else { "mode".into() })
                        .w_full().h(px(28.)).flex().items_center().justify_center().rounded_md().cursor_pointer()
                        .hover(|s|s.bg(rgba(GLASS_HOVER)))
                        .child(icon(glyph,16.,color))
                        .on_click(cx.listener(|this,event:&ClickEvent,window,cx|this.toolbar_action(Action::Permissions,event.position(),window,cx)))
                        .into_any_element()
                },
                DELEGATE => div()
                    .id("delegate-task")
                    .debug_selector(|| "delegate-task".into())
                    .size(px(26.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|s| s.bg(rgba(GLASS_HOVER)))
                    .child(icon("assets/kai/status/agents.svg", 16., MUTED))
                    .on_click(cx.listener(|this, event: &ClickEvent, window, cx| {
                        this.toolbar_action(Action::Delegate, event.position(), window, cx)
                    }))
                    .into_any_element(),
                VOICE => self.toolbar_detail_button(
                    "conversation-voice",
                    "assets/kai/status/listening.svg",
                    "RealTime",
                    plan.detail[i],
                    Action::Voice,
                    cx,
                ),
                ATTACH => self.attachment_button(cx).into_any_element(),
                MORE => div()
                    .id("composer-more")
                    .debug_selector(|| "composer-more".into())
                    .size(px(28.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(rgba(if self.theme.borderless_composer {
                        0
                    } else if self.modal == Some(Modal::ComposerControls) {
                        GLASS_HOVER
                    } else {
                        0xffffff08
                    }))
                    .hover(|s| s.bg(rgba(GLASS_HOVER)))
                    .child(icon("more", 16., MUTED))
                    .on_click(cx.listener(|this, event: &ClickEvent, window, cx| {
                        this.toggle_panel_from_click(
                            Modal::ComposerControls,
                            event.position() + point(px(-12.), px(-12.)),
                            true,
                            window,
                            cx,
                        )
                    }))
                    .into_any_element(),
                _ => unreachable!(),
            };
            let key = KEYS[i];
            controls = controls.child(
                div()
                    .debug_selector(move || format!("toolbar-slot-{key}"))
                    .w(px(plan.widths[i]))
                    .flex_none()
                    .mr(px(GAP * (plan.widths[i] / 28.).min(1.)))
                    .overflow_hidden()
                    .opacity(fraction)
                    .child(content),
            );
        }
        let send_id = if quick { "quick-send" } else { "send" };
        div()
            .id("composer-toolbar")
            .debug_selector(|| "composer-toolbar".into())
            .w_full()
            .min_w(px(0.))
            .flex_none()
            .flex()
            .flex_col()
            .px_2()
            .pb_2()
            .child(
                div()
                    .w_full()
                    .min_w(px(0.))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .child(controls)
                    .child(
                        div()
                            .debug_selector(|| "composer-primary-actions".into())
                            .flex_none()
                            .h(px(32.))
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .child(div().debug_selector(||"composer-realtime-anchor".into())
                                .w(px(if window.viewport_size().width < px(600.) {28.} else {47.+text_width(window,"RealTime")}))
                                .flex_none().child(self.toolbar_detail_button("conversation-voice", "assets/kai/status/listening.svg", "RealTime", if window.viewport_size().width < px(600.) {0.} else {1.}, Action::Voice, cx)))
                            .child(self.dictation_button(cx).flex_none())
                            .child(
                                div()
                                    .id(send_id)
                                    .debug_selector(move || send_id.into())
                                    .size(px(32.))
                                    .flex_none()
                                    .cursor_pointer()
                                    .hover(|s| s.opacity(0.88))
                                    .child(self.send_symbol(stopping))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        if !this.cancel_send_handoff(cx) {
                                            this.send(window, cx);
                                        }
                                        input.read(cx).focus_handle.focus(window);
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }
    pub(super) fn composer_more_controls(&self, cx: &Context<Self>) -> AnyElement {
        let mut rows = vec![
            (
                "model",
                "Choose model".to_owned(),
                self.effective_model_label(),
                "sparkles",
                Action::Model,
            ),
            (
                "project",
                "Choose project".into(),
                self.project
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                "folder",
                Action::Project,
            ),
            (
                "permissions",
                "Workspace access".into(),
                self.toolbar_permission_label().into(),
                "shield",
                Action::Permissions,
            ),
        ];
        if self
            .effort_model_choice()
            .is_some_and(|m| !m.reasoning_efforts.is_empty())
        {
            let effort = self
                .unified_effort()
                .map(str::to_owned)
                .or_else(|| self.effective_model_settings().1)
                .unwrap_or_default();
            rows.push((
                "thinking",
                "Thinking level".into(),
                composer_design::title(&effort),
                "sparkles",
                Action::Thinking,
            ));
        }
        {
            rows.push((
                "orchestrate",
                "Orchestrate".into(),
                if self.orchestrator_active() {
                    "On"
                } else {
                    "Off"
                }
                .into(),
                "branch",
                Action::Orchestrate,
            ));
        }
        if self.fast_supported() {
            rows.push((
                "fast",
                "Fast mode".into(),
                if self.fast_enabled() { "On" } else { "Off" }.into(),
                "bolt",
                Action::Fast,
            ));
        }
        if runtime::supports_native_delegation(self.agent) {
            rows.push((
                "delegate",
                "Delegate task".into(),
                "".into(),
                "assets/kai/status/agents.svg",
                Action::Delegate,
            ));
        }
        if self.agent == AgentKind::Codex || self.voice_call.is_some() {
            rows.push((
                "voice",
                "Real-time voice".into(),
                "".into(),
                "assets/kai/status/listening.svg",
                Action::Voice,
            ));
        }
        rows.push((
            "attach",
            "Attach files".into(),
            "".into(),
            "paperclip",
            Action::Attach,
        ));
        div()
            .p_2()
            .flex()
            .flex_col()
            .gap_1()
            .children(rows.into_iter().map(|(key, title, value, glyph, action)| {
                div()
                    .id(SharedString::from(format!("composer-action-{key}")))
                    .debug_selector(move || format!("composer-action-{key}"))
                    .w_full()
                    .min_h(px(38.))
                    .p_2()
                    .rounded_md()
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .hover(|s| s.bg(rgba(GLASS_HOVER)))
                    .child(icon(glyph, 14., MUTED).flex_none())
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .text_size(theme::ui_px(12.))
                            .child(title)
                            .when(!value.is_empty(), |s| {
                                s.child(
                                    div()
                                        .text_size(theme::ui_px(10.))
                                        .text_color(rgb(MUTED))
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .child(value),
                                )
                            }),
                    )
                    .child(icon("chevron-right", 10., MUTED).flex_none())
                    .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                        this.toolbar_action(action, event.position(), window, cx)
                    }))
            }))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;

    fn crowded() -> Plan {
        Plan {
            widths: [26., 238., 110., 90., 28., 120., 26., 112., 28., 0.],
            detail: [1.; COUNT],
        }
    }

    #[test]
    fn adaptive_budget_keeps_primary_identity_and_every_hidden_action_reachable() {
        for width in 66..1000 {
            let plan = fit(crowded(), width as f32);
            assert!(plan.footprint() <= width as f32 + 0.01, "{width}: {plan:?}");
            assert!(plan.widths[MODEL] >= 28.);
            if [
                PROJECT,
                ORCHESTRATE,
                THINKING,
                FAST,
                PERMISSIONS,
                DELEGATE,
                VOICE,
                ATTACH,
            ]
            .iter()
            .any(|i| plan.widths[*i] == 0.)
            {
                assert_eq!(plan.widths[MORE], 28.);
            }
        }
        let spacious = fit(crowded(), 1000.);
        assert_eq!(spacious.widths[MORE], 0.);
        assert_eq!(spacious.detail[VOICE], 1.);
        let short = fit(crowded(), 800.);
        assert_eq!(short.detail[VOICE], 0.);
        assert_eq!(short.detail[PERMISSIONS], 0.);
        assert_eq!(short.detail[ORCHESTRATE], 1.);
    }

    #[test]
    fn toolbar_resize_reverses_from_its_current_pose_and_settles_without_a_loop() {
        let mut state = State::default();
        let wide = fit(crowded(), 1000.);
        let narrow = fit(crowded(), 250.);
        assert_eq!(state.update(wide, true), wide);
        state.update(narrow, true);
        state.started -= Duration::from_millis(100);
        let before = state.sample(true);
        let reversed = state.update(wide, true);
        for i in 0..COUNT {
            assert!((before.widths[i] - reversed.widths[i]).abs() < 0.1);
        }
        state.started -= Duration::from_secs(1);
        for i in 0..COUNT {
            assert!((state.sample(true).widths[i] - wide.widths[i]).abs() < 0.001);
        }
        assert_eq!(state.sample(false), wide);
    }

    fn prepare(app: &mut Orbit, cx: &mut Context<Orbit>) {
        app.sidebar = false;
        app.sidebar_transition = None;
        app.sidebar_width = 0.;
        app.sidebar_split = false;
        app.motion = false;
        app.agent = AgentKind::Codex;
        app.model = Some("gpt-6.1-terra".into());
        app.native_models.insert(
            AgentKind::Codex,
            vec![agent_controls::ModelChoice {
                id: "gpt-6.1-terra".into(),
                label: "GPT-6.1 Terra".into(),
                reasoning_efforts: vec!["low".into(), "medium".into(), "high".into()],
            }],
        );
        let key = app.effort_key();
        app.reasoning_efforts.insert(key, "medium".into());
        let mut info = agent_controls::ControlInfo {
            defaults: Default::default(),
            auth: agent_controls::AuthState::Unknown,
            models: vec![],
            note: None,
            account: None,
            limits: vec![],
            catalog: None,
        };
        info.defaults
            .fast_models
            .insert("gpt-6.1-terra".into(), "priority".into());
        app.controls.insert("codex", (Instant::now(), info));
        app.composer
            .update(cx, |input, cx| input.set_text("Keep this draft", cx));
        cx.notify();
    }

    fn draw(view: &Entity<Orbit>, cx: &mut VisualTestContext, width: f32) {
        cx.simulate_resize(size(px(width), px(790.)));
        for _ in 0..3 {
            cx.draw(point(px(0.), px(0.)), size(px(width), px(790.)), |_, _| {
                view.clone().into_any_element()
            });
            cx.run_until_parked();
        }
    }

    #[gpui::test]
    fn measured_composer_never_wraps_microphone_or_send_at_narrow_widths(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update(cx, prepare);
        for width in [1040., 820., 680., 500., 380., 280.] {
            draw(&view, cx, width);
            let toolbar = cx.debug_bounds("composer-toolbar").unwrap();
            let primary = cx.debug_bounds("composer-primary-actions").unwrap();
            let send = cx.debug_bounds("send").unwrap();
            let mic = cx.debug_bounds("dictate").unwrap();
            let controls = cx.debug_bounds("composer-adaptive-controls").unwrap();
            if width == 1040. {
                let measurements = view.update(cx, |app, _| {
                    app.composer_toolbars
                        .borrow()
                        .values()
                        .map(|state| (state.width.get(), state.target.widths))
                        .collect::<Vec<_>>()
                });
                assert!(
                    cx.debug_bounds("conversation-voice").is_some(),
                    "Wide controls did not expand: {measurements:?}, area={controls:?}"
                );
            }
            assert!(
                (send.center().y - mic.center().y).abs() < px(0.5),
                "Mic wrapped at {width}"
            );
            assert!(
                send.right() <= toolbar.right() + px(0.5)
                    && primary.left() >= toolbar.left() - px(0.5),
                "Primary actions overflow at {width}: toolbar={toolbar:?}, primary={primary:?}, send={send:?}"
            );
            assert!(mic.right() < send.left());
            let target = view.update(cx, |app, _| {
                app.composer_toolbars
                    .borrow()
                    .values()
                    .next()
                    .unwrap()
                    .target
            });
            for (i, (key, selector)) in KEYS
                .iter()
                .zip([
                    "toolbar-slot-project",
                    "toolbar-slot-model",
                    "toolbar-slot-orchestrate",
                    "toolbar-slot-thinking",
                    "toolbar-slot-fast",
                    "toolbar-slot-permissions",
                    "toolbar-slot-delegate",
                    "toolbar-slot-voice",
                    "toolbar-slot-attach",
                    "toolbar-slot-more",
                ])
                .enumerate()
            {
                if target.widths[i] == 0. {
                    continue;
                }
                if let Some(slot) = cx.debug_bounds(selector) {
                    assert!(
                        slot.right() <= controls.right() + px(1.),
                        "Clipped {key} at {width}: {slot:?}, {controls:?}"
                    );
                    assert!((slot.center().y - send.center().y).abs() < px(2.));
                }
            }
            if width <= 500. {
                assert!(cx.debug_bounds("composer-more").is_some());
            }
            view.update(cx, |app, cx| {
                assert_eq!(app.composer.read(cx).text(), "Keep this draft")
            });
        }
    }

    #[gpui::test]
    fn grouped_identity_and_fixed_voice_survive_borderless_preferences(cx: &mut TestAppContext) {
        let (view,cx) = crate::ui_tests::fixture(cx);
        view.update(cx, prepare);
        draw(&view,cx,1040.);
        let model=cx.debug_bounds("choose-agent").unwrap();
        let thinking=cx.debug_bounds("thinking-chip").unwrap();
        let fast=cx.debug_bounds("fast-mode").unwrap();
        let orchestrate=cx.debug_bounds("orchestrator-mode").unwrap();
        assert!(model.right() <= thinking.left() && thinking.right() <= fast.left() && fast.right() <= orchestrate.left());
        assert!(cx.debug_bounds("delegate-task").is_none());
        assert!(cx.debug_bounds("attach-image").is_none());
        let voice=cx.debug_bounds("conversation-voice").unwrap();
        let primary=cx.debug_bounds("composer-primary-actions").unwrap();
        assert!(voice.left() >= primary.left() && voice.right() <= primary.right());
        assert!(voice.left() > orchestrate.right());
        view.update(cx,|app,cx| {
            app.theme.borderless_composer=true;
            let restored:theme::Theme=serde_json::from_str(&serde_json::to_string(&app.theme).unwrap()).unwrap();
            assert!(restored.borderless_composer);
            assert_eq!(app.unified_snapshot(cx)["borderlessComposer"],true);
            cx.notify();
        });
        draw(&view,cx,1040.);
        let borderless=cx.debug_bounds("choose-agent").unwrap();
        assert_eq!(model,borderless,"Borderless preference changed the hit target");
        crate::ui_tests::click_selector(cx,"mode");
        view.update(cx,|app,_|assert!(app.modal==Some(Modal::Permissions)));
    }

    #[gpui::test]
    fn collapsed_controls_open_on_click_toggle_and_keep_the_draft(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update(cx, prepare);
        draw(&view, cx, 380.);
        crate::ui_tests::click_selector(cx, "composer-more");
        cx.run_until_parked();
        view.update(cx, |app, _| {
            assert!(app.modal == Some(Modal::ComposerControls))
        });
        crate::ui_tests::click_selector(cx, "composer-more");
        cx.run_until_parked();
        view.update(cx, |app, _| assert!(app.modal.is_none()));
        crate::ui_tests::click_selector(cx, "composer-more");
        cx.run_until_parked();
        crate::ui_tests::click_selector(cx, "composer-action-thinking");
        cx.run_until_parked();
        view.update(cx, |app, cx| {
            assert!(
                app.agent_picker
                    .as_ref()
                    .is_some_and(|picker| picker.effort_only && !picker.closing)
            );
            assert_eq!(app.composer.read(cx).text(), "Keep this draft");
        });
        cx.simulate_click(point(px(30.), px(100.)), Modifiers::none());
        cx.run_until_parked();
        crate::ui_tests::click_selector(cx, "composer-more");
        cx.run_until_parked();
        crate::ui_tests::click_selector(cx, "composer-action-permissions");
        cx.run_until_parked();
        view.update(cx, |app, cx| {
            assert!(app.modal == Some(Modal::Permissions));
            assert_eq!(app.composer.read(cx).text(), "Keep this draft");
        });
    }
}
