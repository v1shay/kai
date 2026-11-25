//! AppKit backdrop/filter handling adapted from MonoCode (MIT; LICENSES/Monocode.txt).
//! Siblings below GPUI's unchanged Metal/event host; no whole-window alpha.
use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, rc::Retained,
    runtime::AnyObject,
};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSColor, NSView, NSVisualEffectBlendingMode, NSVisualEffectMaterial,
    NSVisualEffectState, NSVisualEffectView, NSWindowOrderingMode,
};
use objc2_foundation::{NSArray, NSNumber, NSRect, NSString};
use objc2_quartz_core::CALayer;
use std::cell::Cell;
const BACKDROP: &str = "kai.workspace-backdrop";
const TINT: &str = "kai.workspace-tint";
#[derive(Default)]
struct Ivars {
    radius: Cell<u8>,
}
define_class!(
 #[unsafe(super(NSVisualEffectView))]
 #[name="KaiWorkspaceBackdrop"]
 #[ivars=Ivars]
 struct Backdrop;
 impl Backdrop{
  #[unsafe(method(updateLayer))]
  fn update_layer(&self){unsafe{let _:()=msg_send![super(self),updateLayer];}self.configure();}
  #[unsafe(method(kaiBackdropRadius))]
  fn backdrop_radius(&self)->usize{self.ivars().radius.get() as usize}
  #[unsafe(method_id(copyKaiTransitionBackdrop))]
  fn copy_transition_backdrop(&self)->Retained<NSView>{
      let view=Backdrop::new(self.frame(),self.mtm());
      view.ivars().radius.set(self.ivars().radius.get());
      view.setMaterial(self.material());view.setBlendingMode(self.blendingMode());view.setState(self.state());
      view.configure();
      view.into_super().into_super()
  }
  #[unsafe(method(hitTest:))]
  fn hit_test(&self,_point:objc2_foundation::NSPoint)->*mut AnyObject{std::ptr::null_mut()}
 }
);
define_class!(
 #[unsafe(super(NSView))]
 #[name="KaiWorkspaceTint"]
 struct Tint;
 impl Tint{
  #[unsafe(method(hitTest:))]
  fn hit_test(&self,_point:objc2_foundation::NSPoint)->*mut AnyObject{std::ptr::null_mut()}
 }
);
impl Backdrop {
    fn new(frame: NSRect, mtm: MainThreadMarker) -> Retained<Self> {
        let allocated = Self::alloc(mtm).set_ivars(Ivars {
            radius: Cell::new(24),
        });
        let view: Retained<Self> = unsafe { msg_send![super(allocated),initWithFrame:frame] };
        view.setMaterial(NSVisualEffectMaterial::UnderWindowBackground);
        view.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        view.setState(NSVisualEffectState::Active);
        view.setWantsLayer(true);
        view
    }
    fn configure(&self) {
        if let Some(layer) = self.layer() {
            let _ = objc2::exception::catch(std::panic::AssertUnwindSafe(|| {
                configure_layer(&layer, self.ivars().radius.get())
            }));
        }
    }
}
fn configure_layer(layer: &CALayer, radius: u8) {
    layer.setBackgroundColor(None);
    if layer.class().name().to_bytes() == b"CAChameleonLayer" {
        layer.setHidden(true);
        return;
    }
    if let Some(filters) = layer.filters() {
        let mut retained = Vec::with_capacity(filters.len());
        let mut changed = false;
        for filter in filters.iter() {
            let description: Retained<NSString> = unsafe { msg_send![&*filter, description] };
            let description = description.to_string();
            if description.contains("Saturat") {
                changed = true;
                continue;
            }
            if description.contains("Blur") {
                let key = NSString::from_str("inputRadius");
                let current: Option<Retained<NSNumber>> =
                    unsafe { msg_send![&*filter,valueForKey:&*key] };
                if current.as_ref().map(|n| n.doubleValue()) != Some(radius as f64) {
                    let replacement: Retained<AnyObject> = unsafe { msg_send![&*filter, copy] };
                    let number = NSNumber::numberWithDouble(radius as f64);
                    unsafe {
                        let _: () = msg_send![&*replacement,setValue:&*number,forKey:&*key];
                    }
                    retained.push(replacement);
                    changed = true;
                    continue;
                }
            }
            retained.push(filter);
        }
        if changed {
            unsafe {
                layer.setFilters(Some(&NSArray::from_retained_slice(&retained)));
            }
        }
    }
    if let Some(children) = unsafe { layer.sublayers() } {
        for child in children.iter() {
            configure_layer(&child, radius)
        }
    }
}
unsafe fn content<'a>(pointer: *mut AnyObject) -> &'a NSView {
    unsafe { &*pointer.cast::<NSView>() }
}
fn find(content: &NSView, id: &str) -> Option<Retained<NSView>> {
    use objc2_app_kit::NSUserInterfaceItemIdentification;
    let key = NSString::from_str(id);
    content
        .subviews()
        .iter()
        .find(|v| v.identifier().as_deref() == Some(&key))
}
pub unsafe fn set_radius(pointer: *mut AnyObject, radius: u8) {
    if find(unsafe { content(pointer) }, BACKDROP).is_none() {
        unsafe {
            install(pointer, true, 0);
        }
    }
    let content = unsafe { content(pointer) };
    if let Some(view) = find(content, BACKDROP) {
        if let Some(view) = view.downcast_ref::<Backdrop>() {
            view.ivars().radius.set(radius);
            view.configure();
            view.setNeedsDisplay(true);
        }
    }
}
pub unsafe fn install(pointer: *mut AnyObject, enabled: bool, tint: u32) {
    use objc2_app_kit::NSUserInterfaceItemIdentification;
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let content = unsafe { content(pointer) };
    let backing = if let Some(view) = find(content, BACKDROP) {
        view
    } else {
        if !enabled {
            return;
        }
        let view = Backdrop::new(content.bounds(), mtm);
        view.setIdentifier(Some(&NSString::from_str(BACKDROP)));
        view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        content.addSubview_positioned_relativeTo(&view, NSWindowOrderingMode::Below, None);
        view.into_super().into_super()
    };
    backing.setHidden(!enabled);
    let tint_view = if let Some(view) = find(content, TINT) {
        view
    } else {
        let view: Retained<Tint> =
            unsafe { msg_send![Tint::alloc(mtm),initWithFrame:content.bounds()] };
        view.setIdentifier(Some(&NSString::from_str(TINT)));
        view.setWantsLayer(true);
        view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        content.addSubview_positioned_relativeTo(
            &view,
            NSWindowOrderingMode::Above,
            Some(&backing),
        );
        view.into_super()
    };
    tint_view.setHidden(!enabled);
    if let Some(layer) = tint_view.layer() {
        let color = NSColor::colorWithSRGBRed_green_blue_alpha(
            ((tint >> 24) & 255) as f64 / 255.,
            ((tint >> 16) & 255) as f64 / 255.,
            ((tint >> 8) & 255) as f64 / 255.,
            (tint & 255) as f64 / 255.,
        );
        layer.setBackgroundColor(Some(&color.CGColor()));
    }
    if let Some(backdrop) = backing.downcast_ref::<Backdrop>() {
        backdrop.configure();
    }
}

