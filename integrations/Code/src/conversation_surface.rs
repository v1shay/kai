//! Presentation only. Every form renders the original Orbit in its original
//! window; no composer, transcript, approval queue or provider is transferred.
use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Form { #[default] Workspace, Notch, Mini, Menu }
impl Form {
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value { "workspace"=>Self::Workspace,"notch"=>Self::Notch,"mini"=>Self::Mini,"menu"=>Self::Menu,_=>return None })
    }
    pub fn compact(self)->bool { self != Self::Workspace }
}
impl Orbit {
    pub(super) fn set_conversation_form(&mut self, form: Form, cx: &mut Context<Self>) {
        // Deliberately no send/resume/open/history/draft/focus operation here.
        self.conversation_form=form;
        cx.notify();
    }
    pub(super) fn conversation_chrome(&self, content:AnyElement, cx:&Context<Self>)->AnyElement {
        if !self.conversation_form.compact(){return content;}
        div().flex_1().size_full().min_w(px(0.)).min_h(px(0.)).flex().flex_col()
            .child(div().h(px(32.)).flex_none().px_2().flex().items_center().gap_2()
                .child(context_ui::header_control("surface-navigation",self.surface_navigation)
                    .child(icon("sidebar",15.,TEXT))
                    .on_click(cx.listener(|app,_,_,cx|{app.surface_navigation=!app.surface_navigation;cx.notify();})))
                .child(div().flex_1().h_full().on_mouse_down(MouseButton::Left,|_,w,_|native::drag(w)))
                .child(context_ui::header_control("surface-expand",false).child(icon("panel",14.,TEXT))
                    .on_click(|_,_,_|unified::reveal()))
                .child(context_ui::header_control("surface-dismiss",false).child(icon("close",14.,TEXT))
                    .on_click(|_,_,_|unified::emit(&serde_json::json!({"type":"workspaceDismiss"})))))
            .child(content).into_any_element()
    }
}
