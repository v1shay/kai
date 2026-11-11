//! Split groups reference the existing PTYs; moving a tab never creates a shell.
use super::*;
use terminal_dock::Side;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(super) enum Node<T = EntityId> {
    Pane { id: u64, tabs: Vec<T>, active: T },
    Split { id: u64, horizontal: bool, ratio: f32, first: Box<Node<T>>, second: Box<Node<T>> },
}
impl<T: Copy> Node<T> {
    fn remap<U: Copy>(&self, next: &mut u64, translate: &mut impl FnMut(T) -> Option<U>) -> Option<Node<U>> {
        *next += 1;
        let id = *next;
        match self {
            Self::Pane { tabs, active, .. } => {
                let active = translate(*active);
                let tabs: Vec<_> = tabs.iter().copied().filter_map(translate).collect();
                Some(Node::Pane { id, active: active.or_else(|| tabs.first().copied())?, tabs })
            }
            Self::Split { horizontal, ratio, first, second, .. } => {
                match (first.remap(next, translate), second.remap(next, translate)) {
                    (Some(first), Some(second)) => Some(Node::Split {
                        id, horizontal: *horizontal, ratio: ratio.clamp(0., 1.),
                        first: Box::new(first), second: Box::new(second),
                    }),
                    (first, second) => first.or(second),
                }
            }
        }
    }
}
impl Node {
    fn pane_for(&self, terminal: EntityId) -> Option<u64> {
        match self {
            Self::Pane { id, tabs, .. } => tabs.contains(&terminal).then_some(*id),
            Self::Split { first, second, .. } => first.pane_for(terminal).or_else(|| second.pane_for(terminal)),
        }
    }
    fn pane_mut(&mut self, pane: u64) -> Option<&mut Self> {
        match self {
            Self::Pane { id, .. } if *id == pane => Some(self),
            Self::Split { first, second, .. } => first.pane_mut(pane).or_else(|| second.pane_mut(pane)),
            _ => None,
        }
    }
    fn active_in(&self, pane: u64) -> Option<EntityId> {
        match self {
            Self::Pane { id, active, .. } => (*id == pane).then_some(*active),
            Self::Split { first, second, .. } => first.active_in(pane).or_else(|| second.active_in(pane)),
        }
    }
    fn first_active(&self) -> EntityId {
        match self { Self::Pane { active, .. } => *active, Self::Split { first, .. } => first.first_active() }
    }
    fn visible(&self, terminal: EntityId) -> bool {
        match self {
            Self::Pane { active, .. } => *active == terminal,
            Self::Split { first, second, .. } => first.visible(terminal) || second.visible(terminal),
        }
    }
    fn without(self, terminal: EntityId) -> Option<Self> {
        match self {
            Self::Pane { id, mut tabs, mut active } => {
                if let Some(index) = tabs.iter().position(|id| *id == terminal) {
                    tabs.remove(index);
                    if tabs.is_empty() { return None; }
                    if active == terminal { active = tabs[index.min(tabs.len() - 1)]; }
                }
                Some(Self::Pane { id, tabs, active })
            }
            Self::Split { id, horizontal, ratio, first, second } => match (first.without(terminal), second.without(terminal)) {
                (Some(first), Some(second)) => Some(Self::Split { id, horizontal, ratio, first: Box::new(first), second: Box::new(second) }),
                (first, second) => first.or(second),
            },
        }
    }
    fn minimum(&self) -> (f32, f32) {
        match self {
            Self::Pane { .. } => (220., 130.),
            Self::Split { horizontal, first, second, .. } => {
                let (aw, ah) = first.minimum(); let (bw, bh) = second.minimum();
                if *horizontal { (aw + bw, ah.max(bh)) } else { (aw.max(bw), ah + bh) }
            }
        }
    }
    fn resize(&mut self, split: u64, position: Point<Pixels>, bounds: Bounds<Pixels>) {
        match self {
            Self::Split { id, horizontal, ratio, first, second } if *id == split => {
                let (total, offset, a, b) = if *horizontal {
                    (f32::from(bounds.size.width), f32::from(position.x - bounds.left()), first.minimum().0, second.minimum().0)
                } else {
                    (f32::from(bounds.size.height), f32::from(position.y - bounds.top()), first.minimum().1, second.minimum().1)
                };
                if total > a + b { *ratio = (offset / total).clamp(a / total, 1. - b / total); }
                else { *ratio = a / (a + b); }
            }
            Self::Split { first, second, .. } => { first.resize(split, position, bounds); second.resize(split, position, bounds); }
            _ => {}
        }
    }
}
#[derive(Clone, Copy, PartialEq)]
struct Target { pane: u64, side: Option<Side> }
#[derive(Default)]
pub(super) struct Layout { root: Option<Node>, next: u64, hover: Option<Target>, viewport: Size<Pixels> }
impl Layout {
    fn next_id(&mut self) -> u64 { self.next += 1; self.next }
    pub fn snapshot(&self, mut id: impl FnMut(EntityId) -> Option<uuid::Uuid>) -> Option<Node<uuid::Uuid>> {
        self.root.as_ref()?.remap(&mut 0, &mut id)
    }
    pub fn restore(&mut self, root: Node<uuid::Uuid>, mut id: impl FnMut(uuid::Uuid) -> Option<EntityId>) {
        if let Some(root) = root.remap(&mut self.next, &mut id) { self.root = Some(root); }
    }
    pub fn clear_hover(&mut self) -> bool { self.hover.take().is_some() }
    fn minimum(&self) -> (f32, f32) { self.root.as_ref().map_or((220., 130.), Node::minimum) }
    pub fn insert(&mut self, terminal: EntityId, anchor: Option<EntityId>) {
        let pane = self.root.as_ref().and_then(|root| anchor.and_then(|anchor| root.pane_for(anchor)).or_else(|| root.pane_for(root.first_active())));
        if let Some(Node::Pane { tabs, active, .. }) = pane.and_then(|pane| self.root.as_mut()?.pane_mut(pane)) {
            tabs.push(terminal); *active = terminal;
        } else {
            let id = self.next_id();
            self.root = Some(Node::Pane { id, tabs: vec![terminal], active: terminal });
        }
    }
    pub fn select(&mut self, terminal: EntityId) {
        let pane = self.root.as_ref().and_then(|root| root.pane_for(terminal));
        if let Some(Node::Pane { active, .. }) = pane.and_then(|pane| self.root.as_mut()?.pane_mut(pane)) { *active = terminal; }
    }
    pub fn remove(&mut self, terminal: EntityId) -> Option<EntityId> {
        let pane = self.root.as_ref().and_then(|root| root.pane_for(terminal));
        self.root = self.root.take().and_then(|root| root.without(terminal));
        self.hover = None;
        self.root.as_ref().map(|root| pane.and_then(|pane| root.active_in(pane)).unwrap_or_else(|| root.first_active()))
    }
    pub fn visible(&self, terminal: EntityId) -> bool { self.root.as_ref().is_some_and(|root| root.visible(terminal)) }
    fn move_tab(&mut self, terminal: EntityId, target: Target) -> bool {
        let Some(root) = &self.root else { return false; };
        let Some(source) = root.pane_for(terminal) else { return false; };
        if root.active_in(target.pane).is_none() { return false; }
        if source == target.pane {
            let Some(Node::Pane { tabs, .. }) = self.root.as_mut().and_then(|root| root.pane_mut(source)) else { return false; };
            if target.side.is_none() || tabs.len() == 1 { self.select(terminal); return true; }
        }
        self.remove(terminal);
        let new_pane = self.next_id(); let split = self.next_id();
        let Some(node) = self.root.as_mut().and_then(|root| root.pane_mut(target.pane)) else { return false; };
        if let Some(side) = target.side {
            let original = Box::new(node.clone());
            let moved = Box::new(Node::Pane { id: new_pane, tabs: vec![terminal], active: terminal });
            let (first, second) = if side.before() { (moved, original) } else { (original, moved) };
            *node = Node::Split { id: split, horizontal: side.horizontal(), ratio: 0.5, first, second };
        } else if let Node::Pane { tabs, active, .. } = node {
            tabs.push(terminal); *active = terminal;
        }
        true
    }
}
#[derive(Clone)]
struct TabDrag { terminal: EntityId, title: SharedString }
impl Render for TabDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().px_3().py_2().rounded_md().bg(rgba(0x242424ee)).text_color(rgb(TEXT)).text_size(theme::ui_px(11.)).child(self.title.clone())
    }
}
#[derive(Clone, Copy)]
struct SplitDrag { split: u64 }
impl Render for SplitDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement { div().size(px(1.)) }
}
impl Orbit {
    pub(super) fn render_terminal_panes(&self, peer: Option<&SessionSummary>, cx: &Context<Self>) -> AnyElement {
        let deck = peer.and_then(|session| self.parked_terminals.get(&workspace_ui::TerminalOwner::Chat(session.id.clone())));
        let (layout, sessions) = deck.map_or((&self.terminal_panes, self.terminals.as_slice()), |deck| (&deck.layout, deck.sessions.as_slice()));
        let Some(root) = layout.root.as_ref() else { return div().into_any_element(); };
        let minimum = root.minimum();
        let available = size(px(f32::from(layout.viewport.width).max(minimum.0)), px(f32::from(layout.viewport.height).max(minimum.1)));
        let owner = cx.entity();
        let peer_id = peer.map(|session| session.id.clone());
        div().id("terminal-pane-viewport").size_full().min_w(px(0.)).min_h(px(0.)).relative().overflow_x_scroll().overflow_y_scroll()
            .on_drag_move(cx.listener(|app,event: &DragMoveEvent<TabDrag>,_,cx| {
                if !event.bounds.contains(&event.event.position) {
                    if app.terminal_panes.clear_hover() { cx.notify(); }
                    cx.stop_propagation();
                }
            }))
            .child(div().w(available.width).h(available.height).flex_none().child(self.render_terminal_node(root, available, sessions, layout, peer, cx)))
            .child(canvas(move |bounds,_,cx| {
                owner.update(cx, |app,cx| {
                    let layout = if let Some(id) = peer_id.as_ref() {
                        let Some(deck) = app.parked_terminals.get_mut(&workspace_ui::TerminalOwner::Chat(id.clone())) else { return; };
                        &mut deck.layout
                    } else { &mut app.terminal_panes };
                    if layout.viewport != bounds.size { layout.viewport = bounds.size; cx.notify(); }
                });
            }, |_,_,_,_|{}).absolute().size_full())
            .into_any_element()
    }
    fn render_terminal_node(&self, node: &Node, available: Size<Pixels>, sessions: &[terminal_ui::Session], layout: &Layout, peer: Option<&SessionSummary>, cx: &Context<Self>) -> AnyElement {
        match node {
            Node::Pane { id, tabs, active } => self.terminal_pane(*id, tabs, *active, sessions, layout, peer, cx),
            Node::Split { id, horizontal, ratio, first, second } => {
                let split = *id; let horizontal = *horizontal;
                let (a,b,total) = if horizontal { (first.minimum().0, second.minimum().0, f32::from(available.width)) } else { (first.minimum().1, second.minimum().1, f32::from(available.height)) };
                let ratio = if total > a + b { ratio.clamp(a / total, 1. - b / total) } else { a / (a + b) };
                let first_size = if horizontal { size(available.width * ratio, available.height) } else { size(available.width, available.height * ratio) };
                let second_size = if horizontal { size(available.width - first_size.width, available.height) } else { size(available.width, available.height - first_size.height) };
                div().id(("terminal-split", split as usize)).size_full().min_w(px(0.)).min_h(px(0.)).relative().flex()
                    .when(!horizontal, |s| s.flex_col())
                    .child(div().flex_none().min_w(px(0.)).min_h(px(0.)).overflow_hidden()
                        .when(horizontal, |s| s.w(relative(ratio)).h_full())
                        .when(!horizontal, |s| s.h(relative(ratio)).w_full()).child(self.render_terminal_node(first, first_size, sessions, layout, peer, cx)))
                    .child(div().flex_1().min_w(px(0.)).min_h(px(0.)).overflow_hidden()
                        .when(horizontal, |s| s.h_full()).when(!horizontal, |s| s.w_full()).child(self.render_terminal_node(second, second_size, sessions, layout, peer, cx)))
                    .child(div().id(("terminal-split-grip", split as usize)).debug_selector(move ||format!("terminal-split-grip-{split}"))
                        .absolute().bg(rgba(0xffffff18)).hover(|s| s.bg(rgba(0x88bfff88)))
                        .when(horizontal, |s| s.left(relative(ratio)).ml(px(-2.)).top_0().bottom_0().w(px(5.)).cursor_col_resize())
                        .when(!horizontal, |s| s.top(relative(ratio)).mt(px(-2.)).left_0().right_0().h(px(5.)).cursor_row_resize())
                        .on_mouse_down(MouseButton::Left, |_, window, cx| { native::panel_drag_guard(window,true); cx.stop_propagation(); })
                        .on_drag(SplitDrag { split }, |drag,_,_,cx| cx.new(|_| *drag)))
                    .on_drag_move(cx.listener(move |app, event: &DragMoveEvent<SplitDrag>, _, cx| {
                        if event.drag(cx).split == split {
                            if let Some(root) = &mut app.terminal_panes.root { root.resize(split, event.event.position, event.bounds); }
                            cx.notify();
                        }
                    }))
                    .on_drop(cx.listener(|app, _: &SplitDrag, window, cx| {
                        native::panel_drag_guard(window, false);
                        app.queue_workspace_save(cx);
                    }))
                    .into_any_element()
            }
        }
    }
    fn terminal_pane(&self, pane: u64, tabs: &[EntityId], active: EntityId, sessions: &[terminal_ui::Session], layout: &Layout, peer: Option<&SessionSummary>, cx: &Context<Self>) -> AnyElement {
        let peer_id = peer.map(|session| session.id.clone());
        let highlight = layout.hover.filter(|target| target.pane == pane && cx.has_active_drag());
        div().id(("terminal-pane",pane as usize)).debug_selector(move ||format!("terminal-pane-{pane}"))
            .size_full().min_w(px(0.)).min_h(px(0.)).flex().flex_col().relative().overflow_hidden()
            .capture_any_mouse_down(cx.listener(move |app,event: &MouseDownEvent,window,cx| {
                if event.button == MouseButton::Left {
                    if !peer_id.as_deref().is_none_or(|id| app.focus_split_chat(id, window, cx)) { return; }
                    if app.active_terminal != Some(active) { app.select_terminal(active,window,cx); }
                }
            }))
            .child(div().h(px(32.)).flex_none().w_full().min_w(px(0.)).flex().items_center().gap_1().px_1()
                .child(div().id(("terminal-session-tabs",pane as usize)).flex_1().min_w(px(0.)).h_full().flex().items_center().gap_1().overflow_x_scroll()
                    .children(tabs.iter().filter_map(|id| sessions.iter().find(|session|session.panel.entity_id() == *id)).map(|session| {
                        let id=session.panel.entity_id(); let number=session.number; let panel=session.panel.clone();
                        let title=session.title(cx);
                        let drag=TabDrag {terminal:id,title:title.clone().into()};
                        div().id(("terminal-session",number as usize)).debug_selector(move ||format!("terminal-session-{number}"))
                            .flex_none().h(px(26.)).px_2().flex().items_center().gap_2().rounded_md().cursor_pointer()
                            .text_size(theme::ui_px(11.)).text_color(rgb(MUTED))
                            .when(active == id,|s|s.bg(rgba(0xffffff18)).text_color(rgb(TEXT))).hover(|s|s.bg(rgba(0xffffff10)))
                            .child(div().max_w(px(240.)).min_w(px(0.)).truncate().child(title))
                            .child(div().id(("close-terminal-session",number as usize)).p_1().cursor_pointer().child(icon("close",10.,MUTED))
                                .on_mouse_down(MouseButton::Left,|_,_,cx|cx.stop_propagation())
                                .on_click(move|_,_,cx|{cx.stop_propagation();panel.update(cx,|panel,cx|panel.close(cx));}))
                            .on_mouse_down(MouseButton::Left,|_,window,cx|{native::panel_drag_guard(window,true);cx.stop_propagation();})
                            .on_drag(drag, {let owner=cx.weak_entity();move|drag,_,_,cx|{
                                let _=owner.update(cx,|app,cx|{app.terminal_panes.hover=None;cx.notify();});
                                cx.new(|_|drag.clone())
                            }})
                            .on_click(cx.listener(move|app,_,window,cx|app.select_terminal(id,window,cx)))
                    })))
                .child(div().id(("new-terminal-session",pane as usize)).debug_selector(move ||format!("new-terminal-session-{pane}"))
                    .flex_none().size(px(26.)).flex().items_center().justify_center().rounded_md().cursor_pointer().hover(|s|s.bg(rgba(0xffffff14)))
                    .child(icon("plus",13.,TEXT)).on_click(cx.listener(move|app,_,window,cx|{app.select_terminal(active,window,cx);app.new_terminal(window,cx);}))))
            .when_some(sessions.iter().find(|session|session.panel.entity_id()==active), |s,session|
                s.child(div().flex_1().min_h(px(0.)).child(session.panel.clone())))
            .on_drag_move(cx.listener(move|app,event: &DragMoveEvent<TabDrag>,_,cx|{
                if !event.bounds.contains(&event.event.position) || !app.terminals.iter().any(|session| session.panel.entity_id() == event.drag(cx).terminal) { return; }
                let target=Target {pane,side:Side::target(event.bounds,event.event.position)};
                if app.terminal_panes.hover != Some(target) {app.terminal_panes.hover=Some(target);cx.notify();}
                cx.stop_propagation();
            }))
            .on_drop(cx.listener(move|app,drag: &TabDrag,window,cx|{
                native::panel_drag_guard(window,false);
                if let Some(target)=app.terminal_panes.hover.filter(|target|target.pane==pane) {
                    if app.terminal_panes.move_tab(drag.terminal,target) {
                        let minimum=app.terminal_panes.minimum();
                        if app.terminal_dock.side.horizontal() {app.terminal_dock.width=app.terminal_dock.width.max(minimum.0);} else {app.terminal_dock.height=app.terminal_dock.height.max(minimum.1);}
                        app.select_terminal(drag.terminal,window,cx);
                        app.queue_workspace_save(cx);
                    }
                }
                app.terminal_panes.hover=None;
                cx.notify();cx.stop_propagation();
            }))
            .when_some(highlight, |s,target|s.child(canvas(|bounds,_,_|bounds,move|mut bounds,_,window,_|{
                if let Some(side)=target.side {match side {
                    Side::Left=>bounds.size.width*=0.5,
                    Side::Right=>{bounds.origin.x+=bounds.size.width*0.5;bounds.size.width*=0.5;},
                    Side::Top=>bounds.size.height*=0.5,
                    Side::Bottom=>{bounds.origin.y+=bounds.size.height*0.5;bounds.size.height*=0.5;},
                }}
                window.paint_quad(fill(bounds,rgba(0x8dbbff38)));
            }).absolute().size_full()))
            .into_any_element()
    }
}
