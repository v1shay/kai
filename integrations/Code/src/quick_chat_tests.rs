//! Quick Chat uses native GPUI layout/input, never registers a real shortcut or
//! launches an agent. Fixtures keep all filesystem work in a temporary project.
use super::*;
use gpui::{TestAppContext, VisualTestContext};
use std::prelude::v1::test;
use std::{cell::RefCell, rc::Rc};

fn fixture(
    cx: &mut TestAppContext,
) -> (
    Entity<Orbit>,
    Entity<quick_chat::QuickChat>,
    &mut VisualTestContext,
) {
    let (orbit, main) = ui_tests::fixture(cx);
    main.simulate_input("Draft 🦀");
    let state = orbit.clone();
    let (panel, cx) =
        cx.add_window_view(move |window, cx| quick_chat::QuickChat::new(state, window, cx));
    cx.simulate_resize(size(px(680.), px(520.)));
    cx.update(|window, cx| panel.update(cx, |panel, cx| panel.show(window, cx)));
    draw(&panel, cx);
    (orbit, panel, cx)
}
fn draw(panel: &Entity<quick_chat::QuickChat>, cx: &mut VisualTestContext) {
    cx.draw(point(px(0.), px(0.)), size(px(680.), px(520.)), |_, _| {
        panel.clone().into_any_element()
    });
}
fn click(cx: &mut VisualTestContext, selector: &'static str) {
    let bounds = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("missing {selector}"));
    cx.simulate_click(bounds.center(), Modifiers::none());
}

#[gpui::test]
fn quick_loaded_chat_reveals_without_an_active_main_window(cx: &mut TestAppContext) {
    let (orbit, panel, cx) = fixture(cx);
    cx.run_until_parked();
    orbit.update(cx, |app, cx| {
        app.motion = true;
        app.active = Some("fixture-session".into());
        app.loading = true;
        app.receive(
            Event::Loaded(
                "fixture-session".into(),
                app.generation,
                (0..1_000)
                    .map(|ix| engine::ParsedMessage {
                        role: "assistant:codex".into(),
                        blocks: vec![MessageBlock::Paragraph(format!("Existing message {ix}"))],
                    })
                    .collect(),
                true,
                false,
            ),
            cx,
        );
        app.rendered_rows.set(0);
        app.transcript_reveal.as_mut().unwrap().test_elapsed = Some(0.);
    });
    draw(&panel, cx);
    cx.update(|window, cx| {
        assert!(window.is_window_active());
        let app = orbit.read(cx);
        assert_ne!(app.primary_window, window.window_handle().window_id());
        let reveal = app.transcript_reveal.as_ref().unwrap();
        assert!(
            reveal.started.is_some(),
            "Quick Chat must start its own reveal clock"
        );
        assert!(
            app.rendered_rows.get() < 100,
            "Only the bounded tail is mounted, got {}",
            app.rendered_rows.get()
        );
        assert!(
            app.has_older,
            "Opening Quick Chat must preserve paged history"
        );
    });
    assert!(cx.debug_bounds("transcript-content-999").is_some());
    orbit.update(cx, |app, cx| {
        app.transcript_reveal.as_mut().unwrap().test_elapsed = Some(2.);
        cx.notify();
    });
    draw(&panel, cx);
    cx.update(|_, cx| assert!(orbit.read(cx).transcript_reveal.is_none()));

    // A new chunk from the same agent must also finish revealing in this window.
    orbit.update(cx, |app, cx| {
        app.rows.push(Row {
            role: Some("assistant:codex".into()),
            block: MessageBlock::Paragraph("Live response".into()),
        });
        app.transcript_reveal = Some(ListReveal::appended(1_000));
        cx.notify();
    });
    draw(&panel, cx);
    cx.update(|_, cx| {
        assert!(
            orbit
                .read(cx)
                .transcript_reveal
                .as_ref()
                .unwrap()
                .started
                .is_some()
        );
    });
    assert!(cx.debug_bounds("transcript-content-1000").is_some());
}

