//! Actual Orbit split-pane geometry, using only temporary test selectors.
use super::*;
use gpui::{TestAppContext, VisualTestContext};
use std::prelude::v1::test;
thread_local! {static PAINT_MASKS:std::cell::RefCell<HashMap<&'static str,Bounds<Pixels>>>=Default::default();}
pub(super) fn tag<E: InteractiveElement + ParentElement>(
    element: E,
    peer: Option<&SessionSummary>,
    slot: &'static str,
) -> E {
    let peer = peer.is_some();
    let key = match (slot, peer) {
        ("card", false) => "composer-card",
        ("card", true) => "footer-card-peer",
        ("row", false) => "footer-row-active",
        ("row", true) => "footer-row-peer",
        ("status", false) => "footer-status-active",
        ("status", true) => "footer-status-peer",
        ("hint", false) => "footer-hint-active",
        ("hint", true) => "footer-hint-peer",
        ("usage", false) => "provider-usage-footer",
        ("usage", true) => "footer-usage-peer",
        ("context", _) => "context-window-indicator",
        _ => unreachable!(),
    };
    element.debug_selector(move || key.into()).child(
        canvas(
            |bounds, _, _| bounds,
            move |_, _, window, _| {
                PAINT_MASKS.with(|m| {
                    m.borrow_mut().insert(key, window.content_mask().bounds);
                });
            },
        )
        .absolute()
        .inset_0(),
    )
}
fn rect(cx: &mut VisualTestContext, name: &'static str) -> Bounds<Pixels> {
    cx.debug_bounds(name)
        .unwrap_or_else(|| panic!("Missing {name}"))
}
#[gpui::test]
fn active_and_peer_footer_match_cards_and_keep_minimum_label_separation(cx: &mut TestAppContext) {
    let (view, cx) = crate::ui_tests::fixture(cx);
    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            app.active = Some("fixture-session".into());
            let mut peer = app.sessions[0].clone();
            peer.id = "footer-peer".into();
            peer.title = "Footer peer".into();
            app.sessions.push(peer.clone());
            app.split_chat(peer, terminal_dock::Side::Left, window, cx);
            app.motion = false;
            app.loading = false;
            app.discovering = false;
            app.composer
                .update(cx, |i, cx| i.set_text("Independent active draft", cx));
            cx.notify();
        })
    });
    let mut failures = Vec::new();
    for width in [1440., 1000., 760.] {
        for long_status in [false, true] {
            for middle in 0..3 {
                cx.simulate_resize(size(px(width), px(790.)));
                view.update(cx,|app,cx|{
            app.usage.clear();app.controls.clear();
            if middle>0 {app.usage.insert(app.active.clone().unwrap(),usage::Usage{context_used:Some(32_000),context_limit:Some(128_000),limit_used_percent:(middle==2).then_some(25.),..Default::default()});}
            app.dictation_status=long_status.then_some("Capturing audio and transcribing a deliberately long local dictation status");cx.notify();});
                PAINT_MASKS.with(|m| m.borrow_mut().clear());
                crate::ui_tests::settle_composer(&view, cx);
                for (side, card_name, row_name, status_name, hint_name) in [
                    (
                        "active",
                        "composer-card",
                        "footer-row-active",
                        "footer-status-active",
                        "footer-hint-active",
                    ),
                    (
                        "peer",
                        "footer-card-peer",
                        "footer-row-peer",
                        "footer-status-peer",
                        "footer-hint-peer",
                    ),
                ] {
                    let card = rect(cx, card_name);
                    let row = rect(cx, row_name);
                    let status = rect(cx, status_name);
                    let hint = rect(cx, hint_name);
                    let gap = f32::from(hint.left() - status.right());
                    eprintln!(
                        "FOOTER_GEOMETRY {}",
                        serde_json::json!({"width":width,"long_status":long_status,"middle":middle,"side":side,"card":[f32::from(card.left()),f32::from(card.top()),f32::from(card.size.width),f32::from(card.size.height)],"row":[f32::from(row.left()),f32::from(row.top()),f32::from(row.size.width),f32::from(row.size.height)],"status":[f32::from(status.left()),f32::from(status.size.width)],"hint":[f32::from(hint.left()),f32::from(hint.size.width)],"gap":gap})
                    );
                    if f32::from(row.size.width - card.size.width).abs() > 1.
                        || f32::from(row.left() - card.left()).abs() > 1.
                    {
                        failures.push(format!("{width}/{long_status}/{side}: footer {:?} does not align with card {:?}",row,card));
                    }
                    if gap < 7.5 {
                        failures.push(format!(
                            "{width}/{long_status}/{side}: labels have only{gap}px separation"
                        ));
                    }
                    if status.left() < row.left() - px(0.5) || hint.right() > row.right() + px(0.5)
                    {
                        failures.push(format!(
                            "{width}/{long_status}/{side}: truncated label boxes escape row"
                        ));
                    }
                    for name in [status_name, hint_name] {
                        let clip = PAINT_MASKS.with(|m| *m.borrow().get(name).unwrap());
                        if clip.size.width > px(0.)
                            && clip.size.height > px(0.)
                            && (clip.left() < row.left() - px(0.5)
                                || clip.right() > row.right() + px(0.5)
                                || clip.top() < row.top() - px(0.5)
                                || clip.bottom() > row.bottom() + px(0.5))
                        {
                            failures.push(format!("{width}/{long_status}/{side}: painted outer label clip {:?} escapes footer {:?}",clip,row));
                        }
                    }
                    let usage_name = if middle == 2 {
                        Some(if side == "active" {
                            "provider-usage-footer"
                        } else {
                            "footer-usage-peer"
                        })
                    } else if middle == 1 && side == "active" {
                        Some("context-window-indicator")
                    } else {
                        None
                    };
                    if let Some(name) = usage_name {
                        let usage = rect(cx, name);
                        eprintln!(
                            "FOOTER_MIDDLE {}",
                            serde_json::json!({"width":width,"long_status":long_status,"middle":middle,"side":side,"bounds":[f32::from(usage.left()),f32::from(usage.top()),f32::from(usage.size.width),f32::from(usage.size.height)],"row":[f32::from(row.left()),f32::from(row.top()),f32::from(row.size.width),f32::from(row.size.height)]})
                        );
                        if usage.left() < row.left() - px(0.5)
                            || usage.right() > row.right() + px(0.5)
                            || usage.top() < row.top() - px(0.5)
                            || usage.bottom() > row.bottom() + px(0.5)
                        {
                            failures.push(format!("{width}/{long_status}/{side}/middle{middle}: usage {:?} escapes footer {:?}",usage,row));
                        }
                        if usage.left() - status.right() < px(7.5)
                            || hint.left() - usage.right() < px(7.5)
                        {
                            failures.push(format!("{width}/{long_status}/{side}/middle{middle}: usage and text do not keep minimum separation"));
                        }
                        let mask = PAINT_MASKS
                            .with(|m| *m.borrow().get(name).expect("Middle paint probe missing"));
                        eprintln!(
                            "FOOTER_MIDDLE_PAINT {}",
                            serde_json::json!({"width":width,"long_status":long_status,"middle":middle,"side":side,"mask":[f32::from(mask.left()),f32::from(mask.top()),f32::from(mask.size.width),f32::from(mask.size.height)]})
                        );
                        if mask.left() < row.left() - px(0.5)
                            || mask.right() > row.right() + px(0.5)
                            || mask.top() < row.top() - px(0.5)
                            || mask.bottom() > row.bottom() + px(0.5)
                        {
                            failures.push(format!("{width}/{long_status}/{side}/middle{middle}: painted middle clip {:?} escapes footer {:?}",mask,row));
                        }
                        if side == "active" {
                            let context = rect(cx, "context-window-indicator");
                            let clip = PAINT_MASKS
                                .with(|m| *m.borrow().get("context-window-indicator").unwrap());
                            eprintln!(
                                "FOOTER_CONTEXT_PAINT {}",
                                serde_json::json!({"width":width,"long_status":long_status,"middle":middle,"context":[f32::from(context.left()),f32::from(context.size.width)],"clip":[f32::from(clip.left()),f32::from(clip.size.width)]})
                            );
                            if clip.size.width < px(11.5) {
                                failures.push(format!("{width}/{long_status}/{side}/middle{middle}: context icon does not retain12px of visible clipping width: {:?}",clip));
                            }
                        }
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
