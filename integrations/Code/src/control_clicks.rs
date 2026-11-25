//! Preserve the identity of an outside-dismissed control until its click arrives.
use super::*;
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Control {
    Panel(Modal),
    Model(bool),
    Github(github_ui::Tab),
    Context,
    FileContext,
    Prompts,
    Teams,
    Timeline,
    Terminal,
    ChatTerminal,
    Files,
    Changes,
    Usage,
    Sidebar,
    Conversations,
    Children,
    SplitBeside,
    SplitBelow,
    Orchestrator,
    Dictation,
    Voice,
    FoldProject,
    FoldWork,
    Keyboard,
}

impl Control {
    pub(super) fn from_shortcut(id: &str) -> Option<Self> {
        Some(match id {
            "terminal" => Self::Terminal,
            "terminalize" => Self::ChatTerminal,
            "files" => Self::Files,
            "changes" => Self::Changes,
            "usage" => Self::Usage,
            "sidebar" => Self::Sidebar,
            "conversations" => Self::Conversations,
            "children" => Self::Children,
            "split_beside" => Self::SplitBeside,
            "split_below" => Self::SplitBelow,
            "orchestrator" => Self::Orchestrator,
            "dictate" => Self::Dictation,
            "voice" => Self::Voice,
            "fold_project" => Self::FoldProject,
            "fold_work" => Self::FoldWork,
            "keyboard" => Self::Keyboard,
            "search" => Self::Panel(Modal::Search),
            "settings" => Self::Panel(Modal::Settings),
            "projects" => Self::Panel(Modal::Projects),
            "model" => Self::Model(false),
            "thinking" => Self::Model(true),
            "github" => Self::Github(github_ui::Tab::Repository),
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    fn open(app: &Orbit, control: Control) -> bool {
        match control {
            Control::Github(tab) => app
                .github
                .panel
                .as_ref()
                .is_some_and(|p| !p.closing && p.tab == tab),
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
            _ => unreachable!("Unhandled control in the popover fixture"),
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
                ("chat-background", Control::Panel(Modal::Background)),
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
            cx.update(|window, cx| view.update(cx, |app, cx| {
                app.toggle_top_control(Control::Teams, point(px(400.), px(200.)), window, cx);
            }));
            cx.run_until_parked();
            crate::ui_tests::click_selector(cx, "chat-background");
            cx.run_until_parked();
            view.update(cx, |app, _| {
                assert!(app.team_panel.is_none());
                assert!(app.modal == Some(Modal::Background));
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
    pub(super) fn activate_control(&mut self, control: Control, window: &mut Window, cx: &mut Context<Self>) {
        match control {
            Control::Terminal => self.toggle_terminal(window, cx),
            Control::ChatTerminal => self.terminalize_chat(window, cx),
            Control::Files => self.toggle_files(window, cx),
            Control::Changes => self.toggle_diffs(cx),
            Control::Usage => self.open_usage_dashboard(window, cx),
            Control::Sidebar => self.toggle_sidebar(),
            Control::Conversations => self.toggle_conversations(cx),
            Control::Children => self.toggle_children(window, cx),
            Control::SplitBeside => self.workspace_split_key("split_beside", window, cx),
            Control::SplitBelow => self.workspace_split_key("split_below", window, cx),
            Control::Orchestrator => self.toggle_orchestrator(window, cx),
            Control::Dictation => self.toggle_dictation(window, cx),
            Control::Voice => self.toggle_voice(window, cx),
            Control::FoldProject => self.toggle_project_branch(self.project.clone(), cx),
            Control::FoldWork => {
                if let Some(row) = self.flow_index.work.iter().rev().find_map(|span| span.as_ref().map(|range| range.start)) {
                    self.toggle_work(row, cx);
                }
            }
            Control::Keyboard => {
                self.show_modal(Modal::Settings, window, cx);
                if let Some(input) = &self.appearance_search {
                    input.update(cx, |input, cx| input.set_text("Keyboard", cx));
                }
            }
            Control::Panel(kind) => self.show_modal(kind, window, cx),
            Control::Model(thinking) => {
                let anchor = point(window.viewport_size().width - px(300.), window.viewport_size().height - px(80.));
                if self.remote_workspace.active() {
                    let panel = if thinking { remote_workspace::ComposerPanel::Thinking } else { remote_workspace::ComposerPanel::Model };
                    self.open_remote_composer_panel(panel, anchor, window, cx);
                } else {
                    self.toggle_picker_from_click(thinking, anchor, window, cx);
                }
            }
            Control::Github(tab) => self.open_github(tab, window, cx),
            control => self.toggle_top_control(control, window.mouse_position(), window, cx),
        }
        cx.notify();
    }

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
