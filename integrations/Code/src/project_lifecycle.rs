//! Code-owned names and organization. Never mutate a CLI's source history or
//! rename a working directory. Folder creation runs only after an explicit save.
use super::*;

#[derive(Clone)]
enum Target {
    Project(Project),
    Chat(SessionSummary),
    Create(PathBuf),
}
pub(super) struct Editor {
    target: Target,
    input: Entity<TextInput>,
    _subscription: Subscription,
    anchor: Point<Pixels>,
    pub started: Instant,
    saving: bool,
    error: Option<String>,
}

fn folder_name(value: &str) -> Result<&str, String> {
    let value = value.trim();
    if !preferences::valid_name(value)
        || matches!(value, "." | "..")
        || value.contains(['/', ':', '\\'])
    {
        return Err("Use a folder name under 256 bytes without /, : or \\.".into());
    }
    Ok(value)
}
pub(super) fn create_folder(parent: &std::path::Path, name: &str) -> Result<Project, String> {
    let name = folder_name(name)?;
    // create_dir deliberately refuses existing files, folders and symlinks.
    let path = parent.join(name);
    std::fs::create_dir(&path).map_err(|error| format!("Could not create folder: {error}"))?;
    Ok(Project {
        name: name.into(),
        path,
    })
}

impl Orbit {
    pub(super) fn project_label(&self, project: &Project) -> String {
        self.lifecycle
            .project_names
            .get(&project.path)
            .cloned()
            .unwrap_or_else(|| project.name.clone())
    }
    pub(super) fn project_path_label(&self, path: &std::path::Path) -> String {
        if let Some(name)=self.lifecycle.project_names.get(path){return name.clone()}
        // The project catalog owns imported frontend names. Folder-derived
        // labels are only a fallback for paths that aren't in that catalog.
        if let Some(project)=self.projects.iter().filter(|p|path.starts_with(&p.path))
            .max_by_key(|p|p.path.components().count()) {
            return self.project_label(project);
        }
        if let Some(project)=self.custom_projects.iter().chain(&self.source_projects).find(|p|p.path==path){
            return self.project_label(project);
        }
        path.file_name().unwrap_or_default().to_string_lossy().into_owned()
    }
    pub(super) fn chat_label(&self, session: &SessionSummary) -> String {
        self.lifecycle
            .chat_names
            .get(&session.id)
            .cloned()
            .unwrap_or_else(|| session.title.clone())
    }
    pub(super) fn refresh_archive_index(&mut self) {
        let mut ids: Vec<_> = self.lifecycle.archived_chats.iter().cloned().collect();
        ids.sort();
        if self.archive_ids != ids {
            self.archive_ids = ids;
            self.archive_generation = self.archive_generation.wrapping_add(1);
            self.archive_offset = 0;
            self.archive_page_end = 0;
            self.archive_loading = false;
        }
    }
    pub(super) fn toggle_archived_view(&mut self, cx: &mut Context<Self>) {
        self.archived_view = !self.archived_view;
        self.refresh_archive_index();
        if self.archived_view {
            self.load_archived_page(cx);
        }
        self.rebuild_workspace_sessions();
        cx.notify();
    }
    pub(super) fn load_archived_page(&mut self, cx: &mut Context<Self>) {
        if self.quit_pending
            || !self.archived_view
            || self.archive_loading
            || self.archive_offset >= self.archive_ids.len()
        {
            return;
        }
        self.archive_page_end = self
            .archive_offset
            .saturating_add(engine::ARCHIVED_PAGE_SIZE)
            .min(self.archive_ids.len());
        self.archive_loading = true;
        self.engine.load_archived(
            self.archive_ids[self.archive_offset..self.archive_page_end].to_vec(),
            self.archive_generation,
        );
        cx.notify();
    }
    fn persist_lifecycle(&mut self, cx: &mut Context<Self>) {
        // Initial preferences arrive asynchronously. The load handler merges
        // these early edits and persists the complete snapshot in FIFO order.
        if self.preferences_ready {
            self.preferences.save_lifecycle(self.lifecycle.clone());
        }
        self.refresh_archive_index();
        if self.archived_view {
            self.load_archived_page(cx);
        }
        self.rebuild_projects();
        self.rebuild_workspace_sessions();
        cx.notify();
    }
    fn edit_lifecycle(
        &mut self,
        target: Target,
        anchor: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.quit_pending {
            return;
        }
        let title = match &target {
            Target::Project(project) => self.project_label(project),
            Target::Chat(session) => self.chat_label(session),
            Target::Create(_) => String::new(),
        };
        let input = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_placeholder("Name…", cx);
            input.set_text(&title, cx);
            input
        });
        let subscription = cx.subscribe_in(&input, window, |this, _, event, window, cx| {
            if matches!(event, InputEvent::Submit) {
                this.save_lifecycle_editor(window, cx);
            }
        });
        input.read(cx).focus_handle.focus(window);
        self.modal = None;
        self.panel_popover = None;
        self.action_menu = None;
        self.lifecycle_editor = Some(Editor {
            target,
            input,
            _subscription: subscription,
            anchor,
            started: Instant::now(),
            saving: false,
            error: None,
        });
        cx.notify();
    }
    pub(super) fn new_project_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.quit_pending {
            return;
        }
        let selection = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose parent for new project".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = selection.await
                && let Some(parent) = paths.first().cloned()
            {
                let _ = this.update_in(cx, |this, window, cx| {
                    this.edit_lifecycle(
                        Target::Create(parent),
                        point(px(208.), px(88.)),
                        window,
                        cx,
                    );
                });
            }
        })
        .detach();
    }
    fn save_lifecycle_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.quit_pending {
            return;
        }
        let Some(editor) = self.lifecycle_editor.as_mut() else {
            return;
        };
        if editor.saving {
            return;
        }
        let value = editor.input.read(cx).text().trim().to_owned();
        if !preferences::valid_name(&value) {
            editor.error = Some("Enter a name under 256 bytes without control characters.".into());
            cx.notify();
            return;
        }
        match editor.target.clone() {
            Target::Project(project) => {
                self.lifecycle.project_names.insert(project.path, value);
                self.lifecycle_editor = None;
                self.composer.read(cx).focus_handle.focus(window);
                self.persist_lifecycle(cx);
            }
            Target::Chat(session) => {
                self.lifecycle.chat_names.insert(session.id, value);
                self.lifecycle_editor = None;
                self.composer.read(cx).focus_handle.focus(window);
                self.persist_lifecycle(cx);
            }
            Target::Create(parent) => {
                if let Err(error) = folder_name(&value) {
                    editor.error = Some(error);
                    cx.notify();
                    return;
                }
                editor.saving = true;
                self.pending_project_creations += 1;
                let operation = editor.started;
                cx.spawn_in(window, async move |this, cx| {
                    let result = cx
                        .background_executor()
                        .spawn(async move { create_folder(&parent, &value) })
                        .await;
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.pending_project_creations =
                            this.pending_project_creations.saturating_sub(1);
                        // Finish adding a successfully created project even when its
                        // editor was dismissed; never leave a duplicate save enabled.
                        match result {
                            Ok(project) => {
                                this.hidden_projects.remove(&project.path);
                                this.restored_projects.insert(project.path.clone());
                                this.preferences.add(project.clone());
                                this.custom_projects.retain(|p| p.path != project.path);
                                this.custom_projects.push(project.clone());
                                this.rebuild_projects();
                                if !this.quit_pending
                                    && this
                                        .lifecycle_editor
                                        .as_ref()
                                        .is_some_and(|e| e.started == operation)
                                {
                                    this.lifecycle_editor = None;
                                    this.archived_view = false;
                                    this.select_project(project.path, window, cx);
                                }
                            }
                            Err(error) => {
                                if let Some(editor) = this
                                    .lifecycle_editor
                                    .as_mut()
                                    .filter(|e| e.started == operation)
                                {
                                    editor.saving = false;
                                    editor.error = Some(error);
                                } else {
                                    this.notice = Some(error);
                                }
                            }
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
        }
    }
    pub(super) fn lifecycle_menu_actions(
        &self,
        target: &MenuTarget,
        anchor: Point<Pixels>,
        closing: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let target = match target {
            MenuTarget::Project(p) => Target::Project(p.clone()),
            MenuTarget::Chat(s) => Target::Chat(s.clone()),
        };
        let pinned = match &target {
            Target::Project(p) => self.lifecycle.pinned_projects.contains(&p.path),
            Target::Chat(s) => self.lifecycle.pinned_chats.contains(&s.id),
            Target::Create(_) => false,
        };
        let rename = target.clone();
        let pin = target.clone();
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("rename-item")
                    .debug_selector(|| "rename-item".into())
                    .h(px(30.))
                    .px_2()
                    .rounded_md()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(crate::theme::ui_px(12.))
                    .child(icon("file", 13., MUTED))
                    .child("Rename…")
                    .when(!closing, |s| {
                        s.cursor_pointer()
                            .hover(|s| s.bg(rgba(GLASS_HOVER)))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.edit_lifecycle(rename.clone(), anchor, window, cx)
                            }))
                    }),
            )
            .child(
                div()
                    .id("pin-item")
                    .debug_selector(|| "pin-item".into())
                    .h(px(30.))
                    .px_2()
                    .rounded_md()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(crate::theme::ui_px(12.))
                    .child(icon("pin", 13., MUTED))
                    .child(if pinned { "Unpin" } else { "Pin to top" })
                    .when(!closing, |s| {
                        s.cursor_pointer()
                            .hover(|s| s.bg(rgba(GLASS_HOVER)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if this.quit_pending {
                                    return;
                                }
                                match &pin {
                                    Target::Project(p) => {
                                        if !this.lifecycle.pinned_projects.remove(&p.path) {
                                            this.lifecycle.pinned_projects.insert(p.path.clone());
                                        }
                                    }
                                    Target::Chat(s) => {
                                        if !this.lifecycle.pinned_chats.remove(&s.id) {
                                            this.lifecycle.pinned_chats.insert(s.id.clone());
                                        }
                                    }
                                    _ => {}
                                }
                                this.dismiss_action_menu();
                                this.persist_lifecycle(cx);
                            }))
                    }),
            )
            .when_some(
                match target {
                    Target::Chat(s) => Some(s),
                    _ => None,
                },
                |row, session| {
                    let archived = self.lifecycle.archived_chats.contains(&session.id);
                    let enabled = self.can_delete_chat(&session.id);
                    row.child(
                        div()
                            .id("archive-chat")
                            .debug_selector(|| "archive-chat".into())
                            .h(px(30.))
                            .px_2()
                            .rounded_md()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_size(crate::theme::ui_px(12.))
                            .text_color(rgb(if enabled { TEXT } else { MUTED }))
                            .child(icon("inbox", 13., MUTED))
                            .child(if archived {
                                "Unarchive"
                            } else {
                                "Archive chat"
                            })
                            .when(!closing && enabled, |s| {
                                s.cursor_pointer()
                                    .hover(|s| s.bg(rgba(GLASS_HOVER)))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        if this.quit_pending || !this.can_delete_chat(&session.id) {
                                            return;
                                        }
                                        if !this.lifecycle.archived_chats.remove(&session.id) {
                                            this.lifecycle
                                                .archived_chats
                                                .insert(session.id.clone());
                                            this.close_tab(&session.id, window, cx);
                                        }
                                        this.dismiss_action_menu();
                                        this.persist_lifecycle(cx);
                                    }))
                            }),
                    )
                },
            )
    }
    pub(super) fn lifecycle_editor_view(&self, cx: &Context<Self>) -> impl IntoElement {
        let editor = self.lifecycle_editor.as_ref().expect("editor visible");
        let progress = if self.motion {
            presentation::spring((editor.started.elapsed().as_secs_f32() / 0.32).min(1.))
                .clamp(0., 1.)
        } else {
            1.
        };
        let create = matches!(editor.target, Target::Create(_));
        anchored()
            .position(editor.anchor + point(px(0.), px(6. * (1. - progress))))
            .snap_to_window_with_margin(px(12.))
            .child(
                div()
                    .id("lifecycle-editor")
                    .debug_selector(|| "lifecycle-editor".into())
                    .w(px(300.))
                    .p_3()
                    .rounded_xl()
                    .bg(rgba(appearance::MENU_SURFACE))
                    .border_1()
                    .border_color(rgba(0xffffff18))
                    .opacity(progress)
                    .occlude()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_size(crate::theme::ui_px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(if create {
                                "New project"
                            } else {
                                "Rename in Code"
                            }),
                    )
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(rgba(GLASS_SURFACE))
                            .child(editor.input.clone()),
                    )
                    .child(
                        div()
                            .text_size(crate::theme::ui_px(10.))
                            .text_color(rgb(MUTED))
                            .child(if create {
                                "Creates an empty folder in the selected location."
                            } else {
                                "Display name only. Files and CLI sessions stay in place."
                            }),
                    )
                    .when_some(editor.error.clone(), |s, error| {
                        s.child(
                            div()
                                .text_size(crate::theme::ui_px(11.))
                                .text_color(rgb(0xf2a6a6))
                                .child(error),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                div()
                                    .id("cancel-lifecycle")
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(rgba(GLASS_HOVER)))
                                    .text_size(crate::theme::ui_px(12.))
                                    .child("Cancel")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.lifecycle_editor = None;
                                        this.focus.focus(window);
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id("save-lifecycle")
                                    .debug_selector(|| "save-lifecycle".into())
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .bg(rgba(GLASS_HOVER))
                                    .text_size(crate::theme::ui_px(12.))
                                    .child(if editor.saving {
                                        "Creating…"
                                    } else if create {
                                        "Create"
                                    } else {
                                        "Save"
                                    })
                                    .when(!editor.saving, |s| {
                                        s.cursor_pointer().on_click(cx.listener(
                                            |this, _, window, cx| {
                                                this.save_lifecycle_editor(window, cx)
                                            },
                                        ))
                                    }),
                            ),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    #[gpui::test]
    fn renaming_and_pinning_survive_discovery_and_archived_chats_can_be_restored(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                let original = app.sessions[0].clone();
                let mut newer = original.clone();
                newer.id = "a-newer".into();
                newer.updated_at += 500;
                app.sessions.push(newer);
                app.lifecycle.pinned_chats.insert(original.id.clone());
                app.lifecycle
                    .chat_names
                    .insert(original.id.clone(), "Renamed locally".into());
                app.rebuild_workspace_sessions();
                assert_eq!(app.workspace_sessions[0].id, "a-newer", "User prompt recency takes precedence over pins");
                assert!(app.lifecycle.pinned_chats.contains(&original.id));
                app.activity.prompted(&original.id, (original.updated_at + 500) * 1000);
                app.rebuild_workspace_sessions();
                assert_eq!(app.workspace_sessions[0].id, original.id, "Pins break equal-time ties");
                assert_eq!(app.chat_label(&original), "Renamed locally");
                assert_eq!(app.sessions[0].title, "Fixture conversation");
                app.lifecycle.archived_chats.insert(original.id.clone());
                app.rebuild_workspace_sessions();
                assert!(app.workspace_sessions.iter().all(|s| s.id != original.id));
                app.archived_view = true;
                app.rebuild_workspace_sessions();
                assert_eq!(app.workspace_sessions.len(), 1);
                assert_eq!(app.workspace_sessions[0].id, original.id);
                app.lifecycle.archived_chats.remove(&original.id);
                app.rebuild_workspace_sessions();
                assert!(app.workspace_sessions.is_empty());
                app.archived_view = false;
                app.rebuild_workspace_sessions();
                app.edit_lifecycle(
                    Target::Chat(original),
                    point(px(240.), px(100.)),
                    window,
                    cx,
                );
                app.lifecycle_editor
                    .as_ref()
                    .unwrap()
                    .input
                    .update(cx, |input, cx| input.set_text("Name from editor", cx));
                app.save_lifecycle_editor(window, cx);
                assert_eq!(app.chat_label(&app.sessions[0]), "Name from editor");
                assert!(app.composer.read(cx).focus_handle.is_focused(window));
            })
        });
        crate::ui_tests::draw_frame(&view, cx);
    }
    #[gpui::test]
    fn lifecycle_editor_is_native_and_name_validation_keeps_it_open(cx: &mut gpui::TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                app.edit_lifecycle(
                    Target::Project(app.projects[0].clone()),
                    point(px(240.), px(100.)),
                    window,
                    cx,
                );
                app.lifecycle_editor
                    .as_ref()
                    .unwrap()
                    .input
                    .update(cx, |input, cx| input.set_text("   ", cx));
                app.save_lifecycle_editor(window, cx);
                assert!(app.lifecycle_editor.as_ref().unwrap().error.is_some());
                assert!(app.lifecycle.project_names.is_empty());
            })
        });
        crate::ui_tests::draw_frame(&view, cx);
        assert!(cx.debug_bounds("lifecycle-editor").is_some());
        assert!(cx.debug_bounds("save-lifecycle").is_some());
    }
    #[gpui::test]
    fn archive_pages_recover_old_summaries_and_retry_without_advancing_or_reading_transcripts(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        cx.update(|_, cx| {
            view.update(cx, |app, cx| {
                let mut old = app.sessions[0].clone();
                old.id = "archived-000".into();
                old.updated_at = 1;
                app.lifecycle.archived_chats =
                    (0..260).map(|n| format!("archived-{n:03}")).collect();
                app.toggle_archived_view(cx);
                assert_eq!(app.archive_ids.len(), 260);
                assert_eq!(app.archive_page_end, 128);
                assert!(app.archive_loading);
                let generation = app.archive_generation;
                app.receive(
                    Event::ArchivedError(generation, "Retry archive lookup".into()),
                    cx,
                );
                assert!(!app.archive_loading);
                assert_eq!(app.archive_offset, 0);
                app.load_archived_page(cx);
                assert_eq!(app.archive_page_end, 128);
                app.receive(Event::ArchivedLoaded(generation, vec![old.clone()]), cx);
                assert_eq!(app.archive_offset, 128);
                assert!(app.workspace_sessions.iter().any(|s| s.id == old.id));
                app.load_archived_page(cx);
                app.receive(
                    Event::ArchivedLoaded(generation.wrapping_sub(1), vec![]),
                    cx,
                );
                assert_eq!(app.archive_offset, 128);
                assert!(app.archive_loading);
                app.receive(Event::ArchivedLoaded(generation, vec![]), cx);
                assert_eq!(app.archive_offset, 256);
                app.load_archived_page(cx);
                assert_eq!(app.archive_page_end, 260);
                old.id = "archived-259".into();
                app.receive(Event::ArchivedLoaded(generation, vec![old]), cx);
                assert_eq!(app.archive_offset, 260);
                assert_eq!(app.workspace_sessions.len(), 2);
            })
        });
        crate::ui_tests::draw_frame(&view, cx);
    }
    #[gpui::test]
    fn creating_projects_are_tracked_after_editor_dismissal_and_quit_rejects_new_work(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        let created = cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                let parent = app.project.clone();
                let created = parent.join("Created in background");
                app.edit_lifecycle(
                    Target::Create(parent),
                    point(px(240.), px(100.)),
                    window,
                    cx,
                );
                app.lifecycle_editor
                    .as_ref()
                    .unwrap()
                    .input
                    .update(cx, |input, cx| input.set_text("Created in background", cx));
                app.save_lifecycle_editor(window, cx);
                assert_eq!(app.pending_project_creations, 1);
                app.lifecycle_editor = None;
                app.quit_pending = true;
                app.edit_lifecycle(
                    Target::Create(app.project.clone()),
                    point(px(0.), px(0.)),
                    window,
                    cx,
                );
                assert!(app.lifecycle_editor.is_none());
                assert_eq!(app.pending_project_creations, 1);
                created
            })
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            cx.run_until_parked();
            if cx.update(|_, cx| view.read(cx).pending_project_creations == 0) {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        cx.update(|_, cx| {
            let app = view.read(cx);
            assert!(created.is_dir());
            assert!(app.custom_projects.iter().any(|p| p.path == created));
            assert!(app.lifecycle_editor.is_none());
            app.preferences
                .flush()
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .unwrap();
        });
    }
    #[gpui::test]
    fn normal_quit_keeps_the_app_open_when_project_preferences_cannot_be_saved(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                let blocked = app.project.join("blocked-preferences.json");
                let prefs = preferences::Preferences::new(blocked.clone());
                assert!(matches!(
                    prefs.events.recv_blocking().unwrap(),
                    preferences::Event::Loaded(..)
                ));
                std::fs::create_dir(&blocked).unwrap();
                app.preferences = prefs;
                app.preferences.hide_session("must-save".into());
                app.quit_after_save(window, cx);
            })
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            cx.run_until_parked();
            if cx.update(|_, cx| !view.read(cx).quit_pending) {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        cx.update(|_, cx| {
            assert!(
                view.read(cx)
                    .notice
                    .as_ref()
                    .is_some_and(|n| n.contains("Could not save project and chat settings"))
            )
        });
    }
    #[gpui::test]
    fn normal_quit_waits_for_folder_work_even_when_the_editor_is_closed(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        let path = cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                let path = app.project.join("quit-state.json");
                app.workspace_store = Some(std::sync::Arc::new(
                    workspace_state::StateStore::new(path.clone()).unwrap(),
                ));
                app.pending_project_creations = 1;
                app.lifecycle_editor = None;
                cx.spawn(async move |this, cx| {
                    Timer::after(Duration::from_millis(40)).await;
                    let _ = this.update(cx, |app, cx| {
                        app.pending_project_creations = 0;
                        app.composer.update(cx, |input, cx| {
                            input.set_text("Folder completion saved", cx)
                        });
                    });
                })
                .detach();
                app.quit_after_save(window, cx);
                path
            })
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            cx.run_until_parked();
            let saved = workspace_state::StateStore::new(path.clone()).unwrap();
            if saved
                .initial()
                .drafts
                .get("__new_conversation__")
                .is_some_and(|d| d == "Folder completion saved")
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "Quit did not await the pending folder operation before its final flush"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    #[test]
    fn project_creation_preserves_existing_paths_and_rejects_traversal() {
        let root =
            std::env::temp_dir().join(format!("code-create-project-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        for name in [
            "",
            ".",
            "..",
            "../escape",
            "a/b",
            "a:b",
            "a\\b",
            "line\nbreak",
        ] {
            assert!(create_folder(&root, name).is_err(), "{name:?}");
        }
        let project = create_folder(&root, "  Crabs 🦀  ").unwrap();
        assert_eq!(project.name, "Crabs 🦀");
        std::fs::write(project.path.join("keep"), "original").unwrap();
        assert!(create_folder(&root, "Crabs 🦀").is_err());
        assert_eq!(
            std::fs::read_to_string(project.path.join("keep")).unwrap(),
            "original"
        );
        assert!(std::fs::read_dir(&root).unwrap().count() == 1);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)] mod frontend_name_tests {
    use super::*;
    use std::prelude::v1::test;
    #[gpui::test]
    fn project_path_labels_match_the_frontend_catalog_and_nested_workspaces(cx:&mut gpui::TestAppContext){
        let(app,cx)=crate::ui_tests::fixture(cx);
        app.update(cx,|app,_|{
            let root=PathBuf::from("/tmp/folder-name");
            let project=Project{path:root.clone(),name:"Frontend project".into()};
            app.projects=vec![project.clone()];
            assert_eq!(app.project_path_label(&root),app.project_label(&project));
            assert_eq!(app.project_path_label(&root.join("src")),"Frontend project");
            assert_eq!(app.project_path_label(std::path::Path::new("/tmp/folder-name-other")),"folder-name-other");
            app.lifecycle.project_names.insert(root.clone(),"Renamed in Kai".into());
            assert_eq!(app.project_path_label(&root),"Renamed in Kai");
            assert_eq!(app.project_path_label(&root.join("src")),"Renamed in Kai");
            app.projects.push(Project{path:root.join("nested"),name:"Nested project".into()});
            assert_eq!(app.project_path_label(&root.join("nested/src")),"Nested project");
        });
    }
}
