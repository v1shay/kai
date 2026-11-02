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
    if animated{mark.with_animation(SharedString::from(format!("ninja-status-{style}-{scene}")),Animation::new(Duration::from_millis(1300)).repeat(),|v,t|v.opacity(0.72+0.28*(t*std::f32::consts::PI).sin())).into_any_element()}else{mark.into_any_element()}
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
    fn ninja_statuses_keep_six_distinct_cached_palettes_for_every_scene(){
        for scene in kai_icons::SCENES{let mut marks=vec![];for(style,_,_)in ninjas::styles(){let a=status_image(style,scene);assert!(Arc::ptr_eq(&a,&status_image(style,scene)));let png=image::load_from_memory(&a.bytes).unwrap().into_rgba8();assert!(png.pixels().any(|p|p[3]==0));let colors=png.pixels().filter(|p|p[3]>0).map(|p|[p[0],p[1],p[2]]).collect::<HashSet<_>>();assert!(colors.len()>3,"Status must contain a full gradient, not a flat tint");marks.push(a);}for pair in marks.windows(2){assert!(!Arc::ptr_eq(&pair[0],&pair[1]));assert_ne!(pair[0].bytes,pair[1].bytes);}}
    }
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