// Public macOS 26 API, resolved at runtime so older SDKs/OSes remain supported.
// The actual GPUI view becomes contentView; it is never snapshotted or replaced.
const GLASS:&str="kai.workspace-liquid-glass";
const GLASS_CONTENT: &str = "kai.workspace-glass-content";
fn metal_view(view: &NSView) -> Option<Retained<NSView>> {
    view.subviews().iter().find(|child| child.class().name().to_bytes() == b"GPUIView")
}
pub fn liquid_available()->bool{objc2::runtime::AnyClass::get(c"NSGlassEffectView").is_some()}
fn liquid_selected(material:crate::theme::GlassMaterial)->bool{material!=crate::theme::GlassMaterial::Classic&&liquid_available()}
unsafe fn restore_renderer(content:&NSView){
    if let Some(glass)=find(content,GLASS){unsafe{
        let wrapper:Option<Retained<NSView>>=msg_send![&*glass,contentView];
        let renderer = wrapper.as_ref().and_then(|view| metal_view(view));
        if let Some(renderer)=renderer{
            let first=content.window().and_then(|w|w.firstResponder());
            let _:()=msg_send![&*glass,setContentView:std::ptr::null::<AnyObject>()];
            renderer.removeFromSuperview();
            renderer.setFrame(content.bounds());
            content.addSubview_positioned_relativeTo(&renderer,NSWindowOrderingMode::Above,None);
            if let (Some(window),Some(first))=(content.window(),first){window.makeFirstResponder(Some(&first));}
        }
        glass.removeFromSuperview();
    }}
}
pub unsafe fn apply_material(pointer:*mut AnyObject,enabled:bool,tint:u32,material:crate::theme::GlassMaterial){unsafe{
    let content=content(pointer);
    if !enabled||!liquid_selected(material){restore_renderer(content);install(pointer,enabled,tint);return}
    let Some(mtm)=MainThreadMarker::new()else{return};
    let glass=if let Some(glass)=find(content,GLASS){glass}else{
        // GPUI's native container has one renderer/event child. Effects/tint are
        // siblings identified above, never candidates for contentView.
        let renderer=metal_view(content);
        let Some(renderer)=renderer else{return};
        let Some(class)=objc2::runtime::AnyClass::get(c"NSGlassEffectView")else{return};
        let raw:*mut AnyObject=msg_send![class,alloc];
        let initialized:*mut NSView=msg_send![raw,initWithFrame:content.bounds()];
        let Some(glass)=Retained::from_raw(initialized)else{return};
        use objc2_app_kit::NSUserInterfaceItemIdentification;
        glass.setIdentifier(Some(&NSString::from_str(GLASS)));
        glass.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable|NSAutoresizingMaskOptions::ViewHeightSizable);
        let first=content.window().and_then(|w|w.firstResponder());
        let wrapper = NSView::initWithFrame(NSView::alloc(mtm), glass.bounds());
        wrapper.setIdentifier(Some(&NSString::from_str(GLASS_CONTENT)));
        wrapper.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable|NSAutoresizingMaskOptions::ViewHeightSizable);
        renderer.removeFromSuperview();
        wrapper.addSubview(&renderer);
        let _:()=msg_send![&*glass,setContentView:&*wrapper];
        let _:()=msg_send![&*glass,setCornerRadius:12f64];
        content.addSubview_positioned_relativeTo(&glass,NSWindowOrderingMode::Above,None);
        renderer.setFrame(wrapper.bounds());
        if let (Some(window),Some(first))=(content.window(),first){window.makeFirstResponder(Some(&first));}
        glass
    };
    let _=mtm;
    // Liquid Glass owns blur/refraction; the independent public tintColor
    // property leaves text/Metal content fully opaque and interactive.
    let color=NSColor::colorWithSRGBRed_green_blue_alpha(((tint>>24)&255)as f64/255.,((tint>>16)&255)as f64/255.,((tint>>8)&255)as f64/255.,(tint&255)as f64/255.);
    let _:()=msg_send![&*glass,setTintColor:&*color];
    if let Some(view)=find(content,BACKDROP){view.setHidden(true)}
    if let Some(view)=find(content,TINT){view.setHidden(true)}
}}

