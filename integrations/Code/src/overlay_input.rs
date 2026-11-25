//! One input owner for all transient surfaces, including native PTY subviews.
use super::*;
impl Orbit {
    pub(super) fn overlay_owns_input(&self) -> bool {
        self.workspace_trust_open || self.session_conflict_open || self.appearance_editor.is_some()
            || self.slash.is_some()
            || self.modal.is_some()
            || self.panel_popover.is_some()
            || self.agent_picker.is_some()
            || self.action_menu.is_some()
            || self.lifecycle_editor.is_some()
            || self.checkpoint_timeline.is_some()
            || self.prompt_library_picker.is_some()
            || self.file_context_picker.is_some()
            || self.context_picker.is_some()
            || self.team_panel.is_some()
            || self.attachment_preview.is_some()
            || self.split_picker.is_some()
            || self.terminal_launcher.is_some()
            || self.chat_switcher.is_some()
            || self.github.panel.is_some()
            || self.shortcut_capture.is_some()
            || self.usage_dashboard.open
            || self
                .ninjas
                .panel
                .as_ref()
                .is_some_and(|p| p.profile.revision == 0 || p.settings)
    }
    pub(super) fn sync_overlay_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let blocked = self.overlay_owns_input();
        if self.terminal_input_blocked == blocked {
            return;
        }
        self.terminal_input_blocked = blocked;
        // A popup's text editor keeps its focus. Only evict a terminal responder.
        if blocked
            && (self
                .active_terminal_panel()
                .is_some_and(|p| p.read(cx).focus_handle(cx).is_focused(window))
                || self
                    .chat_clis
                    .values()
                    .filter_map(|c| c.fallback.as_ref())
                    .any(|(p, _)| p.read(cx).focus_handle(cx).is_focused(window)))
        {
            self.focus.focus(window);
        }
        unified::emit(&serde_json::json!({"type":"terminalInputGate","blocked":blocked}));
        self.sync_chat_terminal_owner(cx);
        self.sync_terminal_visibility(cx);
        self.sync_split_terminal_visibility(cx);
    }
}
