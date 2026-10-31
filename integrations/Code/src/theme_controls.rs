//! Native, searchable appearance controls. The workspace owns editor focus,
//! persistence and file dialogs; this view only emits explicit user actions.
use crate::theme::{self, ColorSlot, Contrast, DiffPalette, Preset, ResolvedTheme, Scheme, Theme};
use gpui::{prelude::*, *};
use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontSlot {
    Ui,
    Mono,
}
impl FontSlot {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ui => "Interface font",
            Self::Mono => "Code font",
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Scheme(Scheme),
    Preset(Preset),
    EditColor(ColorSlot),
    ClearColor(ColorSlot),
    EditFont(FontSlot),
    FontSize(FontSlot, f32),
    GlassMaterial(theme::GlassMaterial),
    ToggleTransparency,
    ToggleSidebarGlass,
    ToggleMainGlass,
    ToggleBorderlessComposer,
    ToggleHorizontalAgentPicker,
    Contrast(Contrast),
    DiffPalette(DiffPalette),
    Reset,
    Import,
    Export,
}
type Callback<T> = Rc<dyn Fn(&mut T, Action, &mut Window, &mut Context<T>)>;

/// Search is passed in from a normal TextInput. Filtering searches setting
/// titles and aliases; it doesn't hide the only way to reset/import a theme.
pub fn controls<T: 'static>(
    settings: &Theme,
    resolved: &ResolvedTheme,
    query: &str,
    cx: &Context<T>,
    on_action: impl Fn(&mut T, Action, &mut Window, &mut Context<T>) + 'static,
) -> AnyElement {
    let callback: Callback<T> = Rc::new(on_action);
    let mut view = div()
        .id("theme-controls")
        .debug_selector(|| "theme-controls".into())
        .w_full()
        .min_w(px(0.))
        .flex()
        .flex_col()
        .gap_4()
        .text_color(gpui::rgb(resolved.foreground))
        .text_size(theme::ui_px(12.));
    let mut count = 0;
    if matches(query, "Appearance mode system dark light theme") {
        count += 1;
        let mut row = div().flex().gap_1();
        for (value, label) in [
            (Scheme::System, "System"),
            (Scheme::Dark, "Dark"),
            (Scheme::Light, "Light"),
        ] {
            row = row.child(button(
                format!("theme-scheme-{label}"),
                label,
                Action::Scheme(value),
                settings.scheme == value,
                cx,
                callback.clone(),
            ));
        }
        view = view.child(section("Appearance", row.into_any_element()));
    }
    if matches(
        query,
        "Theme palette preset default harbor moss ember orchid tidepool dune rosewood glacier arcade ink",
    ) {
        count += 1;
        let mut row = div().flex().flex_wrap().gap_1();
        for preset in Preset::ALL {
            let (background, foreground, accent) = theme::preset_preview(preset, resolved.dark);
            let selected = settings.preset == preset;
            let action = Action::Preset(preset);
            let change = callback.clone();
            row = row.child(
                div()
                    .id(SharedString::from(format!(
                        "theme-preset-{}",
                        preset.label()
                    )))
                    .debug_selector(move || format!("theme-preset-{}", preset.label()))
                    .w(theme::ui_px(86.))
                    .p_2()
                    .rounded_lg()
                    .border_1()
                    .border_color(gpui::rgba(if selected {
                        (resolved.accent << 8) | 0xff
                    } else {
                        (resolved.border << 8) | 0x80
                    }))
                    .bg(gpui::rgb(background))
                    .text_color(gpui::rgb(foreground))
                    .cursor_pointer()
                    .hover(|s| s.border_color(gpui::rgb(resolved.accent)))
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .items_center()
                            .child(div().size(px(8.)).rounded_full().bg(gpui::rgb(accent)))
                            .child(div().text_size(theme::ui_px(10.)).child(preset.label())),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        change(this, action.clone(), window, cx)
                    })),
            );
        }
        view = view.child(section("Palette", row.into_any_element()));
    }
    if matches(
        query,
        "Colors accent background foreground text sidebar cards code custom hex",
    ) {
        count += 1;
        let mut rows = div().flex().flex_col().gap_2();
        for slot in ColorSlot::ALL {
            let color = resolved.color(slot);
            let edit = callback.clone();
            let clear = callback.clone();
            rows = rows.child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().child(slot.label()))
                    .child(
                        div()
                            .id(SharedString::from(format!("theme-color-{}", slot.label())))
                            .debug_selector(move || format!("theme-color-{}", slot.label()))
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(gpui::rgba(resolved.hover_rgba()))
                            .cursor_pointer()
                            .child(
                                div()
                                    .size(px(14.))
                                    .rounded_sm()
                                    .border_1()
                                    .border_color(gpui::rgb(resolved.border))
                                    .bg(gpui::rgb(color)),
                            )
                            .child(theme::format_hex(color))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                edit(this, Action::EditColor(slot), window, cx)
                            })),
                    )
                    .when(settings.colors.get(slot).is_some(), |s| {
                        s.child(
                            div()
                                .id(SharedString::from(format!(
                                    "theme-color-reset-{}",
                                    slot.label()
                                )))
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .text_size(theme::ui_px(10.))
                                .text_color(gpui::rgb(resolved.muted))
                                .cursor_pointer()
                                .hover(|s| s.bg(gpui::rgba(resolved.hover_rgba())))
                                .child("Reset")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    clear(this, Action::ClearColor(slot), window, cx)
                                })),
                        )
                    }),
            );
        }
        view = view.child(section("Colors", rows.into_any_element()));
    }
    if matches(
        query,
        "Typography font family interface text code monospace size",
    ) {
        count += 1;
        let mut rows = div().flex().flex_col().gap_2();
        for (slot, font, size, min, max) in [
            (
                FontSlot::Ui,
                settings.ui_font.as_str(),
                settings.ui_font_size,
                10.,
                24.,
            ),
            (
                FontSlot::Mono,
                settings.mono_font.as_str(),
                settings.mono_font_size,
                9.,
                24.,
            ),
        ] {
            rows = rows.child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(theme::ui_px(11.))
                            .text_color(gpui::rgb(resolved.muted))
                            .child(slot.label()),
                    )
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(button(
                                format!("theme-font-{slot:?}"),
                                if slot == FontSlot::Ui && font == theme::UI_FONT_DEFAULT {
                                    "System"
                                } else {
                                    font
                                },
                                Action::EditFont(slot),
                                false,
                                cx,
                                callback.clone(),
                            ))
                            .child(div().flex_1())
                            .child(button(
                                format!("theme-font-minus-{slot:?}"),
                                "−",
                                Action::FontSize(slot, (size - 0.5).max(min)),
                                false,
                                cx,
                                callback.clone(),
                            ))
                            .child(div().w(px(38.)).text_center().child(if size.fract() == 0. {
                                format!("{size:.0}")
                            } else {
                                format!("{size:.1}")
                            }))
                            .child(button(
                                format!("theme-font-plus-{slot:?}"),
                                "+",
                                Action::FontSize(slot, (size + 0.5).min(max)),
                                false,
                                cx,
                                callback.clone(),
                            )),
                    ),
            );
        }
        rows = rows
            .child(
                div()
                    .mt_1()
                    .text_size(px(settings.ui_font_size))
                    .font_family(SharedString::from(settings.ui_font.clone()))
                    .child("The quick brown fox"),
            )
            .child(
                div()
                    .text_size(px(settings.mono_font_size))
                    .font_family(SharedString::from(settings.mono_font.clone()))
                    .child("let code = \"yours\";"),
            );
        view = view.child(section("Typography", rows.into_any_element()));
    }
    if matches(query,"Glass material classic Liquid macOS automatic") {
        count+=1;
        let available=crate::native::liquid_available();
        let row=div().flex().flex_wrap().gap_1().children(theme::GlassMaterial::ALL.into_iter().filter(|v|*v!=theme::GlassMaterial::Liquid||available).map(|value|button(format!("glass-material-{value:?}"),value.label(),Action::GlassMaterial(value),settings.glass_material==value,cx,callback.clone())));
        view=view.child(section("Native glass",div().flex().flex_col().gap_2().child(row).child(div().text_size(theme::ui_px(10.)).text_color(gpui::rgb(resolved.muted)).child(if available{"Automatic uses Liquid Glass. Classic keeps the Gaussian desktop backdrop."}else{"Liquid Glass becomes available on macOS 26. Automatic uses classic blur on this Mac."})).into_any_element()));
    }
    if matches(query,"Agent provider model thinking intensity picker horizontal vertical columns layout"){
        count+=1;
        view=view.child(section("Agent picker",div().flex().flex_col().gap_2()
            .child(toggle("Horizontal agent picker",settings.horizontal_agent_picker,Action::ToggleHorizontalAgentPicker,cx,callback.clone()))
            .child(div().text_size(theme::ui_px(10.)).text_color(gpui::rgb(resolved.muted)).child("Providers, models and intensity from left to right. Turn off for a vertical layout. Arrow keys work in both."))
            .into_any_element()));
    }
    if matches(query, "Composer prompt boxes controls borderless invisible frames buttons outlines") {
        count += 1;
        view = view.child(section("Composer", div().flex().flex_col().gap_2()
            .child(toggle("Borderless prompt controls", settings.borderless_composer, Action::ToggleBorderlessComposer, cx, callback.clone()))
            .child(div().text_size(theme::ui_px(10.)).text_color(gpui::rgb(resolved.muted))
                .child("Hide control backgrounds in Code, Quick Chat and the notch. Keep the prompt’s outer border and the same click targets."))
            .into_any_element()));
    }
    if matches(
        query,
        "Glass transparency opaque blur window sidebar main pane panel",
    ) {
        count += 1;
        let rows = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(toggle(
                "Transparent window",
                settings.transparency,
                Action::ToggleTransparency,
                cx,
                callback.clone(),
            ))
            .child(toggle(
                "Sidebar glass",
                settings.sidebar_glass,
                Action::ToggleSidebarGlass,
                cx,
                callback.clone(),
            ))
            .child(toggle(
                "Main pane glass",
                settings.main_glass,
                Action::ToggleMainGlass,
                cx,
                callback.clone(),
            ))
            .child(
                div()
                    .text_size(theme::ui_px(10.))
                    .text_color(gpui::rgb(resolved.muted))
                    .child(if settings.transparency {
                        "Blur and opacity are shared. Text stays fully opaque."
                    } else {
                        "Opaque mode disables the native backdrop."
                    }),
            );
        view = view.child(section("Glass", rows.into_any_element()));
    }
    if matches(
        query,
        "Accessibility contrast high standard diff palette colorblind blue orange red green",
    ) {
        count += 1;
        let mut contrast = div().flex().flex_wrap().gap_1();
        for (value, label) in [
            (Contrast::Standard, "Standard"),
            (Contrast::High, "High contrast"),
        ] {
            contrast = contrast.child(button(
                format!("theme-contrast-{label}"),
                label,
                Action::Contrast(value),
                settings.contrast == value,
                cx,
                callback.clone(),
            ));
        }
        let mut diff = div().flex().flex_wrap().gap_1();
        for (value, label) in [
            (DiffPalette::Standard, "Red / green"),
            (DiffPalette::Colorblind, "Blue / orange"),
            (DiffPalette::HighContrast, "High contrast"),
        ] {
            diff = diff.child(button(
                format!("theme-diff-{label}"),
                label,
                Action::DiffPalette(value),
                settings.diff_palette == value,
                cx,
                callback.clone(),
            ));
        }
        let preview = div()
            .flex()
            .gap_2()
            .text_size(theme::ui_px(11.))
            .child(
                div()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(gpui::rgba(resolved.add_bg))
                    .text_color(gpui::rgb(resolved.add))
                    .child("+ Added"),
            )
            .child(
                div()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(gpui::rgba(resolved.remove_bg))
                    .text_color(gpui::rgb(resolved.remove))
                    .child("− Removed"),
            );
        view = view
            .child(section("Contrast", contrast.into_any_element()))
            .child(section(
                "Diff colors",
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(diff)
                    .child(preview)
                    .into_any_element(),
            ));
    }
    if count == 0 {
        view = view.child(
            div()
                .py_2()
                .text_color(gpui::rgb(resolved.muted))
                .child("No appearance settings match."),
        );
    }
    view.child(div().w_full().h(px(1.)).bg(gpui::rgb(resolved.border)))
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap_1()
                .child(button(
                    "theme-import",
                    "Import theme",
                    Action::Import,
                    false,
                    cx,
                    callback.clone(),
                ))
                .child(button(
                    "theme-export",
                    "Export theme",
                    Action::Export,
                    false,
                    cx,
                    callback.clone(),
                ))
                .child(button(
                    "theme-reset",
                    "Reset appearance",
                    Action::Reset,
                    false,
                    cx,
                    callback,
                )),
        )
        .into_any_element()
}
fn section(label: &'static str, body: AnyElement) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .text_size(theme::ui_px(11.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(label),
        )
        .child(body)
        .into_any_element()
}
fn button<T: 'static>(
    id: impl Into<SharedString>,
    label: &str,
    action: Action,
    selected: bool,
    cx: &Context<T>,
    callback: Callback<T>,
) -> AnyElement {
    let id: SharedString = id.into();
    let selector = id.clone();
    div()
        .id(id)
        .debug_selector(move || selector.to_string())
        .px_2()
        .py_1()
        .rounded_md()
        .border_1()
        .border_color(theme::rgba(if selected { 0x459bf780 } else { 0xebebeb18 }))
        .bg(theme::rgba(if selected { 0x459bf726 } else { 0xebebeb0a }))
        .cursor_pointer()
        .hover(|s| s.bg(theme::rgba(0xebebeb20)))
        .child(SharedString::from(label.to_owned()))
        .on_click(
            cx.listener(move |this, _, window, cx| callback(this, action.clone(), window, cx)),
        )
        .into_any_element()
}
fn toggle<T: 'static>(
    label: &'static str,
    value: bool,
    action: Action,
    cx: &Context<T>,
    callback: Callback<T>,
) -> AnyElement {
    div()
        .w_full()
        .flex()
        .items_center()
        .gap_2()
        .child(div().flex_1().child(label))
        .child(button(
            format!("theme-toggle-{label}"),
            if value { "On" } else { "Off" },
            action,
            value,
            cx,
            callback,
        ))
        .into_any_element()
}
pub fn matches(query: &str, keywords: &str) -> bool {
    let words = keywords.to_lowercase();
    query
        .split_whitespace()
        .all(|part| words.contains(&part.to_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    #[test]
    fn search_handles_multiple_terms_and_whitespace() {
        assert!(matches("  CODE  size ", "Code monospace font size"));
        assert!(!matches("code wallpaper", "Code font size"));
        assert!(matches("", "Anything"));
    }
}