const POPUPS: &str = "kai.workspace-popup-glass";
const POPUP_MATERIAL: &str = "kai.workspace-popup-material";

/// A hit-test-transparent sibling of the Metal renderer preserves GPUI input,
/// accessibility and layout. Windows retain and reuse their own glass views.
pub unsafe fn update_popups(
    renderer: *mut AnyObject,
    surfaces: &[(gpui::Bounds<gpui::Pixels>, f32)],
) {
    use objc2_app_kit::NSUserInterfaceItemIdentification;
    use objc2_foundation::{NSPoint, NSSize};
    let Some(mtm) = MainThreadMarker::new() else { return };
    let Some(class) = objc2::runtime::AnyClass::get(c"NSGlassEffectView") else { return };
    let renderer = unsafe { content(renderer) };
    let Some(parent) = renderer.superview() else { return };
    if surfaces.is_empty() {
        if let Some(layer) = find(&parent, POPUPS) {
            layer.setHidden(true);
            if std::env::var_os("KAI_GLASS_DIAGNOSTIC").is_some() {
                eprintln!("KAI_GLASS_LAYOUT retired popup material");
            }
        }
        return;
    }
    let overlay = if let Some(layer) = find(&parent, POPUPS) {
        layer
    } else {
        let layer: Retained<Tint> = unsafe { msg_send![Tint::alloc(mtm),initWithFrame:parent.bounds()] };
        layer.setIdentifier(Some(&NSString::from_str(POPUPS)));
        parent.addSubview_positioned_relativeTo(&layer, NSWindowOrderingMode::Below, Some(renderer));
        layer.into_super()
    };
    if overlay.frame() != parent.bounds() {
        overlay.setFrame(parent.bounds());
    }
    overlay.setHidden(false);
    if std::env::var_os("KAI_GLASS_DIAGNOSTIC").is_some() {
        eprintln!("KAI_GLASS_LAYOUT parent={} surfaces={:?}",parent.class().name().to_string_lossy(),surfaces);
    }
    let children = overlay.subviews();
    for (index, (bounds, opacity)) in surfaces.iter().enumerate() {
        let x: f32 = bounds.origin.x.into();
        let y: f32 = bounds.origin.y.into();
        let width: f32 = bounds.size.width.into();
        let height: f32 = bounds.size.height.into();
        let y = if renderer.isFlipped() { f64::from(y) } else {
            renderer.bounds().size.height - f64::from(y + height)
        };
        let rect = NSRect::new(NSPoint::new(x.into(), y), NSSize::new(width.into(), height.into()));
        let frame = renderer.convertRect_toView(rect, Some(&overlay));
        let glass = if let Some(view) = children.iter().nth(index) {
            view
        } else {
            let raw: *mut AnyObject = unsafe { msg_send![class,alloc] };
            let initialized: *mut NSView = unsafe { msg_send![raw,initWithFrame:frame] };
            let Some(view) = (unsafe { Retained::from_raw(initialized) }) else { continue };
            let content = NSView::initWithFrame(NSView::alloc(mtm), view.bounds());
            content.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable);
            view.setIdentifier(Some(&NSString::from_str(POPUP_MATERIAL)));
            unsafe {
                let _: () = msg_send![&*view,setContentView:&*content];
                let _: () = msg_send![&*view,setCornerRadius:10f64];
            }
            overlay.addSubview(&view);
            view
        };
        if glass.frame() != frame { glass.setFrame(frame); }
        let alpha = f64::from(opacity.clamp(0., 1.));
        if glass.alphaValue() != alpha { glass.setAlphaValue(alpha); }
        glass.setHidden(alpha == 0.);
    }
    for view in children.iter().skip(surfaces.len()) { view.setHidden(true); }
}

