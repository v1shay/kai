import AppKit

/// A cached native bridge for the part of a morph outside the real window.
/// Its fixed exclusion mask prevents a second backdrop beneath the live glass.
@MainActor final class WorkspaceGlassBridge {
    let panel=NSPanel(contentRect:.zero,styleMask:[.borderless,.nonactivatingPanel],backing:.buffered,defer:false)
    private let root=NSView(),surface=NSView(),exclusion=CAShapeLayer()
    private var profileKey=""
    private var lastCutout:CGRect?
    private(set) var shape=CAShapeLayer()

    init(){
        panel.isOpaque=false;panel.backgroundColor = .clear;panel.hasShadow=false;panel.ignoresMouseEvents=true
        panel.collectionBehavior=[.fullScreenAuxiliary,.canJoinAllSpaces]
        root.wantsLayer=true;surface.wantsLayer=true;root.addSubview(surface)
        exclusion.fillRule = .evenOdd;surface.layer?.mask=exclusion;root.layer?.mask=shape
        panel.contentView=root
    }
    static func named(_ id:String,in view:NSView)->NSView?{
        if view.identifier?.rawValue==id{return view}
        for child in view.subviews{if let found=named(id,in:child){return found}}
        return nil
    }
    static let backgroundIDs:Set<String>=["kai.workspace-backdrop","kai.workspace-tint","kai.workspace-liquid-glass"]
    static func hasNativeGlass(_ window:NSWindow)->Bool{
        guard let root=window.contentView else{return false}
        return ["kai.workspace-backdrop","kai.workspace-liquid-glass"].contains{named($0,in:root).map{!$0.isHidden} ?? false}
    }
    static func foregroundLayers(_ window:NSWindow)->[CALayer]{
        guard let content=window.contentView else{return []}
        func layer(_ view:NSView)->[CALayer]{view.wantsLayer=true;return view.layer.map{[$0]} ?? []}
        guard backgroundIDs.contains(where:{named($0,in:content) != nil}) else{return layer(content)}
        return content.subviews.flatMap{view -> [CALayer] in
            if view.identifier?.rawValue=="kai.workspace-liquid-glass"{
                // Public NSGlassEffectView content remains opaque/interactable;
                // only that content fades, never the material that contains it.
                return (view.value(forKey:"contentView") as? NSView).map{layer($0)} ?? []
            }
            if backgroundIDs.contains(view.identifier?.rawValue ?? ""){return []}
            return layer(view)
        }
    }
    /// Bounding rectangle of union minus the actual window. Usually just the
    /// menu-bar strip; windowed/side-by-side morphs can need a larger bridge.
    static func outsideBounds(_ union:CGRect,excluding window:CGRect)->CGRect?{
        let overlap=union.intersection(window)
        if overlap.isNull{return union}
        let pieces=[
            CGRect(x:union.minX,y:union.minY,width:union.width,height:max(0,overlap.minY-union.minY)),
            CGRect(x:union.minX,y:overlap.maxY,width:union.width,height:max(0,union.maxY-overlap.maxY)),
            CGRect(x:union.minX,y:overlap.minY,width:max(0,overlap.minX-union.minX),height:overlap.height),
            CGRect(x:overlap.maxX,y:overlap.minY,width:max(0,union.maxX-overlap.maxX),height:overlap.height)
        ].filter{$0.width>0 && $0.height>0}
        return pieces.reduce(nil as CGRect?){result,piece in result.map{$0.union(piece)} ?? piece}
    }
    func configure(window:NSWindow,frame:CGRect,opaqueColor:NSColor?){
        let content=window.contentView!
        let liquid=Self.named("kai.workspace-liquid-glass",in:content).flatMap{$0.isHidden ? nil:$0}
        let classic=Self.named("kai.workspace-backdrop",in:content).flatMap{$0.isHidden ? nil:$0}
        let tint=Self.named("kai.workspace-tint",in:content)?.layer?.backgroundColor
        let appearance=window.effectiveAppearance
        let radius=classic.map{($0.value(forKey:"kaiBackdropRadius") as! NSNumber).intValue} ?? 0
        let liquidTint=liquid?.value(forKey:"tintColor") as? NSColor
        let key="\(liquid.map(ObjectIdentifier.init).map(String.init(describing:)) ?? "-")|\(classic.map(ObjectIdentifier.init).map(String.init(describing:)) ?? "-")|\(radius)|\(appearance.name.rawValue)|\(String(describing:tint))|\(String(describing:liquidTint))|\(String(describing:opaqueColor))"
        if key != profileKey{
            surface.subviews.forEach{$0.removeFromSuperview()};surface.layer?.backgroundColor=nil
            if let liquid{
                let glass=type(of:liquid).init(frame:surface.bounds);glass.wantsLayer=true
                glass.setValue(liquidTint,forKey:"tintColor");glass.setValue(0,forKey:"cornerRadius")
                glass.autoresizingMask=[.width,.height];surface.addSubview(glass)
            }else if let classic{
                // Rust's own backdrop class copies its radius/filter policy.
                // This is the same material implementation as the live window.
                let glass=classic.perform(NSSelectorFromString("copyKaiTransitionBackdrop"))!.takeRetainedValue() as! NSView
                glass.frame=surface.bounds;glass.autoresizingMask=[.width,.height];surface.addSubview(glass)
                let overlay=NSView(frame:surface.bounds);overlay.wantsLayer=true;overlay.layer?.backgroundColor=tint
                overlay.autoresizingMask=[.width,.height];surface.addSubview(overlay)
            }else{
                surface.layer?.backgroundColor=opaqueColor?.cgColor
            }
            panel.appearance=appearance;profileKey=key
        }
        if panel.level != window.level{panel.level=window.level}
        if panel.frame != frame{panel.setFrame(frame,display:false)}
        let bounds=CGRect(origin:.zero,size:frame.size),resized=surface.frame != CGRect(origin:.zero,size:frame.size)
        // NSPanel may resize its content root before this call; the material
        // container and both masks still need the new geometry exactly once.
        if root.frame != bounds{root.frame=bounds}
        if resized{surface.frame=root.bounds;shape.frame=root.bounds;exclusion.frame=surface.bounds}
        let cutout=window.frame.intersection(frame).offsetBy(dx:-frame.minX,dy:-frame.minY)
        if resized || lastCutout != cutout{
            let path=CGMutablePath();path.addRect(surface.bounds)
            if !cutout.isNull && cutout.width>0 && cutout.height>0{path.addRect(cutout)}
            exclusion.path=path;lastCutout=cutout
        }
    }
    func hide(){panel.orderOut(nil);shape.removeAnimation(forKey:"workspaceShape")}
}
