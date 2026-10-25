//! Persistable glass settings and native controls. Opacity changes panel tint,
//! never text, controls, the whole window, or the GPUI renderer's alpha.
use crate::theme::{rgb, rgba, ui_px};
use gpui::{prelude::*, *};
use serde::{Deserialize, Serialize};
use std::{cell::Cell, rc::Rc};

pub const MAX_BLUR: u8 = 64;
pub const DEFAULT_OPACITY: f32 = 0.60;
/// Shared dropdown backing, independent of workspace opacity so menu labels
/// remain readable over wallpaper and chat content. Matches the agent picker.
pub const MENU_SURFACE: u32 = 0x242424bb;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub blur_radius: u8,
    pub opacity: f32,
}
impl Default for Appearance {
    fn default() -> Self {
        Self {
            blur_radius: 24,
            opacity: DEFAULT_OPACITY,
        }
    }
}
impl Appearance {
    pub fn normalized(self) -> Self {
        Self {
            blur_radius: self.blur_radius.min(MAX_BLUR),
            opacity: if self.opacity.is_finite() {
                self.opacity.clamp(0., 1.)
            } else {
                DEFAULT_OPACITY
            },
        }
    }
    /// A panel color with exactly the selected opacity. This is deliberately
    /// separate from GPUI's opacity style, which would also fade its children.
    #[cfg(test)]
    pub fn panel_rgba(self, rgb: u32) -> u32 {
        ((rgb & 0x00ff_ffff) << 8) | (self.normalized().opacity * 255.).round() as u32
    }
    /// Preserve the existing relative tint levels at the default setting.
    pub fn tint(self, rgba: u32) -> u32 {
        let alpha = ((rgba & 0xff) as f32 * self.normalized().opacity / DEFAULT_OPACITY)
            .round()
            .clamp(0., 255.) as u32;
        (rgba & 0xffff_ff00) | alpha
    }
}

type Change<T> = Rc<dyn Fn(&mut T, Appearance, &mut Window, &mut Context<T>)>;

/// Sliders are pure native elements. The owner handles persistence; callback
/// changes only user-adjusted values, and duplicate positions are suppressed.
pub fn controls<T: 'static>(
    appearance: Appearance,
    cx: &Context<T>,
    on_change: impl Fn(&mut T, Appearance, &mut Window, &mut Context<T>) + 'static,
) -> AnyElement {
    let appearance = appearance.normalized();
    let callback: Change<T> = Rc::new(on_change);
    div()
        .debug_selector(|| "appearance-controls".into())
        .w_full()
        .min_w(px(0.))
        .flex()
        .flex_col()
        .gap_3()
        .child(
            div()
                .text_size(ui_px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .child("Glass"),
        )
        .child(slider(
            "glass-blur-slider",
            "Blur",
            appearance.blur_radius as f32 / MAX_BLUR as f32,
            format!("{} px", appearance.blur_radius),
            appearance,
            true,
            cx,
            callback.clone(),
        ))
        .child(slider(
            "glass-opacity-slider",
            "Opacity",
            appearance.opacity,
            format!("{}%", (appearance.opacity * 100.).round() as u32),
            appearance,
            false,
            cx,
            callback,
        ))
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn slider<T: 'static>(
    id: &'static str,
    label: &'static str,
    fraction: f32,
    value: String,
    appearance: Appearance,
    blur: bool,
    cx: &Context<T>,
    change: Change<T>,
) -> AnyElement {
    const WIDTH: f32 = 174.;
    const THUMB: f32 = 12.;
    let bounds = Rc::new(Cell::new(Bounds::default()));
    let painted_bounds = bounds.clone();
    let down_bounds = bounds.clone();
    let down_change = change.clone();
    let last = Rc::new(Cell::new(appearance));
    let down_last = last.clone();
    div()
        .w_full()
        .flex()
        .items_center()
        .gap_3()
        .child(div().w(px(58.)).text_size(ui_px(12.)).child(label))
        .child(
            div()
                .id(id)
                .debug_selector(move || id.into())
                .w(px(WIDTH))
                .h(px(26.))
                .relative()
                .cursor_pointer()
                .child(
                    canvas(move |next, _, _| painted_bounds.set(next), |_, _, _, _| {}).size_full(),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(THUMB / 2.))
                        .right(px(THUMB / 2.))
                        .top(px(11.))
                        .h(px(4.))
                        .rounded_full()
                        .bg(rgba(0xebebeb24)),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(THUMB / 2.))
                        .top(px(11.))
                        .w(px((WIDTH - THUMB) * fraction))
                        .h(px(4.))
                        .rounded_full()
                        .bg(rgba(0xdce7ff88)),
                )
                .child(
                    div()
                        .absolute()
                        .left(px((WIDTH - THUMB) * fraction))
                        .top(px(7.))
                        .size(px(THUMB))
                        .rounded_full()
                        .bg(rgb(0xebebeb)),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                        let Some(next) =
                            at_position(appearance, blur, down_bounds.get(), event.position)
                        else {
                            return;
                        };
                        if down_last.replace(next) != next {
                            down_change(this, next, window, cx);
                        }
                    }),
                )
                .on_mouse_move(
                    cx.listener(move |this, event: &MouseMoveEvent, window, cx| {
                        if event.pressed_button != Some(MouseButton::Left) {
                            return;
                        }
                        let Some(next) =
                            at_position(appearance, blur, bounds.get(), event.position)
                        else {
                            return;
                        };
                        if last.replace(next) != next {
                            change(this, next, window, cx);
                        }
                    }),
                ),
        )
        .child(
            div()
                .w(px(38.))
                .text_size(ui_px(11.))
                .text_color(rgb(0xa9a9a9))
                .child(value),
        )
        .into_any_element()
}