#[gpui::test]
fn quick_codex_voice_is_visible_distinct_from_dictation_and_uses_shared_launch(
    cx: &mut TestAppContext,
) {
    let (orbit, panel, cx) = fixture(cx);
    for width in [540., 680.] {
        cx.simulate_resize(size(px(width), px(520.)));
        cx.draw(point(px(0.), px(0.)), size(px(width), px(520.)), |_, _| {
            panel.clone().into_any_element()
        });
        let voice = cx
            .debug_bounds("conversation-voice")
            .expect("Visible Codex Voice control");
        let card = cx.debug_bounds("quick-composer-card").unwrap();
        assert!(
            voice.size.width > px(70.),
            "Voice must have a readable label"
        );
        assert!(voice.left() >= card.left() && voice.right() <= card.right());
    }
    click(cx, "conversation-voice");
    draw(&panel, cx);
    assert!(cx.debug_bounds("realtime-popover").is_some());
    cx.update(|_, cx| {
        let app = orbit.read(cx);
        assert!(app.modal == Some(Modal::Realtime));
        assert!(
            app.voice_call.is_none(),
            "Opening settings must not start recording"
        );
        assert!(app.notice.is_none());
    });
    click(cx, "voice-name-ember");
    cx.update(|_, cx| assert_eq!(orbit.read(cx).voice_choice, "ember"));
    draw(&panel, cx);
    click(cx, "voice-start");
    cx.update(|_, cx| {
        let app = orbit.read(cx);
        assert!(app.voice_call.is_none());
        assert!(
            app.notice
                .as_deref()
                .is_some_and(|notice| notice.contains("Codex"))
        );
    });
}

#[gpui::test]
fn main_codex_voice_control_has_a_readable_label_and_fits_the_composer(cx: &mut TestAppContext) {
    let (orbit, cx) = ui_tests::fixture(cx);
    ui_tests::draw_frame(&orbit, cx);
    let voice = cx
        .debug_bounds("conversation-voice")
        .expect("Visible Codex Voice control");
    let card = cx.debug_bounds("composer-card").unwrap();
    assert!(voice.size.width > px(70.));
    assert!(voice.left() >= card.left() && voice.right() <= card.right());
    click(cx, "conversation-voice");
    ui_tests::draw_frame(&orbit, cx);
    assert!(cx.debug_bounds("realtime-popover").is_some());
    cx.update(|_, cx| {
        assert!(orbit.read(cx).modal == Some(Modal::Realtime));
        assert!(orbit.read(cx).voice_call.is_none());
    });
    click(cx, "voice-name-ember");
    cx.update(|_, cx| assert_eq!(orbit.read(cx).voice_choice, "ember"));
    ui_tests::draw_frame(&orbit, cx);
    click(cx, "voice-start");
    cx.update(|_, cx| {
        let app = orbit.read(cx);
        assert!(app.voice_call.is_none());
        assert!(
            app.notice
                .as_deref()
                .is_some_and(|notice| notice.contains("Install Codex"))
        );
    });
}

#[gpui::test]
fn quick_composer_has_separate_geometry_and_bidirectional_unicode_drafts(cx: &mut TestAppContext) {
    let (orbit, panel, cx) = fixture(cx);
    cx.update(|_, cx| {
        let main = orbit.read(cx).composer.clone();
        let quick = panel.read(cx).composer.clone();
        assert_ne!(main.entity_id(), quick.entity_id());
        assert_eq!(quick.read(cx).text(), "Draft 🦀");
        assert_ne!(main.read(cx).bounds(), quick.read(cx).bounds());
    });
    cx.simulate_input(" 日本語");
    cx.simulate_keystrokes("left");
    cx.simulate_input("!");
    cx.update(|_, cx| {
        assert_eq!(orbit.read(cx).composer.read(cx).text(), "Draft 🦀 日本!語");
        assert_eq!(panel.read(cx).composer.read(cx).text(), "Draft 🦀 日本!語");
        orbit
            .read(cx)
            .composer
            .clone()
            .update(cx, |input, cx| input.set_text("Changed in Code 🦀", cx));
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert_eq!(
            panel.read(cx).composer.read(cx).text(),
            "Changed in Code 🦀"
        )
    });
    cx.simulate_keystrokes("enter");
    cx.update(|window, cx| {
        let app = orbit.read(cx);
        assert!(
            app.runs.is_empty(),
            "Fixture must never launch an actual provider"
        );
        assert!(
            app.notice
                .as_deref()
                .is_some_and(|notice| notice.contains("Install Codex"))
        );
        assert!(
            panel
                .read(cx)
                .composer
                .read(cx)
                .focus_handle
                .is_focused(window)
        );
    });
}

