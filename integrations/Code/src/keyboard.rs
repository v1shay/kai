//! One command catalog for bindings, native-notch routing and discoverable help.
use super::*;
#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(namespace = kai, no_json)]
pub struct Execute {
    pub id: String,
}
pub(super) const MAX_CHAT_TABS:usize=9;
pub(super) fn tab_index(id:&str)->Option<usize>{id.strip_prefix("tab_")?.parse::<usize>().ok().filter(|n|(1..=MAX_CHAT_TABS).contains(n)).map(|n|n-1)}
pub const COMMANDS: &[(&str, &str, &str)] = &[
    ("ctrl-t", "new", "New chat"),
    ("cmd-1", "tab_1", "Select chat tab 1"),
    ("cmd-2", "tab_2", "Select chat tab 2"),
    ("cmd-3", "tab_3", "Select chat tab 3"),
    ("cmd-4", "tab_4", "Select chat tab 4"),
    ("cmd-5", "tab_5", "Select chat tab 5"),
    ("cmd-6", "tab_6", "Select chat tab 6"),
    ("cmd-7", "tab_7", "Select chat tab 7"),
    ("cmd-8", "tab_8", "Select chat tab 8"),
    ("cmd-9", "tab_9", "Select chat tab 9"),

    ("ctrl-w", "close_tab", "Close tab"),
    ("cmd-w", "close_tab", "Close tab"),
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
    ("ctrl-shift-g", "github", "GitHub repositories and pull requests"),
    ("ctrl-j", "terminal", "Terminal"),
    (
        "ctrl-shift-j",
        "dock_terminal",
        "Move terminal to next edge",
    ),
    ("ctrl-u", "usage", "Context and usage"),
    ("ctrl-o", "orchestrator", "Toggle orchestration"),
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
            Some(if tab_index(id).is_some(){"Orbit"}else{"Orbit && !CodeTerminal"}),
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
    pub(super) fn open_tab_sessions(&self)->Vec<&SessionSummary>{self.open_tabs.iter().filter_map(|id|self.sessions.iter().find(|s|&s.id==id && !self.hidden_sessions.contains(id))).collect()}
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
        if let Some(index)=tab_index(id){
            if let Some(session)=self.open_tab_sessions().into_iter().nth(index).cloned(){
                if self.active.as_ref()!=Some(&session.id){self.open_session(session,window,cx);}
            }
            return;
        }
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
            "orchestrator" => self.toggle_orchestrator(window, cx),
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
            "github"=>self.open_github(github_ui::Tab::Repository,window,cx),
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
    #[gpui::test]
    fn command_w_closes_tabs_and_keeps_the_workspace_and_empty_draft(cx:&mut TestAppContext){
        let(view,cx)=crate::ui_tests::fixture(cx);
        view.update(cx,|app,cx|{
            let mut second=app.sessions[0].clone();second.id="second-tab".into();app.sessions.push(second);
            app.open_tabs=vec!["fixture-session".into(),"second-tab".into()];app.active=Some("second-tab".into());
            app.composer.update(cx,|input,cx|input.set_text("Preserved closed-tab draft",cx));cx.notify();
        });
        crate::ui_tests::draw_frame(&view,cx);
        cx.simulate_keystrokes("cmd-w");
        view.update(cx,|app,_|{assert_eq!(app.open_tabs,["fixture-session"]);assert_eq!(app.active.as_deref(),Some("fixture-session"));assert_eq!(app.drafts["second-tab"],"Preserved closed-tab draft");assert_eq!(app.sessions.len(),2);});
        cx.simulate_keystrokes("cmd-w");
        view.update(cx,|app,cx|{assert!(app.open_tabs.is_empty());assert!(app.active.is_none());app.composer.update(cx,|input,cx|input.set_text("Keep empty-tab draft",cx));});
        cx.simulate_keystrokes("cmd-w cmd-w");
        view.update(cx,|app,cx|{assert!(app.active.is_none());assert_eq!(app.composer.read(cx).text(),"Keep empty-tab draft");});
        assert_eq!(cx.update(|_,cx|cx.windows().len()),1,"Cmd-W removed the Code window");
    }
    #[gpui::test]
    fn numbered_chat_shortcuts_follow_visible_tab_order_and_preserve_drafts(cx:&mut TestAppContext){
        let(view,cx)=crate::ui_tests::fixture(cx);
        view.update(cx,|app,cx|{
            let base=app.sessions[0].clone();app.sessions=(0..9).map(|n|{let mut s=base.clone();s.id=format!("tab-{n}");s.agent=if n%2==0{AgentKind::Codex}else{AgentKind::Gemini};s}).collect();
            app.open_tabs=vec!["missing".into()];app.open_tabs.extend((0..9).rev().map(|n|format!("tab-{n}")));app.active=Some("tab-8".into());app.composer.update(cx,|i,cx|i.set_text("Keep tab eight draft",cx));cx.notify();
        });
        crate::ui_tests::draw_frame(&view,cx);
        cx.simulate_keystrokes("cmd-9");
        view.update(cx,|app,cx|{assert_eq!(app.active.as_deref(),Some("tab-0"));assert_eq!(app.drafts["tab-8"],"Keep tab eight draft");app.composer.update(cx,|i,cx|i.set_text("Keep tab zero draft",cx));});
        cx.simulate_keystrokes("cmd-1");
        view.update(cx,|app,cx|{assert_eq!(app.active.as_deref(),Some("tab-8"));assert_eq!(app.composer.read(cx).text(),"Keep tab eight draft");assert_eq!(app.drafts["tab-0"],"Keep tab zero draft");});
        let generation=view.update(cx,|app,_|app.generation);
        cx.simulate_keystrokes("cmd-1");view.update(cx,|app,_|assert_eq!(app.generation,generation,"Selecting current tab restarted it"));
        for n in 1..=9{cx.simulate_keystrokes(&format!("cmd-{n}"));view.update(cx,|app,_|assert_eq!(app.active.as_deref(),Some(format!("tab-{}",9-n).as_str())));}
        view.update(cx,|app,cx|{app.open_tabs=vec!["tab-0".into()];app.composer.update(cx,|i,cx|i.set_text("No missing-slot navigation",cx));cx.notify();});
        cx.simulate_keystrokes("cmd-9");view.update(cx,|app,cx|{assert_eq!(app.active.as_deref(),Some("tab-0"));assert_eq!(app.composer.read(cx).text(),"No missing-slot navigation");});
    }
    #[gpui::test]
    fn numbered_native_noops_keep_captured_context(cx:&mut TestAppContext){
        let(view,cx)=crate::ui_tests::fixture(cx);
        view.update_in(cx,|app,w,cx|{
            app.active=Some("fixture-session".into());app.open_tabs=vec!["fixture-session".into()];app.unified_context=Some("Keep captured app context".into());
            for id in ["tab_1","tab_9"]{
                let request=serde_json::json!({"action":"ui_command","command":id,"ownerToken":app.unified_token(),"draftBase":app.composer.read(cx).text()});app.unified_command(&request,w,cx).unwrap();assert_eq!(app.unified_context.as_deref(),Some("Keep captured app context"));
            }
        });
    }
    #[test]
    fn command_w_and_catalog_use_unique_simple_keys() {
        let mut keys = std::collections::HashSet::new();
        for (key, id, label) in COMMANDS {
            assert!(keys.insert(key));
            assert!(!id.is_empty() && !label.is_empty());
            let parts: Vec<_> = key.split('-').collect();
            assert!(matches!(
                parts.as_slice(),
                ["ctrl", _] | ["ctrl", "shift", _] | ["cmd", _] | ["cmd", "shift", _]
            ));
        }
        assert_eq!(catalog().as_array().unwrap().len(), COMMANDS.len());
    }
}