fn at_position(
    current: Appearance,
    blur: bool,
    bounds: Bounds<Pixels>,
    point: Point<Pixels>,
) -> Option<Appearance> {
    let width = f32::from(bounds.size.width);
    if !width.is_finite() || width <= 12. {
        return None;
    }
    let fraction = ((f32::from(point.x - bounds.origin.x) - 6.) / (width - 12.)).clamp(0., 1.);
    let mut next = current.normalized();
    if blur {
        next.blur_radius = (fraction * MAX_BLUR as f32).round() as u8;
    } else {
        next.opacity = (fraction * 100.).round() / 100.;
    }
    Some(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    #[test]
    fn settings_normalize_and_partial_saved_files_keep_defaults() {
        let saved: Appearance = serde_json::from_str(r#"{"blur_radius":255}"#).unwrap();
        assert_eq!(
            saved.normalized(),
            Appearance {
                blur_radius: MAX_BLUR,
                opacity: DEFAULT_OPACITY
            }
        );
        for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(
                Appearance {
                    opacity: invalid,
                    ..Default::default()
                }
                .normalized()
                .opacity,
                DEFAULT_OPACITY
            );
        }
        assert_eq!(
            Appearance {
                opacity: 9.,
                ..Default::default()
            }
            .normalized()
            .opacity,
            1.
        );
        assert_eq!(
            Appearance {
                opacity: -2.,
                ..Default::default()
            }
            .normalized()
            .opacity,
            0.
        );
        let original = Appearance {
            blur_radius: 48,
            opacity: 0.23,
        };
        let encoded = serde_json::to_vec(&original).unwrap();
        assert_eq!(
            serde_json::from_slice::<Appearance>(&encoded).unwrap(),
            original
        );
    }
    #[test]
    fn opacity_only_changes_panel_alpha_and_preserves_default_tints() {
        assert_eq!(Appearance::default().tint(0x17171799), 0x17171799);
        assert_eq!(Appearance::default().tint(0x171717b3), 0x171717b3);
        assert_eq!(
            Appearance {
                opacity: 0.,
                ..Default::default()
            }
            .panel_rgba(0x242424),
            0x24242400
        );
        assert_eq!(
            Appearance {
                opacity: 1.,
                ..Default::default()
            }
            .panel_rgba(0x242424),
            0x242424ff
        );
        assert_eq!(
            Appearance {
                opacity: 1.,
                ..Default::default()
            }
            .tint(0xabcdef99),
            0xabcdefff
        );
    }
    #[test]
    fn sliders_clamp_round_and_do_not_modify_other_control() {
        let bounds = Bounds::new(point(px(20.), px(30.)), size(px(174.), px(26.)));
        let current = Appearance {
            blur_radius: 33,
            opacity: 0.42,
        };
        assert_eq!(
            at_position(current, true, bounds, point(px(-100.), px(30.))).unwrap(),
            Appearance {
                blur_radius: 0,
                ..current
            }
        );
        assert_eq!(
            at_position(current, true, bounds, point(px(999.), px(30.))).unwrap(),
            Appearance {
                blur_radius: 64,
                ..current
            }
        );
        assert_eq!(
            at_position(current, false, bounds, point(px(107.), px(30.))).unwrap(),
            Appearance {
                opacity: 0.5,
                ..current
            }
        );
        assert!(at_position(current, true, Bounds::default(), point(px(0.), px(0.))).is_none());
    }
}
