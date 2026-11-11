//! The live terminal split tree docks at any edge without restarting its PTYs.
use super::*;
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Left,
    Right,
    Top,
    #[default]
    Bottom,
}
impl Side {
    pub const ALL: [Self; 4] = [Self::Left, Self::Right, Self::Top, Self::Bottom];
    pub fn horizontal(self) -> bool {
        matches!(self, Self::Left | Self::Right)
    }
    pub fn before(self) -> bool {
        matches!(self, Self::Left | Self::Top)
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Left => "Left",
            Self::Right => "Right",
            Self::Top => "Top",
            Self::Bottom => "Bottom",
        }
    }
    pub fn glyph(self) -> &'static str {
        match self {
            Self::Left => "←",
            Self::Right => "→",
            Self::Top => "↑",
            Self::Bottom => "↓",
        }
    }
    pub fn target(bounds: Bounds<Pixels>, p: Point<Pixels>) -> Option<Self> {
        let w = f32::from(bounds.size.width).max(1.);
        let h = f32::from(bounds.size.height).max(1.);
        let x = f32::from(p.x - bounds.origin.x) / w;
        let y = f32::from(p.y - bounds.origin.y) / h;
        if !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) {
            return None;
        }
        [
            (Self::Left, x),
            (Self::Right, 1. - x),
            (Self::Top, y),
            (Self::Bottom, 1. - y),
        ]
        .into_iter()
        .filter(|(_, d)| *d <= 0.25)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(side, _)| side)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Dock {
    pub side: Side,
    pub width: f32,
    pub height: f32,
}
impl Default for Dock {
    fn default() -> Self {
        Self {
            side: Side::Bottom,
            width: 360.,
            height: 270.,
        }
    }
}
impl Dock {
    pub fn normalized(mut self) -> Self {
        self.width = if self.width.is_finite() {
            self.width.clamp(220., 1000.)
        } else {
            360.
        };
        self.height = if self.height.is_finite() {
            self.height.clamp(130., 800.)
        } else {
            270.
        };
        self
    }
    fn extent(self, available: Size<Pixels>) -> f32 {
        let (n, total, min) = if self.side.horizontal() {
            (self.width, f32::from(available.width), 220.)
        } else {
            (self.height, f32::from(available.height), 130.)
        };
        n.min((total * 0.65).max(min)).max(min)
    }
}
#[derive(Clone, Copy)]
pub struct MoveTerminal;
impl Render for MoveTerminal {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_3()
            .py_2()
            .rounded_md()
            .bg(rgba(0x242424ee))
            .text_color(rgb(TEXT))
            .text_size(px(12.))
            .child("Move terminal")
    }
}
#[derive(Clone, Copy)]
pub struct ResizeTerminal;
impl Render for ResizeTerminal {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size(px(1.))
    }
}
impl Orbit {
    pub(super) fn dock_terminal(&mut self, side: Side, cx: &mut Context<Self>) {
        self.terminal_dock.side = side;
        self.pane_layout.terminal_reposition = Some(Instant::now());
        self.terminal_dock_hover = None;
        self.queue_workspace_save(cx);
        cx.notify();
    }
    pub(super) fn docked_conversation(
        &self,
        content: AnyElement,
        window: &Window,
        cx: &Context<Self>,
    ) -> AnyElement {
        if self.active_chat_cli().is_some_and(|cli| cli.selected && (cli.metal_visible || cli.fallback.is_some())) {
            return content;
        }
        self.docked_chat(content, None, window, cx)
    }
    pub(super) fn docked_peer_conversation(
        &self,
        content: AnyElement,
        peer: &SessionSummary,
        window: &Window,
        cx: &Context<Self>,
    ) -> AnyElement {
        self.docked_chat(content, Some(peer), window, cx)
    }
    fn docked_chat(
        &self,
        content: AnyElement,
        peer: Option<&SessionSummary>,
        window: &Window,
        cx: &Context<Self>,
    ) -> AnyElement {
        let visible = peer.map_or(self.terminal_visible, |session| {
            self.parked_terminals
                .get(&workspace_ui::TerminalOwner::Chat(session.id.clone()))
                .is_some_and(|deck| deck.visible)
        });
        let resize_owner = peer.map(|session| session.id.clone());
        if !visible {
            return content;
        }
        let dock = self.terminal_dock.normalized();
        let horizontal = dock.side.horizontal();
        // Clip a full-size terminal during the reveal. Resizing the grid on
        // each animation frame would force full-screen programs to redraw.
        let body_extent = dock.extent(window.viewport_size());
        let extent = if self.split_peer().is_some() {
            body_extent
        } else {
            self.pane_extent(pane_layout::Pane::Terminal).min(body_extent)
        };
        let terminal = |this: &Self| {
            div()
                .debug_selector(|| "terminal-dock".into())
                .overflow_hidden()
                .flex_none()
                .relative()
                .min_w(px(0.))
                .min_h(px(0.))
                .when(horizontal, |s| {
                    s.w(px(extent)).max_w(relative(0.65)).h_full()
                })
                .when(!horizontal, |s| {
                    s.h(px(extent)).max_h(relative(0.65)).w_full()
                })
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .map(|s| {
                    let body = div()
                        .flex()
                        .flex_col()
                        .min_w(px(0.))
                        .min_h(px(0.))
                        .when(horizontal, |s| s.w(px(body_extent)).h_full())
                        .when(!horizontal, |s| s.h(px(body_extent)).w_full())
                        .child(this.render_terminal_panes(peer, cx));
                    if this.motion && !native::reduced_motion() {
                        if let Some(at) = this
                            .pane_layout
                            .terminal_reposition
                            .filter(|at| at.elapsed() < workspace_motion::PANE_DURATION)
                        {
                            return s.child(body.with_animation(
                                SharedString::from(format!("terminal-position-{at:?}")),
                                Animation::new(workspace_motion::PANE_DURATION),
                                |s, t| {
                                    let progress = workspace_motion::ease_out(t);
                                    s.opacity(progress).relative().top(px(8. * (1. - progress)))
                                },
                            ));
                        }
                    }
                    s.child(body)
                })
                .child(
                    div()
                        .id("terminal-resize")
                        .debug_selector(|| "terminal-resize".into())
                        .absolute()
                        .when(horizontal, |s| {
                            s.top_0()
                                .bottom_0()
                                .w(px(5.))
                                .cursor_col_resize()
                                .when(dock.side == Side::Left, |s| s.right_0())
                                .when(dock.side == Side::Right, |s| s.left_0())
                        })
                        .when(!horizontal, |s| {
                            s.left_0()
                                .right_0()
                                .h(px(5.))
                                .cursor_row_resize()
                                .when(dock.side == Side::Top, |s| s.bottom_0())
                                .when(dock.side == Side::Bottom, |s| s.top_0())
                        })
                        .hover(|s| s.bg(rgba(0x88bfff88)))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener({
                                let owner = resize_owner.clone();
                                move |app, _, window, cx| {
                                    if owner
                                        .as_deref()
                                        .is_none_or(|id| app.focus_split_chat(id, window, cx))
                                    {
                                        native::panel_drag_guard(window, true);
                                        cx.stop_propagation();
                                    }
                                }
                            }),
                        )
                        .on_drag(ResizeTerminal, |_, _, _, cx| cx.new(|_| ResizeTerminal)),
                )
        };
        let highlighted = self.terminal_dock_hover.filter(|_| peer.is_none() && cx.has_active_drag());
        div()
            .id("conversation-dock-area")
            .debug_selector(|| "conversation-dock-area".into())
            .flex_1()
            .size_full()
            .min_w(px(0.))
            .min_h(px(0.))
            .relative()
            .flex()
            .when(!horizontal, |s| s.flex_col())
            .when(visible && dock.side.before(), |s| s.child(terminal(self)))
            .child(content)
            .when(visible && !dock.side.before(), |s| s.child(terminal(self)))
            .on_drag_move(
                cx.listener(|this, event: &DragMoveEvent<MoveTerminal>, _, cx| {
                    let next = Side::target(event.bounds, event.event.position);
                    if next != this.terminal_dock_hover {
                        this.terminal_dock_hover = next;
                        cx.notify();
                    }
                }),
            )
            .on_drop(cx.listener(|this, _: &MoveTerminal, window, cx| {
                native::panel_drag_guard(window, false);
                if let Some(side) = this.terminal_dock_hover {
                    this.dock_terminal(side, cx);
                }
            }))
            .on_drag_move(
                cx.listener(|this, event: &DragMoveEvent<ResizeTerminal>, _, cx| {
                    let p = event.event.position;
                    let b = event.bounds;
                    let side = this.terminal_dock.side;
                    let value = f32::from(match side {
                        Side::Left => p.x - b.left(),
                        Side::Right => b.right() - p.x,
                        Side::Top => p.y - b.top(),
                        Side::Bottom => b.bottom() - p.y,
                    });
                    this.resize_pane(pane_layout::Pane::Terminal, value, cx);
                }),
            )
            .on_drop(cx.listener(|this, _: &ResizeTerminal, window, cx| {
                native::panel_drag_guard(window, false);
                this.queue_workspace_save(cx)
            }))
            .when_some(highlighted, |s, side| {
                s.child(
                    canvas(
                        |bounds, _, _| bounds,
                        move |bounds, _, window, _| {
                            let mut target = bounds;
                            match side {
                                Side::Left => target.size.width *= 0.28,
                                Side::Right => {
                                    target.size.width *= 0.28;
                                    target.origin.x = bounds.right() - target.size.width
                                }
                                Side::Top => target.size.height *= 0.28,
                                Side::Bottom => {
                                    target.size.height *= 0.28;
                                    target.origin.y = bounds.bottom() - target.size.height
                                }
                            };
                            window.paint_quad(fill(target, rgba(0x8dbbff38)));
                        },
                    )
                    .absolute()
                    .size_full(),
                )
            })
            .into_any_element()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    #[gpui::test]
    fn terminal_keeps_one_entity_and_real_drag_moves_it_to_all_four_edges(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        let original = cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                app.new_terminal(window, cx);
                let panel = app.active_terminal_panel().unwrap().clone();
                panel.update(cx, |p, _| p.stop());
                let id = panel.entity_id();
                app.terminal_visible = true;
                cx.notify();
                id
            })
        });
        cx.run_until_parked();
        for side in [Side::Left, Side::Top, Side::Right, Side::Bottom] {
            let area = cx.debug_bounds("conversation-dock-area").unwrap();
            let header = cx.debug_bounds("terminal-drag-handle").unwrap();
            let chrome = cx.debug_bounds("workspace-tab-strip").unwrap();
            assert!(
                header.top() >= chrome.bottom(),
                "terminal grip overlaps window chrome"
            );
            let from = header.center();
            let target = match side {
                Side::Left => point(area.left() + px(8.), area.center().y),
                Side::Right => point(area.right() - px(8.), area.center().y),
                Side::Top => point(area.center().x, area.top() + px(8.)),
                Side::Bottom => point(area.center().x, area.bottom() - px(8.)),
            };
            cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
            cx.simulate_mouse_move(
                from + point(px(20.), px(0.)),
                Some(MouseButton::Left),
                Modifiers::none(),
            );
            cx.simulate_mouse_move(target, Some(MouseButton::Left), Modifiers::none());
            cx.simulate_mouse_up(target, MouseButton::Left, Modifiers::none());
            cx.run_until_parked();
            view.update(cx, |app, cx| {
                assert_eq!(app.terminal_dock.side, side);
                assert_eq!(app.active_terminal_panel().unwrap().entity_id(), original);
                assert_eq!(app.workspace_snapshot(cx).terminal_dock.side, side);
            });
            let chat = cx.debug_bounds("chat-pane").unwrap();
            let terminal = cx.debug_bounds("terminal-dock").unwrap();
            let chrome = cx.debug_bounds("workspace-tab-strip").unwrap();
            assert!(terminal.top() >= chrome.bottom());
            assert!(chat.size.width > px(200.) && chat.size.height > px(200.));
            match side {
                Side::Left => assert!(terminal.right() <= chat.left() + px(1.)),
                Side::Right => assert!(terminal.left() >= chat.right() - px(1.)),
                Side::Top => assert!(terminal.bottom() <= chat.top() + px(1.)),
                Side::Bottom => assert!(terminal.top() >= chat.bottom() - px(1.)),
            };
        }
    }
    #[gpui::test]
    fn resize_grip_changes_the_live_panel_and_keeps_room_for_chat(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                app.new_terminal(window, cx);
                let panel = app.active_terminal_panel().unwrap().clone();
                panel.update(cx, |p, _| p.stop());
                app.terminal_visible = true;
                cx.notify();
            })
        });
        cx.run_until_parked();
        for side in Side::ALL {
            view.update(cx, |app, cx| app.dock_terminal(side, cx));
            cx.run_until_parked();
            let grip = cx.debug_bounds("terminal-resize").unwrap();
            let area = cx.debug_bounds("conversation-dock-area").unwrap();
            let from = grip.center();
            let to = match side {
                Side::Left => point(area.left() + px(260.), from.y),
                Side::Right => point(area.right() - px(260.), from.y),
                Side::Top => point(from.x, area.top() + px(180.)),
                Side::Bottom => point(from.x, area.bottom() - px(180.)),
            };
            cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
            cx.simulate_mouse_move(
                from + point(px(8.), px(8.)),
                Some(MouseButton::Left),
                Modifiers::none(),
            );
            cx.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::none());
            cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
            cx.run_until_parked();
            let after = cx.debug_bounds("terminal-dock").unwrap();
            let chat = cx.debug_bounds("chat-pane").unwrap();
            if side.horizontal() {
                assert!((f32::from(after.size.width) - 260.).abs() < 2.);
                assert!(after.size.width <= area.size.width * 0.65 + px(1.));
                assert!(chat.size.width >= area.size.width * 0.34);
            } else {
                assert!((f32::from(after.size.height) - 180.).abs() < 2.);
                assert!(chat.size.height >= area.size.height * 0.34);
            }
            view.update(cx, |app, cx| {
                let dock = app.workspace_snapshot(cx).terminal_dock;
                if side.horizontal() {
                    assert!((dock.width - 260.).abs() < 2.)
                } else {
                    assert!((dock.height - 180.).abs() < 2.)
                }
            });
        }
    }
    #[test]
    fn edge_targets_resize_limits_and_restart_state_are_bounded() {
        let bounds = Bounds::new(point(px(100.), px(50.)), size(px(1000.), px(600.)));
        for (p, expected) in [
            (point(px(110.), px(350.)), Some(Side::Left)),
            (point(px(1090.), px(350.)), Some(Side::Right)),
            (point(px(600.), px(55.)), Some(Side::Top)),
            (point(px(600.), px(645.)), Some(Side::Bottom)),
            (bounds.center(), None),
            (point(px(0.), px(0.)), None),
        ] {
            assert_eq!(Side::target(bounds, p), expected)
        }
        let invalid = Dock {
            width: f32::NAN,
            height: f32::INFINITY,
            ..Default::default()
        }
        .normalized();
        assert_eq!(invalid, Dock::default());
        for side in Side::ALL {
            let dock = Dock {
                side,
                width: 700.,
                height: 500.,
            };
            let loaded: Dock = serde_json::from_slice(&serde_json::to_vec(&dock).unwrap()).unwrap();
            assert_eq!(loaded, dock);
            assert!(dock.extent(size(px(940.), px(620.))) <= 611.);
        }
    }
}