pub unsafe fn visible_popups(renderer: *mut AnyObject) -> usize {
    use objc2_app_kit::NSUserInterfaceItemIdentification;
    let renderer = unsafe { content(renderer) };
    let Some(parent) = renderer.superview() else { return 0 };
    let Some(overlay) = find(&parent, POPUPS) else { return 0 };
    let children = overlay.subviews();
    if std::env::var_os("KAI_GLASS_DIAGNOSTIC").is_some() {
        eprintln!("KAI_GLASS_INSPECT hidden={} children={:?}", overlay.isHidden(), children.iter()
            .map(|view| (view.class().name().to_string_lossy().into_owned(), view.isHidden(), view.frame()))
            .collect::<Vec<_>>());
    }
    let Some(class) = objc2::runtime::AnyClass::get(c"NSGlassEffectView") else { return 0 };
    let identifier = NSString::from_str(POPUP_MATERIAL);
    if overlay.isHidden() { return 0; }
    children.iter().filter(|view| {
        let native: objc2::runtime::Bool = unsafe { msg_send![&**view,isKindOfClass:class] };
        native.as_bool() && view.identifier().as_deref() == Some(&identifier) && !view.isHidden()
            && view.frame().size.width > 0. && view.frame().size.height > 0.
    }).count()
}