#[gpui::test]
fn quick_hide_preserves_live_agents_and_reopens_with_current_draft(cx: &mut TestAppContext) {
    let (orbit, panel, cx) = fixture(cx);
    cx.update(|window, cx| {
        orbit.update(cx, |app, _| {
            let id = app.sessions[0].id.clone();
            app.active = Some(id.clone());
            app.runs
                .insert(id, ui_tests::idle_run("Still working".into()));
        });
        panel.update(cx, |panel, cx| panel.hide(window, cx));
        assert!(!panel.read(cx).is_visible());
        let app = orbit.read(cx);
        assert_eq!(app.runs.len(), 1);
        assert!(!app.runs.values().next().unwrap().handle.is_cancelled());
        assert!(app.overlay_window.is_none());
        app.composer
            .clone()
            .update(cx, |input, cx| input.set_text("While hidden", cx));
    });
    cx.update(|window, cx| panel.update(cx, |panel, cx| panel.show(window, cx)));
    draw(&panel, cx);
    cx.update(|window, cx| {
        assert!(panel.read(cx).is_visible());
        assert_eq!(panel.read(cx).composer.read(cx).text(), "While hidden");
        let app = orbit.read(cx);
        assert_eq!(app.runs.len(), 1);
        assert!(!app.runs.values().next().unwrap().handle.is_cancelled());
        assert_eq!(app.overlay_window, Some(window.window_handle().window_id()));
    });
}

#[gpui::test]
fn quick_project_agent_and_send_controls_act_on_the_shared_workspace(cx: &mut TestAppContext) {
    let (orbit, panel, cx) = fixture(cx);
    click(cx, "quick-project");
    draw(&panel, cx);
    assert!(cx.debug_bounds("workspace-popover").is_some());
    cx.update(|_, cx| assert!(orbit.read(cx).modal == Some(Modal::Projects)));
    cx.simulate_keystrokes("escape");
    draw(&panel, cx);
    click(cx, "quick-agent-model");
    draw(&panel, cx);
    cx.update(|_, cx| assert!(orbit.read(cx).agent_picker.is_some()));
    cx.simulate_keystrokes("escape");
    draw(&panel, cx);
    click(cx, "quick-send");
    cx.update(|_, cx| {
        let app = orbit.read(cx);
        assert!(app.runs.is_empty());
        assert!(
            app.notice
                .as_deref()
                .is_some_and(|notice| notice.contains("Install Codex"))
        );
        assert_eq!(app.composer.read(cx).text(), "Draft 🦀");
    });
}

#[gpui::test]
fn quick_wallpaper_snapshot_does_not_resize_or_reprepare_main_artwork(cx: &mut TestAppContext) {
    let (orbit, panel, cx) = fixture(cx);
    let original = cx.update(|_, cx| orbit.read(cx).background.read(cx).snapshot_geometry());
    for _ in 0..4 {
        draw(&panel, cx);
    }
    cx.update(|_, cx| {
        assert_eq!(
            orbit.read(cx).background.read(cx).snapshot_geometry(),
            original
        )
    });
}

#[gpui::test]
fn quick_wallpaper_changes_invalidate_only_the_visible_panel(cx: &mut TestAppContext) {
    let (orbit, panel, cx) = fixture(cx);
    let notifications = Rc::new(RefCell::new(0));
    let captured = notifications.clone();
    let _subscription =
        cx.update(|_, cx| cx.observe(&panel, move |_, _| *captured.borrow_mut() += 1));
    let background = cx.update(|_, cx| orbit.read(cx).background.clone());
    let geometry = cx.update(|_, cx| background.read(cx).snapshot_geometry());
    cx.update(|_, cx| background.update(cx, |_, cx| cx.notify()));
    cx.run_until_parked();
    assert!(
        *notifications.borrow() > 0,
        "A prepared wallpaper/frame notification must repaint a visible Quick panel"
    );
    cx.update(|window, cx| panel.update(cx, |panel, cx| panel.hide(window, cx)));
    cx.run_until_parked();
    let hidden = *notifications.borrow();
    cx.update(|_, cx| background.update(cx, |_, cx| cx.notify()));
    cx.run_until_parked();
    assert_eq!(*notifications.borrow(), hidden);
    cx.update(|_, cx| assert_eq!(background.read(cx).snapshot_geometry(), geometry));
}

#[gpui::test]
fn shortcut_recorder_rejects_unmodified_input_and_records_one_chord(cx: &mut TestAppContext) {
    let (recorder, cx) = cx.add_window_view(|_, cx| {
        quick_chat::ShortcutRecorder::new(global_hotkey::Shortcut::default(), cx)
    });
    cx.simulate_resize(size(px(340.), px(120.)));
    let recorded = Rc::new(RefCell::new(vec![]));
    let capture = recorded.clone();
    let _subscription = cx.update(|_, cx| {
        cx.subscribe(&recorder, move |_, event, _| {
            if let quick_chat::RecorderEvent::Recorded(shortcut) = event {
                capture.borrow_mut().push(shortcut.clone());
            }
        })
    });
    cx.draw(point(px(0.), px(0.)), size(px(340.), px(120.)), |_, _| {
        recorder.clone().into_any_element()
    });
    click(cx, "record-quick-chat-shortcut");
    cx.simulate_keystrokes("k");
    cx.update(|_, cx| assert!(recorder.read(cx).is_recording()));
    assert!(recorded.borrow().is_empty());
    cx.simulate_keystrokes("ctrl-alt-k");
    assert_eq!(recorded.borrow().len(), 1);
    assert_eq!(recorded.borrow()[0].label(), "⌃⌥K");
    cx.update(|window, cx| recorder.update(cx, |recorder, cx| recorder.begin(window, cx)));
    cx.simulate_keystrokes("escape");
    cx.update(|_, cx| assert!(!recorder.read(cx).is_recording()));
    assert_eq!(recorded.borrow().len(), 1);
}

