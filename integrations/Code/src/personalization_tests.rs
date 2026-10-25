//! Native appearance interaction tests; no real agents or account requests.
use super::*;
use gpui::TestAppContext;
use std::prelude::v1::test;
use theme_controls::{Action, FontSlot};
use ui_tests::{click_selector, draw_frame, fixture, idle_run};

#[gpui::test]
fn search_filters_settings_and_native_color_editor_applies_without_touching_draft(
    cx: &mut TestAppContext,
) {
    let (view, cx) = fixture(cx);
    cx.simulate_input("Keep my prompt 🦀");
    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            app.show_modal(Modal::Settings, window, cx);
            app.appearance_search
                .as_ref()
                .unwrap()
                .update(cx, |input, cx| input.set_text("accent", cx));
        })
    });
    draw_frame(&view, cx);
    assert!(cx.debug_bounds("theme-color-Accent").is_some());
    assert!(cx.debug_bounds("theme-preset-Harbor").is_none());
    assert!(cx.debug_bounds("glass-blur-slider").is_none());
    click_selector(cx, "theme-color-Accent");
    draw_frame(&view, cx);
    cx.simulate_keystrokes("cmd-a");
    cx.simulate_input("#9237E4");
    cx.simulate_keystrokes("enter");
    draw_frame(&view, cx);
    cx.update(|_, cx| {
        let app = view.read(cx);
        assert_eq!(app.theme.colors.accent, Some(0x9237e4));
        assert_eq!(app.composer.read(cx).text(), "Keep my prompt 🦀");
        assert_eq!(theme::rgb(0x459bf7), gpui::rgb(app.resolved_theme.accent));
    });
}

#[gpui::test]
fn scheme_opaque_and_font_settings_repaint_existing_inputs_and_persist(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    cx.simulate_input("Two lines\nStill editable");
    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            app.appearance_action(Action::Scheme(theme::Scheme::Light), window, cx);
            app.appearance_action(Action::ToggleTransparency, window, cx);
            app.appearance_action(Action::FontSize(FontSlot::Ui, 20.), window, cx);
            app.appearance_action(Action::FontSize(FontSlot::Mono, 18.), window, cx);
        })
    });
    draw_frame(&view, cx);
    cx.update(|_, cx| {
        let app = view.read(cx);
        let resolved = &app.resolved_theme;
        assert!(!resolved.dark);
        assert!(!resolved.native_transparency);
        assert_eq!(resolved.main_rgba() & 255, 255);
        assert_eq!(resolved.sidebar_rgba() & 255, 255);
        assert_eq!(theme::rgb(TEXT), gpui::rgb(resolved.foreground));
        assert!(
            f32::from(app.composer.read(cx).rendered_line_height() - theme::ui_line_height(22.))
                .abs()
                <= 1.
        );
        assert_eq!(f32::from(theme::ui_px(14.)), 20.);
        assert_eq!(f32::from(theme::mono_px(12.)), 18.);
        let saved = app.workspace_snapshot(cx);
        assert_eq!(saved.theme, app.theme);
        assert_eq!(saved.quick_shortcut, app.quick_shortcut);
        let roundtrip: workspace_state::WorkspaceState =
            serde_json::from_slice(&serde_json::to_vec(&saved).unwrap()).unwrap();
        assert_eq!(roundtrip.normalized().theme, app.theme);
    });
}

#[gpui::test]
fn appearance_edits_leave_running_agent_and_wallpaper_generation_untouched(
    cx: &mut TestAppContext,
) {
    let (view, cx) = fixture(cx);
    let (epoch, generation) = cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            let id = app.sessions[0].id.clone();
            app.active = Some(id.clone());
            let run = idle_run("Already streamed agent output".into());
            let epoch = run.parse_epoch;
            app.runs.insert(id, run);
            let generation = app.background.read(cx).snapshot_geometry().2;
            for action in [
                Action::Preset(theme::Preset::Harbor),
                Action::Contrast(theme::Contrast::High),
                Action::DiffPalette(theme::DiffPalette::Colorblind),
                Action::ToggleMainGlass,
            ] {
                app.appearance_action(action, window, cx);
            }
            (epoch, generation)
        })
    });
    cx.update(|_, cx| {
        let app = view.read(cx);
        let run = &app.runs[&app.sessions[0].id];
        assert_eq!(run.parse_epoch, epoch);
        assert!(!run.handle.is_cancelled());
        assert_eq!(run.text, "Already streamed agent output");
        assert_eq!(app.background.read(cx).snapshot_geometry().2, generation);
    });
}

#[gpui::test]
fn code_input_and_terminal_metrics_follow_the_selected_monospace_size(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    let input = cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            app.appearance_action(Action::FontSize(FontSlot::Mono, 18.), window, cx)
        });
        let input = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_editor_mode();
            input.set_text("fn main() { 🦀 }", cx);
            input
        });
        theme::ensure_font_metrics(window);
        input
    });
    cx.draw(point(px(0.), px(0.)), size(px(400.), px(180.)), |_, _| {
        input.clone().into_any_element()
    });
    cx.update(|_, cx| {
        assert!(
            f32::from(input.read(cx).rendered_line_height() - theme::mono_line_height(22.)).abs()
                <= 1.
        );
        assert_eq!(theme::mono_cell_height(), 27.);
        assert!(theme::mono_cell_width() > 0. && theme::mono_cell_width() < 36.);
        assert_eq!(input.read(cx).text(), "fn main() { 🦀 }");
    });
}
