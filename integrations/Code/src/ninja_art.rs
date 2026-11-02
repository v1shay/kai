//! Shared, cached pixel artwork and Ninja-colored native intensity shimmer.
use super::*;
use std::sync::Arc;
pub(super) fn color(style: &str) -> u32 {
    ninjas::styles()
        .iter()
        .find(|(id, _, _)| *id == style)
        .map(|(_, _, c)| *c)
        .unwrap_or(0xef6758)
}
fn images() -> &'static [Arc<Image>; 6] {
    static IMAGES: std::sync::OnceLock<[Arc<Image>; 6]> = std::sync::OnceLock::new();
    IMAGES.get_or_init(|| {
        [
            include_bytes!("../assets/ninja-fire.gif").as_slice(),
            include_bytes!("../assets/ninja-lightning.gif").as_slice(),
            include_bytes!("../assets/ninja-earth.gif").as_slice(),
            include_bytes!("../assets/ninja-ice.gif").as_slice(),
            include_bytes!("../assets/ninja-energy.gif").as_slice(),
            include_bytes!("../assets/ninja-water.gif").as_slice(),
        ]
        .map(|bytes| Arc::new(Image::from_bytes(ImageFormat::Gif, bytes.to_vec())))
    })
}
pub(super) fn glyph(style:&str)->&'static str { match style {"lightning"=>"ninja-lightning","earth"=>"ninja-earth","ice"=>"ninja-ice","energy"=>"ninja-energy","water"=>"ninja-water",_=>"ninja-fire"} }
pub(super) fn wallpaper(_style:&str,_animated:bool)->AnyElement {
    div().id("ninja-wallpaper").debug_selector(||"ninja-wallpaper".into()).absolute().inset_0()
        .child(canvas(|bounds,_,_|unified::atmosphere_slot(bounds),|_,_,_,_|{}).size_full()).into_any_element()
}
pub(super) fn art(style: &str, size: f32, animated: bool) -> AnyElement {
    if !animated {
        return icon(
            glyph(style),
            size,
            color(style),
        )
        .into_any_element();
    }
    let i = ninjas::styles()
        .iter()
        .position(|(id, _, _)| *id == style)
        .unwrap_or(0);
    img(images()[i].clone())
        .id(SharedString::from(format!("ninja-art-{style}-{size}")))
        .w(px(size))
        .h(px(size * 56. / 48.))
        .object_fit(ObjectFit::Contain)
        .into_any_element()
}
pub(super) fn nameplate(name: String, style: &str, active: bool, motion: bool) -> AnyElement {
    let color = color(style);
    let contents = move || {
        div()
            .relative()
            .min_w(px(0.))
            .truncate()
            .text_size(theme::ui_px(12.))
            .child(name.clone())
    };
    let body = div()
        .relative()
        .overflow_hidden()
        .rounded_md()
        .flex_1()
        .min_w(px(0.))
        .py_1();
    if active && motion {
        body.with_animation(
            SharedString::from(format!("ninja-name-{style}")),
            Animation::new(Duration::from_secs(5)).repeat(),
            move |v, t| {
                v.child(composer_design::shimmer_tiles(Some(t * 5.), color, true))
                    .child(contents())
            },
        )
        .into_any_element()
    } else {
        body.child(contents()).into_any_element()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use image::AnimationDecoder;
    use std::prelude::v1::test;
    #[test]
    fn generated_loops_are_complete_transparent_and_small() {
        for style in ninjas::styles() {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(format!("assets/ninja-{}.gif", style.0));
            let bytes = std::fs::read(path).unwrap();
            assert!(bytes.len() < 80_000);
            let frames = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(bytes))
                .unwrap()
                .into_frames()
                .collect_frames()
                .unwrap();
            assert_eq!(frames.len(), 24);
            assert_eq!(frames[0].buffer().dimensions(), (48, 56));
            assert_eq!(frames[0].buffer().get_pixel(0, 0).0[3], 0);
            assert_ne!(frames[0].buffer(), frames[4].buffer());
        }
    }
}
