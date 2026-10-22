import AppKit

/// A bounded, native backdrop surface. Text stays outside the material hierarchy.
@MainActor final class NotchGlassBackground:NSView {
    private let frost=NSVisualEffectView()
    private var nativeGlass:NSView?
    private let fade=CAGradientLayer(),ink=CAGradientLayer(),tint=CAGradientLayer(),haze=CAGradientLayer(),rim=CAGradientLayer()
    override init(frame:NSRect){
        super.init(frame:frame)
        wantsLayer=true
        frost.material = .hudWindow;frost.blendingMode = .behindWindow;frost.state = .active
        frost.appearance=NSAppearance(named:.darkAqua)
        addSubview(frost)
        // Resolve the public AppKit class at runtime so older SDK builds also
        // use Liquid Glass when running on a system that provides it.
        if #available(macOS 26.0,*),let type=NSClassFromString("NSGlassEffectView") as? NSView.Type {
            let glass=type.init(frame:.zero);glass.wantsLayer=true
            addSubview(glass);nativeGlass=glass;frost.isHidden=true
        }
        let material=nativeGlass ?? frost;material.wantsLayer=true;material.layer?.mask=fade
        fade.colors=[NSColor.white.cgColor,NSColor.white.cgColor,NSColor.clear.cgColor]
        fade.locations=[0,0.3,1];fade.startPoint=CGPoint(x:0.5,y:0);fade.endPoint=CGPoint(x:0.5,y:1)
        for item in [ink,haze,tint,rim]{layer?.addSublayer(item);item.startPoint=CGPoint(x:0.5,y:0);item.endPoint=CGPoint(x:0.5,y:1)}
        ink.colors=[NSColor.black.withAlphaComponent(0.60).cgColor,NSColor.black.withAlphaComponent(0.80).cgColor,NSColor.black.cgColor]
        ink.locations=[0,0.65,1]
        rim.colors=[NSColor.white.withAlphaComponent(0.17).cgColor,NSColor.clear.cgColor]
        isHidden=true
    }
    required init?(coder:NSCoder){fatalError("init(coder:) has not been implemented")}
    override func hitTest(_ point:NSPoint)->NSView?{nil}
    static func depth(for height:CGFloat)->CGFloat{height*CGFloat(GlassPreferences().height)}
    func update(canvas:CGSize,width:CGFloat,enabled:Bool,color:NSColor,preferences:GlassPreferences=GlassPreferences()){
        CATransaction.begin();CATransaction.setDisableActions(true);defer{CATransaction.commit()}
        frame=CGRect(origin:.zero,size:canvas);isHidden = !enabled
        guard canvas.height>0,width>0 else{return}
        let offset=min(canvas.height*0.25,CGFloat(preferences.offset)*canvas.height)
        let depth=min(canvas.height-offset,canvas.height*CGFloat(preferences.height))
        let area=CGRect(x:(canvas.width-width)/2,y:offset,width:width,height:depth)
        frost.frame=area;nativeGlass?.frame=area;fade.frame=CGRect(origin:.zero,size:area.size)
        frost.material=preferences.blur == 0 ? .underWindowBackground : preferences.blur == 2 ? .hudWindow:.popover
        let useNative=nativeGlass != nil && preferences.blur == 0
        nativeGlass?.isHidden = !useNative;frost.isHidden=useNative
        frost.layer?.mask=nil;nativeGlass?.layer?.mask=nil
        (useNative ? nativeGlass : frost)?.layer?.mask=fade
        func feather(_ t:Double)->Double{let edge=offset>0 ? min(1,t/0.18):1;return preferences.blend.strength(t)*edge*edge*(3-2*edge)}
        var locations=[NSNumber](),maskColors=[CGColor](),tintColors=[CGColor](),inkColors=[CGColor]()
        // Dense samples of curves with a flat derivative at either endpoint
        // avoid the visible seam of a short three-stop gradient.
        for i in 0...32 {
            let t=Double(i)/32,strength=feather(t)
            locations.append(NSNumber(value:t))
            maskColors.append(NSColor.white.withAlphaComponent(CGFloat(strength*preferences.opacity*preferences.blurAmount)).cgColor)
            tintColors.append(color.withAlphaComponent(CGFloat(strength*preferences.tint*preferences.opacity)).cgColor)
        }
        fade.locations=locations;fade.colors=maskColors
        ink.frame=bounds
        var inkLocations=[NSNumber]()
        let lower=max(0,offset/canvas.height),upper=(offset+depth)/canvas.height
        if lower>0 {inkLocations.append(0);inkColors.append(NSColor.black.cgColor)}
        for i in 0...32 {
            let t=Double(i)/32,strength=feather(t)
            inkLocations.append(NSNumber(value:Double(lower)+(Double(upper-lower)*t)))
            let alpha=1-strength*preferences.opacity*(0.65-preferences.haze*0.35)
            inkColors.append(NSColor.black.withAlphaComponent(CGFloat(alpha)).cgColor)
        }
        if upper<1 {inkLocations.append(1);inkColors.append(NSColor.black.cgColor)}
        ink.locations=inkLocations;ink.colors=inkColors
        haze.frame=area;haze.locations=locations;haze.colors=(0...32).map{NSColor.white.withAlphaComponent(CGFloat(feather(Double($0)/32)*preferences.haze*preferences.opacity*0.12)).cgColor}
        tint.frame=area;tint.locations=locations;tint.colors=tintColors
        rim.colors=[color.blended(withFraction:0.7,of:.white)?.withAlphaComponent(CGFloat(preferences.rim)).cgColor ?? NSColor.clear.cgColor,NSColor.clear.cgColor]
        rim.frame=CGRect(x:area.minX+CGFloat(preferences.radius),y:offset,width:max(0,width-CGFloat(preferences.radius)*2),height:1)
    }
}
