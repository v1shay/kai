//! Demand-created settings inputs, font discovery and background theme I/O.
use super::*;
use theme_controls::{Action, FontSlot};

#[derive(Clone, Copy, Debug)]
pub(super) enum Field {
    Color(theme::ColorSlot),
    Font(FontSlot),
}
pub(super) struct Editor {
    field: Field,
    input: Entity<TextInput>,
    error: Option<String>,
    fonts: Vec<String>,
    _subscription: Subscription,
}
impl Orbit {
    pub(super) fn prepare_appearance_settings(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.prepare_quick_shortcut_controls(cx);
        if self.appearance_search.is_none() {
            let input = cx.new(|cx| {
                let mut input = TextInput::new(window, cx);
                input.set_placeholder("Search appearance…", cx);
                input
            });
            self.appearance_search_subscription = Some(cx.subscribe(&input, |_, _, event, cx| {
                if matches!(event, InputEvent::Changed) {
                    cx.notify();
                }
            }));
            self.appearance_search = Some(input);
        }
    }
    pub(super) fn appearance_settings_panel(&self, cx: &Context<Self>) -> AnyElement {
        let query = self
            .appearance_search
            .as_ref()
            .map(|input| input.read(cx).text())
            .unwrap_or_default();
        let mut panel = div().w_full().flex().flex_col().gap_4();
        if let Some(input) = &self.appearance_search {
            panel = panel.child(
                div()
                    .id("appearance-search")
                    .debug_selector(|| "appearance-search".into())
                    .p_2()
                    .rounded_md()
                    .border_1()
                    .border_color(rgba(0xebebeb18))
                    .bg(rgba(GLASS_SURFACE))
                    .child(input.clone()),
            );
        }
        if let Some(editor) = &self.appearance_editor {
            panel = panel.child(self.appearance_editor_panel(editor, cx));
        }
        if theme_controls::matches(&query,"Startup launch window notch workspace automatic chat titles"){panel=panel.child(self.startup_controls(cx));}
        panel = panel.child(theme_controls::controls(
            &self.theme,
            &self.resolved_theme,
            &query,
            cx,
            |this, action, window, cx| this.appearance_action(action, window, cx),
        ));
        if theme_controls::matches(&query, "Glass transparency opacity blur frosted") {
            panel = panel.child(appearance::controls(
                self.appearance,
                cx,
                |this, value, window, cx| this.change_appearance(value, window, cx),
            ));
        }
        if theme_controls::matches(&query, "Wallpaper background image gif ascii haze effects") {
            panel = panel.child(
                div()
                    .id("wallpaper-settings")
                    .debug_selector(|| "wallpaper-settings".into())
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|s| s.bg(rgba(GLASS_HOVER)))
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(theme::ui_px(12.))
                    .child(icon("file-image", 14., MUTED))
                    .child("Wallpaper")
                    .child(div().flex_1())
                    .child(icon("chevron-right", 12., MUTED))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.show_modal(Modal::Background, window, cx)
                    })),
            );
        }
        if theme_controls::matches(&query, "Quick Chat global keyboard shortcut hotkey") {
            panel = panel.child(self.quick_shortcut_controls(cx));
        }
        if query.is_empty()||query.to_lowercase().contains("keyboard")||query.eq_ignore_ascii_case("commands"){
            panel=panel.child(self.keyboard_controls());
        }
        panel.into_any_element()
    }
    pub(super) fn appearance_action(
        &mut self,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            Action::EditColor(slot) => {
                self.open_appearance_editor(Field::Color(slot), window, cx);
                return;
            }
            Action::EditFont(slot) => {
                self.open_appearance_editor(Field::Font(slot), window, cx);
                return;
            }
            Action::Import => {
                self.import_theme(window, cx);
                return;
            }
            Action::Export => {
                self.export_theme(cx);
                return;
            }
            Action::Scheme(value) => self.theme.scheme = value,
            Action::Preset(value) => {
                self.theme.preset = value;
                self.theme.colors = Default::default();
            }
            Action::ClearColor(slot) => self.theme.colors.set(slot, None),
            Action::FontSize(FontSlot::Ui, value) => self.theme.ui_font_size = value,
            Action::FontSize(FontSlot::Mono, value) => self.theme.mono_font_size = value,
            Action::GlassMaterial(value)=>self.theme.glass_material=value,
            Action::ToggleTransparency => self.theme.transparency = !self.theme.transparency,
            Action::ToggleSidebarGlass => self.theme.sidebar_glass = !self.theme.sidebar_glass,
            Action::ToggleHorizontalAgentPicker => self.theme.horizontal_agent_picker = !self.theme.horizontal_agent_picker,
            Action::ToggleBorderlessComposer => self.theme.borderless_composer = !self.theme.borderless_composer,
            Action::ToggleMainGlass => self.theme.main_glass = !self.theme.main_glass,
            Action::Contrast(value) => self.theme.contrast = value,
            Action::DiffPalette(value) => self.theme.diff_palette = value,
            Action::Reset => {
                self.theme = Default::default();
                self.change_appearance(Default::default(), window, cx);
                self.appearance_editor = None;
            }
        }
        self.theme = self.theme.normalized();
        self.apply_personalization(window, cx);
        self.queue_workspace_save(cx);
        cx.notify();
    }
    fn open_appearance_editor(
        &mut self,
        field: Field,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let value = match field {
            Field::Color(slot) => theme::format_hex(
                self.theme
                    .colors
                    .get(slot)
                    .unwrap_or(self.resolved_theme.color(slot)),
            ),
            Field::Font(FontSlot::Ui) => self.theme.ui_font.clone(),
            Field::Font(FontSlot::Mono) => self.theme.mono_font.clone(),
        };
        let input = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_text(if matches!(field,Field::Font(_)){""}else{&value}, cx);
            input.set_placeholder(
                match field {
                    Field::Color(_) => "#459BF7",
                    Field::Font(_) => "Search installed fonts…",
                },
                cx,
            );
            input
        });
        let subscription =
            cx.subscribe_in(&input, window, |this, _, event, window, cx| match event {
                InputEvent::Submit => this.save_appearance_editor(window, cx),
                InputEvent::Changed => {
                    if let Some(editor) = &mut this.appearance_editor {
                        editor.error = None;
                    }
                    cx.notify();
                }
                _ => {}
            });
        // Enumerate installed fonts only when opening the font picker, never
        // in rendering, startup, the streaming loop or the settings search.
        let fonts = if matches!(field, Field::Font(_)) {
            cx.text_system().all_font_names()
        } else {
            Vec::new()
        };
        self.appearance_editor = Some(Editor {
            field,
            input: input.clone(),
            error: None,
            fonts,
            _subscription: subscription,
        });
        input.read(cx).focus_handle.focus(window);
        cx.notify();
    }
    fn save_appearance_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = &mut self.appearance_editor else {
            return;
        };
        let value = editor.input.read(cx).text();
        match editor.field {
            Field::Color(slot) => match theme::parse_hex(&value) {
                Ok(value) => self.theme.colors.set(slot, Some(value)),
                Err(error) => {
                    editor.error = Some(error.to_string());
                    cx.notify();
                    return;
                }
            },
            Field::Font(slot) => {
                let value = value.trim();
                let system = matches!(
                    value,
                    theme::UI_FONT_DEFAULT | theme::MONO_FONT_DEFAULT | "System"
                );
                let found = editor
                    .fonts
                    .iter()
                    .find(|font| font.eq_ignore_ascii_case(value));
                if value.is_empty() || value.len() > 128 || (!system && found.is_none()) {
                    editor.error = Some("Choose an installed font family from the list.".into());
                    cx.notify();
                    return;
                }
                let value = if value == "System" {
                    theme::UI_FONT_DEFAULT.to_owned()
                } else {
                    found.cloned().unwrap_or_else(|| value.to_owned())
                };
                match slot {
                    FontSlot::Ui => self.theme.ui_font = value,
                    FontSlot::Mono => self.theme.mono_font = value,
                }
            }
        }
        self.appearance_editor = None;
        self.apply_personalization(window, cx);
        self.queue_workspace_save(cx);
        self.focus.focus(window);
        cx.notify();
    }
    fn appearance_editor_panel(&self, editor: &Editor, cx: &Context<Self>) -> AnyElement {
        let title = match editor.field {
            Field::Color(slot) => slot.label(),
            Field::Font(slot) => slot.label(),
        };
        let mut view = div()
            .id("appearance-value-editor")
            .debug_selector(|| "appearance-value-editor".into())
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(rgba(0x459bf780))
            .bg(rgba(GLASS_SURFACE))
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_size(theme::ui_px(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title),
            )
            .child(
                div()
                    .id("appearance-value-input")
                    .p_2()
                    .rounded_md()
                    .bg(rgba(0xebebeb0a))
                    .child(editor.input.clone()),
            );
        if let Field::Font(_) = editor.field {
            let query = editor.input.read(cx).text().to_lowercase();
            let fonts:Vec<_>=editor.fonts.iter().filter(|font|font.to_lowercase().contains(query.trim())).cloned().collect();
            let names=uniform_list("appearance-font-options",fonts.len(),cx.processor(move |_,range:std::ops::Range<usize>,_,cx|{
                range.filter_map(|ix|{let font=fonts.get(ix)?.clone();let value=font.clone();Some(div()
                        .id(("appearance-font", ix))
                        .h(px(28.))
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .cursor_pointer()
                        .overflow_hidden()
                        .text_ellipsis()
                        .hover(|s| s.bg(rgba(GLASS_HOVER)))
                        .child(font)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if let Some(editor) = &this.appearance_editor {
                                editor
                                    .input
                                    .update(cx, |input, cx| input.set_text(&value, cx));
                            }
                            this.save_appearance_editor(window, cx);
                        }))) }).collect()
            })).h(px(140.)).w_full();
            view = view.child(names);
        }
        if let Some(error) = &editor.error {
            view = view.child(
                div()
                    .text_size(theme::ui_px(10.))
                    .text_color(rgb(0xe58c8c))
                    .child(error.clone()),
            );
        }
        view.child(
            div()
                .flex()
                .gap_2()
                .child(
                    div()
                        .id("appearance-value-save")
                        .debug_selector(|| "appearance-value-save".into())
                        .px_3()
                        .py_1()
                        .rounded_md()
                        .bg(rgba(0x459bf726))
                        .cursor_pointer()
                        .child("Apply")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.save_appearance_editor(window, cx)
                        })),
                )
                .child(
                    div()
                        .id("appearance-value-cancel")
                        .px_3()
                        .py_1()
                        .rounded_md()
                        .cursor_pointer()
                        .child("Cancel")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.appearance_editor = None;
                            this.focus.focus(window);
                            cx.notify();
                        })),
                ),
        )
        .into_any_element()
    }
    fn import_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import Code theme".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let result = cx
                .background_executor()
                .spawn(async move {
                    theme::Theme::import_file_with_appearance(&path)
                        .map_err(|error| error.to_string())
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok((theme, glass)) => {
                        this.theme = theme;
                        if let Some(glass) = glass {
                            this.change_appearance(glass, window, cx);
                        }
                        this.appearance_editor = None;
                        this.apply_personalization(window, cx);
                        this.queue_workspace_save(cx);
                    }
                    Err(error) => this.notice = Some(format!("Could not import theme: {error}")),
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn export_theme(&mut self, cx: &mut Context<Self>) {
        let directory = dirs::download_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let picked = cx.prompt_for_new_path(&directory, Some("Code-theme.json"));
        let theme = self.theme.clone();
        let appearance = self.appearance;
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(path))) = picked.await else {
                return;
            };
            let result = cx
                .background_executor()
                .spawn(async move {
                    theme
                        .export_file_with_appearance(&path, appearance)
                        .map_err(|error| error.to_string())
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Err(error) = result {
                    this.notice = Some(format!("Could not export theme: {error}"));
                }
                cx.notify();
            });
        })
        .detach();
    }
}
