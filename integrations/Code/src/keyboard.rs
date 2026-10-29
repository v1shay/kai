//! One command catalog for bindings, native-notch routing and discoverable help.
use super::*;
#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(namespace = kai, no_json)]
pub struct Execute {
    pub id: String,
}
pub const COMMANDS: &[(&str, &str, &str)] = &[
    ("ctrl-t", "new", "New chat"),
    ("ctrl-w", "close_tab", "Close tab"),
    ("ctrl-l", "composer", "Focus message"),
    ("ctrl-shift-l", "conversation", "Focus conversation"),
    ("ctrl-m", "model", "Choose agent and model"),
    ("ctrl-shift-m", "thinking", "Thinking level"),
    ("ctrl-p", "projects", "Switch project"),
    ("ctrl-shift-p", "fold_project", "Fold project"),
    ("ctrl-k", "search", "Search chats"),
    ("ctrl-b", "sidebar", "Project sidebar"),
    ("ctrl-shift-b", "conversations", "Conversation sidebar"),
    ("ctrl-e", "files", "Files"),
    ("ctrl-g", "changes", "Source control"),
    ("ctrl-j", "terminal", "Terminal"),
    (
        "ctrl-shift-j",
        "dock_terminal",
        "Move terminal to next edge",
    ),
    ("ctrl-u", "usage", "Context and usage"),
    ("ctrl-o", "orchestrator", "New orchestrator"),
    ("ctrl-r", "dictate", "Dictate"),
    ("ctrl-shift-r", "voice", "Real-time voice"),
    ("ctrl-shift-a", "children", "Subagents"),
    ("ctrl-f", "fold_work", "Fold latest work"),
    ("ctrl-n", "next_tab", "Next tab"),
    ("ctrl-shift-n", "previous_tab", "Previous tab"),
    ("ctrl-shift-t", "next_project", "Next project"),
    ("ctrl-shift-y", "previous_project", "Previous project"),
    ("ctrl-shift-x", "stop", "Stop agent"),
    ("ctrl-shift-h", "keyboard", "Keyboard shortcuts"),
    ("cmd-shift-o", "open_project", "Open folder"),
    ("cmd-shift-n", "new", "New chat"),
    ("cmd-shift-k", "notch", "Return to notch"),
];
pub fn bind(cx: &mut App) {
    cx.bind_keys(COMMANDS.iter().map(|(key, id, _)| {
        KeyBinding::new(
            key,
            Execute { id: (*id).into() },
            Some("Orbit && !CodeTerminal"),
        )
    }));
}
pub fn catalog() -> serde_json::Value {
    serde_json::json!(
        COMMANDS
            .iter()
            .map(|(key, id, label)| serde_json::json!({"key":key,"id":id,"label":label}))
            .collect::<Vec<_>>()
    )
}
impl Orbit {
    pub(super) fn keyboard_controls(&self) -> AnyElement {
        div()
            .id("keyboard-help")
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Keyboard shortcuts"),
            )
            .children(COMMANDS.iter().map(|(key, _, label)| {
                div()
                    .flex()
                    .justify_between()
                    .py_1()
                    .gap_4()
                    .text_size(theme::ui_px(12.))
                    .child(*label)
                    .child(
                        div().text_color(rgb(MUTED)).child(
                            key.replace("ctrl", "⌃")
                                .replace("cmd", "⌘")
                                .replace("shift", "⇧")
                                .replace('-', " ")
                                .to_uppercase(),
                        ),
                    )
            }))
            .into_any_element()
    }
    pub(super) fn execute_shortcut(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match id {
            "new" => self.new_conversation(window, cx),
            "close_tab" => {
                if let Some(id) = self.active.clone() {
                    self.close_tab(&id, window, cx)
                }
            }
            "composer" => self.composer.read(cx).focus_handle.focus(window),
            "conversation" => self.focus.focus(window),
            "model" | "thinking" => {
                let same = self
                    .agent_picker
                    .as_ref()
                    .is_some_and(|p| !p.closing && p.effort_only == (id == "thinking"));
                self.picker_at_pointer_down = if same { Some(id == "thinking") } else { None };
                let p = point(
                    window.viewport_size().width - px(300.),
                    window.viewport_size().height - px(80.),
                );
                self.toggle_picker_from_click(id == "thinking", p, window, cx);
            }
            "projects" => self.show_modal(Modal::Projects, window, cx),
            "search" => self.show_modal(Modal::Search, window, cx),
            "sidebar" => self.toggle_sidebar(),
            "conversations" => self.toggle_conversations(cx),
            "files" => self.toggle_files(window, cx),
            "changes" => self.toggle_diffs(cx),
            "terminal" => self.toggle_terminal(window, cx),
            "dock_terminal" => {
                let sides = terminal_dock::Side::ALL;
                let i = sides
                    .iter()
                    .position(|s| *s == self.terminal_dock.side)
                    .unwrap_or(0);
                self.dock_terminal(sides[(i + 1) % sides.len()], cx)
            }
            "usage" => self.show_modal(Modal::Status, window, cx),
            "orchestrator" => self.start_orchestrator(window, cx),
            "dictate" => self.toggle_dictation(window, cx),
            "voice" => self.toggle_voice(window, cx),
            "children" => self.toggle_children(window, cx),
            "stop" => self.interrupt_native_session(cx),
            "fold_project" => {
                let path = self.project.clone();
                self.toggle_project_branch(path, cx)
            }
            "fold_work" => {
                if let Some(i) = self.flow_index.work.iter().rev().find_map(|span|span.as_ref().map(|range|range.start)) {
                    self.toggle_work(i, cx)
                }
            }
            "next_tab" | "previous_tab" => {
                let n = self.open_tabs.len();
                if n > 0 {
                    let i = self
                        .active
                        .as_ref()
                        .and_then(|id| self.open_tabs.iter().position(|s| s == id))
                        .unwrap_or(0);
                    let step = if id == "next_tab" { 1 } else { n - 1 };
                    if let Some(session) = self
                        .sessions
                        .iter()
                        .find(|s| s.id == self.open_tabs[(i + step) % n])
                        .cloned()
                    {
                        self.open_session(session, window, cx)
                    }
                }
            }
            "next_project" | "previous_project" => {
                let n = self.projects.len();
                if n > 0 {
                    let i = self
                        .projects
                        .iter()
                        .position(|p| p.path == self.project)
                        .unwrap_or(0);
                    let step = if id == "next_project" { 1 } else { n - 1 };
                    self.select_project(self.projects[(i + step) % n].path.clone(), window, cx)
                }
            }
            "keyboard" => {
                self.show_modal(Modal::Settings, window, cx);
                if let Some(input) = &self.appearance_search {
                    input.update(cx, |i, cx| i.set_text("Keyboard", cx));
                }
            }
            "open_project" => self.open_folder(window, cx),
            "notch" => unified::collapse(),
            _ => return,
        }
        cx.notify();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    #[gpui::test]
    fn keyboard_navigation_preserves_chat_drafts_and_focuses_the_existing_composer(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                app.active = Some("fixture-session".into());
                app.composer
                    .update(cx, |i, cx| i.set_text("Keep my original draft", cx));
                app.execute_shortcut("new", window, cx);
                assert!(app.active.is_none());
                assert_eq!(
                    app.drafts.get("fixture-session").map(String::as_str),
                    Some("Keep my original draft")
                );
                app.execute_shortcut("composer", window, cx);
                assert!(app.composer.read(cx).focus_handle.is_focused(window));
                app.execute_shortcut("keyboard", window, cx);
                assert_eq!(
                    app.appearance_search.as_ref().unwrap().read(cx).text(),
                    "Keyboard"
                );
            })
        });
    }
    #[test]
    fn catalog_has_unique_simple_keys_and_ids_are_shared() {
        let mut keys = std::collections::HashSet::new();
        for (key, id, label) in COMMANDS {
            assert!(keys.insert(key));
            assert!(!id.is_empty() && !label.is_empty());
            let parts: Vec<_> = key.split('-').collect();
            assert!(matches!(
                parts.as_slice(),
                ["ctrl", _] | ["ctrl", "shift", _] | ["cmd", "shift", _]
            ));
        }
        assert_eq!(catalog().as_array().unwrap().len(), COMMANDS.len());
    }
}
