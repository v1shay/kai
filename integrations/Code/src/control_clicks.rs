//! Preserve the identity of an outside-dismissed control until its click arrives.
use super::*;
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Control {
    Panel(Modal),
    Model(bool),
    Context,
    FileContext,
    Prompts,
    Teams,
    Timeline,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    fn open(app: &Orbit, control: Control) -> bool {
        match control {
            Control::Context => app.context_picker.is_some(),
            Control::FileContext => app.file_context_picker.is_some(),
            Control::Prompts => app.prompt_library_picker.is_some(),
            Control::Teams => app.team_panel.is_some(),
            Control::Timeline => app.checkpoint_timeline.is_some(),
            Control::Panel(kind) => app.modal == Some(kind),
            Control::Model(effort) => app
                .agent_picker
                .as_ref()
                .is_some_and(|p| !p.closing && p.effort_only == effort),
        }
    }
    #[gpui::test]
    fn build_and_header_popovers_toggle_switch_and_dismiss_without_reopening(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update(cx, |app, cx| {
            app.active = Some("fixture-session".into());
            cx.notify();
        });
        crate::ui_tests::settle_composer(&view, cx);
        for motion in [false, true] {
            view.update(cx, |app, cx| {
                app.motion = motion;
                cx.notify();
            });
            for (selector, control) in [
                ("mode", Control::Panel(Modal::Permissions)),
                ("attach-chat-context", Control::Context),
                ("attach-local-context", Control::FileContext),
                ("prompt-library", Control::Prompts),
                ("agent-teams", Control::Teams),
                ("task-timeline", Control::Timeline),
            ] {
                crate::ui_tests::click_selector(cx, selector);
                cx.run_until_parked();
                view.update(cx, |app, _| {
                    assert!(open(app, control), "{selector} did not open")
                });
                crate::ui_tests::click_selector(cx, selector);
                cx.run_until_parked();
                view.update(cx, |app, _| {
                    assert!(
                        !open(app, control),
                        "{selector} reopened on the second click"
                    )
                });
                crate::ui_tests::click_selector(cx, selector);
                cx.run_until_parked();
                view.update(cx, |app, _| {
                    assert!(
                        open(app, control),
                        "{selector} could not reopen after its exit"
                    )
                });
                cx.simulate_click(point(px(5.), px(500.)), Modifiers::none());
                cx.run_until_parked();
                view.update(cx, |app, _| {
                    assert!(!open(app, control), "{selector} ignored an outside click")
                });
            }
            crate::ui_tests::click_selector(cx, "attach-chat-context");
            cx.run_until_parked();
            crate::ui_tests::click_selector(cx, "prompt-library");
            cx.run_until_parked();
            view.update(cx, |app, _| {
                assert!(app.context_picker.is_none());
                assert!(app.prompt_library_picker.is_some());
            });
            cx.simulate_click(point(px(5.), px(500.)), Modifiers::none());
            cx.run_until_parked();
        }
    }
}
pub(super) struct ClosedPress {
    control: Control,
    position: Point<Pixels>,
    at: Instant,
}
impl Orbit {
    pub(super) fn dismissed_on_press(&mut self, control: Control, position: Point<Pixels>) {
        self.closed_control_press = Some(ClosedPress {
            control,
            position,
            at: Instant::now(),
        });
    }
    pub(super) fn consumed_control_click(&mut self, control: Control, window: &Window) -> bool {
        self.closed_control_press.take().is_some_and(|press| {
            press.control == control
                && press.at.elapsed() < Duration::from_secs(3)
                && (press.position.x - window.mouse_position().x).abs() < px(6.)
                && (press.position.y - window.mouse_position().y).abs() < px(6.)
        })
    }
    pub(super) fn toggle_top_control(
        &mut self,
        control: Control,
        anchor: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.consumed_control_click(control, window) {
            cx.notify();
            return;
        }
        if control != Control::Context {
            self.context_picker = None;
        }
        if control != Control::FileContext {
            self.file_context_picker = None;
        }
        if control != Control::Prompts {
            self.prompt_library_picker = None;
        }
        if control != Control::Teams {
            self.team_panel = None;
        }
        if control != Control::Timeline {
            self.checkpoint_timeline = None;
        }
        self.agent_picker = None;
        self.modal = None;
        self.panel_popover = None;
        match control {
            Control::Context => self.toggle_context_picker(window, cx),
            Control::FileContext => self.toggle_file_context(anchor, window, cx),
            Control::Prompts => self.toggle_prompt_library(anchor, window, cx),
            Control::Teams => self.toggle_team_panel(window, cx),
            Control::Timeline => self.toggle_checkpoint_timeline(anchor, cx),
            _ => {}
        }
    }
}
