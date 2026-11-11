//! Shared finite pane reveals and resize-to-dismiss. Closing never compresses its contents.
use super::*;
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Pane {
    Terminal,
    Files,
    Children,
    Changes,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    fn drag(cx: &mut VisualTestContext, from: Point<Pixels>, to: Point<Pixels>) {
        cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_move(
            from + point(px(9.), px(0.)),
            Some(MouseButton::Left),
            Modifiers::none(),
        );
        cx.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::none());
        cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
        cx.run_until_parked();
    }
    #[gpui::test]
    fn inspector_handles_resize_real_files_children_and_changes_and_remember_sizes(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        for (pane, grip, shell) in [
            (Pane::Files, "files-resize", "files-resizable-pane"),
            (Pane::Children, "children-resize", "children-resizable-pane"),
            (Pane::Changes, "changes-resize", "changes-resizable-pane"),
        ] {
            cx.update(|window, cx| {
                view.update(cx, |app, cx| match pane {
                    Pane::Files => app.toggle_files(window, cx),
                    Pane::Children => app.toggle_children(window, cx),
                    Pane::Changes => app.open_changes(cx),
                    _ => {}
                })
            });
            crate::ui_tests::settle_composer(&view, cx);
            cx.run_until_parked();
            let bounds = cx.debug_bounds(shell).expect("Real inspector wrapper");
            let from = cx.debug_bounds(grip).expect("Resize handle").center();
            drag(cx, from, point(bounds.right() - px(270.), from.y));
            let after = cx.debug_bounds(shell).unwrap();
            assert!(
                (f32::from(after.size.width) - 270.).abs() < 2.,
                "{pane:?}: {after:?}"
            );
            view.update(cx, |app, cx| {
                let saved = app.workspace_snapshot(cx);
                let width = match pane {
                    Pane::Files => saved.inspector_widths.files,
                    Pane::Children => saved.inspector_widths.children,
                    Pane::Changes => saved.inspector_widths.changes,
                    _ => 0.,
                };
                assert!((width - 270.).abs() < 2.);
            });
            let from = cx.debug_bounds(grip).unwrap().center();
            drag(cx, from, point(after.right() - px(160.), from.y));
            view.update(cx, |app, _| {
                assert!(
                    !app.pane_visible(pane),
                    "{pane:?} did not close after dragging past the minimum"
                )
            });
        }
    }
    #[gpui::test]
    fn collapse_is_reversible_and_holds_full_size_content_until_finished(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                app.toggle_files(window, cx);
                app.motion = true;
            })
        });
        view.update(cx, |app, cx| {
            app.pane_layout.seen[Pane::Files.index()] = true;
            app.pane_layout.motions.remove(&Pane::Files);
            let before = app.inspector_widths.files;
            app.close_pane(Pane::Files, cx);
            app.pane_layout.motions.get_mut(&Pane::Files).unwrap().at -= Duration::from_millis(80);
            assert!(app.pane_extent(Pane::Files) > 0. && app.pane_extent(Pane::Files) < before);
            assert_eq!(
                app.pane_body_extent(Pane::Files),
                before,
                "Closing must clip the full-size viewer rather than squeeze it"
            );
            assert!(app.reopen_closing_pane(Pane::Files, cx));
            assert!(app.files_visible);
            assert!(!app.pane_layout.motions[&Pane::Files].closing);
        });
        cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                app.close_pane(Pane::Files, cx);
                app.pane_layout.motions.get_mut(&Pane::Files).unwrap().at -= Duration::from_secs(1);
                app.tick_panes(window, cx);
                assert!(!app.files_visible);
                assert!(!app.pane_layout.motions.contains_key(&Pane::Files));
            })
        });
    }
    #[gpui::test]
    fn terminal_drag_to_minimize_keeps_the_same_owned_panel_at_every_edge(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        let id = cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                app.new_terminal(window, cx);
                let panel = app.active_terminal_panel().unwrap().clone();
                panel.update(cx, |panel, _| panel.stop());
                let id = panel.entity_id();
                app.terminal_visible = true;
                id
            })
        });
        for side in terminal_dock::Side::ALL {
            view.update(cx, |app, cx| {
                app.terminal_visible = true;
                app.terminal_dock.side = side;
                app.resize_pane(
                    Pane::Terminal,
                    if side.horizontal() { 160. } else { 75. },
                    cx,
                );
                assert!(!app.terminal_visible);
                assert_eq!(app.active_terminal_panel().unwrap().entity_id(), id);
            });
        }
    }
    #[test]
    fn pane_width_preferences_are_backwards_compatible_and_bounded() {
        let old: workspace_state::WorkspaceState = serde_json::from_str("{}").unwrap();
        assert_eq!(old.inspector_widths.files, 360.);
        let widths = Widths {
            files: f32::NAN,
            children: -1.,
            changes: 100000.,
        }
        .normalized();
        assert!(widths.files.is_finite());
        assert_eq!(widths.children, 220.);
        assert_eq!(widths.changes, 1000.);
    }
}
impl Pane {
    pub fn index(self) -> usize {
        match self {
            Self::Terminal => 0,
            Self::Files => 1,
            Self::Children => 2,
            Self::Changes => 3,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Terminal => "terminal",
            Self::Files => "files",
            Self::Children => "children",
            Self::Changes => "changes",
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Widths {
    pub files: f32,
    pub children: f32,
    pub changes: f32,
}
impl Default for Widths {
    fn default() -> Self {
        Self {
            files: 360.,
            children: 380.,
            changes: 440.,
        }
    }
}
impl Widths {
    pub fn normalized(mut self) -> Self {
        for width in [&mut self.files, &mut self.children, &mut self.changes] {
            *width = if width.is_finite() {
                width.clamp(220., 1000.)
            } else {
                380.
            }
        }
        self
    }
}
pub(super) struct Motion {
    at: Instant,
    from: f32,
    to: f32,
    closing: bool,
}
impl Motion {
    fn value(&self) -> f32 {
        let progress = self.at.elapsed().as_secs_f32() / workspace_motion::PANE_DURATION.as_secs_f32();
        self.from + (self.to - self.from) * workspace_motion::ease_out(progress)
    }
}
#[derive(Default)]
pub(super) struct Layout {
    motions: HashMap<Pane, Motion>,
    seen: [bool; 4],
    pub terminal_reposition: Option<Instant>,
}
#[derive(Clone, Copy)]
pub(super) struct ResizePane(pub Pane);
impl Render for ResizePane {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size(px(1.))
    }
}
impl Orbit {
    fn pane_visible(&self, pane: Pane) -> bool {
        match pane {
            Pane::Terminal => self.terminal_visible,
            Pane::Files => self.files_visible,
            Pane::Children => self.broker_panel.is_some(),
            Pane::Changes => self.diff_view.is_some(),
        }
    }
    fn pane_width(&self, pane: Pane) -> f32 {
        match pane {
            Pane::Terminal => {
                if self.terminal_dock.side.horizontal() {
                    self.terminal_dock.width
                } else {
                    self.terminal_dock.height
                }
            }
            Pane::Files => self.inspector_widths.files,
            Pane::Children => self.inspector_widths.children,
            Pane::Changes => self.inspector_widths.changes,
        }
    }
    pub(super) fn pane_extent(&self, pane: Pane) -> f32 {
        self.pane_layout
            .motions
            .get(&pane)
            .map_or_else(|| self.pane_width(pane), Motion::value)
    }
    pub(super) fn pane_body_extent(&self, pane: Pane) -> f32 {
        self.pane_layout
            .motions
            .get(&pane)
            .filter(|m| m.closing)
            .map_or_else(|| self.pane_width(pane), |m| m.from)
    }
    pub(super) fn close_pane(&mut self, pane: Pane, cx: &mut Context<Self>) {
        if !self.pane_visible(pane) {
            return;
        }
        if !self.motion || native::reduced_motion() {
            self.finish_pane_close(pane, cx);
            return;
        }
        let from = self.pane_extent(pane);
        let at = Instant::now();
        self.pane_layout.motions.insert(
            pane,
            Motion {
                at,
                from,
                to: 0.,
                closing: true,
            },
        );
        // Complete once even if the window becomes hidden during the reveal.
        // A new gesture changes the token, so a late completion cannot close it.
        cx.spawn(async move |this, cx| {
            Timer::after(workspace_motion::PANE_DURATION).await;
            let _ = this.update(cx, |app, cx| {
                if app
                    .pane_layout
                    .motions
                    .get(&pane)
                    .is_some_and(|m| m.closing && m.at == at)
                {
                    app.finish_pane_close(pane, cx);
                }
            });
        })
        .detach();
        cx.notify();
    }
    fn finish_pane_close(&mut self, pane: Pane, cx: &mut Context<Self>) {
        match pane {
            Pane::Terminal => {
                self.terminal_visible = false;
                self.sync_terminal_visibility(cx);
            }
            Pane::Files => {
                self.files_visible = false;
                if let Some(browser) = &self.file_browser {
                    browser.update(cx, |browser, cx| browser.set_visible(false, cx));
                }
            }
            Pane::Children => self.broker_panel = None,
            Pane::Changes => {
                self.diff_view = None;
                self.diff_observer = None;
                self.diff_started = None;
            }
        }
        self.pane_layout.motions.remove(&pane);
        self.pane_layout.seen[pane.index()] = false;
        self.queue_workspace_save(cx);
        cx.notify();
    }
    pub(super) fn reopen_closing_pane(&mut self, pane: Pane, cx: &mut Context<Self>) -> bool {
        if !self.pane_visible(pane) {
            self.pane_layout.motions.remove(&pane);
            return false;
        }
        if self
            .pane_layout
            .motions
            .get(&pane)
            .is_some_and(|m| m.closing)
        {
            let from = self.pane_extent(pane);
            let to = self.pane_width(pane);
            self.pane_layout.motions.insert(
                pane,
                Motion {
                    at: Instant::now(),
                    from,
                    to,
                    closing: false,
                },
            );
            cx.notify();
            true
        } else {
            false
        }
    }

    pub(super) fn cancel_terminal_pane_motion(&mut self) {
        self.pane_layout.motions.remove(&Pane::Terminal);
        self.pane_layout.seen[Pane::Terminal.index()] = false;
    }
    pub(super) fn tick_panes(&mut self, window: &Window, cx: &mut Context<Self>) {
        for pane in [Pane::Terminal, Pane::Files, Pane::Children, Pane::Changes] {
            let visible = self.pane_visible(pane);
            let ix = pane.index();
            if visible
                && !self.pane_layout.seen[ix]
                && !self.pane_layout.motions.contains_key(&pane)
                && self.motion
                && !native::reduced_motion()
            {
                let to = self.pane_width(pane);
                self.pane_layout.motions.insert(
                    pane,
                    Motion {
                        at: Instant::now(),
                        from: 0.,
                        to,
                        closing: false,
                    },
                );
            }
            self.pane_layout.seen[ix] = visible;
            if !visible {
                self.pane_layout.motions.remove(&pane);
                continue;
            }
            if let Some(m) = self.pane_layout.motions.get(&pane) {
                if !self.motion
                    || native::reduced_motion()
                    || m.at.elapsed() >= workspace_motion::PANE_DURATION
                {
                    let closing = m.closing;
                    self.pane_layout.motions.remove(&pane);
                    if closing {
                        self.finish_pane_close(pane, cx);
                    }
                } else if window.is_window_active() {
                    request_motion_frame(window, cx);
                }
            }
        }
    }
    pub(super) fn resize_pane(&mut self, pane: Pane, value: f32, cx: &mut Context<Self>) {
        let minimum = if pane == Pane::Terminal && !self.terminal_dock.side.horizontal() {
            130.
        } else {
            220.
        };
        if value <= minimum - 40. {
            if !self
                .pane_layout
                .motions
                .get(&pane)
                .is_some_and(|m| m.closing)
            {
                self.close_pane(pane, cx);
            }
            return;
        }
        self.pane_layout.motions.remove(&pane);
        let value = value.clamp(minimum, 1000.);
        match pane {
            Pane::Terminal => {
                if self.terminal_dock.side.horizontal() {
                    self.terminal_dock.width = value
                } else {
                    self.terminal_dock.height = value
                }
            }
            Pane::Files => self.inspector_widths.files = value,
            Pane::Children => self.inspector_widths.children = value,
            Pane::Changes => self.inspector_widths.changes = value,
        }
        cx.notify();
    }
    fn inspector_available_width(&self, window: &Window) -> f32 {
        let sidebar = self.sidebar_width.max(0.);
        let conversations = if self.sidebar_split
            && self.conversations_visible
            && window.viewport_size().width >= px(1180.)
        {
            260.
        } else {
            0.
        };
        (f32::from(window.viewport_size().width) - sidebar - conversations).max(360.)
    }
    pub(super) fn prepare_inspector_switch(&mut self, next: Pane) {
        if let Some(old) = [Pane::Files, Pane::Children, Pane::Changes]
            .into_iter()
            .find(|kind| *kind != next && self.pane_visible(*kind))
        {
            let from = self.pane_extent(old);
            let to = self.pane_width(next);
            self.pane_layout.motions.insert(
                next,
                Motion {
                    at: Instant::now(),
                    from,
                    to,
                    closing: false,
                },
            );
            self.pane_layout.seen[next.index()] = true;
        }
    }
    pub(super) fn inspector_shell(
        &self,
        pane: Pane,
        content: AnyElement,
        window: &Window,
        cx: &Context<Self>,
    ) -> AnyElement {
        let extent = self
            .pane_extent(pane)
            .min(self.inspector_available_width(window) * 0.6);
        let body = self
            .pane_body_extent(pane)
            .min(self.inspector_available_width(window) * 0.6);
        let name = pane.name();
        div()
            .id(SharedString::from(format!("{name}-resizable-pane")))
            .debug_selector(move || format!("{name}-resizable-pane"))
            .w(px(extent))
            .h_full()
            .flex_none()
            .min_w(px(0.))
            .overflow_hidden()
            .relative()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_up(MouseButton::Left, |_, window, _| {
                native::panel_drag_guard(window, false)
            })
            .on_mouse_up_out(MouseButton::Left, |_, window, _| {
                native::panel_drag_guard(window, false)
            })
            .child(div().w(px(body)).h_full().child(content))
            .child(
                div()
                    .id(SharedString::from(format!("{name}-resize")))
                    .debug_selector(move || format!("{name}-resize"))
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(px(6.))
                    .cursor_col_resize()
                    .hover(|s| s.bg(rgba(0x88bfff88)))
                    .on_mouse_down(MouseButton::Left, |_, window, cx| {
                        native::panel_drag_guard(window, true);
                        cx.stop_propagation();
                    })
                    .on_drag(ResizePane(pane), |drag, _, _, cx| cx.new(|_| *drag)),
            )
            .on_drag_move(
                cx.listener(move |app, event: &DragMoveEvent<ResizePane>, _, cx| {
                    if event.drag(cx).0 == pane {
                        app.resize_pane(
                            pane,
                            f32::from(event.bounds.right() - event.event.position.x),
                            cx,
                        );
                    }
                }),
            )
            .on_drop(cx.listener(|app, _: &ResizePane, window, cx| {
                native::panel_drag_guard(window, false);
                app.queue_workspace_save(cx)
            }))
            .into_any_element()
    }
}