/// Bounded read-only inspection for the isolated packaged-app verification.
pub unsafe fn inspect(pointer: *mut AnyObject) -> Result<(), String> {
    let content = unsafe { content(pointer) };
    if let Some(glass)=find(content,GLASS){
        let renderer:Option<Retained<NSView>>=unsafe{msg_send![&*glass,contentView]};
        if renderer.as_ref().and_then(|wrapper| metal_view(wrapper)).is_none(){
            return Err("Liquid Glass lost its live renderer".into())
        }
        if find(content,BACKDROP).is_some_and(|v|!v.isHidden()){return Err("Duplicate glass backdrop".into())}
        return Ok(())
    }
    let backing = find(content, BACKDROP).ok_or("No native workspace backdrop")?;
    let tint = find(content, TINT).ok_or("No independent native tint")?;
    let alpha: f64 = unsafe { msg_send![&*backing, alphaValue] };
    if alpha != 1. {
        return Err(format!("Backdrop alpha must be 1, got {alpha}"));
    }
    if backing
        .hitTest(objc2_foundation::NSPoint::new(2., 2.))
        .is_some()
        || tint
            .hitTest(objc2_foundation::NSPoint::new(2., 2.))
            .is_some()
    {
        return Err("Native glass intercepted workspace input".into());
    }
    if backing.isHidden() || tint.isHidden() {
        return Err("Enabled backdrop/tint is hidden".into());
    }
    let mut effects = 0;
    for view in content.subviews().iter() {
        let yes: objc2::runtime::Bool = unsafe {
            msg_send![&*view,isKindOfClass:objc2::runtime::AnyClass::get(c"NSVisualEffectView").unwrap()]
        };
        if yes.as_bool() && !view.isHidden() {
            effects += 1
        }
    }
    if effects != 1 {
        return Err(format!("Expected one native backdrop, found {effects}"));
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn real_gpu_preserves_native_backdrop_through_overlapping_translucent_ui() {
        gpui::verify_macos_translucency().unwrap();
    }
    use objc2::runtime::AnyClass;
    #[test]
    fn gaussian_filter_is_copied_and_saturation_is_removed_without_churning_unchanged_filters() {
        unsafe {
            let class = AnyClass::get(c"CAFilter").expect("Core Animation filters");
            let blur: Retained<AnyObject> =
                msg_send![class,filterWithType:&*NSString::from_str("gaussianBlur")];
            let saturation: Retained<AnyObject> =
                msg_send![class,filterWithType:&*NSString::from_str("colorSaturate")];
            let key = NSString::from_str("inputRadius");
            let before = NSNumber::numberWithDouble(12.);
            let _: () = msg_send![&*blur,setValue:&*before,forKey:&*key];
            let layer = CALayer::new();
            layer.setFilters(Some(&NSArray::from_retained_slice(&[
                blur.clone(),
                saturation,
            ])));
            for radius in [24u8, 48, 0, 64] {
                let previous = layer.filters().unwrap().objectAtIndex(0);
                let old: Retained<NSNumber> = msg_send![&*previous,valueForKey:&*key];
                configure_layer(&layer, radius);
                let filters = layer.filters().unwrap();
                assert_eq!(filters.len(), 1);
                let current = filters.objectAtIndex(0);
                let value: Retained<NSNumber> = msg_send![&*current,valueForKey:&*key];
                assert_eq!(value.doubleValue(), radius as f64);
                assert!(!std::ptr::eq(&*previous, &*current));
                let unchanged: Retained<NSNumber> = msg_send![&*previous,valueForKey:&*key];
                assert_eq!(old.doubleValue(), unchanged.doubleValue());
                configure_layer(&layer, radius);
                assert!(std::ptr::eq(
                    &*current,
                    &*layer.filters().unwrap().objectAtIndex(0)
                ));
            }
        }
    }
}
