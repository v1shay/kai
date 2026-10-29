//! The initial surface is a preference, independent of restored chat selection.
use super::*;
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Surface {
    #[default]
    Workspace,
    Extended,
    Compact,
}
impl Surface {
    pub const ALL: [Self; 3] = [Self::Workspace, Self::Extended, Self::Compact];
    pub fn title(self) -> &'static str {
        match self {
            Self::Workspace => "Code workspace",
            Self::Extended => "Extended notch",
            Self::Compact => "Compact notch",
        }
    }
    pub fn packet(self) -> &'static str {
        match self {
            Self::Workspace => "launchWorkspace",
            Self::Extended => "launchExtended",
            Self::Compact => "launchCompact",
        }
    }
}
impl Orbit {
    pub(super) fn startup_controls(&self, cx: &Context<Self>) -> AnyElement {
        div().id("startup-settings").flex().flex_col().gap_2()
        .child(div().text_size(theme::ui_px(13.)).font_weight(FontWeight::SEMIBOLD).child("Open at startup"))
        .children(Surface::ALL.into_iter().map(|surface|div().id(surface.packet()).h(px(32.)).px_3().rounded_md().flex().items_center().gap_2().cursor_pointer().bg(rgba(if self.startup_surface==surface{0xffffff18}else{0xffffff06})).child(icon(if self.startup_surface==surface{"check"}else{"chevron-right"},13.,MUTED)).child(surface.title()).on_click(cx.listener(move|this,_,_,cx|{this.startup_surface=surface;this.queue_workspace_save(cx);cx.notify();}))))
        .child(div().id("automatic-chat-titles").h(px(32.)).px_3().rounded_md().flex().items_center().gap_2().cursor_pointer().child(icon(if self.automatic_titles{"check"}else{"chevron-right"},13.,MUTED)).child("Automatic chat titles").on_click(cx.listener(|this,_,_,cx|{this.automatic_titles=!this.automatic_titles;if !this.automatic_titles{this.title_queue.clear();this.title_work=None;}this.queue_workspace_save(cx);cx.notify();})))
        .child(div().text_size(theme::ui_px(11.)).text_color(rgb(MUTED)).child("Your selected chat and drafts are restored in every view. Automatic titles use a short, separate Codex request after the first response."))
        .into_any_element()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    #[test]
    fn missing_startup_preference_opens_workspace_and_all_choices_roundtrip() {
        let state: workspace_state::WorkspaceState = serde_json::from_str("{}").unwrap();
        assert_eq!(state.startup_surface, Surface::Workspace);
        for surface in Surface::ALL {
            let state = workspace_state::WorkspaceState {
                startup_surface: surface,
                ..Default::default()
            };
            let saved = serde_json::to_vec(&state).unwrap();
            let loaded: workspace_state::WorkspaceState = serde_json::from_slice(&saved).unwrap();
            assert_eq!(loaded.startup_surface, surface);
        }
    }
}
