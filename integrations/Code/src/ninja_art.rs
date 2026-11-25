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
// Tiny, cached color rasters preserve the SVG's alpha and full Ninja gradient.
// They are shared across status rows; animation never rerasterizes the icon.
fn status_image(style:&str,scene:&'static str)->Arc<Image>{
    static CACHE:std::sync::OnceLock<std::sync::Mutex<HashMap<(String,&'static str),Arc<Image>>>>=std::sync::OnceLock::new();
    let mut cache=CACHE.get_or_init(Default::default).lock().unwrap();
    let key=(style.to_owned(),scene);
    cache.entry(key).or_insert_with(||{
        let bytes=svg_assets::asset(kai_icons::status_asset(scene)).expect("embedded status SVG");
        let tree=resvg::usvg::Tree::from_data(bytes,&Default::default()).expect("valid status SVG");
        let mut raster=resvg::tiny_skia::Pixmap::new(48,48).unwrap();
        resvg::render(&tree,resvg::tiny_skia::Transform::from_scale(48./tree.size().width(),48./tree.size().height()),&mut raster.as_mut());
        let base=color(style);
        let mut pixels=raster.take();
        for (i,pixel) in pixels.chunks_exact_mut(4).enumerate(){
            let t=((i%48+i/48) as f32/94.).clamp(0.,1.);
            for (c,shift) in [16,8,0].into_iter().enumerate(){let v=((base>>shift)&255) as f32;pixel[c]=if t<0.5{(v+(255.-v)*0.45*(1.-t*2.))as u8}else{(v*(1.-0.38*(t*2.-1.)))as u8};}
        }
        let mut bytes=std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_raw(48,48,pixels).unwrap().write_to(&mut bytes,image::ImageFormat::Png).unwrap();
        Arc::new(Image::from_bytes(ImageFormat::Png,bytes.into_inner()))
    }).clone()
}
pub(super) fn status(style:&str,scene:&'static str,animated:bool)->AnyElement{
    let mark=img(status_image(style,scene)).size(px(14.)).flex_none();
    if animated{mark.opacity_animation(SharedString::from(format!("ninja-status-{style}-{scene}")),gpui::OpacityAnimation { period:Duration::from_millis(1300),baseline: 0.72,amplitude:0.28, waveform: gpui::OpacityWaveform::HalfSinePi }).into_any_element()}else{mark.into_any_element()}
}
pub(super) fn emblem(style: &str) -> Arc<Image> {
    static IMAGES: std::sync::OnceLock<[Arc<Image>; 13]> = std::sync::OnceLock::new();
    let index = ninjas::styles().iter().position(|(id, _, _)| *id == style).unwrap_or(0);
    IMAGES.get_or_init(|| [
        include_bytes!("../assets/ninja-fire.png").as_slice(),
        include_bytes!("../assets/ninja-lightning.png").as_slice(),
        include_bytes!("../assets/ninja-earth.png").as_slice(),
        include_bytes!("../assets/ninja-ice.png").as_slice(),
        include_bytes!("../assets/ninja-energy.png").as_slice(),
        include_bytes!("../assets/ninja-water.png").as_slice(),
        include_bytes!("../assets/ninja-stone-fists.png").as_slice(),
        include_bytes!("../assets/ninja-ice-shards.png").as_slice(),
        include_bytes!("../assets/ninja-water-swirls.png").as_slice(),
        include_bytes!("../assets/ninja-lightning-burst.png").as_slice(),
        include_bytes!("../assets/ninja-green-spirals.png").as_slice(),
        include_bytes!("../assets/ninja-flame-phoenix.png").as_slice(),
        include_bytes!("../assets/ninja-gold-white-knot.png").as_slice(),
    ].map(|bytes| Arc::new(Image::from_bytes(ImageFormat::Png, bytes.to_vec()))))[index].clone()
}
pub(super) fn wallpaper(_style:&str,_animated:bool)->AnyElement {
    div().id("ninja-wallpaper").debug_selector(||"ninja-wallpaper".into()).absolute().inset_0()
        .child(canvas(|bounds,_,_|unified::atmosphere_slot(bounds),|_,_,_,_|{}).size_full()).into_any_element()
}
pub(super) fn art(style: &str, size: f32, animated: bool) -> AnyElement {
    let mark = img(emblem(style)).size(px(size)).flex_none().object_fit(ObjectFit::Contain);
    if animated {
        mark.opacity_pulse(
            SharedString::from(format!("ninja-emblem-{style}-{size}")),
            Duration::from_secs(3),
            0.88,
            0.12,
        )
        .into_any_element()
    } else {
        mark.into_any_element()
    }
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
        body.child(composer_design::shimmer_repeat(
            SharedString::from(format!("ninja-name-{style}")), color,
        )).child(contents())
        .into_any_element()
    } else {
        body.child(contents()).into_any_element()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    #[test]
    fn ninja_statuses_keep_distinct_cached_palettes_for_every_scene(){
        for scene in kai_icons::SCENES{let mut marks=vec![];for(style,_,_)in ninjas::styles(){let a=status_image(style,scene);assert!(Arc::ptr_eq(&a,&status_image(style,scene)));let png=image::load_from_memory(&a.bytes).unwrap().into_rgba8();assert!(png.pixels().any(|p|p[3]==0));let colors=png.pixels().filter(|p|p[3]>0).map(|p|[p[0],p[1],p[2]]).collect::<HashSet<_>>();assert!(colors.len()>3,"Status must contain a full gradient, not a flat tint");marks.push(a);}for pair in marks.windows(2){assert!(!Arc::ptr_eq(&pair[0],&pair[1]));assert_ne!(pair[0].bytes,pair[1].bytes);}}
    }
    #[test]
    fn all_thirteen_picker_emblems_are_distinct_and_transparent() {
        let mut seen = HashSet::new();
        for (style, _, _) in ninjas::styles() {
            let art = emblem(style);
            assert!(seen.insert(art.bytes.clone()));
            let rgba = image::load_from_memory(&art.bytes).unwrap().into_rgba8();
            assert!(rgba.pixels().any(|pixel| pixel[3] == 0), "{style}");
            assert!(rgba.pixels().any(|pixel| pixel[3] == 255), "{style}");
        }
    }
}