#[gpui::test]
fn shortcut_recording_stops_on_focus_change_settings_dismiss_and_window_deactivation(
    cx: &mut TestAppContext,
) {
    let (orbit, cx) = ui_tests::fixture(cx);
    let recorder = cx.update(|window, cx| {
        orbit.update(cx, |app, cx| app.show_modal(Modal::Settings, window, cx));
        orbit
            .read(cx)
            .appearance_search
            .clone()
            .unwrap()
            .update(cx, |input, cx| input.set_text("shortcut", cx));
        orbit.read(cx).shortcut_recorder.clone().unwrap()
    });
    ui_tests::draw_frame(&orbit, cx);
    let cancellations = Rc::new(RefCell::new(0));
    let captured = cancellations.clone();
    let _subscription = cx.update(|_, cx| {
        cx.subscribe(&recorder, move |_, event, _| {
            if matches!(event, quick_chat::RecorderEvent::Cancelled) {
                *captured.borrow_mut() += 1;
            }
        })
    });
    click(cx, "record-quick-chat-shortcut");
    ui_tests::draw_frame(&orbit, cx);
    cx.update(|window, cx| {
        assert!(recorder.read(cx).is_capturing(window));
        orbit.read(cx).focus.focus(window);
        // Carbon must stop capturing immediately, before the next native focus
        // notification arrives during paint.
        assert!(!recorder.read(cx).is_capturing(window));
    });
    ui_tests::draw_frame(&orbit, cx);
    cx.update(|_, cx| assert!(!recorder.read(cx).is_recording()));
    assert_eq!(*cancellations.borrow(), 1);

    click(cx, "record-quick-chat-shortcut");
    ui_tests::draw_frame(&orbit, cx);
    cx.update(|window, cx| {
        assert!(recorder.read(cx).is_capturing(window));
        orbit.update(cx, |app, cx| app.dismiss_panel(window, cx));
    });
    ui_tests::draw_frame(&orbit, cx);
    cx.update(|_, cx| assert!(!recorder.read(cx).is_recording()));
    assert_eq!(*cancellations.borrow(), 2);

    cx.update(|window, cx| {
        orbit.update(cx, |app, cx| app.show_modal(Modal::Settings, window, cx));
    });
    ui_tests::draw_frame(&orbit, cx);
    click(cx, "record-quick-chat-shortcut");
    ui_tests::draw_frame(&orbit, cx);
    cx.update(|window, cx| assert!(recorder.read(cx).is_capturing(window)));
    cx.deactivate_window();
    cx.update(|window, cx| {
        assert!(!recorder.read(cx).is_capturing(window));
        assert!(!recorder.read(cx).is_recording());
    });
    assert_eq!(*cancellations.borrow(), 3);
}

#[gpui::test]
fn quick_dictation_mirrors_unselected_text_and_follows_wrapped_lines(cx: &mut TestAppContext) {
    let (orbit, panel, cx) = fixture(cx);
    let words = (0..60)
        .map(|i| format!("Dictated line {i} 日本語 and a longer sentence\n"))
        .collect::<String>();
    let main = cx.update(|_, cx| orbit.read(cx).composer.clone());
    cx.update(|_, cx| {
        main.update(cx, |input, cx| {
            input.begin_dictation(cx);
            assert!(input.apply_dictation(&words, cx));
        })
    });
    cx.run_until_parked();
    draw(&panel, cx);
    cx.run_until_parked();
    draw(&panel, cx);
    cx.update(|_, cx| {
        let input = panel.read(cx).composer.read(cx);
        assert_eq!(input.text(), main.read(cx).text());
        assert!(
            input.scroll_position().y < px(-100.),
            "Long dictation should follow its last lines in the Quick composer"
        );
    });
    cx.update(|_, cx| main.update(cx, |input, cx| input.end_dictation(cx)));
}
